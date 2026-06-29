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

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock as StdRwLock};
use std::{collections::HashMap, time::Duration};

use tokio::sync::RwLock;
use tokio::task::JoinHandle;

use crate::core::context::SDKContext;
use crate::core::model::circuitbreaker::{MethodResource, Resource, ResourceStat, ServiceResource};
use crate::core::model::error::PoleError;
use crate::core::model::naming::{ServiceInstancesChangeEvent, ServiceKey};
use crate::core::plugin::cache::ResourceListener;
use crate::discovery::api::{ConsumerAPI, LosslessAPI, ProviderAPI};
use crate::discovery::req::{
    BaseInstance, GetAllInstanceRequest, GetHealthInstanceRequest, GetOneInstanceRequest,
    GetServiceRuleRequest, InstanceDeregisterRequest, InstanceHeartbeatRequest, InstanceProperties,
    InstanceRegisterRequest, InstanceRegisterResponse, InstancesResponse, LosslessActionProvider,
    ReportServiceContractRequest, ServiceCallResult, ServiceRuleResponse, WatchInstanceRequest,
};
use crate::traffic::router::api::RouterAPI;
use crate::traffic::router::default::DefaultRouterAPI;
use crate::traffic::router::req::{ProcessLoadBalanceRequest, ProcessRouteRequest};
use pole_specification::v1::{delay_register, DelayRegister, LosslessRule, Warmup};

use super::req::{InstanceResponse, WatchInstanceResponse};

struct InstanceWatcher {
    req: WatchInstanceRequest,
}

pub struct InstanceResourceListener {
    // watcher_id: 监听 listener key 的唯一标识
    watcher_id: Arc<AtomicU64>,
    // watchers: namespace#service -> InstanceWatcher
    watchers: Arc<RwLock<HashMap<String, HashMap<u64, InstanceWatcher>>>>,
}

impl InstanceResourceListener {
    pub async fn cancel_watch(&self, watch_key: &str, watch_id: u64) {
        let mut watchers = self.watchers.write().await;
        let items = watchers.get_mut(watch_key);
        if let Some(vals) = items {
            vals.remove(&watch_id);
        }
    }
}

#[async_trait::async_trait]
impl ResourceListener for InstanceResourceListener {
    async fn on_event(
        &self,
        _action: crate::core::plugin::cache::Action,
        val: crate::core::model::cache::ServerEvent,
    ) {
        let event_key = val.event_key;
        let mut watch_key = event_key.namespace.clone();
        let service = event_key.filter.get("service");
        watch_key.push('#');
        watch_key.push_str(service.unwrap().as_str());

        let watchers = self.watchers.read().await;
        if let Some(watchers) = watchers.get(&watch_key) {
            let ins_cache_opt = val.value.to_service_instances();
            match ins_cache_opt {
                Some(ins_cache_val) => {
                    for watcher in watchers {
                        (watcher.1.req.call_back)(ServiceInstancesChangeEvent {
                            service: ins_cache_val.get_service_info(),
                            instances: ins_cache_val.list_instances(false).await,
                        })
                    }
                }
                None => {
                    // do nothing
                }
            }
        }
    }

    fn watch_key(&self) -> crate::core::model::cache::EventType {
        crate::core::model::cache::EventType::Instance
    }
}

/// DefaultConsumerAPI
pub struct DefaultConsumerAPI {
    manage_sdk: bool,
    context: Arc<SDKContext>,
    router_api: Box<DefaultRouterAPI>,
    // watchers: namespace#service -> InstanceWatcher
    watchers: Arc<InstanceResourceListener>,
    // register_resource_watcher: 是否已经注册资源监听器
    register_resource_watcher: AtomicBool,
}

impl DefaultConsumerAPI {
    pub fn new_raw(context: SDKContext) -> Self {
        let ctx = Arc::new(context);
        Self {
            manage_sdk: true,
            context: ctx.clone(),
            router_api: Box::new(DefaultRouterAPI::new(ctx)),
            watchers: Arc::new(InstanceResourceListener {
                watcher_id: Arc::new(AtomicU64::new(0)),
                watchers: Arc::new(RwLock::new(HashMap::new())),
            }),
            register_resource_watcher: AtomicBool::new(false),
        }
    }

    pub fn new(context: Arc<SDKContext>) -> Self {
        Self {
            manage_sdk: true,
            context: context.clone(),
            router_api: Box::new(DefaultRouterAPI::new(context.clone())),
            watchers: Arc::new(InstanceResourceListener {
                watcher_id: Arc::new(AtomicU64::new(0)),
                watchers: Arc::new(RwLock::new(HashMap::new())),
            }),
            register_resource_watcher: AtomicBool::new(false),
        }
    }
}

impl Drop for DefaultConsumerAPI {
    fn drop(&mut self) {
        if !self.manage_sdk {
            return;
        }
        let ctx = self.context.to_owned();
        let ret = Arc::try_unwrap(ctx);

        match ret {
            Ok(ctx) => {
                drop(ctx);
            }
            Err(_) => {
                // do nothing
            }
        }
    }
}

#[async_trait::async_trait]
impl ConsumerAPI for DefaultConsumerAPI {
    async fn get_one_instance(
        &self,
        req: GetOneInstanceRequest,
    ) -> Result<InstanceResponse, PoleError> {
        let check_ret = req.check_valid();
        check_ret?;

        let engine = self.context.get_engine();
        let rsp = engine
            .get_service_instances(
                GetAllInstanceRequest {
                    flow_id: req.flow_id.clone(),
                    timeout: req.timeout,
                    service: req.service.clone(),
                    namespace: req.namespace.clone(),
                },
                true,
            )
            .await;

        // 重新设置被调服务数据信息
        let mut route_info = req.route_info;
        route_info.callee = ServiceKey {
            namespace: req.namespace.clone(),
            name: req.service.clone(),
        };

        match rsp {
            Ok(rsp) => {
                let instances = rsp.instances;
                let criteria = req.criteria;

                // 执行路由逻辑
                let route_ret = self
                    .router_api
                    .router(ProcessRouteRequest {
                        service_instances: instances,
                        route_info: route_info,
                        mirror_request: None,
                    })
                    .await;

                if route_ret.is_err() {
                    return Err(route_ret.err().unwrap());
                }
                let route_response = route_ret.unwrap();
                if let Some(response) = mock_instance_response(&route_response) {
                    return Ok(response);
                }

                // 执行负载均衡逻辑
                let balance_ret = self
                    .router_api
                    .load_balance(ProcessLoadBalanceRequest {
                        service_instances: route_response.service_instances,
                        criteria,
                    })
                    .await;

                if balance_ret.is_err() {
                    return Err(balance_ret.err().unwrap());
                }

                Ok(InstanceResponse {
                    instance: balance_ret.unwrap().instance,
                    mock_response: None,
                })
            }
            Err(e) => {
                return Err(e);
            }
        }
    }

    async fn get_health_instance(
        &self,
        req: GetHealthInstanceRequest,
    ) -> Result<InstancesResponse, PoleError> {
        let check_ret = req.check_valid();
        check_ret?;

        let engine = self.context.get_engine();
        let rsp: Result<InstancesResponse, PoleError> = engine
            .get_service_instances(
                GetAllInstanceRequest {
                    flow_id: req.flow_id,
                    timeout: req.timeout,
                    service: req.service,
                    namespace: req.namespace,
                },
                true,
            )
            .await;

        rsp
    }

    async fn get_all_instance(
        &self,
        req: GetAllInstanceRequest,
    ) -> Result<InstancesResponse, PoleError> {
        let check_ret = req.check_valid();
        check_ret?;

        let engine = self.context.get_engine();

        engine.get_service_instances(req, false).await
    }

    async fn watch_instance(
        &self,
        req: WatchInstanceRequest,
    ) -> Result<WatchInstanceResponse, PoleError> {
        if self
            .register_resource_watcher
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::SeqCst)
            .is_ok()
        {
            // 延迟注册资源监听器
            self.context
                .get_engine()
                .register_resource_listener(self.watchers.clone())
                .await;
        }

        let mut watchers = self.watchers.watchers.write().await;

        let watch_key = req.get_key();
        let items = watchers.entry(watch_key.clone()).or_insert(HashMap::new());
        let watch_id = self.watchers.watcher_id.fetch_add(1, Ordering::Relaxed);

        items.insert(watch_id, InstanceWatcher { req });
        Ok(WatchInstanceResponse::new(
            watch_id,
            watch_key,
            self.watchers.clone(),
        ))
    }

    async fn get_service_rule(
        &self,
        req: GetServiceRuleRequest,
    ) -> Result<ServiceRuleResponse, PoleError> {
        let engine = self.context.get_engine();
        engine.get_service_rule(req).await
    }

    async fn report_service_call(&self, req: ServiceCallResult) {
        let extensions = self.context.get_engine().get_extensions();
        let flow = crate::core::flow::CircuitBreakerFlow::new(extensions);
        let stat = ResourceStat {
            resource: service_call_result_resource(&req),
            ret_code: req.ret_code,
            delay: req.delay,
            status: req.status,
        };

        if let Err(err) = flow.report_stat(stat).await {
            crate::error!(
                "[pole][discovery][consumer] report service call failed: {:?}",
                err
            );
        }
    }
}

fn mock_instance_response(
    route_response: &crate::traffic::router::req::ProcessRouteResponse,
) -> Option<InstanceResponse> {
    route_response
        .traffic_governance
        .mock
        .as_ref()
        .map(|mock| InstanceResponse {
            instance: Default::default(),
            mock_response: Some(mock.clone()),
        })
}

fn service_call_result_resource(req: &ServiceCallResult) -> Resource {
    match (&req.protocol, &req.method, &req.path) {
        (Some(protocol), Some(method), Some(path)) if !path.is_empty() => {
            if let Some(caller) = &req.caller_service {
                Resource::MethodResource(MethodResource::new_waith_caller(
                    caller.clone(),
                    req.callee_service.clone(),
                    protocol.clone(),
                    method.clone(),
                    path.clone(),
                ))
            } else {
                Resource::MethodResource(MethodResource::new(
                    req.callee_service.clone(),
                    protocol.clone(),
                    method.clone(),
                    path.clone(),
                ))
            }
        }
        _ => {
            if let Some(caller) = &req.caller_service {
                Resource::ServiceResource(ServiceResource::new_waith_caller(
                    caller.clone(),
                    req.callee_service.clone(),
                ))
            } else {
                Resource::ServiceResource(ServiceResource::new(req.callee_service.clone()))
            }
        }
    }
}

/// DefaultProviderAPI
pub struct DefaultProviderAPI
where
    Self: Send + Sync,
{
    manage_sdk: bool,
    context: Arc<SDKContext>,
    beat_tasks: Arc<RwLock<HashMap<String, JoinHandle<()>>>>,
}

impl DefaultProviderAPI {
    pub fn new_raw(context: SDKContext) -> Self {
        Self {
            context: Arc::new(context),
            manage_sdk: true,
            beat_tasks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn new(context: Arc<SDKContext>) -> Self {
        Self {
            context,
            manage_sdk: false,
            beat_tasks: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Drop for DefaultProviderAPI {
    fn drop(&mut self) {
        if !self.manage_sdk {
            return;
        }
        let ctx = self.context.to_owned();
        let ret = Arc::try_unwrap(ctx);

        match ret {
            Ok(ctx) => {
                drop(ctx);
            }
            Err(_) => {
                // do nothing
            }
        }
    }
}

#[async_trait::async_trait]
impl ProviderAPI for DefaultProviderAPI {
    async fn register(
        &self,
        req: InstanceRegisterRequest,
    ) -> Result<InstanceRegisterResponse, PoleError> {
        let auto_heartbeat = req.auto_heartbeat;
        let ttl = req.ttl;
        let beat_req = req.to_heartbeat_request();
        crate::info!("[pole][discovery][provider] register instance request: {req:?}");
        let rsp = self.context.get_engine().register_instance(req).await;
        let engine = self.context.get_engine();
        if rsp.is_ok() && auto_heartbeat {
            let task_key = beat_req.beat_key();
            crate::info!(
                "[pole][discovery][heartbeat] add one auto_beat task={} duration={}s",
                task_key,
                ttl,
            );
            let beat_engine = engine.clone();
            // 开启了心跳自动上报功能，这里需要维护一个自动心跳上报的任务
            let handler = engine.get_executor().spawn(async move {
                loop {
                    tokio::time::sleep(tokio::time::Duration::from_secs(u64::from(ttl))).await;
                    crate::debug!(
                        "[pole][discovery][heartbeat] start to auto_beat instance: {beat_req:?}"
                    );
                    let beat_ret = beat_engine.instance_heartbeat(beat_req.clone()).await;
                    if let Err(e) = beat_ret {
                        crate::error!(
                            "[pole][discovery][heartbeat] auto_beat instance to server fail: {e}"
                        );
                    }
                }
            });
            self.beat_tasks.write().await.insert(task_key, handler);
        }
        return rsp;
    }

    async fn deregister(&self, req: InstanceDeregisterRequest) -> Result<(), PoleError> {
        let beat_req = req.to_heartbeat_request();
        let task_key = beat_req.beat_key();
        let wait_remove_task = self.beat_tasks.write().await.remove(&task_key);
        if let Some(task) = wait_remove_task {
            crate::info!(
                "[pole][discovery][heartbeat] remove one auto_beat task={}",
                task_key,
            );
            task.abort();
        }

        let engine = self.context.get_engine();
        engine.deregister_instance(req).await
    }

    async fn heartbeat(&self, req: InstanceHeartbeatRequest) -> Result<(), PoleError> {
        let engine = self.context.get_engine();
        engine.instance_heartbeat(req).await
    }

    async fn report_service_contract(
        &self,
        req: ReportServiceContractRequest,
    ) -> Result<(), PoleError> {
        self.context
            .get_engine()
            .report_service_contract(req)
            .await
            .map(|_| ())
    }

    async fn close(&mut self) {}
}

pub struct DefaultLosslessAPI {
    action_registry: Arc<LosslessActionRegistry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LosslessExecutionPlan {
    pub(crate) delay_register: Option<LosslessDelayRegisterPlan>,
    pub(crate) warmup: Option<LosslessWarmupPlan>,
    pub(crate) readiness_enabled: bool,
    pub(crate) offline_enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LosslessDelayRegisterPlan {
    DelayByTime(Duration),
    DelayByHealthCheck(LosslessHealthCheckPlan),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LosslessHealthCheckPlan {
    pub(crate) protocol: String,
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) interval: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LosslessWarmupPlan {
    pub(crate) interval: Duration,
    pub(crate) overload_protection_enabled: bool,
    pub(crate) overload_protection_threshold: i32,
    pub(crate) curvature: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LosslessEndpointResponse {
    pub(crate) status_code: u16,
    pub(crate) body: String,
}

pub(crate) struct LosslessEndpointState {
    readiness_enabled: bool,
    offline_enabled: bool,
    registered: AtomicBool,
    offline: AtomicBool,
}

impl LosslessEndpointState {
    pub(crate) fn new(readiness_enabled: bool, offline_enabled: bool) -> Self {
        Self {
            readiness_enabled,
            offline_enabled,
            registered: AtomicBool::new(!readiness_enabled),
            offline: AtomicBool::new(false),
        }
    }

    pub(crate) fn from_plan(plan: &LosslessExecutionPlan) -> Self {
        Self::new(plan.readiness_enabled, plan.offline_enabled)
    }

    pub(crate) fn mark_registered(&self) {
        self.offline.store(false, Ordering::SeqCst);
        self.registered.store(true, Ordering::SeqCst);
    }

    pub(crate) fn mark_offline(&self) {
        self.registered.store(false, Ordering::SeqCst);
        self.offline.store(true, Ordering::SeqCst);
    }

    pub(crate) fn is_offline(&self) -> bool {
        self.offline.load(Ordering::SeqCst)
    }

    pub(crate) fn handle_path(&self, path: &str) -> LosslessEndpointResponse {
        match path {
            "/readiness" => self.readiness_response(),
            "/offline" => self.offline_response(),
            _ => LosslessEndpointResponse {
                status_code: 404,
                body: "not found".to_string(),
            },
        }
    }

    fn readiness_response(&self) -> LosslessEndpointResponse {
        if !self.readiness_enabled {
            return LosslessEndpointResponse {
                status_code: 404,
                body: "readiness disabled".to_string(),
            };
        }

        if self.registered.load(Ordering::SeqCst) && !self.is_offline() {
            LosslessEndpointResponse {
                status_code: 200,
                body: "ready".to_string(),
            }
        } else {
            LosslessEndpointResponse {
                status_code: 503,
                body: "not ready".to_string(),
            }
        }
    }

    fn offline_response(&self) -> LosslessEndpointResponse {
        if !self.offline_enabled {
            return LosslessEndpointResponse {
                status_code: 404,
                body: "offline disabled".to_string(),
            };
        }

        self.mark_offline();
        LosslessEndpointResponse {
            status_code: 200,
            body: "offline".to_string(),
        }
    }
}

impl LosslessExecutionPlan {
    pub(crate) fn from_rule(rule: &LosslessRule) -> Self {
        let online = rule.lossless_online.as_ref();
        Self {
            delay_register: online.and_then(|online| {
                online
                    .delay_register
                    .as_ref()
                    .and_then(lossless_delay_register_plan)
            }),
            warmup: online.and_then(|online| online.warmup.as_ref().and_then(lossless_warmup_plan)),
            readiness_enabled: online
                .and_then(|online| online.readiness.as_ref())
                .map(|readiness| readiness.enable)
                .unwrap_or(false),
            offline_enabled: rule
                .lossless_offline
                .as_ref()
                .map(|offline| offline.enable)
                .unwrap_or(false),
        }
    }
}

fn lossless_delay_register_plan(delay: &DelayRegister) -> Option<LosslessDelayRegisterPlan> {
    if !delay.enable {
        return None;
    }

    match delay_register::DelayStrategy::try_from(delay.strategy)
        .unwrap_or(delay_register::DelayStrategy::DelayByTime)
    {
        delay_register::DelayStrategy::DelayByTime => Some(LosslessDelayRegisterPlan::DelayByTime(
            Duration::from_secs(delay.interval_second.max(0) as u64),
        )),
        delay_register::DelayStrategy::DelayByHealthCheck => Some(
            LosslessDelayRegisterPlan::DelayByHealthCheck(LosslessHealthCheckPlan {
                protocol: default_if_empty(&delay.health_check_protocol, "http"),
                method: default_if_empty(&delay.health_check_method, "GET"),
                path: delay.health_check_path.clone(),
                interval: Duration::from_secs(
                    delay
                        .health_check_interval_second
                        .parse::<u64>()
                        .unwrap_or(30)
                        .max(1),
                ),
            }),
        ),
    }
}

fn lossless_warmup_plan(warmup: &Warmup) -> Option<LosslessWarmupPlan> {
    if !warmup.enable {
        return None;
    }

    Some(LosslessWarmupPlan {
        interval: Duration::from_secs(warmup.interval_second.max(0) as u64),
        overload_protection_enabled: warmup.enable_overload_protection,
        overload_protection_threshold: warmup.overload_protection_threshold,
        curvature: warmup.curvature,
    })
}

fn default_if_empty(value: &str, default_value: &str) -> String {
    if value.is_empty() {
        default_value.to_string()
    } else {
        value.to_string()
    }
}

impl DefaultLosslessAPI {
    pub fn new(_context: SDKContext) -> Self {
        Self {
            action_registry: Arc::new(LosslessActionRegistry::new()),
        }
    }

    fn schedule_register_plan(
        &self,
        ins: Arc<dyn BaseInstance>,
        plan: LosslessExecutionPlan,
    ) -> JoinHandle<()> {
        self.action_registry
            .set_endpoint_state(Arc::new(LosslessEndpointState::from_plan(&plan)));
        schedule_lossless_register(self.action_registry.clone(), ins, plan)
    }
}

struct LosslessActionRegistry {
    action_providers: StdRwLock<HashMap<String, Arc<dyn LosslessActionProvider>>>,
    endpoint_state: StdRwLock<Arc<LosslessEndpointState>>,
}

impl LosslessActionRegistry {
    fn new() -> Self {
        Self {
            action_providers: StdRwLock::new(HashMap::new()),
            endpoint_state: StdRwLock::new(Arc::new(LosslessEndpointState::new(false, false))),
        }
    }

    fn set_action_provider(
        &self,
        ins: Arc<dyn BaseInstance>,
        action: Arc<dyn LosslessActionProvider>,
    ) {
        let mut providers = self.action_providers.write().unwrap();
        providers.insert(instance_key(ins.as_ref()), action);
    }

    fn set_endpoint_state(&self, endpoint_state: Arc<LosslessEndpointState>) {
        *self.endpoint_state.write().unwrap() = endpoint_state;
    }

    fn endpoint_state(&self) -> Arc<LosslessEndpointState> {
        self.endpoint_state.read().unwrap().clone()
    }

    fn lossless_register(&self, ins: Arc<dyn BaseInstance>) {
        if let Some(action) = self.find_action_provider(ins.as_ref()) {
            action.do_register(InstanceProperties {});
        }
        self.endpoint_state().mark_registered();
    }

    fn lossless_deregister(&self, ins: Arc<dyn BaseInstance>) {
        if let Some(action) = self.find_action_provider(ins.as_ref()) {
            action.do_deregister();
        }
        self.endpoint_state().mark_offline();
    }

    fn handle_endpoint_path(
        &self,
        path: &str,
        ins: Arc<dyn BaseInstance>,
    ) -> LosslessEndpointResponse {
        let endpoint_state = self.endpoint_state();
        if path == "/offline" {
            let response = endpoint_state.handle_path(path);
            if response.status_code == 200 {
                self.lossless_deregister(ins);
            }
            return response;
        }

        endpoint_state.handle_path(path)
    }

    fn lossless_healthcheck_ready(&self, ins: &dyn BaseInstance) -> bool {
        let Some(action) = self.find_action_provider(ins) else {
            return true;
        };
        if !action.is_enable_healthcheck() {
            return true;
        }
        action.do_healthcheck()
    }

    fn find_action_provider(
        &self,
        ins: &dyn BaseInstance,
    ) -> Option<Arc<dyn LosslessActionProvider>> {
        let providers = self.action_providers.read().unwrap();
        providers.get(&instance_key(ins)).cloned()
    }
}

fn instance_key(ins: &dyn BaseInstance) -> String {
    format!(
        "{}#{}#{}#{}",
        ins.get_namespace(),
        ins.get_service(),
        ins.get_ip(),
        ins.get_port()
    )
}

fn schedule_lossless_register(
    registry: Arc<LosslessActionRegistry>,
    ins: Arc<dyn BaseInstance>,
    plan: LosslessExecutionPlan,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        match plan.delay_register {
            Some(LosslessDelayRegisterPlan::DelayByTime(delay)) => {
                tokio::time::sleep(delay).await;
            }
            Some(LosslessDelayRegisterPlan::DelayByHealthCheck(health_check)) => {
                while !registry.lossless_healthcheck_ready(ins.as_ref()) {
                    tokio::time::sleep(health_check.interval).await;
                }
            }
            None => {}
        }
        registry.lossless_register(ins);
    })
}

fn spawn_lossless_endpoint_server(
    listener: tokio::net::TcpListener,
    registry: Arc<LosslessActionRegistry>,
    ins: Arc<dyn BaseInstance>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let registry = registry.clone();
            let ins = ins.clone();
            tokio::spawn(async move {
                if let Err(err) = handle_lossless_endpoint_stream(stream, registry, ins).await {
                    crate::error!("[lossless][endpoint] handle request failed: {:?}", err);
                }
            });
        }
    })
}

async fn handle_lossless_endpoint_stream(
    mut stream: tokio::net::TcpStream,
    registry: Arc<LosslessActionRegistry>,
    ins: Arc<dyn BaseInstance>,
) -> std::io::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut buf = vec![0_u8; 1024];
    let n = stream.read(&mut buf).await?;
    let path = parse_http_request_path(&buf[..n]).unwrap_or("/");
    let response = registry.handle_endpoint_path(path, ins);
    let status_text = match response.status_code {
        200 => "OK",
        404 => "Not Found",
        503 => "Service Unavailable",
        _ => "OK",
    };
    let body = response.body;
    let raw_response = format!(
        "HTTP/1.1 {} {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        response.status_code,
        status_text,
        body.len(),
        body
    );
    stream.write_all(raw_response.as_bytes()).await
}

fn parse_http_request_path(req: &[u8]) -> Option<&str> {
    let first_line = std::str::from_utf8(req).ok()?.lines().next()?;
    first_line.split_whitespace().nth(1)
}

impl LosslessAPI for DefaultLosslessAPI {
    fn set_action_provider(
        &self,
        ins: Arc<dyn BaseInstance>,
        action: Arc<dyn LosslessActionProvider>,
    ) {
        self.action_registry.set_action_provider(ins, action);
    }

    fn lossless_register(&self, ins: Arc<dyn BaseInstance>) {
        self.action_registry.lossless_register(ins);
    }

    fn lossless_deregister(&self, ins: Arc<dyn BaseInstance>) {
        self.action_registry.lossless_deregister(ins);
    }

    fn schedule_register(&self, ins: Arc<dyn BaseInstance>, rule: &LosslessRule) -> JoinHandle<()> {
        self.schedule_register_plan(ins, LosslessExecutionPlan::from_rule(rule))
    }

    fn serve_endpoint(
        &self,
        listener: tokio::net::TcpListener,
        ins: Arc<dyn BaseInstance>,
    ) -> JoinHandle<()> {
        spawn_lossless_endpoint_server(listener, self.action_registry.clone(), ins)
    }
}

#[cfg(test)]
mod lossless_tests {
    use super::*;
    use crate::discovery::req::InstanceProperties;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    struct TestInstance {
        namespace: String,
        service: String,
        ip: String,
        port: u32,
    }

    impl BaseInstance for TestInstance {
        fn get_namespace(&self) -> String {
            self.namespace.clone()
        }

        fn get_service(&self) -> String {
            self.service.clone()
        }

        fn get_ip(&self) -> String {
            self.ip.clone()
        }

        fn get_port(&self) -> u32 {
            self.port
        }
    }

    struct RecordingAction {
        register_count: AtomicUsize,
        deregister_count: AtomicUsize,
        health_results: Mutex<VecDeque<bool>>,
    }

    impl RecordingAction {
        fn new() -> Self {
            Self {
                register_count: AtomicUsize::new(0),
                deregister_count: AtomicUsize::new(0),
                health_results: Mutex::new(VecDeque::new()),
            }
        }

        fn with_health_results(results: impl IntoIterator<Item = bool>) -> Self {
            Self {
                register_count: AtomicUsize::new(0),
                deregister_count: AtomicUsize::new(0),
                health_results: Mutex::new(results.into_iter().collect()),
            }
        }
    }

    impl LosslessActionProvider for RecordingAction {
        fn get_name(&self) -> String {
            "recording".to_string()
        }

        fn do_register(&self, _prop: InstanceProperties) {
            self.register_count.fetch_add(1, Ordering::SeqCst);
        }

        fn do_deregister(&self) {
            self.deregister_count.fetch_add(1, Ordering::SeqCst);
        }

        fn is_enable_healthcheck(&self) -> bool {
            !self.health_results.lock().unwrap().is_empty()
        }

        fn do_healthcheck(&self) -> bool {
            self.health_results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(true)
        }
    }

    fn test_instance() -> Arc<dyn BaseInstance> {
        Arc::new(TestInstance {
            namespace: "default".to_string(),
            service: "svc-a".to_string(),
            ip: "127.0.0.1".to_string(),
            port: 8080,
        })
    }

    #[test]
    fn lossless_action_registry_invokes_bound_provider() {
        let registry = LosslessActionRegistry::new();
        let instance = test_instance();
        let action = Arc::new(RecordingAction::new());

        registry.set_action_provider(instance.clone(), action.clone());
        registry.lossless_register(instance.clone());
        registry.lossless_deregister(instance);

        assert_eq!(action.register_count.load(Ordering::SeqCst), 1);
        assert_eq!(action.deregister_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn lossless_endpoint_state_tracks_readiness_and_offline_paths() {
        let state = LosslessEndpointState::new(true, true);

        assert_eq!(state.handle_path("/readiness").status_code, 503);

        state.mark_registered();
        assert_eq!(state.handle_path("/readiness").status_code, 200);

        let offline = state.handle_path("/offline");
        assert_eq!(offline.status_code, 200);
        assert!(state.is_offline());
        assert_eq!(state.handle_path("/readiness").status_code, 503);
        assert_eq!(state.handle_path("/missing").status_code, 404);
    }

    #[test]
    fn lossless_registry_offline_endpoint_triggers_deregister() {
        let registry = LosslessActionRegistry::new();
        let endpoint_state = Arc::new(LosslessEndpointState::new(true, true));
        registry.set_endpoint_state(endpoint_state);
        let instance = test_instance();
        let action = Arc::new(RecordingAction::new());
        registry.set_action_provider(instance.clone(), action.clone());

        registry.lossless_register(instance.clone());
        assert_eq!(
            registry
                .handle_endpoint_path("/readiness", instance.clone())
                .status_code,
            200
        );

        let offline = registry.handle_endpoint_path("/offline", instance);

        assert_eq!(offline.status_code, 200);
        assert_eq!(action.deregister_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            registry
                .handle_endpoint_path("/readiness", test_instance())
                .status_code,
            503
        );
    }

    #[tokio::test]
    async fn lossless_endpoint_server_exposes_readiness_and_offline_paths() {
        let registry = Arc::new(LosslessActionRegistry::new());
        registry.set_endpoint_state(Arc::new(LosslessEndpointState::new(true, true)));
        let instance = test_instance();
        let action = Arc::new(RecordingAction::new());
        registry.set_action_provider(instance.clone(), action.clone());
        registry.lossless_register(instance.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = spawn_lossless_endpoint_server(listener, registry, instance);

        assert_eq!(http_status(addr, "/readiness").await, 200);
        assert_eq!(http_status(addr, "/offline").await, 200);
        assert_eq!(action.deregister_count.load(Ordering::SeqCst), 1);
        assert_eq!(http_status(addr, "/readiness").await, 503);

        handle.abort();
    }

    async fn http_status(addr: std::net::SocketAddr, path: &str) -> u16 {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(format!("GET {} HTTP/1.1\r\nHost: localhost\r\n\r\n", path).as_bytes())
            .await
            .unwrap();
        let mut buf = vec![0_u8; 256];
        let n = stream.read(&mut buf).await.unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        response
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse::<u16>()
            .unwrap()
    }

    #[test]
    fn lossless_execution_plan_enables_delay_by_time_readiness_warmup_and_offline() {
        let rule = pole_specification::v1::LosslessRule {
            lossless_online: Some(pole_specification::v1::LosslessOnline {
                delay_register: Some(pole_specification::v1::DelayRegister {
                    enable: true,
                    strategy: pole_specification::v1::delay_register::DelayStrategy::DelayByTime
                        .into(),
                    interval_second: 15,
                    ..pole_specification::v1::DelayRegister::default()
                }),
                warmup: Some(pole_specification::v1::Warmup {
                    enable: true,
                    interval_second: 60,
                    enable_overload_protection: true,
                    overload_protection_threshold: 40,
                    curvature: 2,
                }),
                readiness: Some(pole_specification::v1::Readiness { enable: true }),
            }),
            lossless_offline: Some(pole_specification::v1::LosslessOffline { enable: true }),
            ..pole_specification::v1::LosslessRule::default()
        };

        let plan = LosslessExecutionPlan::from_rule(&rule);

        assert_eq!(
            plan.delay_register,
            Some(LosslessDelayRegisterPlan::DelayByTime(Duration::from_secs(
                15
            )))
        );
        assert_eq!(
            plan.warmup,
            Some(LosslessWarmupPlan {
                interval: Duration::from_secs(60),
                overload_protection_enabled: true,
                overload_protection_threshold: 40,
                curvature: 2,
            })
        );
        assert!(plan.readiness_enabled);
        assert!(plan.offline_enabled);
    }

    #[test]
    fn lossless_execution_plan_preserves_delay_by_health_check_probe() {
        let rule = pole_specification::v1::LosslessRule {
            lossless_online: Some(pole_specification::v1::LosslessOnline {
                delay_register: Some(pole_specification::v1::DelayRegister {
                    enable: true,
                    strategy:
                        pole_specification::v1::delay_register::DelayStrategy::DelayByHealthCheck
                            .into(),
                    health_check_protocol: "http".to_string(),
                    health_check_method: "GET".to_string(),
                    health_check_path: "/ready".to_string(),
                    health_check_interval_second: "5".to_string(),
                    ..pole_specification::v1::DelayRegister::default()
                }),
                ..pole_specification::v1::LosslessOnline::default()
            }),
            ..pole_specification::v1::LosslessRule::default()
        };

        let plan = LosslessExecutionPlan::from_rule(&rule);

        assert_eq!(
            plan.delay_register,
            Some(LosslessDelayRegisterPlan::DelayByHealthCheck(
                LosslessHealthCheckPlan {
                    protocol: "http".to_string(),
                    method: "GET".to_string(),
                    path: "/ready".to_string(),
                    interval: Duration::from_secs(5),
                }
            ))
        );
    }

    #[tokio::test]
    async fn lossless_scheduler_delays_register_until_plan_delay_elapsed() {
        let registry = Arc::new(LosslessActionRegistry::new());
        let instance = test_instance();
        let action = Arc::new(RecordingAction::new());
        registry.set_action_provider(instance.clone(), action.clone());

        let plan = LosslessExecutionPlan {
            delay_register: Some(LosslessDelayRegisterPlan::DelayByTime(
                Duration::from_millis(20),
            )),
            warmup: None,
            readiness_enabled: false,
            offline_enabled: false,
        };

        let handle = schedule_lossless_register(registry, instance, plan);
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert_eq!(action.register_count.load(Ordering::SeqCst), 0);

        handle.await.unwrap();

        assert_eq!(action.register_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn lossless_scheduler_waits_for_healthcheck_success_before_registering() {
        let registry = Arc::new(LosslessActionRegistry::new());
        let instance = test_instance();
        let action = Arc::new(RecordingAction::with_health_results([false, false, true]));
        registry.set_action_provider(instance.clone(), action.clone());

        let plan = LosslessExecutionPlan {
            delay_register: Some(LosslessDelayRegisterPlan::DelayByHealthCheck(
                LosslessHealthCheckPlan {
                    protocol: "http".to_string(),
                    method: "GET".to_string(),
                    path: "/ready".to_string(),
                    interval: Duration::from_millis(10),
                },
            )),
            warmup: None,
            readiness_enabled: false,
            offline_enabled: false,
        };

        let start = Instant::now();
        let handle = schedule_lossless_register(registry, instance, plan);
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert_eq!(action.register_count.load(Ordering::SeqCst), 0);

        handle.await.unwrap();

        assert!(start.elapsed() >= Duration::from_millis(20));
        assert_eq!(action.register_count.load(Ordering::SeqCst), 1);
    }
}

#[cfg(test)]
mod provider_tests {
    use super::*;
    use crate::core::config::config::Configuration;
    use crate::core::context::SDKContext;
    use crate::core::model::naming::ServiceContract;

    fn test_configuration() -> Configuration {
        serde_yaml::from_str(
            r#"
global:
  api:
    timeout: 1ms
    maxRetryTimes: 1
    retryInterval: 1ms
    reportInterval: 1s
  serverConnectors:
    addresses:
      - discover://127.0.0.1:1
    protocol: grpc
    connectTimeout: 1ms
    serverSwitchInterval: 1s
    messageTimeout: 1ms
    connectionIdleTimeout: 1s
    reconnectInterval: 1ms
  statReporter:
    enable: false
  location:
    providers:
      - name: local
        options: {}
  client:
    id: test-client
    labels: {}
consumer:
  serviceRouter:
    beforeChain: []
    coreChain: []
    afterChain: []
  circuitBreaker:
    enable: false
    enableRemotePull: false
  loadBalancer:
    defaultPolicy: weightedRandom
    plugins: []
  localCache:
    name: memory
    serviceExpireEnable: false
    serviceExpireTime: 1s
    serviceRefreshInterval: 1s
    serviceListRefreshInterval: 1s
    persistEnable: false
    persistDir: ./target/test-cache
provider:
  rateLimit:
    enable: false
    service: pole.limiter
    namespace: Pole
    maxWindowCount: 1
    fallbackOnExceedWindowCount: pass
    remoteSyncTimeout: 1ms
    maxQueuingTime: 1ms
    reportMetrics: false
  lossless:
    enable: false
    host: 127.0.0.1
    port: 0
    delayRegisterInterval: 1ms
    healthCheckInterval: 1ms
config:
  propertiesValueCacheSize: 1
  propertiesValueExpireTime: 1
  configFilter:
    enable: false
    chain: []
    plugin: {}
"#,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn provider_report_service_contract_delegates_without_panicking() {
        let context = SDKContext::create_by_configuration(test_configuration()).unwrap();
        let provider = DefaultProviderAPI::new(Arc::new(context));

        let result = provider
            .report_service_contract(ReportServiceContractRequest {
                flow_id: "flow-a".to_string(),
                timeout: Duration::from_millis(1),
                contract: ServiceContract {
                    name: "contract-a".to_string(),
                    namespace: "default".to_string(),
                    service: "svc-a".to_string(),
                    version: "v1".to_string(),
                    protocol: "http".to_string(),
                    content: "{}".to_string(),
                    interfaces: Vec::new(),
                    metadata: HashMap::new(),
                },
            })
            .await;

        assert!(result.is_err());
    }
}

#[cfg(test)]
mod consumer_tests {
    use super::*;
    use crate::core::config::config::Configuration;
    use crate::core::context::SDKContext;
    use crate::core::model::circuitbreaker::{Resource, RetStatus, ServiceResource, Status};
    use crate::core::model::naming::ServiceKey;
    use crate::traffic::router::req::ProcessRouteResponse;
    use pole_specification::v1::MockResponse;
    use pole_specification::v1::{
        trigger_condition, BlockConfig, CircuitBreakerPolicy, CircuitBreakerRule, TriggerCondition,
    };

    fn test_configuration() -> Configuration {
        serde_yaml::from_str(
            r#"
global:
  api:
    timeout: 1ms
    maxRetryTimes: 1
    retryInterval: 1ms
    reportInterval: 1s
  serverConnectors:
    addresses:
      - discover://127.0.0.1:1
    protocol: grpc
    connectTimeout: 1ms
    serverSwitchInterval: 1s
    messageTimeout: 1ms
    connectionIdleTimeout: 1s
    reconnectInterval: 1ms
  statReporter:
    enable: false
  location:
    providers:
      - name: local
        options: {}
  client:
    id: test-client
    labels: {}
consumer:
  serviceRouter:
    beforeChain: []
    coreChain: []
    afterChain: []
  circuitBreaker:
    enable: true
    enableRemotePull: false
  loadBalancer:
    defaultPolicy: weightedRandom
    plugins: []
  localCache:
    name: memory
    serviceExpireEnable: false
    serviceExpireTime: 1s
    serviceRefreshInterval: 1s
    serviceListRefreshInterval: 1s
    persistEnable: false
    persistDir: ./target/test-cache
provider:
  rateLimit:
    enable: false
    service: pole.limiter
    namespace: Pole
    maxWindowCount: 1
    fallbackOnExceedWindowCount: pass
    remoteSyncTimeout: 1ms
    maxQueuingTime: 1ms
    reportMetrics: false
  lossless:
    enable: false
    host: 127.0.0.1
    port: 0
    delayRegisterInterval: 1ms
    healthCheckInterval: 1ms
config:
  propertiesValueCacheSize: 1
  propertiesValueExpireTime: 1
  configFilter:
    enable: false
    chain: []
    plugin: {}
"#,
        )
        .unwrap()
    }

    fn consecutive_error_rule(error_count: u32) -> CircuitBreakerRule {
        CircuitBreakerRule {
            id: "cb-1".to_string(),
            name: "consumer-report-consecutive-error".to_string(),
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

    #[tokio::test]
    async fn consumer_report_service_call_updates_circuit_breaker_stats() {
        let context = Arc::new(SDKContext::create_by_configuration(test_configuration()).unwrap());
        let consumer = DefaultConsumerAPI::new(context.clone());
        let extensions = context.get_engine().get_extensions();
        let circuit_breaker = extensions.circuit_breaker.as_ref().unwrap().clone();
        circuit_breaker.update_rules(vec![consecutive_error_rule(2)]);

        let caller = ServiceKey {
            namespace: "default".to_string(),
            name: "frontend".to_string(),
        };
        let callee = ServiceKey {
            namespace: "default".to_string(),
            name: "orders".to_string(),
        };

        for _ in 0..2 {
            consumer
                .report_service_call(ServiceCallResult {
                    caller_service: Some(caller.clone()),
                    callee_service: callee.clone(),
                    delay: Duration::from_millis(10),
                    ret_code: "500".to_string(),
                    status: RetStatus::RetFail,
                    protocol: None,
                    method: None,
                    path: None,
                })
                .await;
        }

        let status = circuit_breaker
            .check_resource(Resource::ServiceResource(
                ServiceResource::new_waith_caller(caller, callee),
            ))
            .await
            .unwrap();

        assert_eq!(status.status, Status::Open);
        assert_eq!(status.circuit_breaker, "consumer-report-consecutive-error");
    }

    #[test]
    fn service_call_result_resource_uses_method_when_method_fields_present() {
        let caller = ServiceKey {
            namespace: "default".to_string(),
            name: "frontend".to_string(),
        };
        let callee = ServiceKey {
            namespace: "default".to_string(),
            name: "orders".to_string(),
        };

        let resource = service_call_result_resource(&ServiceCallResult {
            caller_service: Some(caller.clone()),
            callee_service: callee.clone(),
            delay: Duration::from_millis(3),
            ret_code: "0".to_string(),
            status: RetStatus::RetSuccess,
            protocol: Some("http".to_string()),
            method: Some("GET".to_string()),
            path: Some("/v1/orders".to_string()),
        });

        match resource {
            Resource::MethodResource(resource) => {
                assert_eq!(resource.caller, Some(caller));
                assert_eq!(resource.callee, callee);
                assert_eq!(resource.protocol, "http");
                assert_eq!(resource.method, "GET");
                assert_eq!(resource.path, "/v1/orders");
            }
            Resource::ServiceResource(_) | Resource::InstanceResource(_) => {
                panic!("expected method resource")
            }
        }
    }

    #[test]
    fn consumer_mock_route_response_returns_mock_without_instance() {
        let mock = MockResponse {
            code: "MOCKED".to_string(),
            body: "mocked".to_string(),
            ..MockResponse::default()
        };
        let route_response = ProcessRouteResponse {
            service_instances: Default::default(),
            traffic_governance: crate::traffic::policy::req::TrafficGovernanceResult {
                mock: Some(mock.clone()),
                ..Default::default()
            },
        };

        let response = mock_instance_response(&route_response)
            .expect("mock route response should short circuit");

        assert!(response.instance.id.is_empty());
        assert!(response.instance.service.is_empty());
        assert_eq!(response.mock_response, Some(mock));
    }
}
