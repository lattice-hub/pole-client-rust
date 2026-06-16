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

use once_cell::sync::Lazy;
use std::collections::HashMap;

use crate::core::{
    config::consumer::ServiceRouterPluginConfig,
    model::{
        error::{ErrorCode, PoleError},
        naming::{Instance, Location, ServiceInstances},
        router::{RouteResult, RouteState, DEFAULT_ROUTER_NEARBY},
    },
    plugin::{
        location::LocationSupplier,
        plugins::Plugin,
        router::{RouteContext, ServiceRouter},
    },
};

static KEY_METADATA_NEARBY: &str = "internal-enable-nearby";
static DEFAULT_NEARBY_MATCH_LEVEL: &str = "zone";
static DEFAULT_NEARBY_MAX_MATCH_LEVEL: &str = "all";
static MATCH_LEVEL: Lazy<HashMap<String, u16>> = Lazy::new(|| {
    [
        ("unknown".to_string(), 0),
        ("campus".to_string(), 1),
        ("zone".to_string(), 2),
        ("region".to_string(), 3),
        ("all".to_string(), 4),
    ]
    .iter()
    .cloned()
    .collect()
});

static ORDER_MATCH_LEVEL: Lazy<HashMap<u16, String>> = Lazy::new(|| {
    [
        (0, "unknown".to_string()),
        (1, "campus".to_string()),
        (2, "zone".to_string()),
        (3, "region".to_string()),
        (4, "all".to_string()),
    ]
    .iter()
    .cloned()
    .collect()
});

pub fn new_service_router(conf: &ServiceRouterPluginConfig) -> Box<dyn ServiceRouter> {
    if conf.options.is_none() {
        // 默认就近区域：默认城市 matchLevel: zone # 最大就近区域，默认为空（全匹配） maxMatchLevel: all #
        // 假如开启了严格就近，插件的初始化会等待地域信息获取成功才返回，假如获取失败（server获取失败或者IP地域信息缺失），则会初始化失败，而且必须按照 strictNearby: false #
        // 是否启用按服务不健康实例比例进行降级 enableDegradeByUnhealthyPercent: true，假如不启用，则不会降级#
        // 需要进行降级的实例比例，不健康实例达到百分之多少才进行降级。值(0, 100]。 # 默认100，即全部不健康才进行切换。
        return Box::new(NearbyRouter {
            strict_nearby: false,
            match_level: "zone".to_string(),
            max_match_level: "all".to_string(),
            enable_degrade_unhealthy_percent: false,
            unhealthy_percent_to_degrade: 100,
        });
    }
    // #描述: 就近路由的最小匹配级别。region(大区)、zone(区域)、campus(园区)
    // matchLevel: zone
    // #描述: 最大匹配级别
    // maxMatchLevel: all
    // #描述: 强制就近
    // strictNearby: false
    // #描述: 全部实例不健康时是否降级其他地域
    // enableDegradeByUnhealthyPercent: false
    // #描述: 达到降级标准的不健康实例百分比
    // unhealthyPercentToDegrade: 100
    // #描述: 是否通过上报方式获取地域信息
    // enableReportLocalAddress: false
    let options = conf.options.clone().unwrap();
    Box::new(NearbyRouter {
        strict_nearby: options
            .get("strictNearby")
            .unwrap()
            .parse()
            .expect("strictNearby must be a boolean"),
        match_level: options.get("matchLevel").unwrap().to_string(),
        max_match_level: options.get("maxMatchLevel").unwrap().to_string(),
        enable_degrade_unhealthy_percent: options
            .get("enableDegradeByUnhealthyPercent")
            .unwrap()
            .parse()
            .expect("enableDegradeByUnhealthyPercent must be a boolean"),
        unhealthy_percent_to_degrade: options
            .get("unhealthyPercentToDegrade")
            .unwrap()
            .parse()
            .expect("unhealthyPercentToDegrade must be a number, range [0, 100]"),
    })
}

pub struct NearbyRouter {
    pub strict_nearby: bool,
    pub match_level: String,
    pub max_match_level: String,
    pub enable_degrade_unhealthy_percent: bool,
    pub unhealthy_percent_to_degrade: u16,
}

impl NearbyRouter {
    pub fn builder() -> (
        fn(&ServiceRouterPluginConfig) -> Box<dyn ServiceRouter>,
        String,
    ) {
        (new_service_router, DEFAULT_ROUTER_NEARBY.to_string())
    }

    fn select_instances(
        &self,
        local_loc: Location,
        match_level: &str,
        instances: &ServiceInstances,
    ) -> (ServiceInstances, u32) {
        let mut ret = Vec::<Instance>::with_capacity(instances.instances.len());

        let mut total_weight: u64 = 0;
        let mut health_ins_cnt = 0 as u32;
        for (_, ins) in instances.instances.iter().enumerate() {
            let matched = match match_level {
                "campus" => local_loc.campus == "" || ins.location.campus == local_loc.campus,
                "zone" => local_loc.zone == "" || ins.location.zone == local_loc.zone,
                "region" => local_loc.region == "" || ins.location.region == local_loc.region,
                _ => true,
            };
            if !matched {
                continue;
            }
            if ins.health {
                health_ins_cnt += 1;
            }
            total_weight += ins.weight as u64;
            ret.push(ins.clone());
        }

        (
            ServiceInstances {
                instances: ret,
                service: instances.service.clone(),
                total_weight: total_weight,
            },
            health_ins_cnt,
        )
    }

    fn should_degrade(
        &self,
        match_level: &str,
        instances: &ServiceInstances,
        health_cnt: u32,
    ) -> bool {
        if !self.enable_degrade_unhealthy_percent || match_level == DEFAULT_NEARBY_MAX_MATCH_LEVEL {
            return false;
        }
        if instances.instances.is_empty() {
            return true;
        }
        let unhealthy_cnt = instances.instances.len() as u32 - health_cnt;
        let unhealthy_percent = unhealthy_cnt * 100 / instances.instances.len() as u32;
        unhealthy_percent >= self.unhealthy_percent_to_degrade as u32
    }

    fn choose_instances_with_location(
        &self,
        location: Location,
        instances: ServiceInstances,
    ) -> Result<RouteResult, PoleError> {
        let mut min_available_level = self.match_level.clone();
        if min_available_level.is_empty() {
            min_available_level = DEFAULT_NEARBY_MATCH_LEVEL.to_string();
        }
        let mut max_match_level = if self.strict_nearby {
            min_available_level.clone()
        } else {
            self.max_match_level.clone()
        };
        if max_match_level.is_empty() {
            max_match_level = DEFAULT_NEARBY_MAX_MATCH_LEVEL.to_string();
        }

        if grater_match_level(min_available_level.as_str(), max_match_level.as_str()) {
            let (ret_ins, _health_cnt) =
                self.select_instances(location.clone(), min_available_level.as_str(), &instances);
            if ret_ins.instances.is_empty() {
                return Err(PoleError::new(ErrorCode::LocationMismatch, format!("")));
            }
            return Ok(RouteResult {
                instances: ret_ins,
                state: RouteState::Next,
            });
        }

        let min_level_ord = MATCH_LEVEL
            .get(min_available_level.as_str())
            .unwrap()
            .clone();
        let max_level_ord = MATCH_LEVEL.get(max_match_level.as_str()).unwrap().clone();

        let mut cur_level = min_available_level.clone();
        let mut ret_ins: Option<ServiceInstances> = None;
        for i in min_level_ord..=max_level_ord {
            let cur_match_level = ORDER_MATCH_LEVEL.get(&i).unwrap();
            let (tmp_ins, health_cnt) =
                self.select_instances(location.clone(), cur_match_level, &instances);
            cur_level = cur_match_level.to_string().clone();
            if tmp_ins.instances.is_empty() {
                continue;
            }
            let should_degrade = self.should_degrade(cur_match_level, &tmp_ins, health_cnt);
            ret_ins = Some(tmp_ins);
            if !should_degrade {
                break;
            }
        }

        if ret_ins.is_none() {
            return Err(PoleError::new(
                ErrorCode::LocationMismatch,
                format!("can not find any instance by level {}", cur_level),
            ));
        }

        Ok(RouteResult {
            instances: ret_ins.unwrap(),
            state: RouteState::Next,
        })
    }
}

impl Plugin for NearbyRouter {
    fn init(&mut self) {}

    fn destroy(&self) {}

    fn name(&self) -> String {
        DEFAULT_ROUTER_NEARBY.to_string()
    }
}

#[async_trait::async_trait]
impl ServiceRouter for NearbyRouter {
    /// choose_instances 实例路由
    async fn choose_instances(
        &self,
        route_info: RouteContext,
        instances: ServiceInstances,
    ) -> Result<RouteResult, PoleError> {
        let locatin_provider = route_info
            .extensions
            .clone()
            .unwrap()
            .get_location_provider();

        let location = locatin_provider.get_location();
        self.choose_instances_with_location(location, instances)
    }

    /// enable 是否启用
    async fn enable(&self, _route_info: RouteContext, instances: ServiceInstances) -> bool {
        let svc_info = instances.service.clone();
        let meta_val = svc_info.metadata.get(KEY_METADATA_NEARBY);
        if meta_val.is_none() {
            return false;
        }
        return meta_val.unwrap() != "true";
    }
}

fn grater_match_level(a: &str, b: &str) -> bool {
    let a_level = MATCH_LEVEL.get(a).unwrap();
    let b_level = MATCH_LEVEL.get(b).unwrap();
    a_level > b_level
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::naming::ServiceInfo;

    fn router(enable_degrade: bool, strict_nearby: bool) -> NearbyRouter {
        NearbyRouter {
            strict_nearby,
            match_level: "zone".to_string(),
            max_match_level: "all".to_string(),
            enable_degrade_unhealthy_percent: enable_degrade,
            unhealthy_percent_to_degrade: 100,
        }
    }

    fn location(region: &str, zone: &str, campus: &str) -> Location {
        Location {
            region: region.to_string(),
            zone: zone.to_string(),
            campus: campus.to_string(),
        }
    }

    fn instance(id: &str, region: &str, zone: &str, healthy: bool) -> Instance {
        Instance {
            id: id.to_string(),
            namespace: "default".to_string(),
            service: "svc-a".to_string(),
            health: healthy,
            weight: 100,
            location: location(region, zone, ""),
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

    fn selected_ids(result: RouteResult) -> Vec<String> {
        result
            .instances
            .instances
            .into_iter()
            .map(|ins| ins.id)
            .collect()
    }

    #[test]
    fn nearby_router_keeps_zone_when_healthy_instances_are_available() {
        let result = router(true, false)
            .choose_instances_with_location(
                location("r1", "z1", ""),
                service_instances(vec![
                    instance("zone-healthy", "r1", "z1", true),
                    instance("region-healthy", "r1", "z2", true),
                ]),
            )
            .expect("zone should match");

        assert_eq!(selected_ids(result), vec!["zone-healthy"]);
    }

    #[test]
    fn nearby_router_degrades_when_selected_level_is_fully_unhealthy() {
        let result = router(true, false)
            .choose_instances_with_location(
                location("r1", "z1", ""),
                service_instances(vec![
                    instance("zone-unhealthy", "r1", "z1", false),
                    instance("region-healthy", "r1", "z2", true),
                ]),
            )
            .expect("region should match after zone degradation");

        assert_eq!(
            selected_ids(result),
            vec!["zone-unhealthy", "region-healthy"]
        );
    }

    #[test]
    fn strict_nearby_router_does_not_degrade_to_wider_level() {
        let result = router(true, true)
            .choose_instances_with_location(
                location("r1", "z1", ""),
                service_instances(vec![
                    instance("zone-unhealthy", "r1", "z1", false),
                    instance("region-healthy", "r1", "z2", true),
                ]),
            )
            .expect("strict nearby should still return nearest level");

        assert_eq!(selected_ids(result), vec!["zone-unhealthy"]);
    }
}
