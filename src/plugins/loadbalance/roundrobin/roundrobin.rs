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

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicI64, AtomicU32},
        Arc, RwLock,
    },
    time::Instant,
};

use crate::core::{
    model::{
        error::{ErrorCode, PoleError},
        naming::Instance,
    },
    plugin::{loadbalance::LoadBalancer, plugins::Plugin},
};

static PLUGIN_NAME: &str = "weightedRoundRobin";

/// WeightedRoundRobinBalancer 权重轮训负载均衡
pub struct WeightedRoundRobinBalancer {
    round_robin_cache: Arc<RwLock<HashMap<String, WeightedRoundRobins>>>,
}

impl WeightedRoundRobinBalancer {
    pub fn builder() -> (fn() -> Box<dyn LoadBalancer>, String) {
        (new_instance, PLUGIN_NAME.to_string())
    }
}

fn new_instance() -> Box<dyn LoadBalancer> {
    Box::new(WeightedRoundRobinBalancer {
        round_robin_cache: Arc::new(RwLock::new(HashMap::new())),
    })
}

impl Plugin for WeightedRoundRobinBalancer {
    fn name(&self) -> String {
        PLUGIN_NAME.to_string()
    }

    fn init(&mut self) {}

    fn destroy(&self) {}
}

impl LoadBalancer for WeightedRoundRobinBalancer {
    fn choose_instance(
        &self,
        _criteria: crate::core::model::loadbalance::Criteria,
        instances: crate::core::model::naming::ServiceInstances,
    ) -> Result<crate::core::model::naming::Instance, crate::core::model::error::PoleError> {
        if instances.instances.is_empty() || instances.get_total_weight() == 0 {
            return Err(PoleError::new(
                ErrorCode::InstanceInfoError,
                "instances is empty or total weight is 0".to_string(),
            ));
        }

        let cache_key = instances.get_cache_key();
        {
            let mut round_robin_cache = self.round_robin_cache.write().unwrap();
            round_robin_cache
                .entry(cache_key.clone())
                .and_modify(|round_robins| {
                    for instance in instances.instances.iter() {
                        let mut inss_round_robin = round_robins.round_robins.write().unwrap();
                        let weight_robin = inss_round_robin
                            .entry(instance.id.clone())
                            .or_insert_with(|| WeightedRoundRobin::new(instance.weight));
                        if weight_robin.is_expire() {
                            round_robins
                                .round_robins
                                .write()
                                .unwrap()
                                .remove(&cache_key);
                            return;
                        }
                        // 如果没过期，但是实例数据出现变化，则更新
                        if weight_robin.get_ins_weight() != instance.weight {
                            weight_robin.reset(instance.weight);
                        }
                    }
                })
                .or_insert_with(|| WeightedRoundRobins::new(&instances.instances));
        }

        let mut selected_wrr: Option<WeightedRoundRobin> = None;
        let mut selected_ins: Option<&Instance> = None;
        let mut max_weight: i64 = i64::MIN;

        let svc_ins_cache_repo = self.round_robin_cache.read().unwrap();
        let svc_ins_cache = svc_ins_cache_repo.get(&cache_key).unwrap();
        for (_, ins) in instances.instances.iter().enumerate() {
            let inss_round_robin = svc_ins_cache.round_robins.write().unwrap();
            let weight_robin = inss_round_robin.get(&ins.id).unwrap();
            let cur_weight = weight_robin.increase_cur_weight();

            weight_robin.update_last_fetch();

            if cur_weight > max_weight {
                max_weight = cur_weight;
                selected_wrr = Some(weight_robin.clone());
                selected_ins = Some(ins);
            }
        }

        selected_wrr
            .expect("weighted round robin should select from non-empty instances")
            .decrease_cur_weight(instances.get_total_weight() as u32);
        Ok(selected_ins
            .expect("weighted round robin should select from non-empty instances")
            .clone())
    }
}

struct WeightedRoundRobins {
    round_robins: Arc<RwLock<HashMap<String, WeightedRoundRobin>>>,
}

impl WeightedRoundRobins {
    fn new(instances: &[Instance]) -> Self {
        Self {
            round_robins: Arc::new(RwLock::new(HashMap::from_iter(instances.iter().map(
                |instance| {
                    (
                        instance.id.clone(),
                        WeightedRoundRobin::new(instance.weight),
                    )
                },
            )))),
        }
    }
}

#[derive(Clone)]
struct WeightedRoundRobin {
    cur_weight: Arc<AtomicI64>,
    weight: Arc<AtomicU32>,
    last_fetch: Arc<RwLock<Instant>>,
}

impl WeightedRoundRobin {
    fn new(weight: u32) -> Self {
        Self {
            cur_weight: Arc::new(AtomicI64::new(0)),
            weight: Arc::new(AtomicU32::new(weight)),
            last_fetch: Arc::new(RwLock::new(Instant::now())),
        }
    }

    fn reset(&self, weight: u32) {
        self.cur_weight
            .store(0, std::sync::atomic::Ordering::Relaxed);
        self.weight
            .store(weight, std::sync::atomic::Ordering::Relaxed);
    }

    fn get_ins_weight(&self) -> u32 {
        self.weight.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn increase_cur_weight(&self) -> i64 {
        let weight = self.weight.load(std::sync::atomic::Ordering::Relaxed) as i64;
        self.cur_weight
            .fetch_add(weight, std::sync::atomic::Ordering::Relaxed)
            + weight
    }

    fn decrease_cur_weight(&self, weight: u32) {
        self.cur_weight
            .fetch_sub(weight as i64, std::sync::atomic::Ordering::Relaxed);
    }

    fn update_last_fetch(&self) {
        *self.last_fetch.write().unwrap() = Instant::now();
    }

    /// is_expire 超过 60s 未被使用则认为过期
    fn is_expire(&self) -> bool {
        self.last_fetch.read().unwrap().elapsed().as_secs() > 60
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{
        loadbalance::Criteria,
        naming::{Instance, ServiceInfo, ServiceInstances},
    };

    fn instance(id: &str, weight: u32) -> Instance {
        Instance {
            id: id.to_string(),
            namespace: "default".to_string(),
            service: "orders".to_string(),
            ip: format!("127.0.0.{}", weight),
            port: 8080 + weight,
            weight,
            health: true,
            ..Instance::default()
        }
    }

    fn service_instances() -> ServiceInstances {
        ServiceInstances::new(
            ServiceInfo {
                namespace: "default".to_string(),
                name: "orders".to_string(),
                revision: "rev-1".to_string(),
                ..ServiceInfo::default()
            },
            vec![instance("a", 1), instance("b", 2)],
        )
    }

    fn criteria() -> Criteria {
        Criteria {
            policy: "weightedRoundRobin".to_string(),
            hash_key: "".to_string(),
        }
    }

    #[test]
    fn weighted_round_robin_lifecycle_methods_do_not_panic() {
        let (supplier, _) = WeightedRoundRobinBalancer::builder();
        let mut balancer = supplier();

        balancer.init();
        balancer.destroy();
    }

    #[test]
    fn weighted_round_robin_first_call_initializes_cache_and_honors_weight() {
        let (supplier, _) = WeightedRoundRobinBalancer::builder();
        let balancer = supplier();

        let selected = (0..3)
            .map(|_| {
                balancer
                    .choose_instance(criteria(), service_instances())
                    .unwrap()
                    .id
            })
            .collect::<Vec<_>>();

        assert_eq!(selected.iter().filter(|id| id.as_str() == "a").count(), 1);
        assert_eq!(selected.iter().filter(|id| id.as_str() == "b").count(), 2);
    }
}
