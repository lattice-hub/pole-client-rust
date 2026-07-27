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
    config::consumer::ServiceRouterPluginConfig,
    model::{
        cache::{EventType, ResourceEventKey},
        error::PoleError,
        naming::{ServiceInstances, ServiceRule},
        router::{RouteResult, RouteState, DEFAULT_ROUTER_LANE},
    },
    plugin::{
        cache::Filter,
        plugins::Extensions,
        plugins::Plugin,
        router::{RouteContext, ServiceRouter},
    },
};
use pole_specification::v1::{lane_rule, LaneGroup, LaneRule};
use std::{collections::HashMap, sync::Arc, time::Duration};

use super::super::rule::helper::traffic_match_rule_match;

pub fn new_service_router(_conf: &ServiceRouterPluginConfig) -> Box<dyn ServiceRouter> {
    Box::new(LaneRouter {})
}

pub struct LaneRouter {}

impl LaneRouter {
    pub fn builder() -> (
        fn(&ServiceRouterPluginConfig) -> Box<dyn ServiceRouter>,
        String,
    ) {
        (new_service_router, DEFAULT_ROUTER_LANE.to_string())
    }

    async fn fetch_lane_groups(
        &self,
        extensions: Arc<Extensions>,
        route_ctx: &RouteContext,
    ) -> Result<Vec<LaneGroup>, PoleError> {
        let local_cache = extensions.get_resource_cache();
        let callee = &route_ctx.route_info.callee;

        // 泳道规则以被调服务为发布和缓存主键，主调条件在 LaneRule 的
        // traffic_match_rule 中表达。
        let mut filter = HashMap::<String, String>::new();
        filter.insert("service".to_string(), callee.name.clone());
        let service_rule = local_cache
            .load_service_rule(Filter {
                resource_key: ResourceEventKey {
                    namespace: callee.namespace.clone(),
                    event_type: EventType::LaneRule,
                    filter,
                },
                internal_request: false,
                include_cache: true,
                timeout: Duration::from_secs(1),
            })
            .await?;

        lane_groups_from_service_rule(service_rule)
    }
}

impl Plugin for LaneRouter {
    fn init(&mut self) {}

    fn destroy(&self) {}

    fn name(&self) -> String {
        DEFAULT_ROUTER_LANE.to_string()
    }
}

#[async_trait::async_trait]
impl ServiceRouter for LaneRouter {
    /// choose_instances 实例路由
    async fn choose_instances(
        &self,
        route_info: RouteContext,
        instances: ServiceInstances,
    ) -> Result<RouteResult, PoleError> {
        let Some(extensions) = route_info.extensions.clone() else {
            return Ok(RouteResult {
                instances,
                state: RouteState::Next,
            });
        };
        let lane_groups = self.fetch_lane_groups(extensions, &route_info).await?;
        if lane_groups.is_empty() {
            return Ok(RouteResult {
                instances,
                state: RouteState::Next,
            });
        }

        let instances = filter_instances_by_lane_groups(&route_info, &instances, &lane_groups);
        Ok(RouteResult {
            instances,
            state: RouteState::Next,
        })
    }

    /// enable 是否启用
    async fn enable(&self, route_info: RouteContext, _instances: ServiceInstances) -> bool {
        if !route_info.route_info.chain.exist_route(DEFAULT_ROUTER_LANE) {
            return false;
        }
        let Some(extensions) = route_info.extensions.clone() else {
            return false;
        };

        self.fetch_lane_groups(extensions, &route_info)
            .await
            .map(|rules| !rules.is_empty())
            .unwrap_or(false)
    }
}

fn lane_groups_from_service_rule(service_rule: ServiceRule) -> Result<Vec<LaneGroup>, PoleError> {
    // ResourceCache 统一返回 Any，这里只接受 LaneGroup，避免路由链误用其它规则。
    let mut groups = Vec::with_capacity(service_rule.rules.len());
    for rule in service_rule.rules {
        let type_id = rule.type_id();
        match rule.downcast::<LaneGroup>() {
            Ok(group) => groups.push(*group),
            Err(_) => {
                return Err(PoleError::new(
                    crate::core::model::error::ErrorCode::InvalidRule,
                    format!("rule type error, expect LaneGroup, but got {:?}", type_id),
                ));
            }
        }
    }

    Ok(groups)
}

fn filter_instances_by_lane_groups(
    route_ctx: &RouteContext,
    instances: &ServiceInstances,
    lane_groups: &[LaneGroup],
) -> ServiceInstances {
    // 多个泳道组最终展开为 LaneRule 列表，并按 priority 从小到大尝试命中。
    let mut rules = lane_groups
        .iter()
        .flat_map(|group| group.rules.iter())
        .filter(|rule| rule.enable)
        .collect::<Vec<&LaneRule>>();
    rules.sort_by(|a, b| a.priority.cmp(&b.priority));

    for rule in rules {
        let Some(traffic_match_rule) = &rule.traffic_match_rule else {
            continue;
        };
        // 泳道先匹配请求流量，再用实例 metadata 上的泳道标签选目标实例。
        if !traffic_match_rule_match(route_ctx, traffic_match_rule) {
            continue;
        }

        // spec 未显式配置 label_key 时使用默认 lane 标签，兼容最常见的泳道模型。
        let label_key = if rule.label_key.is_empty() {
            "lane"
        } else {
            rule.label_key.as_str()
        };
        let filtered = instances
            .instances
            .iter()
            .filter(|instance| instance.is_available())
            .filter(|instance| {
                instance
                    .metadata
                    .get(label_key)
                    .map(|lane| lane == &rule.default_label_value)
                    .unwrap_or(false)
            })
            .cloned()
            .collect::<Vec<_>>();

        if !filtered.is_empty() {
            return ServiceInstances::new(instances.service.clone(), filtered);
        }

        // 流量命中泳道规则但没有可用泳道实例时，Permissive 回退全量实例，
        // Strict 返回空实例，交由上层路由链处理无实例结果。
        return match rule.match_mode() {
            lane_rule::LaneMatchMode::Permissive => instances.clone(),
            lane_rule::LaneMatchMode::Strict => {
                ServiceInstances::new(instances.service.clone(), vec![])
            }
        };
    }

    instances.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{
        naming::{Instance, ServiceInfo, ServiceRule},
        ArgumentType,
    };
    use pole_specification::v1::{
        lane_rule,
        match_string::{MatchStringType, ValueType},
        source_match, traffic_match_rule, LaneGroup, LaneRule, MatchString, SourceMatch,
        TrafficMatchRule,
    };
    use std::collections::HashMap;

    fn traffic_label_provider(arg_type: ArgumentType, key: &str) -> Option<String> {
        match (arg_type, key) {
            (ArgumentType::Header, "x-lane") => Some("blue".to_string()),
            _ => None,
        }
    }

    fn route_ctx() -> RouteContext {
        let mut route_info = crate::core::model::router::RouteInfo::default();
        route_info.traffic_label_provider = traffic_label_provider;
        RouteContext {
            route_info,
            extensions: None,
            authenticated_caller: None,
        }
    }

    fn lane_rule(match_mode: lane_rule::LaneMatchMode) -> LaneRule {
        LaneRule {
            name: "blue-rule".to_string(),
            enable: true,
            default_label_value: "blue".to_string(),
            label_key: "lane".to_string(),
            match_mode: match_mode.into(),
            traffic_match_rule: Some(TrafficMatchRule {
                arguments: vec![SourceMatch {
                    r#type: source_match::Type::Header.into(),
                    key: "x-lane".to_string(),
                    value: Some(MatchString {
                        r#type: MatchStringType::Exact.into(),
                        value: "blue".to_string(),
                        value_type: ValueType::Text.into(),
                    }),
                }],
                random_percent: 100,
                match_mode: traffic_match_rule::TrafficMatchMode::And.into(),
            }),
            ..LaneRule::default()
        }
    }

    fn lane_group(rule: LaneRule) -> LaneGroup {
        LaneGroup {
            name: "blue".to_string(),
            rules: vec![rule],
            ..LaneGroup::default()
        }
    }

    fn instance(id: &str, lane: &str, healthy: bool) -> Instance {
        Instance {
            id: id.to_string(),
            namespace: "default".to_string(),
            service: "svc-a".to_string(),
            health: healthy,
            weight: 100,
            metadata: HashMap::from([("lane".to_string(), lane.to_string())]),
            ..Instance::default()
        }
    }

    fn service_instances(instances: Vec<Instance>) -> ServiceInstances {
        ServiceInstances::new(
            ServiceInfo {
                namespace: "default".to_string(),
                name: "svc-a".to_string(),
                ..ServiceInfo::default()
            },
            instances,
        )
    }

    #[test]
    fn lane_filter_returns_matching_available_instances() {
        let instances = service_instances(vec![
            instance("blue", "blue", true),
            instance("gray", "gray", true),
            instance("unhealthy-blue", "blue", false),
        ]);

        let filtered = filter_instances_by_lane_groups(
            &route_ctx(),
            &instances,
            &[lane_group(lane_rule(lane_rule::LaneMatchMode::Strict))],
        );

        assert_eq!(filtered.instances.len(), 1);
        assert_eq!(filtered.instances[0].id, "blue");
        assert_eq!(filtered.total_weight, 100);
    }

    #[test]
    fn strict_lane_filter_returns_empty_when_rule_matches_but_no_lane_instance() {
        let instances = service_instances(vec![instance("gray", "gray", true)]);

        let filtered = filter_instances_by_lane_groups(
            &route_ctx(),
            &instances,
            &[lane_group(lane_rule(lane_rule::LaneMatchMode::Strict))],
        );

        assert!(filtered.instances.is_empty());
    }

    #[test]
    fn permissive_lane_filter_falls_back_when_rule_matches_but_no_lane_instance() {
        let instances = service_instances(vec![instance("gray", "gray", true)]);

        let filtered = filter_instances_by_lane_groups(
            &route_ctx(),
            &instances,
            &[lane_group(lane_rule(lane_rule::LaneMatchMode::Permissive))],
        );

        assert_eq!(filtered.instances.len(), 1);
        assert_eq!(filtered.instances[0].id, "gray");
    }

    #[test]
    fn lane_groups_from_service_rule_downcasts_cached_groups() {
        let service_rule = ServiceRule {
            rules: vec![Box::new(lane_group(lane_rule(
                lane_rule::LaneMatchMode::Strict,
            )))],
            revision: "rev-1".to_string(),
            initialized: true,
        };

        let groups = lane_groups_from_service_rule(service_rule).unwrap();

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "blue");
    }
}
