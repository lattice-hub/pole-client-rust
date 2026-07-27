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

use std::{any::Any, collections::HashMap, sync::Arc, time::Duration};

use pole_specification::v1::{CustomRoute, CustomRouteRule, DestinationGroup, RouteRule};
use prost::Message;

use super::helper::{
    match_label_value, route_traffic_match, traffic_match_rule_request_parameters,
};
use crate::core::{
    config::consumer::ServiceRouterPluginConfig,
    model::{
        cache::{EventType, ResourceEventKey},
        error::{ErrorCode, PoleError},
        naming::{Instance, ServiceInstances},
        router::{RouteResult, RouteState, DEFAULT_ROUTER_RULE},
    },
    plugin::{
        cache::Filter,
        plugins::{Extensions, Plugin},
        router::{RouteContext, ServiceRouter},
    },
};
use crate::warn;

#[derive(Debug, PartialEq, Eq)]
pub enum Direction {
    Callee,
    Caller,
}

#[derive(Debug, PartialEq, Eq)]
enum RouteFailoverPolicy {
    All,
    None,
}

#[derive(Debug, PartialEq, Eq)]
enum RuleStatus {
    // 无路由策略
    NoRule,
    // 被调服务路由策略匹配成功
    DestRuleSucc,
    // 被调服务路由策略匹配失败
    DestRuleFail,
    // 主调服务路由策略匹配成功
    SourceRuleSucc,
    // 主调服务路由策略匹配失败
    SourceRuleFail,
}

pub fn new_service_router(_conf: &ServiceRouterPluginConfig) -> Box<dyn ServiceRouter> {
    let mut policy = RouteFailoverPolicy::All;
    if let Some(opt) = _conf.options.clone() {
        let val = opt.get("failover");
        if let Some(val) = val {
            if val == "none" {
                policy = RouteFailoverPolicy::None;
            }
        }
    }
    Box::new(RuleRouter {
        failover_policy: policy,
    })
}

pub struct RuleRouter {
    failover_policy: RouteFailoverPolicy,
}

impl RuleRouter {
    pub fn builder() -> (
        fn(&ServiceRouterPluginConfig) -> Box<dyn ServiceRouter>,
        String,
    ) {
        (new_service_router, DEFAULT_ROUTER_RULE.to_string())
    }
}

impl Plugin for RuleRouter {
    fn init(&mut self) {}

    fn destroy(&self) {}

    fn name(&self) -> String {
        DEFAULT_ROUTER_RULE.to_string()
    }
}

impl RuleRouter {
    async fn fetch_rule(
        &self,
        extensions: Arc<Extensions>,
        rctx: &RouteContext,
        dir: Direction,
    ) -> Result<Vec<CustomRouteRule>, PoleError> {
        let local_cache = extensions.get_resource_cache();

        // 规则路由支持两类规则来源：被调服务规则优先，其次才是主调服务规则。
        // 因此这里按 Direction 选择不同服务键去读取 RouterRule。
        let mut ns = &rctx.route_info.caller.namespace;
        let mut svc = &rctx.route_info.caller.name;
        if dir == Direction::Callee {
            ns = &rctx.route_info.callee.namespace;
            svc = &rctx.route_info.callee.name;
        }

        let mut filter = HashMap::<String, String>::new();
        filter.insert("service".to_string(), svc.to_string());
        let ret = local_cache
            .load_service_rule(Filter {
                resource_key: ResourceEventKey {
                    namespace: ns.to_string(),
                    event_type: EventType::RouterRule,
                    filter,
                },
                internal_request: false,
                include_cache: true,
                timeout: Duration::from_secs(1),
            })
            .await;

        if ret.is_err() {
            return Err(ret.err().unwrap());
        }
        let ret = ret.unwrap();

        let mut rules = Vec::<Box<RouteRule>>::with_capacity(ret.rules.len());
        for ele in ret.rules {
            // RouterRule 通过 Any 从 ResourceCache 传递，这里完成类型边界校验。
            let type_id = ele.type_id();
            match ele.downcast::<RouteRule>() {
                Ok(rule) => rules.push(rule),
                Err(_) => {
                    return Err(PoleError::new(
                        ErrorCode::InvalidRule,
                        format!("rule type error, expect RouteRule, but got {:?}", type_id),
                    ));
                }
            }
        }

        decode_custom_route_rules(rules)
    }

    fn filter_instances(
        &self,
        rctx: &RouteContext,
        instances: &ServiceInstances,
        rules: Vec<CustomRouteRule>,
    ) -> Result<Vec<Instance>, PoleError> {
        for ele in rules {
            // 路由规则先按流量条件命中，再按目的分组优先级筛选可用实例。
            if !route_traffic_match(rctx, &ele) {
                continue;
            }

            let destination = filter_available_destinations(ele.destinations);
            let request_parameters = ele
                .arguments
                .as_ref()
                .map(|rule| traffic_match_rule_request_parameters(rctx, rule))
                .unwrap_or_default();
            for (_, dest) in destination.iter().enumerate() {
                let ret = match_callee_group(dest, instances, &request_parameters);
                if ret.is_empty() {
                    continue;
                }
                // 返回目标实例分组结果
                return Ok(ret);
            }
            // 没有符合的实例分组，需要看下兜底逻辑
        }
        // 返回空实例列表
        Ok(vec![])
    }
}

#[async_trait::async_trait]
impl ServiceRouter for RuleRouter {
    /// choose_instances 实例路由
    async fn choose_instances(
        &self,
        route_ctx: RouteContext,
        instances: ServiceInstances,
    ) -> Result<RouteResult, PoleError> {
        let extensions = route_ctx.extensions.clone().unwrap();

        // 匹配顺序：被调方规则优先，只有未命中时才回退到主调方规则。
        // 这和控制面路由规则的作用域一致，避免主调规则覆盖服务自身规则。
        let mut filtered_ins = Option::<Vec<Instance>>::None;

        let mut status = RuleStatus::NoRule;
        let callee_rules = self
            .fetch_rule(extensions.clone(), &route_ctx, Direction::Callee)
            .await?;
        if !callee_rules.is_empty() {
            status = RuleStatus::DestRuleSucc;
            let ret = self.filter_instances(&route_ctx, &instances, callee_rules)?;
            if ret.is_empty() {
                status = RuleStatus::DestRuleFail;
            } else {
                filtered_ins = Some(ret);
            }
        }

        // 如果被调服务路由规则匹配失败，则判断主调方的路由规则
        if status != RuleStatus::DestRuleSucc {
            let caller_rules = self
                .fetch_rule(extensions, &route_ctx, Direction::Caller)
                .await?;
            if !caller_rules.is_empty() {
                status = RuleStatus::SourceRuleSucc;
                let ret = self.filter_instances(&route_ctx, &instances, caller_rules)?;
                if ret.is_empty() {
                    status = RuleStatus::SourceRuleFail;
                } else {
                    filtered_ins = Some(ret);
                }
            }
        }

        match status {
            RuleStatus::NoRule => Ok(RouteResult {
                instances,
                state: RouteState::Next,
            }),
            RuleStatus::DestRuleSucc | RuleStatus::SourceRuleSucc => {
                // 重新计算总权重，后续负载均衡只能看到过滤后的实例集合。
                let mut total_weight = 0 as u64;
                let filtered_ins = filtered_ins.unwrap();
                for ele in filtered_ins.iter() {
                    total_weight += ele.weight as u64;
                }
                Ok(RouteResult {
                    instances: ServiceInstances {
                        service: instances.service.clone(),
                        instances: filtered_ins,
                        total_weight: total_weight,
                    },
                    state: RouteState::Next,
                })
            }
            _ => {
                warn!(
                    "[router][rule] route rule not match, rule status: {:?}, not matched callee:{:?} caller:{:?}",
                    status,
                    route_ctx.route_info.caller,
                    route_ctx.route_info.callee,
                );
                match self.failover_policy {
                    // All 表示路由规则未命中时退回原始实例集，保持可用性优先。
                    RouteFailoverPolicy::All => Ok(RouteResult {
                        instances,
                        state: RouteState::Next,
                    }),
                    // None 表示严格执行路由规则，未命中时返回空实例集。
                    RouteFailoverPolicy::None => Ok(RouteResult {
                        instances: ServiceInstances {
                            service: instances.service.clone(),
                            instances: vec![],
                            total_weight: 0,
                        },
                        state: RouteState::Next,
                    }),
                }
            }
        }
    }

    /// enable 是否启用
    async fn enable(&self, route_ctx: RouteContext, _instances: ServiceInstances) -> bool {
        let chain = &route_ctx.route_info.chain;
        let has_router = chain.exist_route(DEFAULT_ROUTER_RULE);
        if !has_router {
            return false;
        }

        let caller_ret = self
            .fetch_rule(
                route_ctx.extensions.clone().unwrap(),
                &route_ctx,
                Direction::Caller,
            )
            .await;
        if caller_ret.is_err() {
            return false;
        }
        let caller_empty = caller_ret.unwrap().is_empty();

        let callee_ret = self
            .fetch_rule(
                route_ctx.extensions.clone().unwrap(),
                &route_ctx,
                Direction::Callee,
            )
            .await;
        if callee_ret.is_err() {
            return false;
        }
        let callee_empty = callee_ret.unwrap().is_empty();

        // 其中一个有规则即可
        has_route_rules(!caller_empty, !callee_empty)
    }
}

fn has_route_rules(caller_has_rules: bool, callee_has_rules: bool) -> bool {
    caller_has_rules || callee_has_rules
}

fn decode_custom_route_rules(
    mut rules: Vec<Box<RouteRule>>,
) -> Result<Vec<CustomRouteRule>, PoleError> {
    rules.retain(|rule| rule.enable);
    rules.sort_by(|left, right| left.priority.cmp(&right.priority));

    let mut route_rules = Vec::new();
    for rule in rules {
        if let Some(config) = rule.routing_config {
            // spec 中 RouteRule.routing_config 承载 prost Any 编码的 CustomRoute。
            let custom_route = CustomRoute::decode(config.value.as_slice()).map_err(|err| {
                PoleError::new(
                    ErrorCode::InvalidRule,
                    format!("decode custom route rule fail: {err}"),
                )
            })?;
            route_rules.extend(custom_route.rules);
        }
    }
    Ok(route_rules)
}

fn match_callee_group(
    dest: &DestinationGroup,
    instances: &ServiceInstances,
    request_parameters: &std::collections::HashMap<String, String>,
) -> Vec<Instance> {
    // 目的分组通过服务、命名空间和实例 metadata 标签共同筛选实例。
    instances
        .instances
        .iter()
        .filter(|instance| instance.is_available())
        .filter(|instance| {
            if dest.service != "*" && dest.service != instance.service {
                return false;
            }
            if dest.namespace != "*" && dest.namespace != instance.namespace {
                return false;
            }
            dest.labels.iter().all(|(key, rule_value)| {
                instance
                    .metadata
                    .get(key)
                    .map(|actual| {
                        // PARAMETER + 空 value 表示用同名请求参数动态筛选实例标签。
                        // 只有已在 TrafficMatchRule 中显式采集的参数可参与路由，避免
                        // 目标标签无意读取未声明的请求数据。
                        if rule_value.value_type()
                            == pole_specification::v1::match_string::ValueType::Parameter
                            && rule_value.value.is_empty()
                        {
                            return request_parameters
                                .get(key)
                                .map(|value| value == actual)
                                .unwrap_or(false);
                        }
                        match_label_value(rule_value, actual.clone())
                    })
                    .unwrap_or(false)
            })
        })
        .cloned()
        .collect()
}

fn filter_available_destinations(dests: Vec<DestinationGroup>) -> Vec<DestinationGroup> {
    let mut ret = Vec::<DestinationGroup>::with_capacity(dests.capacity());

    // isolate 的目的分组不参与正常流量选择，仅保留未隔离分组。
    for ele in dests {
        if !ele.isolate {
            ret.push(ele);
        }
    }

    // 优先级按照 0 -> 1 -> 2 -> 3 -> 4 -> 5 -> 6 -> 7 -> 8 -> 9 以此类推
    ret.sort_by(|a, b| {
        let a_priority = a.priority;
        let b_priority = b.priority;
        a_priority.cmp(&b_priority)
    });

    ret
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::naming::ServiceInfo;
    use pole_specification::v1::{
        match_string::{MatchStringType, ValueType},
        MatchString,
    };

    fn instance(id: &str, version: &str, healthy: bool) -> Instance {
        Instance {
            id: id.to_string(),
            namespace: "default".to_string(),
            service: "svc-a".to_string(),
            health: healthy,
            weight: 100,
            metadata: HashMap::from([("version".to_string(), version.to_string())]),
            ..Instance::default()
        }
    }

    fn destination(name: &str, isolate: bool, priority: u32) -> DestinationGroup {
        DestinationGroup {
            name: name.to_string(),
            service: "svc-a".to_string(),
            namespace: "default".to_string(),
            labels: HashMap::from([(
                "version".to_string(),
                MatchString {
                    r#type: MatchStringType::Exact.into(),
                    value: "v1".to_string(),
                    value_type: ValueType::Text.into(),
                },
            )]),
            isolate,
            priority,
            ..DestinationGroup::default()
        }
    }

    fn outer_route_rule(name: &str, enable: bool, priority: u32) -> Box<RouteRule> {
        Box::new(RouteRule {
            name: format!("outer-{name}"),
            enable,
            priority,
            routing_config: Some(prost_types::Any {
                type_url: "type.googleapis.com/pole.io.CustomRoute".to_string(),
                value: CustomRoute {
                    rules: vec![CustomRouteRule {
                        name: name.to_string(),
                        ..CustomRouteRule::default()
                    }],
                    ..CustomRoute::default()
                }
                .encode_to_vec(),
            }),
            ..RouteRule::default()
        })
    }

    #[test]
    fn filter_available_destinations_excludes_isolated_and_sorts_by_priority() {
        let destinations = filter_available_destinations(vec![
            destination("isolated", true, 0),
            destination("low-priority", false, 8),
            destination("high-priority", false, 1),
        ]);

        assert_eq!(destinations.len(), 2);
        assert_eq!(destinations[0].name, "high-priority");
        assert_eq!(destinations[1].name, "low-priority");
    }

    #[test]
    fn match_callee_group_filters_available_instances_by_destination_labels() {
        let instances = ServiceInstances::new(
            ServiceInfo {
                namespace: "default".to_string(),
                name: "svc-a".to_string(),
                ..ServiceInfo::default()
            },
            vec![
                instance("matched", "v1", true),
                instance("wrong-version", "v2", true),
                instance("unhealthy", "v1", false),
            ],
        );

        let matched = match_callee_group(
            &destination("v1-group", false, 0),
            &instances,
            &std::collections::HashMap::new(),
        );

        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].id, "matched");
    }

    #[test]
    fn match_callee_group_uses_captured_request_parameter_for_same_named_label() {
        let instances = ServiceInstances::new(
            ServiceInfo {
                namespace: "default".to_string(),
                name: "svc-a".to_string(),
                ..ServiceInfo::default()
            },
            vec![
                instance("tenant-a", "tenant-a", true),
                instance("tenant-b", "tenant-b", true),
            ],
        );
        let mut dynamic_destination = destination("dynamic-version", false, 0);
        dynamic_destination.labels.insert(
            "version".to_string(),
            MatchString {
                r#type: MatchStringType::Exact.into(),
                value: String::new(),
                value_type: ValueType::Parameter.into(),
            },
        );

        let matched = match_callee_group(
            &dynamic_destination,
            &instances,
            &HashMap::from([("version".to_string(), "tenant-b".to_string())]),
        );

        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].id, "tenant-b");
    }

    #[test]
    fn rule_router_is_enabled_when_either_side_has_rules() {
        assert!(has_route_rules(false, true));
        assert!(has_route_rules(true, false));
        assert!(has_route_rules(true, true));
        assert!(!has_route_rules(false, false));
    }

    #[test]
    fn outer_route_rules_filter_disabled_rules_and_stably_apply_priority() {
        let rules = decode_custom_route_rules(vec![
            outer_route_rule("late", true, 20),
            outer_route_rule("first-same-priority", true, 10),
            outer_route_rule("disabled", false, 0),
            outer_route_rule("second-same-priority", true, 10),
        ])
        .unwrap();

        assert_eq!(
            rules
                .iter()
                .map(|rule| rule.name.as_str())
                .collect::<Vec<_>>(),
            vec!["first-same-priority", "second-same-priority", "late"]
        );
    }
}
