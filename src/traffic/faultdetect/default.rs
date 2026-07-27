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

use pole_specification::v1::{fault_detect_rule, FaultDetectRule};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpStream, UdpSocket},
    sync::OnceCell,
    task::JoinHandle,
    time::timeout,
};

use crate::core::{
    flow::CircuitBreakerFlow,
    model::{
        cache::{
            CacheItemType, EventType, ResourceEventKey, ServerEvent, ServiceInstancesCacheItem,
        },
        circuitbreaker::{Resource, ResourceStat, RetStatus, ServiceResource},
        error::{ErrorCode, PoleError},
        naming::{ServiceInstances, ServiceKey, ServiceRule},
    },
    plugin::cache::{Action, Filter, ResourceCache, ResourceListener},
};
use crate::traffic::faultdetect::{
    api::{FaultDetectProbeExecutor, FaultDetectReporter},
    req::{
        FaultDetectPlan, FaultDetectProtocol, FaultDetectResult, FaultDetectTarget,
        HttpFaultDetectConfig, TcpFaultDetectConfig, UdpFaultDetectConfig,
    },
};

#[derive(Clone)]
pub struct FaultDetectScheduler {
    // executor 只负责实际探测，reporter 负责把探测结果映射到熔断或其它统计面。
    executor: Arc<dyn FaultDetectProbeExecutor>,
    reporter: Arc<dyn FaultDetectReporter>,
}

impl FaultDetectScheduler {
    pub fn new(
        executor: Arc<dyn FaultDetectProbeExecutor>,
        reporter: Arc<dyn FaultDetectReporter>,
    ) -> Self {
        Self { executor, reporter }
    }

    pub async fn run_once(&self, host: &str, plans: Vec<FaultDetectPlan>) {
        // 单次执行用于测试和手动触发，不启动后台循环。
        for plan in plans {
            let result = self.executor.execute(host, &plan).await;
            self.reporter.report(&plan, &result).await;
        }
    }

    pub async fn run_targets_once(&self, targets: Vec<FaultDetectTarget>) {
        for target in targets {
            self.run_once(&target.host, target.plans).await;
        }
    }

    pub fn spawn(
        self: Arc<Self>,
        host: String,
        plans: Vec<FaultDetectPlan>,
    ) -> Vec<JoinHandle<()>> {
        // 每个探测计划独立循环，避免一个慢探测阻塞同实例上的其它协议探测。
        plans
            .into_iter()
            .map(|plan| {
                let scheduler = self.clone();
                let host = host.clone();
                tokio::spawn(async move {
                    loop {
                        let result = scheduler.executor.execute(&host, &plan).await;
                        scheduler.reporter.report(&plan, &result).await;
                        tokio::time::sleep(Duration::from_secs(plan.interval_secs.max(1) as u64))
                            .await;
                    }
                })
            })
            .collect()
    }
}

pub struct DefaultFaultDetectProbeExecutor;

#[async_trait::async_trait]
impl FaultDetectProbeExecutor for DefaultFaultDetectProbeExecutor {
    async fn execute(&self, host: &str, plan: &FaultDetectPlan) -> FaultDetectResult {
        execute_fault_detect_plan(host, plan).await
    }
}

pub struct FaultDetectLifecycleOwner {
    scheduler: Arc<FaultDetectScheduler>,
    // handles 是当前规则和实例快照生成的全部后台探测任务。
    handles: Mutex<Vec<JoinHandle<()>>>,
}

impl FaultDetectLifecycleOwner {
    pub fn new(scheduler: Arc<FaultDetectScheduler>) -> Self {
        Self {
            scheduler,
            handles: Mutex::new(Vec::new()),
        }
    }

    pub fn start_targets(&self, targets: Vec<FaultDetectTarget>) {
        let mut handles = self.handles.lock().unwrap();
        for target in targets {
            handles.extend(self.scheduler.clone().spawn(target.host, target.plans));
        }
    }

    pub fn stop(&self) {
        let mut handles = self.handles.lock().unwrap();
        for handle in handles.drain(..) {
            handle.abort();
        }
    }

    pub fn running_task_count(&self) -> usize {
        self.handles.lock().unwrap().len()
    }
}

impl Drop for FaultDetectLifecycleOwner {
    fn drop(&mut self) {
        self.stop();
    }
}

pub struct FaultDetectServiceWatcher {
    resource_cache: Arc<Box<dyn ResourceCache>>,
    rule_listener: Arc<FaultDetectResourceListener>,
    instance_listener: Arc<FaultDetectResourceListener>,
    listeners_registered: OnceCell<()>,
}

impl FaultDetectServiceWatcher {
    pub fn new(
        resource_cache: Arc<Box<dyn ResourceCache>>,
        owner: Arc<FaultDetectLifecycleOwner>,
    ) -> Self {
        let state = Arc::new(FaultDetectResourceListenerState::default());
        Self {
            resource_cache,
            rule_listener: Arc::new(FaultDetectResourceListener::with_state(
                owner.clone(),
                EventType::FaultDetectRule,
                state.clone(),
            )),
            instance_listener: Arc::new(FaultDetectResourceListener::with_state(
                owner,
                EventType::Instance,
                state,
            )),
            listeners_registered: OnceCell::new(),
        }
    }

    pub async fn watch_service(
        &self,
        namespace: &str,
        service: &str,
        timeout: Duration,
    ) -> Result<(), PoleError> {
        self.ensure_listeners_registered().await;

        let rule_filter =
            fault_detect_service_filter(namespace, service, EventType::FaultDetectRule, timeout);
        let instance_filter =
            fault_detect_service_filter(namespace, service, EventType::Instance, timeout);
        // 两个 future 都必须被轮询，确保即使其中一个加载失败，另一类 watch 仍会建立。
        let (rules, instances) = tokio::join!(
            self.resource_cache.load_service_rule(rule_filter),
            self.resource_cache.load_service_instances(instance_filter)
        );
        let rules = rules?;
        let instances = instances?;

        self.apply_initial_snapshot(namespace, service, rules, &instances)
            .await
    }

    /// 正常服务实例访问的集成入口：同一次明确服务加载建立实例与主动探测规则 watch。
    ///
    /// 主动探测规则暂时不可用时不阻断基础服务发现；实例快照仍会保存，后续规则事件到达
    /// 后即可生成探测目标。
    pub(crate) async fn load_service_instances_for_access(
        &self,
        namespace: &str,
        service: &str,
        timeout: Duration,
    ) -> Result<ServiceInstancesCacheItem, PoleError> {
        self.ensure_listeners_registered().await;

        let rule_filter =
            fault_detect_service_filter(namespace, service, EventType::FaultDetectRule, timeout);
        let mut instance_filter =
            fault_detect_service_filter(namespace, service, EventType::Instance, timeout);
        instance_filter.internal_request = false;
        let (rules, instances) = tokio::join!(
            self.resource_cache.load_service_rule(rule_filter),
            self.resource_cache.load_service_instances(instance_filter)
        );
        let instances = instances?;

        match rules {
            Ok(rules) => {
                if let Err(err) = self
                    .apply_initial_snapshot(namespace, service, rules, &instances)
                    .await
                {
                    crate::warn!(
                        "[pole][fault-detect] apply rules for {}/{} failed: {:?}",
                        namespace,
                        service,
                        err
                    );
                    self.apply_instance_snapshot(namespace, service, &instances)
                        .await;
                    self.rule_listener.reload_targets();
                }
            }
            Err(err) => {
                crate::warn!(
                    "[pole][fault-detect] load rules for {}/{} failed: {:?}",
                    namespace,
                    service,
                    err
                );
                self.apply_instance_snapshot(namespace, service, &instances)
                    .await;
                self.rule_listener.reload_targets();
            }
        }

        Ok(instances)
    }

    async fn ensure_listeners_registered(&self) {
        self.listeners_registered
            .get_or_init(|| async {
                self.resource_cache
                    .register_resource_listener(self.rule_listener.clone())
                    .await;
                self.resource_cache
                    .register_resource_listener(self.instance_listener.clone())
                    .await;
            })
            .await;
    }

    async fn apply_initial_snapshot(
        &self,
        namespace: &str,
        service: &str,
        service_rule: ServiceRule,
        instances: &ServiceInstancesCacheItem,
    ) -> Result<(), PoleError> {
        let mut rules = Vec::with_capacity(service_rule.rules.len());
        for rule in service_rule.rules {
            let type_id = rule.type_id();
            match rule.downcast::<FaultDetectRule>() {
                Ok(rule) => rules.push(*rule),
                Err(_) => {
                    return Err(PoleError::new(
                        ErrorCode::InvalidRule,
                        format!(
                            "rule type error, expect FaultDetectRule, but got {:?}",
                            type_id
                        ),
                    ));
                }
            }
        }

        let key = format!("{}#{}", namespace, service);
        self.rule_listener
            .state
            .plans
            .lock()
            .unwrap()
            .insert(key.clone(), build_fault_detect_plans(&rules));
        self.apply_instance_snapshot(namespace, service, instances)
            .await;
        self.rule_listener.reload_targets();
        Ok(())
    }

    async fn apply_instance_snapshot(
        &self,
        namespace: &str,
        service: &str,
        instances: &ServiceInstancesCacheItem,
    ) {
        let service_instances = ServiceInstances::new(
            instances.get_service_info(),
            instances.list_instances(false).await,
        );
        self.rule_listener
            .state
            .instances
            .lock()
            .unwrap()
            .insert(format!("{}#{}", namespace, service), service_instances);
    }
}

fn fault_detect_service_filter(
    namespace: &str,
    service: &str,
    event_type: EventType,
    timeout: Duration,
) -> Filter {
    Filter {
        resource_key: ResourceEventKey {
            namespace: namespace.to_string(),
            event_type,
            filter: HashMap::from([("service".to_string(), service.to_string())]),
        },
        internal_request: true,
        include_cache: true,
        timeout,
    }
}

#[derive(Default)]
struct FaultDetectResourceListenerState {
    // plans 和 instances 分别来自不同事件流，只有两边都到达后才能生成探测目标。
    plans: Mutex<HashMap<String, Vec<FaultDetectPlan>>>,
    instances: Mutex<HashMap<String, ServiceInstances>>,
}

pub struct FaultDetectResourceListener {
    owner: Arc<FaultDetectLifecycleOwner>,
    watch_key: EventType,
    state: Arc<FaultDetectResourceListenerState>,
}

impl FaultDetectResourceListener {
    pub fn new(owner: Arc<FaultDetectLifecycleOwner>) -> Self {
        Self::with_state(
            owner,
            EventType::FaultDetectRule,
            Arc::new(FaultDetectResourceListenerState::default()),
        )
    }

    fn with_state(
        owner: Arc<FaultDetectLifecycleOwner>,
        watch_key: EventType,
        state: Arc<FaultDetectResourceListenerState>,
    ) -> Self {
        Self {
            owner,
            watch_key,
            state,
        }
    }

    pub fn paired(owner: Arc<FaultDetectLifecycleOwner>) -> Vec<Arc<dyn ResourceListener>> {
        let state = Arc::new(FaultDetectResourceListenerState::default());
        // 规则和实例监听器共享状态，任一侧更新都会重建当前服务的探测任务集合。
        vec![
            Arc::new(Self::with_state(
                owner.clone(),
                EventType::FaultDetectRule,
                state.clone(),
            )),
            Arc::new(Self::with_state(owner, EventType::Instance, state)),
        ]
    }

    async fn apply_event(&self, action: Action, event: ServerEvent) {
        let key = fault_detect_service_key(&event);
        // 删除事件只移除对应快照；新增/更新事件先落快照，再统一 reload_targets。
        match (action, event.value) {
            (Action::Delete, CacheItemType::FaultDetectRule(_)) => {
                self.state.plans.lock().unwrap().remove(&key);
            }
            (Action::Delete, CacheItemType::Instance(_)) => {
                self.state.instances.lock().unwrap().remove(&key);
            }
            (_, CacheItemType::FaultDetectRule(item)) => {
                self.state
                    .plans
                    .lock()
                    .unwrap()
                    .insert(key, build_fault_detect_plans(&item.value));
            }
            (_, CacheItemType::Instance(item)) => {
                let service_instances = ServiceInstances::new(
                    item.get_service_info(),
                    item.list_instances(false).await,
                );
                self.state
                    .instances
                    .lock()
                    .unwrap()
                    .insert(key, service_instances);
            }
            _ => return,
        }
        self.reload_targets();
    }

    fn reload_targets(&self) {
        // 简化生命周期处理：每次规则或实例变化后停止旧任务，用最新快照重建。
        // 这样不会留下引用旧规则或旧实例的后台探测循环。
        // 规则与实例必须通过同一个 namespace#service 事件键配对。不能先展平规则，
        // 否则缓存载荷中的服务信息陈旧时，可能把 A 服务的规则应用到 B 服务事件的实例。
        let plans_by_service = self.state.plans.lock().unwrap().clone();
        let instances_by_service = self.state.instances.lock().unwrap().clone();
        let targets = instances_by_service
            .into_iter()
            .flat_map(|(service_key, service_instances)| {
                let plans = plans_by_service
                    .get(&service_key)
                    .cloned()
                    .unwrap_or_default();
                build_fault_detect_targets(&service_instances, plans)
            })
            .collect::<Vec<_>>();

        self.owner.stop();
        if !targets.is_empty() {
            self.owner.start_targets(targets);
        }
    }
}

fn fault_detect_service_key(event: &ServerEvent) -> String {
    let service = event
        .event_key
        .filter
        .get("service")
        .cloned()
        .unwrap_or_default();
    format!("{}#{}", event.event_key.namespace, service)
}

#[async_trait::async_trait]
impl ResourceListener for FaultDetectResourceListener {
    async fn on_event(&self, action: Action, val: ServerEvent) {
        self.apply_event(action, val).await;
    }

    fn watch_key(&self) -> EventType {
        self.watch_key
    }
}

pub async fn register_fault_detect_resource_listeners(
    resource_cache: Arc<Box<dyn ResourceCache>>,
    owner: Arc<FaultDetectLifecycleOwner>,
) {
    for listener in FaultDetectResourceListener::paired(owner) {
        resource_cache.register_resource_listener(listener).await;
    }
}

pub struct CircuitBreakerFaultDetectReporter {
    flow: Arc<CircuitBreakerFlow>,
}

impl CircuitBreakerFaultDetectReporter {
    pub fn new(flow: Arc<CircuitBreakerFlow>) -> Self {
        Self { flow }
    }
}

#[async_trait::async_trait]
impl FaultDetectReporter for CircuitBreakerFaultDetectReporter {
    async fn report(&self, plan: &FaultDetectPlan, result: &FaultDetectResult) {
        if let Err(err) = self
            .flow
            .report_stat(fault_detect_result_to_resource_stat(plan, result))
            .await
        {
            crate::error!("[faultdetect] report fault detect result failed: {:?}", err);
        }
    }
}

pub fn fault_detect_result_to_resource_stat(
    plan: &FaultDetectPlan,
    result: &FaultDetectResult,
) -> ResourceStat {
    // 主动探测结果汇入熔断统计时按服务资源上报，ret_code 使用规则 ID
    // 便于后续区分是哪条探测规则触发的失败或超时。
    let status = if result.success {
        RetStatus::RetSuccess
    } else if result.message.to_ascii_lowercase().contains("timeout") {
        RetStatus::RetTimeout
    } else {
        RetStatus::RetFail
    };

    ResourceStat {
        resource: Resource::ServiceResource(ServiceResource::new(ServiceKey {
            namespace: plan.namespace.clone(),
            name: plan.service.clone(),
        })),
        ret_code: result.rule_id.clone(),
        delay: Duration::from_secs(0),
        status,
    }
}

pub fn build_fault_detect_plans(rules: &[FaultDetectRule]) -> Vec<FaultDetectPlan> {
    let mut rules = rules.iter().collect::<Vec<_>>();
    rules.sort_by(|a, b| a.priority.cmp(&b.priority));

    // spec 中一条 FaultDetectRule 可以包含多个子探测规则，这里展开为调度器
    // 可直接执行的扁平计划列表。
    rules
        .into_iter()
        .flat_map(|rule| {
            let target = rule.target_service.as_ref()?;
            Some(rule.rules.iter().filter_map(move |sub_rule| {
                if sub_rule.disable {
                    return None;
                }
                let protocol = protocol_from_spec(sub_rule.protocol())?;
                Some(FaultDetectPlan {
                    rule_id: rule.id.clone(),
                    namespace: target.namespace.clone(),
                    service: target.service.clone(),
                    interval_secs: sub_rule.interval,
                    timeout_secs: sub_rule.timeout,
                    port: sub_rule.port,
                    protocol,
                    http_config: sub_rule.http_config.as_ref().map(|config| {
                        HttpFaultDetectConfig {
                            method: config.method.clone(),
                            url: config.url.clone(),
                            headers: config
                                .headers
                                .iter()
                                .map(|header| (header.key.clone(), header.value.clone()))
                                .collect(),
                            body: config.body.clone(),
                        }
                    }),
                    tcp_config: sub_rule
                        .tcp_config
                        .as_ref()
                        .map(|config| TcpFaultDetectConfig {
                            send: config.send.clone(),
                            receive: config.receive.clone(),
                        }),
                    udp_config: sub_rule
                        .udp_config
                        .as_ref()
                        .map(|config| UdpFaultDetectConfig {
                            send: config.send.clone(),
                            receive: config.receive.clone(),
                        }),
                })
            }))
        })
        .flatten()
        .collect()
}

pub fn build_fault_detect_targets(
    service_instances: &ServiceInstances,
    plans: Vec<FaultDetectPlan>,
) -> Vec<FaultDetectTarget> {
    let matching_plans = plans
        .into_iter()
        .filter(|plan| {
            plan.namespace == service_instances.service.namespace
                && plan.service == service_instances.service.name
        })
        .collect::<Vec<_>>();
    if matching_plans.is_empty() {
        return Vec::new();
    }

    service_instances
        .instances
        .iter()
        .filter(|instance| instance.is_available())
        .map(|instance| FaultDetectTarget {
            host: instance.ip.clone(),
            plans: matching_plans.clone(),
        })
        .collect()
}

fn protocol_from_spec(protocol: fault_detect_rule::Protocol) -> Option<FaultDetectProtocol> {
    match protocol {
        fault_detect_rule::Protocol::Http => Some(FaultDetectProtocol::Http),
        fault_detect_rule::Protocol::Tcp => Some(FaultDetectProtocol::Tcp),
        fault_detect_rule::Protocol::Udp => Some(FaultDetectProtocol::Udp),
        fault_detect_rule::Protocol::Unknown => None,
    }
}

pub async fn execute_fault_detect_plan(host: &str, plan: &FaultDetectPlan) -> FaultDetectResult {
    match plan.protocol {
        FaultDetectProtocol::Tcp => execute_tcp_fault_detect(host, plan).await,
        FaultDetectProtocol::Http => execute_http_fault_detect(host, plan).await,
        FaultDetectProtocol::Udp => execute_udp_fault_detect(host, plan).await,
    }
}

async fn execute_http_fault_detect(host: &str, plan: &FaultDetectPlan) -> FaultDetectResult {
    let timeout_duration = Duration::from_secs(plan.timeout_secs.max(1) as u64);
    let address = format!("{}:{}", host, plan.port);
    let target = plan
        .http_config
        .as_ref()
        .map(|config| http_request_target(&config.url))
        .unwrap_or_else(|| "/".to_string());

    match timeout(timeout_duration, send_http_fault_detect(host, plan)).await {
        Ok(Ok(status_code)) if (200..400).contains(&status_code) => FaultDetectResult {
            rule_id: plan.rule_id.clone(),
            success: true,
            message: String::new(),
        },
        Ok(Ok(status_code)) => FaultDetectResult {
            rule_id: plan.rule_id.clone(),
            success: false,
            message: format!(
                "http fault detect request {}{} returned status {}",
                address, target, status_code
            ),
        },
        Ok(Err(err)) => FaultDetectResult {
            rule_id: plan.rule_id.clone(),
            success: false,
            message: format!(
                "http fault detect request {}{} failed: {}",
                address, target, err
            ),
        },
        Err(_) => FaultDetectResult {
            rule_id: plan.rule_id.clone(),
            success: false,
            message: format!("http fault detect request {}{} timeout", address, target),
        },
    }
}

async fn send_http_fault_detect(host: &str, plan: &FaultDetectPlan) -> Result<u16, String> {
    let config = plan
        .http_config
        .clone()
        .unwrap_or_else(default_http_fault_detect_config);
    let method = if config.method.trim().is_empty() {
        "GET".to_string()
    } else {
        config.method.trim().to_ascii_uppercase()
    };
    let target = http_request_target(&config.url);
    let address = format!("{}:{}", host, plan.port);
    let mut stream = TcpStream::connect(address.as_str())
        .await
        .map_err(|err| err.to_string())?;
    let request = build_http_fault_detect_request(&method, &target, host, plan.port, &config);
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|err| err.to_string())?;

    let mut buf = [0; 1024];
    let read_size = stream.read(&mut buf).await.map_err(|err| err.to_string())?;
    if read_size == 0 {
        return Err("empty http response".to_string());
    }

    parse_http_status_code(&buf[..read_size])
}

fn default_http_fault_detect_config() -> HttpFaultDetectConfig {
    HttpFaultDetectConfig {
        method: "GET".to_string(),
        url: "/".to_string(),
        headers: Vec::new(),
        body: String::new(),
    }
}

fn build_http_fault_detect_request(
    method: &str,
    target: &str,
    host: &str,
    port: u32,
    config: &HttpFaultDetectConfig,
) -> String {
    let mut request = format!("{} {} HTTP/1.1\r\n", method, target);
    if !has_header(&config.headers, "host") {
        request.push_str(&format!("Host: {}:{}\r\n", host, port));
    }
    if !has_header(&config.headers, "connection") {
        request.push_str("Connection: close\r\n");
    }
    if !config.body.is_empty() && !has_header(&config.headers, "content-length") {
        request.push_str(&format!("Content-Length: {}\r\n", config.body.len()));
    }
    for (key, value) in &config.headers {
        if !key.trim().is_empty() {
            request.push_str(&format!("{}: {}\r\n", key.trim(), value));
        }
    }
    request.push_str("\r\n");
    request.push_str(&config.body);
    request
}

fn has_header(headers: &[(String, String)], name: &str) -> bool {
    headers
        .iter()
        .any(|(key, _)| key.eq_ignore_ascii_case(name))
}

fn http_request_target(url: &str) -> String {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return "/".to_string();
    }
    let url_without_scheme = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"));
    if let Some(rest) = url_without_scheme {
        let path_index = rest
            .find('/')
            .into_iter()
            .chain(rest.find('?'))
            .min()
            .unwrap_or(rest.len());
        if path_index == rest.len() {
            "/".to_string()
        } else if rest.as_bytes()[path_index] == b'?' {
            format!("/{}", &rest[path_index..])
        } else {
            rest[path_index..].to_string()
        }
    } else if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{}", trimmed)
    }
}

fn parse_http_status_code(response: &[u8]) -> Result<u16, String> {
    let response = String::from_utf8_lossy(response);
    let status_line = response
        .lines()
        .next()
        .ok_or_else(|| "empty http response".to_string())?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| format!("invalid http status line: {}", status_line))?;
    status
        .parse::<u16>()
        .map_err(|_| format!("invalid http status code: {}", status))
}

async fn execute_tcp_fault_detect(host: &str, plan: &FaultDetectPlan) -> FaultDetectResult {
    let timeout_duration = Duration::from_secs(plan.timeout_secs.max(1) as u64);
    let address = format!("{}:{}", host, plan.port);
    match timeout(timeout_duration, send_tcp_fault_detect(host, plan)).await {
        Ok(Ok(())) => FaultDetectResult {
            rule_id: plan.rule_id.clone(),
            success: true,
            message: String::new(),
        },
        Ok(Err(err)) => FaultDetectResult {
            rule_id: plan.rule_id.clone(),
            success: false,
            message: format!("tcp fault detect request {} failed: {}", address, err),
        },
        Err(_) => FaultDetectResult {
            rule_id: plan.rule_id.clone(),
            success: false,
            message: format!("tcp fault detect request {} timeout", address),
        },
    }
}

async fn send_tcp_fault_detect(host: &str, plan: &FaultDetectPlan) -> Result<(), String> {
    let config = plan
        .tcp_config
        .clone()
        .unwrap_or_else(default_tcp_fault_detect_config);
    let address = format!("{}:{}", host, plan.port);
    let mut stream = TcpStream::connect(address.as_str())
        .await
        .map_err(|err| err.to_string())?;
    if !config.send.is_empty() {
        stream
            .write_all(config.send.as_bytes())
            .await
            .map_err(|err| err.to_string())?;
    }
    if config.receive.is_empty() {
        return Ok(());
    }

    let mut buf = [0; 1024];
    let read_size = stream.read(&mut buf).await.map_err(|err| err.to_string())?;
    if read_size == 0 {
        return Err("empty tcp response".to_string());
    }
    ensure_expected_response("tcp", &buf[..read_size], &config.receive)
}

async fn execute_udp_fault_detect(host: &str, plan: &FaultDetectPlan) -> FaultDetectResult {
    let timeout_duration = Duration::from_secs(plan.timeout_secs.max(1) as u64);
    let address = format!("{}:{}", host, plan.port);
    match timeout(timeout_duration, send_udp_fault_detect(host, plan)).await {
        Ok(Ok(())) => FaultDetectResult {
            rule_id: plan.rule_id.clone(),
            success: true,
            message: String::new(),
        },
        Ok(Err(err)) => FaultDetectResult {
            rule_id: plan.rule_id.clone(),
            success: false,
            message: format!("udp fault detect request {} failed: {}", address, err),
        },
        Err(_) => FaultDetectResult {
            rule_id: plan.rule_id.clone(),
            success: false,
            message: format!("udp fault detect request {} timeout", address),
        },
    }
}

async fn send_udp_fault_detect(host: &str, plan: &FaultDetectPlan) -> Result<(), String> {
    let config = plan
        .udp_config
        .clone()
        .unwrap_or_else(default_udp_fault_detect_config);
    let address = format!("{}:{}", host, plan.port);
    let socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|err| err.to_string())?;
    socket
        .connect(address.as_str())
        .await
        .map_err(|err| err.to_string())?;
    if config.send.is_empty() && config.receive.is_empty() {
        return Ok(());
    }
    socket
        .send(config.send.as_bytes())
        .await
        .map_err(|err| err.to_string())?;
    if config.receive.is_empty() {
        return Ok(());
    }

    let mut buf = [0; 1024];
    let read_size = socket.recv(&mut buf).await.map_err(|err| err.to_string())?;
    if read_size == 0 {
        return Err("empty udp response".to_string());
    }
    ensure_expected_response("udp", &buf[..read_size], &config.receive)
}

fn default_tcp_fault_detect_config() -> TcpFaultDetectConfig {
    TcpFaultDetectConfig {
        send: String::new(),
        receive: Vec::new(),
    }
}

fn default_udp_fault_detect_config() -> UdpFaultDetectConfig {
    UdpFaultDetectConfig {
        send: String::new(),
        receive: Vec::new(),
    }
}

fn ensure_expected_response(
    protocol: &str,
    response: &[u8],
    expected_values: &[String],
) -> Result<(), String> {
    let response = String::from_utf8_lossy(response);
    if expected_values
        .iter()
        .any(|expected| response.contains(expected))
    {
        Ok(())
    } else {
        Err(format!(
            "{} response {:?} did not match expected values {:?}",
            protocol, response, expected_values
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::cache::{
        CacheItemType, EventType, FaultDetectRulesCacheItem, ResourceEventKey, ServerEvent,
        ServiceInstancesCacheItem,
    };
    use crate::core::model::circuitbreaker::{ResourceStat, RetStatus};
    use crate::core::model::config::{ConfigFile, ConfigGroup};
    use crate::core::model::error::{ErrorCode, PoleError};
    use crate::core::model::naming::{Instance, ServiceInfo, ServiceInstances};
    use crate::core::model::naming::{ServiceRule, Services};
    use crate::core::plugin::cache::{
        Action, Filter, ResourceCache, ResourceCacheFailover, ResourceListener,
    };
    use crate::core::plugin::plugins::Plugin;
    use pole_specification::v1::{
        fault_detect_rule, http_protocol_config, FaultDetectRule, FaultDetectSubRule,
        HttpProtocolConfig, Service, TcpProtocolConfig, UdpProtocolConfig,
    };
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, UdpSocket},
    };

    #[test]
    fn fault_detect_planner_builds_enabled_probe_plan_for_target_service() {
        let rules = vec![FaultDetectRule {
            id: "fd-1".to_string(),
            name: "orders-http".to_string(),
            target_service: Some(fault_detect_rule::DestinationService {
                namespace: "default".to_string(),
                service: "orders".to_string(),
                ..fault_detect_rule::DestinationService::default()
            }),
            rules: vec![FaultDetectSubRule {
                interval: 5,
                timeout: 2,
                port: 8080,
                protocol: fault_detect_rule::Protocol::Http.into(),
                http_config: Some(HttpProtocolConfig {
                    method: "POST".to_string(),
                    url: "/ready".to_string(),
                    headers: vec![http_protocol_config::MessageHeader {
                        key: "X-Probe".to_string(),
                        value: "true".to_string(),
                    }],
                    body: "probe-body".to_string(),
                }),
                ..FaultDetectSubRule::default()
            }],
            priority: 0,
            ..FaultDetectRule::default()
        }];

        let plans = build_fault_detect_plans(&rules);

        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].rule_id, "fd-1");
        assert_eq!(plans[0].namespace, "default");
        assert_eq!(plans[0].service, "orders");
        assert_eq!(plans[0].interval_secs, 5);
        assert_eq!(plans[0].timeout_secs, 2);
        assert_eq!(plans[0].port, 8080);
        assert_eq!(plans[0].protocol, FaultDetectProtocol::Http);
        assert_eq!(
            plans[0].http_config,
            Some(HttpFaultDetectConfig {
                method: "POST".to_string(),
                url: "/ready".to_string(),
                headers: vec![("X-Probe".to_string(), "true".to_string())],
                body: "probe-body".to_string(),
            })
        );
    }

    #[derive(Default)]
    struct RecordingProbeExecutor {
        calls: Mutex<Vec<(String, String)>>,
    }

    #[async_trait::async_trait]
    impl FaultDetectProbeExecutor for RecordingProbeExecutor {
        async fn execute(&self, host: &str, plan: &FaultDetectPlan) -> FaultDetectResult {
            self.calls
                .lock()
                .unwrap()
                .push((host.to_string(), plan.rule_id.clone()));
            FaultDetectResult {
                rule_id: plan.rule_id.clone(),
                success: true,
                message: String::new(),
            }
        }
    }

    #[derive(Default)]
    struct RecordingFaultDetectReporter {
        results: Mutex<Vec<ResourceStat>>,
    }

    #[async_trait::async_trait]
    impl FaultDetectReporter for RecordingFaultDetectReporter {
        async fn report(&self, plan: &FaultDetectPlan, result: &FaultDetectResult) {
            self.results
                .lock()
                .unwrap()
                .push(fault_detect_result_to_resource_stat(plan, result));
        }
    }

    #[tokio::test]
    async fn fault_detect_scheduler_runs_plans_and_reports_results() {
        let executor = Arc::new(RecordingProbeExecutor::default());
        let reporter = Arc::new(RecordingFaultDetectReporter::default());
        let scheduler = FaultDetectScheduler::new(executor.clone(), reporter.clone());
        let plan = FaultDetectPlan {
            rule_id: "fd-1".to_string(),
            namespace: "default".to_string(),
            service: "orders".to_string(),
            interval_secs: 5,
            timeout_secs: 1,
            port: 8080,
            protocol: FaultDetectProtocol::Tcp,
            http_config: None,
            tcp_config: None,
            udp_config: None,
        };

        scheduler.run_once("127.0.0.1", vec![plan.clone()]).await;

        assert_eq!(
            executor.calls.lock().unwrap().as_slice(),
            &[("127.0.0.1".to_string(), "fd-1".to_string())]
        );
        let results = reporter.results.lock().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, RetStatus::RetSuccess);
    }

    #[tokio::test]
    async fn fault_detect_scheduler_runs_targets_for_each_host() {
        let executor = Arc::new(RecordingProbeExecutor::default());
        let reporter = Arc::new(RecordingFaultDetectReporter::default());
        let scheduler = FaultDetectScheduler::new(executor.clone(), reporter.clone());
        let plan = FaultDetectPlan {
            rule_id: "fd-1".to_string(),
            namespace: "default".to_string(),
            service: "orders".to_string(),
            interval_secs: 5,
            timeout_secs: 1,
            port: 8080,
            protocol: FaultDetectProtocol::Tcp,
            http_config: None,
            tcp_config: None,
            udp_config: None,
        };
        let targets = vec![
            FaultDetectTarget {
                host: "10.0.0.1".to_string(),
                plans: vec![plan.clone()],
            },
            FaultDetectTarget {
                host: "10.0.0.2".to_string(),
                plans: vec![plan],
            },
        ];

        scheduler.run_targets_once(targets).await;

        assert_eq!(
            executor.calls.lock().unwrap().as_slice(),
            &[
                ("10.0.0.1".to_string(), "fd-1".to_string()),
                ("10.0.0.2".to_string(), "fd-1".to_string())
            ]
        );
        assert_eq!(reporter.results.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn fault_detect_lifecycle_owner_starts_and_stops_target_tasks() {
        let executor = Arc::new(RecordingProbeExecutor::default());
        let reporter = Arc::new(RecordingFaultDetectReporter::default());
        let scheduler = Arc::new(FaultDetectScheduler::new(
            executor.clone(),
            reporter.clone(),
        ));
        let owner = FaultDetectLifecycleOwner::new(scheduler);
        let plan = FaultDetectPlan {
            rule_id: "fd-1".to_string(),
            namespace: "default".to_string(),
            service: "orders".to_string(),
            interval_secs: 1,
            timeout_secs: 1,
            port: 8080,
            protocol: FaultDetectProtocol::Tcp,
            http_config: None,
            tcp_config: None,
            udp_config: None,
        };

        owner.start_targets(vec![FaultDetectTarget {
            host: "10.0.0.1".to_string(),
            plans: vec![plan],
        }]);
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert_eq!(owner.running_task_count(), 1);
        assert_eq!(executor.calls.lock().unwrap().len(), 1);

        owner.stop();

        assert_eq!(owner.running_task_count(), 0);
    }

    #[tokio::test]
    async fn fault_detect_resource_listener_starts_targets_after_rules_and_instances_arrive() {
        let executor = Arc::new(RecordingProbeExecutor::default());
        let reporter = Arc::new(RecordingFaultDetectReporter::default());
        let scheduler = Arc::new(FaultDetectScheduler::new(executor.clone(), reporter));
        let owner = Arc::new(FaultDetectLifecycleOwner::new(scheduler));
        let listener = FaultDetectResourceListener::new(owner.clone());
        let mut rule_item = FaultDetectRulesCacheItem::new();
        rule_item.value = vec![FaultDetectRule {
            id: "fd-orders".to_string(),
            target_service: Some(fault_detect_rule::DestinationService {
                namespace: "default".to_string(),
                service: "orders".to_string(),
                ..fault_detect_rule::DestinationService::default()
            }),
            rules: vec![FaultDetectSubRule {
                protocol: fault_detect_rule::Protocol::Tcp.into(),
                interval: 1,
                timeout: 1,
                port: 8080,
                ..FaultDetectSubRule::default()
            }],
            ..FaultDetectRule::default()
        }];

        listener
            .on_event(
                Action::Update,
                ServerEvent {
                    event_key: resource_event_key(EventType::FaultDetectRule),
                    value: CacheItemType::FaultDetectRule(rule_item),
                },
            )
            .await;

        assert_eq!(owner.running_task_count(), 0);

        listener
            .on_event(
                Action::Update,
                ServerEvent {
                    event_key: resource_event_key(EventType::Instance),
                    value: CacheItemType::Instance(service_instances_cache_item().await),
                },
            )
            .await;
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert_eq!(owner.running_task_count(), 1);
        assert_eq!(
            executor.calls.lock().unwrap().as_slice(),
            &[("10.0.0.1".to_string(), "fd-orders".to_string())]
        );

        owner.stop();
    }

    #[tokio::test]
    async fn fault_detect_resource_listener_keeps_two_services_isolated() {
        let executor = Arc::new(RecordingProbeExecutor::default());
        let owner = Arc::new(FaultDetectLifecycleOwner::new(Arc::new(
            FaultDetectScheduler::new(
                executor.clone(),
                Arc::new(RecordingFaultDetectReporter::default()),
            ),
        )));
        let listener = FaultDetectResourceListener::new(owner.clone());

        listener
            .on_event(
                Action::Update,
                fault_detect_rule_event("orders", "fd-orders"),
            )
            .await;
        listener
            .on_event(
                Action::Update,
                fault_detect_rule_event("billing", "fd-billing"),
            )
            .await;
        listener
            .on_event(
                Action::Update,
                service_instances_event("orders", "orders", "10.0.0.1").await,
            )
            .await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        executor.calls.lock().unwrap().clear();

        listener
            .on_event(
                Action::Update,
                service_instances_event("billing", "billing", "10.0.0.2").await,
            )
            .await;
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut calls = executor.calls.lock().unwrap().clone();
        calls.sort();
        assert_eq!(
            calls,
            vec![
                ("10.0.0.1".to_string(), "fd-orders".to_string()),
                ("10.0.0.2".to_string(), "fd-billing".to_string()),
            ]
        );
        owner.stop();
    }

    #[tokio::test]
    async fn fault_detect_resource_listener_never_uses_another_service_plan_for_stale_payload() {
        let executor = Arc::new(RecordingProbeExecutor::default());
        let owner = Arc::new(FaultDetectLifecycleOwner::new(Arc::new(
            FaultDetectScheduler::new(
                executor.clone(),
                Arc::new(RecordingFaultDetectReporter::default()),
            ),
        )));
        let listener = FaultDetectResourceListener::new(owner.clone());

        listener
            .on_event(
                Action::Update,
                fault_detect_rule_event("orders", "fd-orders"),
            )
            .await;
        listener
            .on_event(
                Action::Update,
                fault_detect_rule_event("billing", "fd-billing"),
            )
            .await;
        listener
            .on_event(
                Action::Update,
                service_instances_event("orders", "orders", "10.0.0.1").await,
            )
            .await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        executor.calls.lock().unwrap().clear();

        // 事件键属于 billing，但缓存载荷仍带着旧的 orders 服务信息。
        listener
            .on_event(
                Action::Update,
                service_instances_event("billing", "orders", "10.0.0.2").await,
            )
            .await;
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert_eq!(
            executor.calls.lock().unwrap().as_slice(),
            &[("10.0.0.1".to_string(), "fd-orders".to_string())]
        );
        owner.stop();
    }

    #[derive(Default)]
    struct RecordingResourceCache {
        watch_keys: Arc<Mutex<Vec<EventType>>>,
        load_keys: Arc<Mutex<Vec<ResourceEventKey>>>,
        serve_initial_snapshot: bool,
    }

    impl Plugin for RecordingResourceCache {
        fn init(&mut self) {}

        fn destroy(&self) {}

        fn name(&self) -> String {
            "recording".to_string()
        }
    }

    #[async_trait::async_trait]
    impl ResourceCache for RecordingResourceCache {
        fn set_failover_provider(&mut self, _failover: Arc<dyn ResourceCacheFailover>) {}

        async fn load_service_rule(&self, filter: Filter) -> Result<ServiceRule, PoleError> {
            self.load_keys.lock().unwrap().push(filter.resource_key);
            if self.serve_initial_snapshot {
                return Ok(ServiceRule {
                    rules: vec![Box::new(FaultDetectRule {
                        id: "fd-orders".to_string(),
                        target_service: Some(fault_detect_rule::DestinationService {
                            namespace: "default".to_string(),
                            service: "orders".to_string(),
                            ..fault_detect_rule::DestinationService::default()
                        }),
                        rules: vec![FaultDetectSubRule {
                            protocol: fault_detect_rule::Protocol::Tcp.into(),
                            interval: 1,
                            timeout: 1,
                            port: 8080,
                            ..FaultDetectSubRule::default()
                        }],
                        ..FaultDetectRule::default()
                    })],
                    revision: "rule-rev".to_string(),
                    initialized: true,
                });
            }
            Ok(ServiceRule {
                rules: Vec::new(),
                revision: "rule-rev".to_string(),
                initialized: true,
            })
        }

        async fn load_services(&self, _filter: Filter) -> Result<Services, PoleError> {
            Err(test_error())
        }

        async fn load_service_instances(
            &self,
            filter: Filter,
        ) -> Result<ServiceInstancesCacheItem, PoleError> {
            self.load_keys.lock().unwrap().push(filter.resource_key);
            if self.serve_initial_snapshot {
                return Ok(service_instances_cache_item().await);
            }
            Ok(ServiceInstancesCacheItem::new())
        }

        async fn load_config_file(&self, _filter: Filter) -> Result<ConfigFile, PoleError> {
            Err(test_error())
        }

        async fn load_config_group_files(&self, _filter: Filter) -> Result<ConfigGroup, PoleError> {
            Err(test_error())
        }

        async fn register_resource_listener(&self, listener: Arc<dyn ResourceListener>) {
            self.watch_keys.lock().unwrap().push(listener.watch_key());
        }
    }

    #[tokio::test]
    async fn fault_detect_registers_rule_and_instance_resource_listeners() {
        let cache = RecordingResourceCache::default();
        let watch_keys = cache.watch_keys.clone();
        let owner = Arc::new(FaultDetectLifecycleOwner::new(Arc::new(
            FaultDetectScheduler::new(
                Arc::new(RecordingProbeExecutor::default()),
                Arc::new(RecordingFaultDetectReporter::default()),
            ),
        )));

        register_fault_detect_resource_listeners(Arc::new(Box::new(cache)), owner).await;

        let mut watch_keys = watch_keys.lock().unwrap().clone();
        watch_keys.sort_by_key(|event_type| event_type.to_string());
        assert_eq!(
            watch_keys,
            vec![EventType::FaultDetectRule, EventType::Instance]
        );
    }

    #[tokio::test]
    async fn fault_detect_service_watcher_subscribes_rules_and_instances_for_explicit_target() {
        let cache = RecordingResourceCache::default();
        let load_keys = cache.load_keys.clone();
        let owner = Arc::new(FaultDetectLifecycleOwner::new(Arc::new(
            FaultDetectScheduler::new(
                Arc::new(RecordingProbeExecutor::default()),
                Arc::new(RecordingFaultDetectReporter::default()),
            ),
        )));
        let watcher = FaultDetectServiceWatcher::new(Arc::new(Box::new(cache)), owner);

        watcher
            .watch_service("prod", "orders", Duration::from_millis(50))
            .await
            .unwrap();

        let mut load_keys = load_keys.lock().unwrap().clone();
        load_keys.sort_by_key(|key| key.event_type.to_string());
        assert_eq!(load_keys.len(), 2);
        assert_eq!(load_keys[0].event_type, EventType::FaultDetectRule);
        assert_eq!(load_keys[1].event_type, EventType::Instance);
        for key in load_keys {
            assert_eq!(key.namespace, "prod");
            assert_eq!(key.filter.get("service"), Some(&"orders".to_string()));
        }
    }

    #[tokio::test]
    async fn fault_detect_service_watcher_starts_from_initial_cache_snapshot() {
        let cache = RecordingResourceCache {
            serve_initial_snapshot: true,
            ..RecordingResourceCache::default()
        };
        let executor = Arc::new(RecordingProbeExecutor::default());
        let owner = Arc::new(FaultDetectLifecycleOwner::new(Arc::new(
            FaultDetectScheduler::new(
                executor.clone(),
                Arc::new(RecordingFaultDetectReporter::default()),
            ),
        )));
        let watcher = FaultDetectServiceWatcher::new(Arc::new(Box::new(cache)), owner.clone());

        watcher
            .watch_service("default", "orders", Duration::from_millis(50))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert_eq!(owner.running_task_count(), 1);
        assert_eq!(
            executor.calls.lock().unwrap().as_slice(),
            &[("10.0.0.1".to_string(), "fd-orders".to_string())]
        );
        owner.stop();
    }

    #[tokio::test]
    async fn normal_service_instance_access_starts_fault_detect_watch() {
        let cache = RecordingResourceCache::default();
        let load_keys = cache.load_keys.clone();
        let owner = Arc::new(FaultDetectLifecycleOwner::new(Arc::new(
            FaultDetectScheduler::new(
                Arc::new(RecordingProbeExecutor::default()),
                Arc::new(RecordingFaultDetectReporter::default()),
            ),
        )));
        let watcher = FaultDetectServiceWatcher::new(Arc::new(Box::new(cache)), owner);

        watcher
            .load_service_instances_for_access("prod", "orders", Duration::from_millis(50))
            .await
            .unwrap();

        let keys = load_keys.lock().unwrap();
        assert_eq!(keys.len(), 2);
        assert!(keys.iter().any(|key| {
            key.event_type == EventType::FaultDetectRule
                && key.namespace == "prod"
                && key.filter.get("service") == Some(&"orders".to_string())
        }));
        assert!(keys.iter().any(|key| {
            key.event_type == EventType::Instance
                && key.namespace == "prod"
                && key.filter.get("service") == Some(&"orders".to_string())
        }));
    }

    fn resource_event_key(event_type: EventType) -> ResourceEventKey {
        ResourceEventKey {
            namespace: "default".to_string(),
            event_type,
            filter: HashMap::from([("service".to_string(), "orders".to_string())]),
        }
    }

    fn fault_detect_rule_event(service: &str, rule_id: &str) -> ServerEvent {
        let mut item = FaultDetectRulesCacheItem::new();
        item.value = vec![FaultDetectRule {
            id: rule_id.to_string(),
            target_service: Some(fault_detect_rule::DestinationService {
                namespace: "default".to_string(),
                service: service.to_string(),
                ..fault_detect_rule::DestinationService::default()
            }),
            rules: vec![FaultDetectSubRule {
                protocol: fault_detect_rule::Protocol::Tcp.into(),
                interval: 1,
                timeout: 1,
                port: 8080,
                ..FaultDetectSubRule::default()
            }],
            ..FaultDetectRule::default()
        }];
        ServerEvent {
            event_key: ResourceEventKey {
                namespace: "default".to_string(),
                event_type: EventType::FaultDetectRule,
                filter: HashMap::from([("service".to_string(), service.to_string())]),
            },
            value: CacheItemType::FaultDetectRule(item),
        }
    }

    async fn service_instances_event(
        event_service: &str,
        payload_service: &str,
        ip: &str,
    ) -> ServerEvent {
        let mut item = service_instances_cache_item().await;
        item.svc_info.name = payload_service.to_string();
        for instance in item.value.write().await.iter_mut() {
            instance.service = payload_service.to_string();
            instance.ip = ip.to_string();
        }
        for instance in item.available_instances.write().await.iter_mut() {
            instance.service = payload_service.to_string();
            instance.ip = ip.to_string();
        }
        ServerEvent {
            event_key: ResourceEventKey {
                namespace: "default".to_string(),
                event_type: EventType::Instance,
                filter: HashMap::from([("service".to_string(), event_service.to_string())]),
            },
            value: CacheItemType::Instance(item),
        }
    }

    fn test_error() -> PoleError {
        PoleError::new(ErrorCode::InternalError, "not used".to_string())
    }

    async fn service_instances_cache_item() -> ServiceInstancesCacheItem {
        let mut item = ServiceInstancesCacheItem::new();
        {
            let mut value = item.value.write().await;
            value.push(Instance {
                namespace: "default".to_string(),
                service: "orders".to_string(),
                ip: "10.0.0.1".to_string(),
                health: true,
                isolated: false,
                weight: 100,
                ..Instance::default()
            });
        }
        {
            let mut available = item.available_instances.write().await;
            available.push(Instance {
                namespace: "default".to_string(),
                service: "orders".to_string(),
                ip: "10.0.0.1".to_string(),
                health: true,
                isolated: false,
                weight: 100,
                ..Instance::default()
            });
        }
        item.svc_info = Service {
            namespace: "default".to_string(),
            name: "orders".to_string(),
            ..Service::default()
        };
        item
    }

    #[test]
    fn fault_detect_targets_expand_plans_for_available_matching_instances() {
        let service_instances = ServiceInstances::new(
            ServiceInfo {
                namespace: "default".to_string(),
                name: "orders".to_string(),
                ..ServiceInfo::default()
            },
            vec![
                Instance {
                    namespace: "default".to_string(),
                    service: "orders".to_string(),
                    ip: "10.0.0.1".to_string(),
                    health: true,
                    isolated: false,
                    weight: 100,
                    ..Instance::default()
                },
                Instance {
                    namespace: "default".to_string(),
                    service: "orders".to_string(),
                    ip: "10.0.0.2".to_string(),
                    health: false,
                    isolated: false,
                    weight: 100,
                    ..Instance::default()
                },
                Instance {
                    namespace: "default".to_string(),
                    service: "orders".to_string(),
                    ip: "10.0.0.3".to_string(),
                    health: true,
                    isolated: true,
                    weight: 100,
                    ..Instance::default()
                },
            ],
        );
        let matching_plan = FaultDetectPlan {
            rule_id: "fd-orders".to_string(),
            namespace: "default".to_string(),
            service: "orders".to_string(),
            interval_secs: 5,
            timeout_secs: 1,
            port: 8080,
            protocol: FaultDetectProtocol::Tcp,
            http_config: None,
            tcp_config: None,
            udp_config: None,
        };
        let other_plan = FaultDetectPlan {
            rule_id: "fd-payments".to_string(),
            namespace: "default".to_string(),
            service: "payments".to_string(),
            interval_secs: 5,
            timeout_secs: 1,
            port: 8080,
            protocol: FaultDetectProtocol::Tcp,
            http_config: None,
            tcp_config: None,
            udp_config: None,
        };

        let targets =
            build_fault_detect_targets(&service_instances, vec![matching_plan, other_plan]);

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].host, "10.0.0.1");
        assert_eq!(targets[0].plans.len(), 1);
        assert_eq!(targets[0].plans[0].rule_id, "fd-orders");
    }

    #[test]
    fn fault_detect_result_to_resource_stat_maps_failure_and_timeout() {
        let plan = FaultDetectPlan {
            rule_id: "fd-1".to_string(),
            namespace: "default".to_string(),
            service: "orders".to_string(),
            interval_secs: 5,
            timeout_secs: 1,
            port: 8080,
            protocol: FaultDetectProtocol::Tcp,
            http_config: None,
            tcp_config: None,
            udp_config: None,
        };

        let failed = fault_detect_result_to_resource_stat(
            &plan,
            &FaultDetectResult {
                rule_id: "fd-1".to_string(),
                success: false,
                message: "connection refused".to_string(),
            },
        );
        let timeout = fault_detect_result_to_resource_stat(
            &plan,
            &FaultDetectResult {
                rule_id: "fd-1".to_string(),
                success: false,
                message: "tcp fault detect request timeout".to_string(),
            },
        );

        assert_eq!(failed.status, RetStatus::RetFail);
        assert_eq!(timeout.status, RetStatus::RetTimeout);
    }

    #[test]
    fn fault_detect_planner_copies_tcp_and_udp_payload_configs() {
        let rules = vec![
            FaultDetectRule {
                id: "fd-tcp".to_string(),
                target_service: Some(fault_detect_rule::DestinationService {
                    namespace: "default".to_string(),
                    service: "orders".to_string(),
                    ..fault_detect_rule::DestinationService::default()
                }),
                rules: vec![FaultDetectSubRule {
                    port: 8081,
                    protocol: fault_detect_rule::Protocol::Tcp.into(),
                    tcp_config: Some(TcpProtocolConfig {
                        send: "ping".to_string(),
                        receive: vec!["pong".to_string()],
                    }),
                    ..FaultDetectSubRule::default()
                }],
                priority: 1,
                ..FaultDetectRule::default()
            },
            FaultDetectRule {
                id: "fd-udp".to_string(),
                target_service: Some(fault_detect_rule::DestinationService {
                    namespace: "default".to_string(),
                    service: "orders".to_string(),
                    ..fault_detect_rule::DestinationService::default()
                }),
                rules: vec![FaultDetectSubRule {
                    port: 8082,
                    protocol: fault_detect_rule::Protocol::Udp.into(),
                    udp_config: Some(UdpProtocolConfig {
                        send: "ping".to_string(),
                        receive: vec!["pong".to_string()],
                    }),
                    ..FaultDetectSubRule::default()
                }],
                priority: 2,
                ..FaultDetectRule::default()
            },
        ];

        let plans = build_fault_detect_plans(&rules);

        assert_eq!(plans.len(), 2);
        assert_eq!(
            plans[0].tcp_config,
            Some(TcpFaultDetectConfig {
                send: "ping".to_string(),
                receive: vec!["pong".to_string()],
            })
        );
        assert_eq!(
            plans[1].udp_config,
            Some(UdpFaultDetectConfig {
                send: "ping".to_string(),
                receive: vec!["pong".to_string()],
            })
        );
    }

    #[tokio::test]
    async fn tcp_fault_detect_executor_succeeds_when_port_accepts_connections() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port() as u32;
        let _accept_task = tokio::spawn(async move {
            let _ = listener.accept().await;
        });
        let plan = FaultDetectPlan {
            rule_id: "fd-tcp".to_string(),
            namespace: "default".to_string(),
            service: "orders".to_string(),
            interval_secs: 5,
            timeout_secs: 1,
            port,
            protocol: FaultDetectProtocol::Tcp,
            http_config: None,
            tcp_config: None,
            udp_config: None,
        };

        let result = execute_fault_detect_plan("127.0.0.1", &plan).await;

        assert!(result.success, "{}", result.message);
    }

    #[tokio::test]
    async fn tcp_fault_detect_executor_fails_when_port_is_closed() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port() as u32;
        drop(listener);
        let plan = FaultDetectPlan {
            rule_id: "fd-tcp".to_string(),
            namespace: "default".to_string(),
            service: "orders".to_string(),
            interval_secs: 5,
            timeout_secs: 1,
            port,
            protocol: FaultDetectProtocol::Tcp,
            http_config: None,
            tcp_config: None,
            udp_config: None,
        };

        let result = execute_fault_detect_plan("127.0.0.1", &plan).await;

        assert!(!result.success);
        assert!(result.message.contains("failed"));
    }

    #[tokio::test]
    async fn tcp_fault_detect_executor_matches_expected_response() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port() as u32;
        let _server_task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0; 1024];
            let read_size = stream.read(&mut buf).await.unwrap();
            assert_eq!(&buf[..read_size], b"ping");
            stream.write_all(b"pong").await.unwrap();
        });
        let plan = FaultDetectPlan {
            rule_id: "fd-tcp".to_string(),
            namespace: "default".to_string(),
            service: "orders".to_string(),
            interval_secs: 5,
            timeout_secs: 1,
            port,
            protocol: FaultDetectProtocol::Tcp,
            http_config: None,
            tcp_config: Some(TcpFaultDetectConfig {
                send: "ping".to_string(),
                receive: vec!["pong".to_string()],
            }),
            udp_config: None,
        };

        let result = execute_fault_detect_plan("127.0.0.1", &plan).await;

        assert!(result.success, "{}", result.message);
    }

    #[tokio::test]
    async fn http_fault_detect_executor_succeeds_when_endpoint_returns_success() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port() as u32;
        let _server_task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0; 1024];
            let _ = stream.read(&mut buf).await.unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK")
                .await
                .unwrap();
        });
        let plan = FaultDetectPlan {
            rule_id: "fd-http".to_string(),
            namespace: "default".to_string(),
            service: "orders".to_string(),
            interval_secs: 5,
            timeout_secs: 1,
            port,
            protocol: FaultDetectProtocol::Http,
            http_config: Some(HttpFaultDetectConfig {
                method: "GET".to_string(),
                url: "/ready".to_string(),
                headers: Vec::new(),
                body: String::new(),
            }),
            tcp_config: None,
            udp_config: None,
        };

        let result = execute_fault_detect_plan("127.0.0.1", &plan).await;

        assert!(result.success, "{}", result.message);
    }

    #[tokio::test]
    async fn http_fault_detect_executor_fails_when_endpoint_returns_error() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port() as u32;
        let _server_task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0; 1024];
            let _ = stream.read(&mut buf).await.unwrap();
            stream
                .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        });
        let plan = FaultDetectPlan {
            rule_id: "fd-http".to_string(),
            namespace: "default".to_string(),
            service: "orders".to_string(),
            interval_secs: 5,
            timeout_secs: 1,
            port,
            protocol: FaultDetectProtocol::Http,
            http_config: Some(HttpFaultDetectConfig {
                method: "GET".to_string(),
                url: "/ready".to_string(),
                headers: Vec::new(),
                body: String::new(),
            }),
            tcp_config: None,
            udp_config: None,
        };

        let result = execute_fault_detect_plan("127.0.0.1", &plan).await;

        assert!(!result.success);
        assert!(result.message.contains("503"));
    }

    #[tokio::test]
    async fn udp_fault_detect_executor_matches_expected_response() {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = socket.local_addr().unwrap().port() as u32;
        let _server_task = tokio::spawn(async move {
            let mut buf = [0; 1024];
            let (read_size, peer) = socket.recv_from(&mut buf).await.unwrap();
            assert_eq!(&buf[..read_size], b"ping");
            socket.send_to(b"pong", peer).await.unwrap();
        });
        let plan = FaultDetectPlan {
            rule_id: "fd-udp".to_string(),
            namespace: "default".to_string(),
            service: "orders".to_string(),
            interval_secs: 5,
            timeout_secs: 1,
            port,
            protocol: FaultDetectProtocol::Udp,
            http_config: None,
            tcp_config: None,
            udp_config: Some(UdpFaultDetectConfig {
                send: "ping".to_string(),
                receive: vec!["pong".to_string()],
            }),
        };

        let result = execute_fault_detect_plan("127.0.0.1", &plan).await;

        assert!(result.success, "{}", result.message);
    }
}
