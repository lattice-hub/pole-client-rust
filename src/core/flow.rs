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
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use tokio::{sync::RwLock, task::JoinHandle, time::sleep};

use crate::core::plugin::router::ServiceRouter;
use pole_specification::v1::CircuitBreakerRule;

use super::{
    model::{
        circuitbreaker::{CheckResult, CircuitBreakerStatus, Resource, ResourceStat, Status},
        error::PoleError,
        naming::ServiceInstances,
        router::RouteInfo,
        ClientContext, ReportClientRequest,
    },
    plugin::{
        loadbalance::LoadBalancer, location::LocationSupplier, plugins::Extensions,
        router::RouteContext,
    },
};

pub struct ClientFlow
where
    Self: Send + Sync,
{
    client: Arc<ClientContext>,
    extensions: Arc<Extensions>,

    futures: Vec<JoinHandle<()>>,

    closed: Arc<AtomicBool>,
}

impl ClientFlow {
    pub fn new(client: Arc<ClientContext>, extensions: Arc<Extensions>) -> Self {
        ClientFlow {
            client,
            extensions: extensions,
            futures: vec![],
            closed: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn run_flow(&mut self) {
        let client = self.client.clone();
        let extensions = self.extensions.clone();
        let is_closed = self.closed.clone();

        let f: JoinHandle<()> = self.extensions.runtime.spawn(async move {
            loop {
                ClientFlow::report_client(client.clone(), extensions.clone()).await;
                sleep(Duration::from_secs(60)).await;
                if is_closed.load(Ordering::Relaxed) {
                    return;
                }
            }
        });

        self.futures.push(f);
    }

    /// report_client 上报客户端信息数据
    pub async fn report_client(client: Arc<ClientContext>, extensions: Arc<Extensions>) {
        let server_connector = extensions.get_server_connector();
        let loc_provider = extensions.get_location_provider();
        let loc = loc_provider.get_location();
        let req = ReportClientRequest {
            client_id: client.client_id.clone(),
            host: client.host.clone(),
            version: client.version.clone(),
            location: loc,
        };
        let ret = server_connector.report_client(req).await;
        if let Err(e) = ret {
            crate::error!("report client failed: {:?}", e);
        }
    }

    pub fn stop_flow(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

/// CircuitBreakerFlow
pub struct CircuitBreakerFlow {
    extensions: Arc<Extensions>,
}

impl CircuitBreakerFlow {
    pub fn new(extensions: Arc<Extensions>) -> Self {
        CircuitBreakerFlow { extensions }
    }

    pub async fn check_resource(&self, resource: Resource) -> Result<CheckResult, PoleError> {
        let circuit_breaker_opt = self.extensions.circuit_breaker.clone();
        // 没有熔断插件时直接放行，保持 SDK 能在未启用治理插件时正常工作。
        if circuit_breaker_opt.is_none() {
            return Ok(CheckResult::pass());
        }

        let circuit_breaker = circuit_breaker_opt.unwrap();
        let status = circuit_breaker.check_resource(resource).await?;
        Ok(CircuitBreakerFlow::convert_from_status(status))
    }

    pub async fn report_stat(&self, stat: ResourceStat) -> Result<(), PoleError> {
        let circuit_breaker_opt = self.extensions.circuit_breaker.clone();
        if circuit_breaker_opt.is_none() {
            return Ok(());
        }

        let circuit_breaker = circuit_breaker_opt.unwrap();
        circuit_breaker.report_stat(stat).await
    }

    pub fn update_rules(&self, rules: Vec<CircuitBreakerRule>) {
        let Some(circuit_breaker) = self.extensions.circuit_breaker.clone() else {
            return;
        };
        // 规则刷新只传递到插件层，状态清理和规则解释由具体插件负责。
        circuit_breaker.update_rules(rules);
    }

    fn convert_from_status(ret: CircuitBreakerStatus) -> CheckResult {
        let status = ret.status;
        // Open 才阻断调用，HalfOpen 允许上层进行试探流量。
        CheckResult {
            pass: status != Status::Open,
            rule_name: ret.circuit_breaker,
            fallback_info: ret.fallback_info.clone(),
        }
    }
}

#[cfg(test)]
mod circuit_breaker_flow_tests {
    use super::*;

    fn status(status: Status) -> CircuitBreakerStatus {
        CircuitBreakerStatus {
            circuit_breaker: "rule-a".to_string(),
            status,
            start_ms: 0,
            fallback_info: None,
            destroy: false,
        }
    }

    #[test]
    fn convert_from_status_blocks_open_and_passes_close() {
        let open = CircuitBreakerFlow::convert_from_status(status(Status::Open));
        let close = CircuitBreakerFlow::convert_from_status(status(Status::Close));

        assert!(!open.pass);
        assert!(close.pass);
    }
}

/// RouterFlow 路由流程
pub struct RouterFlow {
    load_balancer: Arc<RwLock<HashMap<String, Arc<Box<dyn LoadBalancer>>>>>,
    extensions: Arc<Extensions>,
}

impl RouterFlow {
    pub fn new(extensions: Arc<Extensions>) -> Self {
        RouterFlow {
            load_balancer: extensions.get_loadbalancers(),
            extensions,
        }
    }

    /// lookup_loadbalancer 查找负载均衡器
    pub async fn lookup_loadbalancer(&self, name: &str) -> Option<Arc<Box<dyn LoadBalancer>>> {
        let lb = self.load_balancer.read().await;
        lb.get(name).cloned()
    }

    pub async fn choose_instances(
        &self,
        route_info: RouteInfo,
        instances: ServiceInstances,
    ) -> Result<ServiceInstances, PoleError> {
        let router_container = self.extensions.get_router_container();

        // RouterFlow 只编排路由链，不解释具体规则；每个 ServiceRouter
        // 通过 RouteContext 读取自身需要的缓存和流量标签。
        let route_ctx = RouteContext {
            route_info,
            extensions: Some(self.extensions.clone()),
            authenticated_caller: None,
        };

        let mut routers = Vec::<Arc<Box<dyn ServiceRouter>>>::new();
        let chain = &route_ctx.route_info.chain;

        // 前置路由通常用于健康、隔离等基础过滤，先于核心规则路由执行。
        chain.before.iter().for_each(|name| {
            if let Some(router) = router_container.before_routers.get(name) {
                routers.push(router.clone());
            }
        });

        let mut tmp_instance = instances;
        for (_, ele) in routers.iter().enumerate() {
            let ret = ele.choose_instances(route_ctx.clone(), tmp_instance).await;
            if let Err(e) = ret {
                return Err(e);
            }
            tmp_instance = ret.unwrap().instances;
        }
        routers.clear();

        // 核心路由承载规则路由、泳道等主要治理能力。
        chain.core.iter().for_each(|name| {
            if let Some(router) = router_container.core_routers.get(name) {
                routers.push(router.clone());
            }
        });

        for (_, ele) in routers.iter().enumerate() {
            let ret = ele.choose_instances(route_ctx.clone(), tmp_instance).await;
            if let Err(e) = ret {
                return Err(e);
            }
            tmp_instance = ret.unwrap().instances;
        }
        routers.clear();

        // 后置路由用于对核心结果继续收敛；每一段都以上一段输出作为输入。
        chain.after.iter().for_each(|name| {
            if let Some(router) = router_container.after_routers.get(name) {
                routers.push(router.clone());
            }
        });

        for (_, ele) in routers.iter().enumerate() {
            let ret = ele.choose_instances(route_ctx.clone(), tmp_instance).await;
            if let Err(e) = ret {
                return Err(e);
            }
            tmp_instance = ret.unwrap().instances;
        }

        Ok(tmp_instance)
    }
}

#[cfg(test)]
mod router_flow_tests {
    use std::sync::{Arc, Mutex};

    use crate::core::{
        model::{
            naming::ServiceInstances,
            router::{RouteInfo, RouteResult, RouteState, RouterChain},
        },
        plugin::{
            plugins::{test_extensions_with_router_container, Plugin},
            router::{RouteContext, RouterContainer, ServiceRouter},
        },
    };

    use super::RouterFlow;

    struct RecordingRouter {
        name: String,
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl RecordingRouter {
        fn new(name: &str, calls: Arc<Mutex<Vec<String>>>) -> Self {
            Self {
                name: name.to_string(),
                calls,
            }
        }
    }

    impl Plugin for RecordingRouter {
        fn init(&mut self) {}

        fn destroy(&self) {}

        fn name(&self) -> String {
            self.name.clone()
        }
    }

    #[async_trait::async_trait]
    impl ServiceRouter for RecordingRouter {
        async fn choose_instances(
            &self,
            _route_info: RouteContext,
            instances: ServiceInstances,
        ) -> Result<RouteResult, crate::core::model::error::PoleError> {
            self.calls.lock().unwrap().push(self.name.clone());
            Ok(RouteResult {
                state: RouteState::Next,
                instances,
            })
        }

        async fn enable(&self, _route_info: RouteContext, _instances: ServiceInstances) -> bool {
            true
        }
    }

    fn recording_router(name: &str, calls: Arc<Mutex<Vec<String>>>) -> Arc<Box<dyn ServiceRouter>> {
        Arc::new(Box::new(RecordingRouter::new(name, calls)))
    }

    #[test]
    fn choose_instances_runs_before_core_and_after_router_containers_in_order() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut container = RouterContainer::new();
        container.before_routers.insert(
            "before-a".to_string(),
            recording_router("before-a", calls.clone()),
        );
        container.core_routers.insert(
            "core-a".to_string(),
            recording_router("core-a", calls.clone()),
        );
        container.after_routers.insert(
            "after-a".to_string(),
            recording_router("after-a", calls.clone()),
        );
        let extensions = test_extensions_with_router_container(container);
        let flow = RouterFlow::new(extensions);

        tokio::runtime::Runtime::new().unwrap().block_on(async {
            flow.choose_instances(
                RouteInfo {
                    chain: RouterChain {
                        before: vec!["before-a".to_string()],
                        core: vec!["core-a".to_string()],
                        after: vec!["after-a".to_string()],
                    },
                    ..RouteInfo::default()
                },
                ServiceInstances::default(),
            )
            .await
            .unwrap();
        });

        assert_eq!(
            calls.lock().unwrap().as_slice(),
            ["before-a", "core-a", "after-a"]
        );
    }
}
