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

use super::helper::{match_label_value, route_traffic_match};
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

        let mut route_rules = Vec::new();
        for rule in rules {
            if let Some(config) = rule.routing_config {
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

    fn filter_instances(
        &self,
        rctx: &RouteContext,
        instances: &ServiceInstances,
        rules: Vec<CustomRouteRule>,
    ) -> Result<Vec<Instance>, PoleError> {
        for ele in rules {
            if !route_traffic_match(rctx, &ele) {
                continue;
            }
            // 匹配实例分组

            let destination = filter_available_destinations(ele.destinations);
            for (_, dest) in destination.iter().enumerate() {
                let ret = match_callee_group(dest, instances);
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

        // 匹配顺序 -> 先按照被调方路由规则匹配，然后再按照主调方规则进行匹配
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
                    RouteFailoverPolicy::All => Ok(RouteResult {
                        instances,
                        state: RouteState::Next,
                    }),
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
        caller_empty || callee_empty
    }
}

fn match_callee_group(dest: &DestinationGroup, instances: &ServiceInstances) -> Vec<Instance> {
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
                    .map(|actual| match_label_value(rule_value, actual.clone()))
                    .unwrap_or(false)
            })
        })
        .cloned()
        .collect()
}

fn filter_available_destinations(dests: Vec<DestinationGroup>) -> Vec<DestinationGroup> {
    let mut ret = Vec::<DestinationGroup>::with_capacity(dests.capacity());

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

        let matched = match_callee_group(&destination("v1-group", false, 0), &instances);

        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].id, "matched");
    }
}
