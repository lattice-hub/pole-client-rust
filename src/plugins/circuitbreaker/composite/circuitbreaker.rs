// Tencent is pleased to support the open source community by making Pole available.
//
// Copyright (C) 2019 THL A29 Limited, a Tencent company. All rights reserved.
//
// Licensed under the BSD 3-Clause License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://opensource.org/licenses/BSD-3-Clause
//
// Unless required by applicable law or agreed to in writing, software distributed
// under the License is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR
// CONDITIONS OF ANY KIND, either express or implied. See the License for the
// specific language governing permissions and limitations under the License.

use crate::core::{
    model::{
        circuitbreaker::{CircuitBreakerStatus, Resource, ResourceStat, RetStatus, Status},
        error::PoleError,
    },
    plugin::{circuitbreaker::CircuitBreaker, plugins::Plugin},
};
use pole_specification::v1::{trigger_condition, CircuitBreakerRule};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

static PLUGIN_NAME: &str = "composite";

fn new_circuir_breaker() -> Box<dyn CircuitBreaker> {
    Box::new(CompositeCircuitBreaker::new())
}

pub struct CompositeCircuitBreaker {
    // rules 是当前服务规则快照；具体资源的运行时状态在 states 中按 resource_key 隔离。
    rules: Mutex<Vec<CircuitBreakerRule>>,
    states: Mutex<HashMap<String, Vec<CircuitBreakerRuleState>>>,
}

impl CompositeCircuitBreaker {
    pub fn builder() -> (fn() -> Box<dyn CircuitBreaker>, String) {
        (new_circuir_breaker, PLUGIN_NAME.to_string())
    }

    fn new() -> Self {
        Self {
            rules: Mutex::new(Vec::new()),
            states: Mutex::new(HashMap::new()),
        }
    }

    #[cfg(test)]
    fn new_with_rules(rules: Vec<CircuitBreakerRule>) -> Self {
        Self {
            rules: Mutex::new(rules),
            states: Mutex::new(HashMap::new()),
        }
    }

    fn update_rules_inner(&self, rules: Vec<CircuitBreakerRule>) {
        let mut current_rules = self.rules.lock().unwrap();
        if *current_rules == rules {
            return;
        }
        // 只有规则内容真实变化时才清空运行态，周期性刷新相同快照不能丢失统计。
        *current_rules = rules;
        drop(current_rules);
        self.states.lock().unwrap().clear();
    }

    fn rules_snapshot(&self) -> Vec<CircuitBreakerRule> {
        self.rules.lock().unwrap().clone()
    }

    fn states_for_resource<'a>(
        states: &'a mut HashMap<String, Vec<CircuitBreakerRuleState>>,
        resource_key: String,
        rules: Vec<CircuitBreakerRule>,
    ) -> &'a mut Vec<CircuitBreakerRuleState> {
        // 每个资源第一次上报统计时，基于当前规则快照创建独立状态机集合。
        states.entry(resource_key).or_insert_with(|| {
            rules
                .into_iter()
                .map(CircuitBreakerRuleState::new)
                .collect()
        })
    }
}

struct CircuitBreakerRuleState {
    rule: CircuitBreakerRule,
    status: Status,
    opened_at: Option<Instant>,
    consecutive_errors: u32,
    consecutive_successes: u32,
    request_count: u32,
}

impl CircuitBreakerRuleState {
    fn new(rule: CircuitBreakerRule) -> Self {
        Self {
            rule,
            status: Status::Close,
            opened_at: None,
            consecutive_errors: 0,
            consecutive_successes: 0,
            request_count: 0,
        }
    }

    fn report_status(&mut self, ret_status: &RetStatus) {
        self.refresh_status();

        // Open 状态下拒绝请求，不再消费失败统计；等待 sleep window 转半开。
        if self.status == Status::Open {
            return;
        }

        if self.status == Status::HalfOpen {
            // 半开状态用连续成功数判断恢复，任一错误立即重新打开。
            if is_error_status(ret_status) {
                self.open();
            } else {
                self.consecutive_successes += 1;
                if self.consecutive_successes >= self.recover_success_threshold() {
                    self.close();
                }
            }
            return;
        }

        self.request_count += 1;
        if is_error_status(ret_status) {
            self.consecutive_errors += 1;
        } else {
            self.consecutive_errors = 0;
        }

        if self.should_open() {
            self.open();
        }
    }

    fn status(&self) -> Status {
        self.status.clone()
    }

    fn current_status(&mut self) -> Status {
        self.refresh_status();
        self.status()
    }

    fn rule_name(&self) -> String {
        self.rule.name.clone()
    }

    fn open(&mut self) {
        self.status = Status::Open;
        self.opened_at = Some(Instant::now());
        self.consecutive_successes = 0;
    }

    fn close(&mut self) {
        self.status = Status::Close;
        self.opened_at = None;
        self.consecutive_errors = 0;
        self.consecutive_successes = 0;
        self.request_count = 0;
    }

    fn refresh_status(&mut self) {
        // Open 到 HalfOpen 的转换由访问时懒触发，避免额外后台定时器。
        if self.status != Status::Open {
            return;
        }
        let Some(recover_condition) = self.recover_condition() else {
            return;
        };
        let Some(opened_at) = self.opened_at else {
            return;
        };
        let sleep_window = Duration::from_secs(recover_condition.sleep_window as u64);
        if opened_at.elapsed() >= sleep_window {
            self.status = Status::HalfOpen;
            self.consecutive_successes = 0;
        }
    }

    fn recover_success_threshold(&self) -> u32 {
        self.recover_condition()
            .map(|condition| condition.consecutive_success.max(1))
            .unwrap_or(1)
    }

    fn recover_condition(&self) -> Option<&pole_specification::v1::RecoverCondition> {
        self.rule
            .block_configs
            .iter()
            .find_map(|policy| policy.recover_condition.as_ref())
    }

    fn should_open(&self) -> bool {
        if !self.rule.enable {
            return false;
        }

        // 当前实现只处理连续错误触发条件，其它触发类型保留给后续扩展。
        for trigger in self
            .rule
            .block_configs
            .iter()
            .filter_map(|policy| policy.block_config.as_ref())
            .flat_map(|block| block.trigger_conditions.iter())
        {
            if trigger.trigger_type() != trigger_condition::TriggerType::ConsecutiveError {
                continue;
            }
            if self.request_count < trigger.minimum_request {
                continue;
            }
            if trigger.error_count > 0 && self.consecutive_errors >= trigger.error_count {
                return true;
            }
        }

        false
    }
}

fn is_error_status(status: &RetStatus) -> bool {
    matches!(
        status,
        RetStatus::RetFail | RetStatus::RetTimeout | RetStatus::RetReject
    )
}

fn applicable_rules(
    resource: &Resource,
    rules: Vec<CircuitBreakerRule>,
) -> Vec<CircuitBreakerRule> {
    let mut rules = rules
        .into_iter()
        .filter(|rule| rule.enable && rule_matches_resource(rule, resource))
        .collect::<Vec<_>>();
    // 稳定排序保证同优先级规则继续沿用控制面返回顺序。
    rules.sort_by(|left, right| left.priority.cmp(&right.priority));
    rules
}

fn rule_matches_resource(rule: &CircuitBreakerRule, resource: &Resource) -> bool {
    let Some(matcher) = rule.rule_matcher.as_ref() else {
        return true;
    };
    let (caller, callee) = match resource {
        Resource::ServiceResource(resource) => (resource.caller.as_ref(), Some(&resource.callee)),
        Resource::MethodResource(resource) => (resource.caller.as_ref(), Some(&resource.callee)),
        Resource::InstanceResource(_) => (None, None),
    };

    matcher
        .source
        .as_ref()
        .map(|source| {
            service_selector_matches(source.namespace.as_str(), source.service.as_str(), caller)
        })
        .unwrap_or(true)
        && matcher
            .destination
            .as_ref()
            .map(|destination| {
                service_selector_matches(
                    destination.namespace.as_str(),
                    destination.service.as_str(),
                    callee,
                )
            })
            .unwrap_or(true)
}

fn service_selector_matches(
    namespace: &str,
    service: &str,
    actual: Option<&crate::core::model::naming::ServiceKey>,
) -> bool {
    let namespace_is_wildcard = namespace.is_empty() || namespace == "*";
    let service_is_wildcard = service.is_empty() || service == "*";
    if namespace_is_wildcard && service_is_wildcard {
        return true;
    }
    actual
        .map(|actual| {
            (namespace_is_wildcard || namespace == actual.namespace)
                && (service_is_wildcard || service == actual.name)
        })
        .unwrap_or(false)
}

fn resource_key(resource: &Resource) -> String {
    // 服务级和方法级资源分别隔离统计，caller 缺失时用 "*" 表示全主调。
    match resource {
        Resource::ServiceResource(resource) => format!(
            "service#{}#{}",
            service_key(&resource.callee),
            resource
                .caller
                .as_ref()
                .map(service_key)
                .unwrap_or_else(|| "*".to_string())
        ),
        Resource::MethodResource(resource) => format!(
            "method#{}#{}#{}#{}#{}",
            service_key(&resource.callee),
            resource
                .caller
                .as_ref()
                .map(service_key)
                .unwrap_or_else(|| "*".to_string()),
            resource.protocol,
            resource.method,
            resource.path,
        ),
        Resource::InstanceResource(_) => "instance".to_string(),
    }
}

fn service_key(key: &crate::core::model::naming::ServiceKey) -> String {
    format!("{}#{}", key.namespace, key.name)
}

impl Plugin for CompositeCircuitBreaker {
    fn init(&mut self) {}

    fn destroy(&self) {}

    fn name(&self) -> String {
        PLUGIN_NAME.to_string()
    }
}

#[async_trait::async_trait]
impl CircuitBreaker for CompositeCircuitBreaker {
    fn update_rules(&self, rules: Vec<CircuitBreakerRule>) {
        self.update_rules_inner(rules);
    }

    /// check_resource 检查资源
    async fn check_resource(&self, _resource: Resource) -> Result<CircuitBreakerStatus, PoleError> {
        let key = resource_key(&_resource);
        let mut states = self.states.lock().unwrap();
        let Some(resource_states) = states.get_mut(&key) else {
            // 尚未有统计上报的资源默认 Close，避免规则加载后立即阻断。
            return Ok(CircuitBreakerStatus {
                status: Status::Close,
                start_ms: 0,
                circuit_breaker: "".to_string(),
                fallback_info: None,
                destroy: false,
            });
        };
        let mut half_open_state = None;
        for state in resource_states.iter_mut() {
            let status = state.current_status();
            if status == Status::Open {
                // 任一规则打开即阻断资源。
                return Ok(CircuitBreakerStatus {
                    status: Status::Open,
                    start_ms: 0,
                    circuit_breaker: state.rule_name(),
                    fallback_info: None,
                    destroy: false,
                });
            }
            if status == Status::HalfOpen && half_open_state.is_none() {
                // 半开不直接阻断，由上层决定是否试探放行。
                half_open_state = Some(state.rule_name());
            }
        }

        if let Some(rule_name) = half_open_state {
            return Ok(CircuitBreakerStatus {
                status: Status::HalfOpen,
                start_ms: 0,
                circuit_breaker: rule_name,
                fallback_info: None,
                destroy: false,
            });
        }

        Ok(CircuitBreakerStatus {
            status: Status::Close,
            start_ms: 0,
            circuit_breaker: "".to_string(),
            fallback_info: None,
            destroy: false,
        })
    }
    /// report_stat 上报统计信息
    async fn report_stat(&self, stat: ResourceStat) -> Result<(), PoleError> {
        let rules = applicable_rules(&stat.resource, self.rules_snapshot());
        let mut states = self.states.lock().unwrap();
        let resource_states =
            Self::states_for_resource(&mut states, resource_key(&stat.resource), rules);
        for state in resource_states.iter_mut() {
            state.report_status(&stat.status);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::circuitbreaker::RetStatus;
    use pole_specification::v1::{
        rule_matcher, trigger_condition, BlockConfig, CircuitBreakerPolicy, CircuitBreakerRule,
        RecoverCondition, RuleMatcher, TriggerCondition,
    };

    fn consecutive_error_rule(error_count: u32) -> CircuitBreakerRule {
        CircuitBreakerRule {
            id: "cb-1".to_string(),
            name: "consecutive-error".to_string(),
            enable: true,
            block_configs: vec![CircuitBreakerPolicy {
                block_config: Some(BlockConfig {
                    trigger_conditions: vec![TriggerCondition {
                        trigger_type: trigger_condition::TriggerType::ConsecutiveError.into(),
                        error_count,
                        minimum_request: error_count,
                        interval: 60,
                        ..TriggerCondition::default()
                    }],
                    ..BlockConfig::default()
                }),
                ..CircuitBreakerPolicy::default()
            }],
            ..CircuitBreakerRule::default()
        }
    }

    fn recoverable_consecutive_error_rule(
        error_count: u32,
        sleep_window: u32,
        consecutive_success: u32,
    ) -> CircuitBreakerRule {
        let mut rule = consecutive_error_rule(error_count);
        rule.block_configs[0].recover_condition = Some(RecoverCondition {
            sleep_window,
            consecutive_success,
        });
        rule
    }

    fn service_resource(service: &str) -> Resource {
        Resource::ServiceResource(crate::core::model::circuitbreaker::ServiceResource::new(
            crate::core::model::naming::ServiceKey {
                namespace: "default".to_string(),
                name: service.to_string(),
            },
        ))
    }

    fn service_resource_with_caller(caller: &str, callee: &str) -> Resource {
        Resource::ServiceResource(
            crate::core::model::circuitbreaker::ServiceResource::new_waith_caller(
                crate::core::model::naming::ServiceKey {
                    namespace: "default".to_string(),
                    name: caller.to_string(),
                },
                crate::core::model::naming::ServiceKey {
                    namespace: "default".to_string(),
                    name: callee.to_string(),
                },
            ),
        )
    }

    fn matched_rule(name: &str, priority: u32, caller: &str, callee: &str) -> CircuitBreakerRule {
        let mut rule = consecutive_error_rule(1);
        rule.name = name.to_string();
        rule.priority = priority;
        rule.rule_matcher = Some(RuleMatcher {
            source: Some(rule_matcher::SourceService {
                namespace: "default".to_string(),
                service: caller.to_string(),
            }),
            destination: Some(rule_matcher::DestinationService {
                namespace: "default".to_string(),
                service: callee.to_string(),
                ..rule_matcher::DestinationService::default()
            }),
        });
        rule
    }

    #[test]
    fn rule_state_opens_after_consecutive_failures_reach_threshold() {
        let mut state = CircuitBreakerRuleState::new(consecutive_error_rule(2));

        state.report_status(&RetStatus::RetFail);
        assert_eq!(state.status(), Status::Close);

        state.report_status(&RetStatus::RetFail);
        assert_eq!(state.status(), Status::Open);
    }

    #[tokio::test]
    async fn composite_circuit_breaker_opens_after_reported_consecutive_failures() {
        let breaker = CompositeCircuitBreaker::new_with_rules(vec![consecutive_error_rule(2)]);

        breaker
            .report_stat(ResourceStat {
                resource: service_resource("svc-a"),
                ret_code: "500".to_string(),
                delay: std::time::Duration::from_millis(10),
                status: RetStatus::RetFail,
            })
            .await
            .unwrap();
        breaker
            .report_stat(ResourceStat {
                resource: service_resource("svc-a"),
                ret_code: "500".to_string(),
                delay: std::time::Duration::from_millis(10),
                status: RetStatus::RetFail,
            })
            .await
            .unwrap();

        let status = breaker
            .check_resource(service_resource("svc-a"))
            .await
            .unwrap();

        assert_eq!(status.status, Status::Open);
        assert_eq!(status.circuit_breaker, "consecutive-error");
    }

    #[tokio::test]
    async fn composite_circuit_breaker_applies_updated_remote_rules() {
        let breaker = CompositeCircuitBreaker::new();

        breaker.update_rules(vec![consecutive_error_rule(1)]);
        breaker
            .report_stat(ResourceStat {
                resource: service_resource("svc-a"),
                ret_code: "500".to_string(),
                delay: std::time::Duration::from_millis(10),
                status: RetStatus::RetFail,
            })
            .await
            .unwrap();

        let status = breaker
            .check_resource(service_resource("svc-a"))
            .await
            .unwrap();

        assert_eq!(status.status, Status::Open);
        assert_eq!(status.circuit_breaker, "consecutive-error");
    }

    #[tokio::test]
    async fn refreshing_identical_rules_preserves_consecutive_error_state() {
        let rule = consecutive_error_rule(2);
        let breaker = CompositeCircuitBreaker::new_with_rules(vec![rule.clone()]);

        breaker
            .report_stat(ResourceStat {
                resource: service_resource("svc-a"),
                ret_code: "500".to_string(),
                delay: std::time::Duration::from_millis(10),
                status: RetStatus::RetFail,
            })
            .await
            .unwrap();
        breaker.update_rules(vec![rule]);
        breaker
            .report_stat(ResourceStat {
                resource: service_resource("svc-a"),
                ret_code: "500".to_string(),
                delay: std::time::Duration::from_millis(10),
                status: RetStatus::RetFail,
            })
            .await
            .unwrap();

        let status = breaker
            .check_resource(service_resource("svc-a"))
            .await
            .unwrap();
        assert_eq!(status.status, Status::Open);
    }

    #[tokio::test]
    async fn circuit_breaker_filters_rule_matchers_and_applies_priority_stably() {
        let breaker = CompositeCircuitBreaker::new_with_rules(vec![
            matched_rule("unmatched", 0, "other-caller", "svc-a"),
            matched_rule("lower-priority", 20, "frontend", "svc-a"),
            matched_rule("higher-priority", 10, "frontend", "svc-a"),
        ]);

        breaker
            .report_stat(ResourceStat {
                resource: service_resource_with_caller("frontend", "svc-a"),
                ret_code: "500".to_string(),
                delay: std::time::Duration::from_millis(10),
                status: RetStatus::RetFail,
            })
            .await
            .unwrap();

        let status = breaker
            .check_resource(service_resource_with_caller("frontend", "svc-a"))
            .await
            .unwrap();
        assert_eq!(status.status, Status::Open);
        assert_eq!(status.circuit_breaker, "higher-priority");
    }

    #[tokio::test]
    async fn composite_circuit_breaker_keeps_states_isolated_by_resource() {
        let breaker = CompositeCircuitBreaker::new_with_rules(vec![consecutive_error_rule(1)]);

        breaker
            .report_stat(ResourceStat {
                resource: service_resource("svc-a"),
                ret_code: "500".to_string(),
                delay: std::time::Duration::from_millis(10),
                status: RetStatus::RetFail,
            })
            .await
            .unwrap();

        let opened = breaker
            .check_resource(service_resource("svc-a"))
            .await
            .unwrap();
        let isolated = breaker
            .check_resource(service_resource("svc-b"))
            .await
            .unwrap();

        assert_eq!(opened.status, Status::Open);
        assert_eq!(isolated.status, Status::Close);
    }

    #[tokio::test]
    async fn composite_circuit_breaker_recovers_after_half_open_successes() {
        let breaker =
            CompositeCircuitBreaker::new_with_rules(vec![recoverable_consecutive_error_rule(
                1, 0, 1,
            )]);

        breaker
            .report_stat(ResourceStat {
                resource: service_resource("svc-a"),
                ret_code: "500".to_string(),
                delay: std::time::Duration::from_millis(10),
                status: RetStatus::RetFail,
            })
            .await
            .unwrap();

        let half_open = breaker
            .check_resource(service_resource("svc-a"))
            .await
            .unwrap();
        assert_eq!(half_open.status, Status::HalfOpen);

        breaker
            .report_stat(ResourceStat {
                resource: service_resource("svc-a"),
                ret_code: "200".to_string(),
                delay: std::time::Duration::from_millis(10),
                status: RetStatus::RetSuccess,
            })
            .await
            .unwrap();

        let recovered = breaker
            .check_resource(service_resource("svc-a"))
            .await
            .unwrap();
        assert_eq!(recovered.status, Status::Close);
    }
}
