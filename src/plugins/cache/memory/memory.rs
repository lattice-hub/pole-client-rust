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

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::RwLock;

use super::failover::DiskCacheFailover;
use crate::core::model::cache::{
    CacheItemType, CircuitBreakerRulesCacheItem, ConfigFileCacheItem, ConfigGroupCacheItem,
    EventType, FaultDetectRulesCacheItem, LaneRulesCacheItem, LosslessRulesCacheItem,
    RatelimitRulesCacheItem, RegistryCacheValue, RemoteData, ResourceEventKey,
    RouterRulesCacheItem, ServerEvent, ServiceInstancesCacheItem, ServicesCacheItem,
    TrafficMirrorRulesCacheItem, TrafficMockRulesCacheItem, TrafficSecurityRulesCacheItem,
};
use crate::core::model::config::{ConfigFile, ConfigGroup};
use crate::core::model::error::{ErrorCode, PoleError};
use crate::core::model::naming::{Instance, ServiceRule, Services};
use crate::core::plugin::cache::{
    Action, Filter, InitResourceCacheOption, ResourceCache, ResourceCacheFailover, ResourceListener,
};
use crate::core::plugin::connector::{Connector, ResourceHandler};
use crate::core::plugin::plugins::Plugin;
use crate::{error, info};
use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, RwLock as SyncRwLock};

static MEMORY_CACHE_NAME: &str = "memory";

struct MemoryResourceHandler {
    failover: SyncRwLock<Option<Arc<dyn ResourceCacheFailover>>>,
    // 资源类型变化监听
    listeners: Arc<RwLock<HashMap<EventType, Vec<Arc<dyn ResourceListener>>>>>,
    // services 服务列表缓存
    services: Arc<RwLock<HashMap<String, ServicesCacheItem>>>,
    // instances 服务实例缓存 key: namespace#service
    instances: Arc<RwLock<HashMap<String, ServiceInstancesCacheItem>>>,
    // router_rules 路由规则缓存 key: namespace#service
    router_rules: Arc<RwLock<HashMap<String, RouterRulesCacheItem>>>,
    // ratelimit_rules 限流规则缓存 key: namespace#service
    ratelimit_rules: Arc<RwLock<HashMap<String, RatelimitRulesCacheItem>>>,
    // circuitbreaker_rules 熔断规则缓存 key: namespace#service
    circuitbreaker_rules: Arc<RwLock<HashMap<String, CircuitBreakerRulesCacheItem>>>,
    // faultdetect_rules 主动探测规则缓存 key: namespace#service
    faultdetect_rules: Arc<RwLock<HashMap<String, FaultDetectRulesCacheItem>>>,
    // lane_rules 泳道规则缓存 key: namespace#service
    lane_rules: Arc<RwLock<HashMap<String, LaneRulesCacheItem>>>,
    // lossless_rules 优雅上下线规则缓存 key: namespace#service
    lossless_rules: Arc<RwLock<HashMap<String, LosslessRulesCacheItem>>>,
    // traffic_security_rules 流量安全规则缓存 key: namespace#service
    traffic_security_rules: Arc<RwLock<HashMap<String, TrafficSecurityRulesCacheItem>>>,
    // traffic_mirror_rules 流量镜像规则缓存 key: namespace#service
    traffic_mirror_rules: Arc<RwLock<HashMap<String, TrafficMirrorRulesCacheItem>>>,
    // traffic_mock_rules 流量 Mock 规则缓存 key: namespace#service
    traffic_mock_rules: Arc<RwLock<HashMap<String, TrafficMockRulesCacheItem>>>,
    // config_groups 配置分组缓存 key: namespace#group_name
    config_groups: Arc<RwLock<HashMap<String, ConfigGroupCacheItem>>>,
    // config_files 配置文件缓存 key: namespace#group_name#file_name
    config_files: Arc<RwLock<HashMap<String, ConfigFileCacheItem>>>,
}

pub struct MemoryCache {
    opt: InitResourceCacheOption,
    // 资源连接器
    server_connector: Arc<Box<dyn Connector>>,
    handler: Arc<MemoryResourceHandler>,
    remote_sender: UnboundedSender<RemoteData>,
}

impl MemoryCache {
    pub fn builder() -> (
        fn(InitResourceCacheOption) -> Box<dyn ResourceCache>,
        String,
    ) {
        (new_resource_cache, MEMORY_CACHE_NAME.to_string())
    }

    async fn run_remote_data_recive(
        handler: Arc<MemoryResourceHandler>,
        remote_reciver: &mut UnboundedReceiver<RemoteData>,
    ) {
        loop {
            tokio::select! {
                remote_data = remote_reciver.recv() => {
                    if let Some(remote_data) = remote_data {
                        MemoryCache::on_spec_event(handler.clone(), remote_data).await;
                    }
                }
            }
        }
    }

    fn submit_resource_watch(&self, event_type: EventType, resource_key: ResourceEventKey) {
        let search_namespace = resource_key.namespace.clone();
        info!(
            "[pole][resource_cache][memory] load remote resource: {:?}",
            resource_key
        );
        let server_connector = self.server_connector.clone();
        let remote_reciver = self.remote_sender.clone();
        // 提交数据异步同步任务
        self.opt.runtime.spawn(async move {
            let register_ret = server_connector
                .register_resource_handler(Box::new(MemoryResourceWatcher::new(
                    remote_reciver,
                    ResourceEventKey {
                        namespace: search_namespace,
                        event_type,
                        filter: resource_key.filter,
                    },
                )))
                .await;
            if register_ret.is_err() {
                error!(
                    "[pole][resource_cache][memory] register resource handler failed: {}, err: {}",
                    resource_key.namespace.clone(),
                    register_ret.err().unwrap()
                );
            }
        });
    }

    async fn recover_service_rule_from_failover(&self, filter: &Filter) {
        if !filter.include_cache {
            return;
        }
        let failover = self
            .handler
            .failover
            .read()
            .ok()
            .and_then(|provider| provider.clone());
        let Some(failover) = failover else {
            return;
        };
        let Ok(response) = failover.failover_naming_load(filter.clone()).await else {
            return;
        };
        if response.service.is_none() {
            return;
        }
        Self::on_spec_event(
            self.handler.clone(),
            RemoteData {
                event_key: filter.resource_key.clone(),
                discover_value: Some(response),
                config_value: None,
            },
        )
        .await;
    }

    async fn on_spec_event(handler: Arc<MemoryResourceHandler>, event: RemoteData) {
        info!("[pole][resource_cache][memory] on spec event: {:?}", event);
        let mut notify_event = ServerEvent {
            event_key: event.event_key.clone(),
            value: CacheItemType::Unknown,
        };

        let event_key = event.event_key.clone();
        let event_type = event_key.event_type;
        let filter = event_key.filter;

        let copy_event = event.clone();

        match event_type {
            EventType::Service => {}
            EventType::Instance => {
                let remote_val = event.discover_value.unwrap();
                let svc = remote_val.service.unwrap();
                let mut safe_map = handler.instances.write().await;
                let cache_val_opt = safe_map
                    .get_mut(format!("{}#{}", svc.namespace.clone(), svc.name.clone()).as_str());
                if cache_val_opt.is_none() {
                    error!(
                        "[pole][resource_cache][memory] service_instance cache not found: namespace={} service={}",
                        svc.namespace,
                        svc.name
                    );
                    return;
                }
                let cache_val = cache_val_opt.unwrap();
                let mut instances = cache_val.value.write().await;
                let mut available_instances = cache_val.available_instances.write().await;

                instances.clear();
                available_instances.clear();
                let remote_instances = remote_val.instances;
                for (_, val) in remote_instances.iter().enumerate() {
                    let instance = Instance::convert_from_spec(val.clone());
                    if instance.is_available() {
                        available_instances.push(instance.clone());
                    }
                    instances.push(instance);
                }

                cache_val.revision = svc.revision;
                cache_val.finish_initialize();
                notify_event.value = CacheItemType::Instance(cache_val.clone());
            }
            EventType::RouterRule => {
                let remote_val = event.discover_value.unwrap();
                let svc = remote_val.service.unwrap();
                let mut safe_map = handler.router_rules.write().await;
                let cache_val_opt = safe_map
                    .get_mut(format!("{}#{}", svc.namespace.clone(), svc.name.clone()).as_str());
                if cache_val_opt.is_none() {
                    error!(
                        "[pole][resource_cache][memory] router_rule cache not found: namespace={} service={}",
                        svc.namespace,
                        svc.name
                    );
                    return;
                }
                let cache_val = cache_val_opt.unwrap();
                let mut rules = cache_val.value.write().await;
                rules.clear();
                rules.extend(remote_val.custom_route_rules);

                cache_val.revision = svc.revision;
                cache_val.finish_initialize();
                notify_event.value = CacheItemType::RouterRule(cache_val.clone());
            }
            EventType::CircuitBreakerRule => {
                let remote_val = event.discover_value.unwrap();
                let svc = remote_val.service.unwrap();
                let mut safe_map = handler.circuitbreaker_rules.write().await;
                let cache_val_opt = safe_map
                    .get_mut(format!("{}#{}", svc.namespace.clone(), svc.name.clone()).as_str());
                if cache_val_opt.is_none() {
                    error!(
                        "[pole][resource_cache][memory] circuit_breaker cache not found: namespace={} service={}",
                        svc.namespace,
                        svc.name
                    );
                    return;
                }
                let cache_val = cache_val_opt.unwrap();
                cache_val.value.clone_from(&remote_val.circuit_breaker);

                cache_val.revision = svc.revision;
                cache_val.finish_initialize();
                notify_event.value = CacheItemType::CircuitBreakerRule(cache_val.clone());
            }
            EventType::RateLimitRule => {
                let remote_val = event.discover_value.unwrap();
                let svc = remote_val.service.unwrap();
                let mut safe_map = handler.ratelimit_rules.write().await;
                let cache_val_opt = safe_map
                    .get_mut(format!("{}#{}", svc.namespace.clone(), svc.name.clone()).as_str());
                if cache_val_opt.is_none() {
                    error!(
                        "[pole][resource_cache][memory] ratelimit cache not found: namespace={} service={}",
                        svc.namespace,
                        svc.name
                    );
                    return;
                }
                let cache_val = cache_val_opt.unwrap();
                cache_val.value.clone_from(&remote_val.rate_limit);

                cache_val.revision = svc.revision;
                cache_val.finish_initialize();
                notify_event.value = CacheItemType::RateLimitRule(cache_val.clone());
            }
            EventType::FaultDetectRule => {
                let remote_val = event.discover_value.unwrap();
                let svc = remote_val.service.unwrap();
                let mut safe_map = handler.faultdetect_rules.write().await;
                let cache_val_opt = safe_map
                    .get_mut(format!("{}#{}", svc.namespace.clone(), svc.name.clone()).as_str());
                if cache_val_opt.is_none() {
                    error!(
                        "[pole][resource_cache][memory] fault_detect cache not found: namespace={} service={}",
                        svc.namespace,
                        svc.name
                    );
                    return;
                }
                let cache_val = cache_val_opt.unwrap();
                cache_val.value.clone_from(&remote_val.fault_detect_rules);

                cache_val.revision = svc.revision;
                cache_val.finish_initialize();
                notify_event.value = CacheItemType::FaultDetectRule(cache_val.clone());
            }
            EventType::LaneRule => {
                let remote_val = event.discover_value.unwrap();
                let svc = remote_val.service.unwrap();
                let mut safe_map = handler.lane_rules.write().await;
                let cache_val_opt = safe_map
                    .get_mut(format!("{}#{}", svc.namespace.clone(), svc.name.clone()).as_str());
                if cache_val_opt.is_none() {
                    error!(
                        "[pole][resource_cache][memory] lane cache not found: namespace={} service={}",
                        svc.namespace,
                        svc.name
                    );
                    return;
                }
                let cache_val = cache_val_opt.unwrap();
                cache_val.value.clone_from(&remote_val.lanes);

                cache_val.revision = svc.revision;
                cache_val.finish_initialize();
                notify_event.value = CacheItemType::LaneRule(cache_val.clone());
            }
            EventType::LosslessRule => {
                let remote_val = event.discover_value.unwrap();
                let svc = remote_val.service.unwrap();
                let mut safe_map = handler.lossless_rules.write().await;
                let cache_val_opt = safe_map
                    .get_mut(format!("{}#{}", svc.namespace.clone(), svc.name.clone()).as_str());
                if cache_val_opt.is_none() {
                    error!(
                        "[pole][resource_cache][memory] lossless cache not found: namespace={} service={}",
                        svc.namespace,
                        svc.name
                    );
                    return;
                }
                let cache_val = cache_val_opt.unwrap();
                cache_val.value.clone_from(&remote_val.lossless_rules);

                cache_val.revision = svc.revision;
                cache_val.finish_initialize();
                notify_event.value = CacheItemType::LosslessRule(cache_val.clone());
            }
            EventType::TrafficSecurityRule => {
                let remote_val = event.discover_value.unwrap();
                let svc = remote_val.service.unwrap();
                let mut safe_map = handler.traffic_security_rules.write().await;
                let cache_val_opt = safe_map
                    .get_mut(format!("{}#{}", svc.namespace.clone(), svc.name.clone()).as_str());
                if cache_val_opt.is_none() {
                    error!(
                        "[pole][resource_cache][memory] traffic_security cache not found: namespace={} service={}",
                        svc.namespace,
                        svc.name
                    );
                    return;
                }
                let cache_val = cache_val_opt.unwrap();
                cache_val
                    .value
                    .clone_from(&remote_val.traffic_security_rules);

                cache_val.revision = svc.revision;
                cache_val.finish_initialize();
                notify_event.value = CacheItemType::TrafficSecurityRule(cache_val.clone());
            }
            EventType::TrafficMirrorRule => {
                let remote_val = event.discover_value.unwrap();
                let svc = remote_val.service.unwrap();
                let mut safe_map = handler.traffic_mirror_rules.write().await;
                let cache_val_opt = safe_map
                    .get_mut(format!("{}#{}", svc.namespace.clone(), svc.name.clone()).as_str());
                if cache_val_opt.is_none() {
                    error!(
                        "[pole][resource_cache][memory] traffic_mirror cache not found: namespace={} service={}",
                        svc.namespace,
                        svc.name
                    );
                    return;
                }
                let cache_val = cache_val_opt.unwrap();
                cache_val.value.clone_from(&remote_val.traffic_mirror_rules);

                cache_val.revision = svc.revision;
                cache_val.finish_initialize();
                notify_event.value = CacheItemType::TrafficMirrorRule(cache_val.clone());
            }
            EventType::TrafficMockRule => {
                let remote_val = event.discover_value.unwrap();
                let svc = remote_val.service.unwrap();
                let mut safe_map = handler.traffic_mock_rules.write().await;
                let cache_val_opt = safe_map
                    .get_mut(format!("{}#{}", svc.namespace.clone(), svc.name.clone()).as_str());
                if cache_val_opt.is_none() {
                    error!(
                        "[pole][resource_cache][memory] traffic_mock cache not found: namespace={} service={}",
                        svc.namespace,
                        svc.name
                    );
                    return;
                }
                let cache_val = cache_val_opt.unwrap();
                cache_val.value.clone_from(&remote_val.traffic_mock_rules);

                cache_val.revision = svc.revision;
                cache_val.finish_initialize();
                notify_event.value = CacheItemType::TrafficMockRule(cache_val.clone());
            }
            EventType::ConfigFile => {
                let search_key = format!(
                    "{}#{}#{}",
                    event_key.namespace.clone(),
                    filter.get("group").unwrap(),
                    filter.get("file").unwrap()
                );

                let mut safe_map = handler.config_files.write().await;
                let cache_val_opt = safe_map.get_mut(search_key.as_str());
                if cache_val_opt.is_none() {
                    error!(
                        "[pole][resource_cache][memory] config_file cache not found: namespace={} group={} file={}",
                        event_key.namespace.clone(),
                        filter.get("group").unwrap(),
                        filter.get("file").unwrap()
                    );
                    return;
                }
                let cache_val = cache_val_opt.unwrap();
                let remote_value = event.config_value.unwrap();
                let revision = remote_value.revision.clone();
                let remote_rules = remote_value.file.unwrap_or_default();
                cache_val.value.clone_from(&remote_rules);

                cache_val.revision = revision;
                cache_val.finish_initialize();
                notify_event.value = CacheItemType::ConfigFile(cache_val.clone());
            }
            EventType::ConfigGroup => {
                let remote_val = event.config_value.unwrap();
                let search_key = format!(
                    "{}#{}",
                    event_key.namespace.clone(),
                    filter.get("group").unwrap(),
                );

                let mut safe_map = handler.config_groups.write().await;
                let cache_val_opt = safe_map.get_mut(search_key.as_str());
                if cache_val_opt.is_none() {
                    error!(
                        "[pole][resource_cache][memory] config_group cache not found: namespace={} group={}",
                        event_key.namespace.clone(),
                        filter.get("group").unwrap()
                    );
                    return;
                }
                let cache_val = cache_val_opt.unwrap();
                let files = &mut cache_val.files.write().await;
                let remote_rules = remote_val.file_names;
                files.clear();
                for ele in remote_rules {
                    files.push(ConfigFile::convert_from_spec(ele));
                }

                cache_val.revision = remote_val.revision;
                cache_val.finish_initialize();
                notify_event.value = CacheItemType::ConfigGroup(cache_val.clone());
            }
            _ => {}
        }

        // 容灾调用
        if copy_event.discover_value.is_some() {
            let failover = handler
                .failover
                .read()
                .ok()
                .and_then(|provider| provider.clone());
            if let Some(failover) = failover {
                let _ = failover
                    .save_naming_failover(copy_event.discover_value.unwrap())
                    .await;
            }
        }
        if copy_event.config_value.is_some() {
            let failover = handler
                .failover
                .read()
                .ok()
                .and_then(|provider| provider.clone());
            if let Some(failover) = failover {
                let _ = failover
                    .save_config_failover(copy_event.config_value.unwrap())
                    .await;
            }
        }

        // 通知所有的 listener
        let listeners = { handler.listeners.read().await.clone() };
        let expect_watcher_opt = listeners.get(&event_type);
        if let Some(expect_watcher) = expect_watcher_opt {
            for (_index, listener) in expect_watcher.iter().enumerate() {
                listener
                    .on_event(Action::Update, notify_event.clone())
                    .await;
            }
        }
    }
}

fn new_resource_cache(opt: InitResourceCacheOption) -> Box<dyn ResourceCache> {
    let (sx, mut rx) = mpsc::unbounded_channel::<RemoteData>();
    let server_connector = opt.server_connector.clone();
    let failover = Arc::new(DiskCacheFailover::new(opt.conf.clone()));

    let mc = MemoryCache {
        opt,
        server_connector,
        handler: Arc::new(MemoryResourceHandler {
            failover: SyncRwLock::new(Some(failover)),
            listeners: Arc::new(RwLock::new(HashMap::new())),
            services: Arc::new(RwLock::new(HashMap::new())),
            instances: Arc::new(RwLock::new(HashMap::new())),
            router_rules: Arc::new(RwLock::new(HashMap::new())),
            ratelimit_rules: Arc::new(RwLock::new(HashMap::new())),
            circuitbreaker_rules: Arc::new(RwLock::new(HashMap::new())),
            faultdetect_rules: Arc::new(RwLock::new(HashMap::new())),
            lane_rules: Arc::new(RwLock::new(HashMap::new())),
            lossless_rules: Arc::new(RwLock::new(HashMap::new())),
            traffic_security_rules: Arc::new(RwLock::new(HashMap::new())),
            traffic_mirror_rules: Arc::new(RwLock::new(HashMap::new())),
            traffic_mock_rules: Arc::new(RwLock::new(HashMap::new())),
            config_groups: Arc::new(RwLock::new(HashMap::new())),
            config_files: Arc::new(RwLock::new(HashMap::new())),
        }),
        remote_sender: sx,
    };

    let handler = mc.handler.clone();
    mc.opt.runtime.spawn(async move {
        MemoryCache::run_remote_data_recive(handler, &mut rx).await;
    });

    Box::new(mc) as Box<dyn ResourceCache + 'static>
}

impl Plugin for MemoryCache {
    fn init(&mut self) {}

    fn destroy(&self) {}

    fn name(&self) -> String {
        MEMORY_CACHE_NAME.to_string()
    }
}

#[async_trait::async_trait]
impl ResourceCache for MemoryCache {
    fn set_failover_provider(&mut self, failover: Arc<dyn ResourceCacheFailover>) {
        *self.handler.failover.write().unwrap() = Some(failover);
    }

    async fn load_service_rule(&self, filter: Filter) -> Result<ServiceRule, PoleError> {
        // 治理规则按 namespace#service 缓存。首次读取时创建占位缓存并提交 watch，
        // 随后在锁外等待初始化完成，最后以 Any 列表返回给各治理能力自行 downcast。
        let event_type = filter.get_event_type();
        let search_namespace = filter.resource_key.namespace.clone();
        let search_service = filter.resource_key.filter.get("service").unwrap();
        let search_key = format!("{}#{}", search_namespace.clone(), search_service);

        match event_type {
            EventType::RouterRule => {
                // 等待资源
                {
                    let resource_key = filter.resource_key.clone();
                    let mut safe_map = self.handler.router_rules.write().await;
                    let _ = safe_map.entry(search_key.clone()).or_insert_with(|| {
                        self.submit_resource_watch(EventType::RouterRule, resource_key);
                        RouterRulesCacheItem::new()
                    });
                }

                // 这里进行无锁等待资源的加载完成
                let waiter = {
                    let safe_map = self.handler.router_rules.read().await;
                    let cache_val = safe_map.get(&search_key).unwrap();

                    cache_val.wait_initialize(filter.timeout).await
                };
                waiter();

                let initialized = {
                    let safe_map = self.handler.router_rules.read().await;
                    safe_map.get(&search_key).unwrap().is_initialized()
                };
                if !initialized {
                    self.recover_service_rule_from_failover(&filter).await;
                }
                let safe_map = self.handler.router_rules.read().await;
                let cache_val = safe_map.get(&search_key).unwrap();
                // 如果还是没有初始化
                if !cache_val.is_initialized() {
                    return Err(PoleError::new(
                        ErrorCode::InternalError,
                        "load remote resource timeout".to_string(),
                    ));
                }

                let mut rules = vec![];
                for (_, val) in cache_val.value.read().await.iter().enumerate() {
                    rules.push(Box::new(val.clone()) as Box<dyn Any + Send>);
                }
                Ok(ServiceRule {
                    rules,
                    revision: cache_val.revision(),
                    initialized: cache_val.is_initialized(),
                })
            }
            EventType::RateLimitRule => {
                // 等待资源
                {
                    let resource_key = filter.resource_key.clone();
                    let mut safe_map = self.handler.ratelimit_rules.write().await;
                    let _ = safe_map.entry(search_key.clone()).or_insert_with(|| {
                        self.submit_resource_watch(EventType::RateLimitRule, resource_key);
                        RatelimitRulesCacheItem::new()
                    });
                }

                // 这里进行无锁等待资源的加载完成
                let waiter = {
                    let safe_map = self.handler.ratelimit_rules.read().await;
                    let cache_val = safe_map.get(&search_key).unwrap();

                    cache_val.wait_initialize(filter.timeout).await
                };
                waiter();

                let initialized = {
                    let safe_map = self.handler.ratelimit_rules.read().await;
                    safe_map.get(&search_key).unwrap().is_initialized()
                };
                if !initialized {
                    self.recover_service_rule_from_failover(&filter).await;
                }
                let safe_map = self.handler.ratelimit_rules.read().await;
                let cache_val = safe_map.get(&search_key).unwrap();
                // 如果还是没有初始化
                if !cache_val.is_initialized() {
                    return Err(PoleError::new(
                        ErrorCode::InternalError,
                        "load remote resource timeout".to_string(),
                    ));
                }

                Ok(ServiceRule {
                    rules: cache_val
                        .value
                        .iter()
                        .map(|val| Box::new(val.clone()) as Box<dyn Any + Send>)
                        .collect(),
                    revision: cache_val.revision(),
                    initialized: cache_val.is_initialized(),
                })
            }
            EventType::CircuitBreakerRule => {
                // 等待资源
                {
                    let resource_key = filter.resource_key.clone();
                    let mut safe_map = self.handler.circuitbreaker_rules.write().await;
                    let _ = safe_map.entry(search_key.clone()).or_insert_with(|| {
                        self.submit_resource_watch(EventType::CircuitBreakerRule, resource_key);
                        CircuitBreakerRulesCacheItem::new()
                    });
                }

                // 这里进行无锁等待资源的加载完成
                let waiter = {
                    let safe_map = self.handler.circuitbreaker_rules.read().await;
                    let cache_val = safe_map.get(&search_key).unwrap();

                    cache_val.wait_initialize(filter.timeout).await
                };
                waiter();

                let initialized = {
                    let safe_map = self.handler.circuitbreaker_rules.read().await;
                    safe_map.get(&search_key).unwrap().is_initialized()
                };
                if !initialized {
                    self.recover_service_rule_from_failover(&filter).await;
                }
                let safe_map = self.handler.circuitbreaker_rules.read().await;
                let cache_val = safe_map.get(&search_key).unwrap();
                // 如果还是没有初始化
                if !cache_val.is_initialized() {
                    return Err(PoleError::new(
                        ErrorCode::InternalError,
                        "load remote resource timeout".to_string(),
                    ));
                }

                Ok(ServiceRule {
                    rules: cache_val
                        .value
                        .iter()
                        .map(|val| Box::new(val.clone()) as Box<dyn Any + Send>)
                        .collect(),
                    revision: cache_val.revision(),
                    initialized: cache_val.is_initialized(),
                })
            }
            EventType::FaultDetectRule => {
                // 等待资源
                {
                    let resource_key = filter.resource_key.clone();
                    let mut safe_map = self.handler.faultdetect_rules.write().await;
                    let _ = safe_map.entry(search_key.clone()).or_insert_with(|| {
                        self.submit_resource_watch(EventType::FaultDetectRule, resource_key);
                        FaultDetectRulesCacheItem::new()
                    });
                }

                // 这里进行无锁等待资源的加载完成
                let waiter = {
                    let safe_map = self.handler.faultdetect_rules.read().await;
                    let cache_val = safe_map.get(&search_key).unwrap();

                    cache_val.wait_initialize(filter.timeout).await
                };
                waiter();

                let initialized = {
                    let safe_map = self.handler.faultdetect_rules.read().await;
                    safe_map.get(&search_key).unwrap().is_initialized()
                };
                if !initialized {
                    self.recover_service_rule_from_failover(&filter).await;
                }
                let safe_map = self.handler.faultdetect_rules.read().await;
                let cache_val = safe_map.get(&search_key).unwrap();
                // 如果还是没有初始化
                if !cache_val.is_initialized() {
                    return Err(PoleError::new(
                        ErrorCode::InternalError,
                        "load remote resource timeout".to_string(),
                    ));
                }

                Ok(ServiceRule {
                    rules: vec![Box::new(cache_val.value.clone()) as Box<dyn Any + Send>],
                    revision: cache_val.revision(),
                    initialized: cache_val.is_initialized(),
                })
            }
            EventType::LaneRule => {
                {
                    let resource_key = filter.resource_key.clone();
                    let mut safe_map = self.handler.lane_rules.write().await;
                    let _ = safe_map.entry(search_key.clone()).or_insert_with(|| {
                        self.submit_resource_watch(EventType::LaneRule, resource_key);
                        LaneRulesCacheItem::new()
                    });
                }

                let waiter = {
                    let safe_map = self.handler.lane_rules.read().await;
                    let cache_val = safe_map.get(&search_key).unwrap();

                    cache_val.wait_initialize(filter.timeout).await
                };
                waiter();

                let initialized = {
                    let safe_map = self.handler.lane_rules.read().await;
                    safe_map.get(&search_key).unwrap().is_initialized()
                };
                if !initialized {
                    self.recover_service_rule_from_failover(&filter).await;
                }
                let safe_map = self.handler.lane_rules.read().await;
                let cache_val = safe_map.get(&search_key).unwrap();
                if !cache_val.is_initialized() {
                    return Err(PoleError::new(
                        ErrorCode::InternalError,
                        "load remote resource timeout".to_string(),
                    ));
                }

                Ok(ServiceRule {
                    rules: cache_val
                        .value
                        .iter()
                        .map(|val| Box::new(val.clone()) as Box<dyn Any + Send>)
                        .collect(),
                    revision: cache_val.revision(),
                    initialized: cache_val.is_initialized(),
                })
            }
            EventType::LosslessRule => {
                {
                    let resource_key = filter.resource_key.clone();
                    let mut safe_map = self.handler.lossless_rules.write().await;
                    let _ = safe_map.entry(search_key.clone()).or_insert_with(|| {
                        self.submit_resource_watch(EventType::LosslessRule, resource_key);
                        LosslessRulesCacheItem::new()
                    });
                }

                let waiter = {
                    let safe_map = self.handler.lossless_rules.read().await;
                    let cache_val = safe_map.get(&search_key).unwrap();

                    cache_val.wait_initialize(filter.timeout).await
                };
                waiter();

                let initialized = {
                    let safe_map = self.handler.lossless_rules.read().await;
                    safe_map.get(&search_key).unwrap().is_initialized()
                };
                if !initialized {
                    self.recover_service_rule_from_failover(&filter).await;
                }
                let safe_map = self.handler.lossless_rules.read().await;
                let cache_val = safe_map.get(&search_key).unwrap();
                if !cache_val.is_initialized() {
                    return Err(PoleError::new(
                        ErrorCode::InternalError,
                        "load remote resource timeout".to_string(),
                    ));
                }

                Ok(ServiceRule {
                    rules: cache_val
                        .value
                        .iter()
                        .map(|val| Box::new(val.clone()) as Box<dyn Any + Send>)
                        .collect(),
                    revision: cache_val.revision(),
                    initialized: cache_val.is_initialized(),
                })
            }
            EventType::TrafficSecurityRule => {
                {
                    let resource_key = filter.resource_key.clone();
                    let mut safe_map = self.handler.traffic_security_rules.write().await;
                    let _ = safe_map.entry(search_key.clone()).or_insert_with(|| {
                        self.submit_resource_watch(EventType::TrafficSecurityRule, resource_key);
                        TrafficSecurityRulesCacheItem::new()
                    });
                }

                let waiter = {
                    let safe_map = self.handler.traffic_security_rules.read().await;
                    let cache_val = safe_map.get(&search_key).unwrap();

                    cache_val.wait_initialize(filter.timeout).await
                };
                waiter();

                let initialized = {
                    let safe_map = self.handler.traffic_security_rules.read().await;
                    safe_map.get(&search_key).unwrap().is_initialized()
                };
                if !initialized {
                    self.recover_service_rule_from_failover(&filter).await;
                }
                let safe_map = self.handler.traffic_security_rules.read().await;
                let cache_val = safe_map.get(&search_key).unwrap();
                if !cache_val.is_initialized() {
                    return Err(PoleError::new(
                        ErrorCode::InternalError,
                        "load remote resource timeout".to_string(),
                    ));
                }

                Ok(ServiceRule {
                    rules: cache_val
                        .value
                        .iter()
                        .map(|val| Box::new(val.clone()) as Box<dyn Any + Send>)
                        .collect(),
                    revision: cache_val.revision(),
                    initialized: cache_val.is_initialized(),
                })
            }
            EventType::TrafficMirrorRule => {
                {
                    let resource_key = filter.resource_key.clone();
                    let mut safe_map = self.handler.traffic_mirror_rules.write().await;
                    let _ = safe_map.entry(search_key.clone()).or_insert_with(|| {
                        self.submit_resource_watch(EventType::TrafficMirrorRule, resource_key);
                        TrafficMirrorRulesCacheItem::new()
                    });
                }

                let waiter = {
                    let safe_map = self.handler.traffic_mirror_rules.read().await;
                    let cache_val = safe_map.get(&search_key).unwrap();

                    cache_val.wait_initialize(filter.timeout).await
                };
                waiter();

                let initialized = {
                    let safe_map = self.handler.traffic_mirror_rules.read().await;
                    safe_map.get(&search_key).unwrap().is_initialized()
                };
                if !initialized {
                    self.recover_service_rule_from_failover(&filter).await;
                }
                let safe_map = self.handler.traffic_mirror_rules.read().await;
                let cache_val = safe_map.get(&search_key).unwrap();
                if !cache_val.is_initialized() {
                    return Err(PoleError::new(
                        ErrorCode::InternalError,
                        "load remote resource timeout".to_string(),
                    ));
                }

                Ok(ServiceRule {
                    rules: cache_val
                        .value
                        .iter()
                        .map(|val| Box::new(val.clone()) as Box<dyn Any + Send>)
                        .collect(),
                    revision: cache_val.revision(),
                    initialized: cache_val.is_initialized(),
                })
            }
            EventType::TrafficMockRule => {
                {
                    let resource_key = filter.resource_key.clone();
                    let mut safe_map = self.handler.traffic_mock_rules.write().await;
                    let _ = safe_map.entry(search_key.clone()).or_insert_with(|| {
                        self.submit_resource_watch(EventType::TrafficMockRule, resource_key);
                        TrafficMockRulesCacheItem::new()
                    });
                }

                let waiter = {
                    let safe_map = self.handler.traffic_mock_rules.read().await;
                    let cache_val = safe_map.get(&search_key).unwrap();

                    cache_val.wait_initialize(filter.timeout).await
                };
                waiter();

                let initialized = {
                    let safe_map = self.handler.traffic_mock_rules.read().await;
                    safe_map.get(&search_key).unwrap().is_initialized()
                };
                if !initialized {
                    self.recover_service_rule_from_failover(&filter).await;
                }
                let safe_map = self.handler.traffic_mock_rules.read().await;
                let cache_val = safe_map.get(&search_key).unwrap();
                if !cache_val.is_initialized() {
                    return Err(PoleError::new(
                        ErrorCode::InternalError,
                        "load remote resource timeout".to_string(),
                    ));
                }

                Ok(ServiceRule {
                    rules: cache_val
                        .value
                        .iter()
                        .map(|val| Box::new(val.clone()) as Box<dyn Any + Send>)
                        .collect(),
                    revision: cache_val.revision(),
                    initialized: cache_val.is_initialized(),
                })
            }
            _ => {
                return Err(PoleError::new(
                    ErrorCode::InternalError,
                    "load remote resource timeout".to_string(),
                ));
            }
        }
    }

    async fn load_services(&self, filter: Filter) -> Result<Services, PoleError> {
        let search_key = filter.resource_key.namespace.clone();
        {
            let resource_key = filter.resource_key.clone();
            let mut safe_map = self.handler.services.write().await;
            let _ = safe_map.entry(search_key.clone()).or_insert_with(|| {
                self.submit_resource_watch(EventType::Service, resource_key);
                ServicesCacheItem::new()
            });
        }

        // 这里进行无锁等待资源的加载完成
        let waiter = {
            let safe_map = self.handler.services.read().await;
            let cache_val = safe_map.get(&search_key).unwrap();

            cache_val.wait_initialize(filter.timeout).await
        };
        waiter();

        let safe_map = self.handler.services.read().await;
        let cache_val = safe_map.get(&search_key).unwrap();
        // 如果还是没有初始化
        if !cache_val.is_initialized() {
            return Err(PoleError::new(
                ErrorCode::InternalError,
                "load remote resource timeout".to_string(),
            ));
        }

        let mut services = vec![];
        services.clone_from_slice(cache_val.value.read().await.as_slice());
        Ok(Services {
            service_list: services,
            initialized: cache_val.is_initialized(),
            revision: cache_val.revision(),
        })
    }

    async fn load_service_instances(
        &self,
        filter: Filter,
    ) -> Result<ServiceInstancesCacheItem, PoleError> {
        let search_namespace = filter.resource_key.namespace.clone();
        let search_service = filter.resource_key.filter.get("service").unwrap();
        let search_key = format!("{}#{}", search_namespace.clone(), search_service);
        {
            let resource_key = filter.resource_key.clone();
            let mut safe_map = self.handler.instances.write().await;
            let _ = safe_map.entry(search_key.clone()).or_insert_with(|| {
                self.submit_resource_watch(EventType::Instance, resource_key);
                ServiceInstancesCacheItem::new()
            });
        }

        // 这里进行无锁等待资源的加载完成
        let waiter = {
            let safe_map = self.handler.instances.read().await;
            let cache_val = safe_map.get(&search_key).unwrap();

            cache_val.wait_initialize(filter.timeout).await
        };
        waiter();

        let safe_map = self.handler.instances.read().await;
        let cache_val = safe_map.get(&search_key).unwrap();
        // 等待资源，这里直接 clone 出来一个对象做数据拷贝，避免 read 锁长期持有
        // 如果还是没有初始化
        if !cache_val.is_initialized() {
            return Err(PoleError::new(
                ErrorCode::InternalError,
                "load remote resource timeout".to_string(),
            ));
        }

        Ok(cache_val.clone())
    }

    async fn load_config_file(&self, filter: Filter) -> Result<ConfigFile, PoleError> {
        let search_namespace = filter.resource_key.namespace.clone();
        let search_group = filter.resource_key.filter.get("group").unwrap();
        let search_file = filter.resource_key.filter.get("file").unwrap();
        let search_key = format!(
            "{}#{}#{}",
            search_namespace.clone(),
            search_group,
            search_file
        );
        {
            let resource_key = filter.resource_key.clone();
            let mut safe_map = self.handler.config_files.write().await;
            let _ = safe_map.entry(search_key.clone()).or_insert_with(|| {
                self.submit_resource_watch(EventType::ConfigFile, resource_key);
                ConfigFileCacheItem::new()
            });
        }

        // 这里进行无锁等待资源的加载完成
        let waiter = {
            let safe_map = self.handler.config_files.read().await;
            let cache_val = safe_map.get(&search_key).unwrap();

            cache_val.wait_initialize(filter.timeout).await
        };
        waiter();
        let safe_map = self.handler.config_files.read().await;
        let cache_val = safe_map.get(&search_key).unwrap();
        // 如果还是没有初始化
        if !cache_val.is_initialized() {
            return Err(PoleError::new(
                ErrorCode::InternalError,
                "load remote resource timeout".to_string(),
            ));
        }

        Ok(cache_val.to_config_file())
    }

    async fn load_config_group_files(&self, filter: Filter) -> Result<ConfigGroup, PoleError> {
        let search_namespace = filter.resource_key.namespace.clone();
        let search_group = filter.resource_key.filter.get("group").unwrap();
        let search_key = format!("{}#{}", search_namespace.clone(), search_group);
        {
            let resource_key = filter.resource_key.clone();
            let mut safe_map = self.handler.config_groups.write().await;
            let _ = safe_map.entry(search_key.clone()).or_insert_with(|| {
                self.submit_resource_watch(EventType::ConfigGroup, resource_key);
                ConfigGroupCacheItem::new()
            });
        }

        // 这里进行无锁等待资源的加载完成
        let waiter = {
            let safe_map = self.handler.config_groups.read().await;
            let cache_val = safe_map.get(&search_key).unwrap();

            cache_val.wait_initialize(filter.timeout).await
        };
        waiter();

        let safe_map = self.handler.config_groups.read().await;
        let cache_val = safe_map.get(&search_key).unwrap();
        // 如果还是没有初始化
        if !cache_val.is_initialized() {
            return Err(PoleError::new(
                ErrorCode::InternalError,
                "load remote resource timeout".to_string(),
            ));
        }
        let ret = ConfigGroup {
            namespace: cache_val.namespace.clone(),
            group: cache_val.group.clone(),
            files: cache_val.files.read().await.clone(),
            revision: cache_val.revision.clone(),
        };

        Ok(ret)
    }

    async fn register_resource_listener(&self, listener: Arc<dyn ResourceListener>) {
        let watch_key = listener.watch_key();
        let mut safe_map = self.handler.listeners.write().await;
        let listeners = safe_map.entry(watch_key).or_insert_with(Vec::new);

        listeners.push(listener);
    }
}

/// MemoryResourceWatcher 用于监听远程资源变化
struct MemoryResourceWatcher {
    event_key: ResourceEventKey,
    processor: UnboundedSender<RemoteData>,
}

impl MemoryResourceWatcher {
    fn new(f: UnboundedSender<RemoteData>, event_key: ResourceEventKey) -> Self {
        Self {
            event_key,
            processor: f,
        }
    }
}

impl ResourceHandler for MemoryResourceWatcher {
    fn handle_event(&self, event: crate::core::model::cache::RemoteData) {
        match self.processor.send(event) {
            Ok(_) => {}
            Err(err) => {
                error!(
                    "[pole][resource_cache][memory] send event to processor failed: {}",
                    err
                );
            }
        }
    }

    fn interest_resource(&self) -> ResourceEventKey {
        self.event_key.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::cache::{
        FaultDetectRulesCacheItem, LaneRulesCacheItem, LosslessRulesCacheItem,
        TrafficMirrorRulesCacheItem, TrafficMockRulesCacheItem, TrafficSecurityRulesCacheItem,
    };
    use crate::core::plugin::cache::ResourceCacheFailover;
    use crate::core::plugin::connector::NoopConnector;
    use pole_specification::v1::{
        discover_response::DiscoverResponseType, CircuitBreakerRule, Code, ConfigDiscoverResponse,
        DiscoverResponse, FaultDetectRule, LaneGroup, LosslessRule, RateLimit, RouteRule, Service,
        TrafficMirror, TrafficMock, TrafficSecurityRule,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    struct NoopFailover;

    #[async_trait::async_trait]
    impl ResourceCacheFailover for NoopFailover {
        async fn failover_naming_load(
            &self,
            _filter: Filter,
        ) -> Result<DiscoverResponse, PoleError> {
            Ok(DiscoverResponse::default())
        }

        async fn save_naming_failover(&self, _value: DiscoverResponse) -> Result<(), PoleError> {
            Ok(())
        }

        async fn failover_config_load(
            &self,
            _filter: Filter,
        ) -> Result<ConfigDiscoverResponse, PoleError> {
            Ok(ConfigDiscoverResponse::default())
        }

        async fn save_config_failover(
            &self,
            _value: ConfigDiscoverResponse,
        ) -> Result<(), PoleError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingFailover {
        load_calls: AtomicUsize,
        save_calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ResourceCacheFailover for RecordingFailover {
        async fn failover_naming_load(
            &self,
            filter: Filter,
        ) -> Result<DiscoverResponse, PoleError> {
            self.load_calls.fetch_add(1, Ordering::SeqCst);
            Ok(governance_response(filter.get_event_type()))
        }

        async fn save_naming_failover(&self, _value: DiscoverResponse) -> Result<(), PoleError> {
            self.save_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn failover_config_load(
            &self,
            _filter: Filter,
        ) -> Result<ConfigDiscoverResponse, PoleError> {
            Ok(ConfigDiscoverResponse::default())
        }

        async fn save_config_failover(
            &self,
            _value: ConfigDiscoverResponse,
        ) -> Result<(), PoleError> {
            Ok(())
        }
    }

    fn governance_response(event_type: EventType) -> DiscoverResponse {
        let mut response = DiscoverResponse {
            code: Code::ExecuteSuccess as u32,
            service: Some(Service {
                namespace: "default".to_string(),
                name: "svc-a".to_string(),
                revision: "failover-rev".to_string(),
                ..Service::default()
            }),
            ..DiscoverResponse::default()
        };
        match event_type {
            EventType::RouterRule => {
                response.set_type(DiscoverResponseType::CustomRouteRule);
                response.custom_route_rules.push(RouteRule::default());
            }
            EventType::RateLimitRule => {
                response.set_type(DiscoverResponseType::RateLimit);
                response.rate_limit.push(RateLimit::default());
            }
            EventType::CircuitBreakerRule => {
                response.set_type(DiscoverResponseType::CircuitBreaker);
                response.circuit_breaker.push(CircuitBreakerRule::default());
            }
            EventType::FaultDetectRule => {
                response.set_type(DiscoverResponseType::FaultDetector);
                response.fault_detect_rules.push(FaultDetectRule::default());
            }
            EventType::LaneRule => {
                response.set_type(DiscoverResponseType::Lane);
                response.lanes.push(LaneGroup::default());
            }
            EventType::LosslessRule => {
                response.set_type(DiscoverResponseType::Lossless);
                response.lossless_rules.push(LosslessRule::default());
            }
            EventType::TrafficSecurityRule => {
                response.set_type(DiscoverResponseType::TrafficSecurityRule);
                response
                    .traffic_security_rules
                    .push(TrafficSecurityRule::default());
            }
            EventType::TrafficMirrorRule => {
                response.set_type(DiscoverResponseType::TrafficMirrorRule);
                response.traffic_mirror_rules.push(TrafficMirror::default());
            }
            EventType::TrafficMockRule => {
                response.set_type(DiscoverResponseType::TrafficMockRule);
                response.traffic_mock_rules.push(TrafficMock::default());
            }
            _ => panic!("not a governance event type: {event_type:?}"),
        }
        response
    }

    fn governance_filter(event_type: EventType) -> Filter {
        Filter {
            resource_key: event_key(event_type),
            include_cache: true,
            timeout: Duration::ZERO,
            ..Filter::default()
        }
    }

    fn memory_cache_for_failover_test(
        runtime: Arc<tokio::runtime::Runtime>,
    ) -> Box<dyn ResourceCache> {
        new_resource_cache(InitResourceCacheOption {
            conf: crate::core::config::global::LocalCacheConfig {
                name: "memory".to_string(),
                service_expire_enable: false,
                service_expire_time: Duration::ZERO,
                service_refresh_interval: Duration::ZERO,
                service_list_refresh_interval: Duration::ZERO,
                persist_enable: false,
                persist_dir: String::new(),
            },
            runtime,
            server_connector: Arc::new(Box::new(NoopConnector::default())),
        })
    }

    fn handler_for_rule_tests() -> Arc<MemoryResourceHandler> {
        let service_key = "default#svc-a".to_string();

        Arc::new(MemoryResourceHandler {
            failover: SyncRwLock::new(Some(Arc::new(NoopFailover))),
            listeners: Arc::new(RwLock::new(HashMap::new())),
            services: Arc::new(RwLock::new(HashMap::new())),
            instances: Arc::new(RwLock::new(HashMap::from([(
                service_key.clone(),
                ServiceInstancesCacheItem::new(),
            )]))),
            router_rules: Arc::new(RwLock::new(HashMap::new())),
            ratelimit_rules: Arc::new(RwLock::new(HashMap::new())),
            circuitbreaker_rules: Arc::new(RwLock::new(HashMap::new())),
            faultdetect_rules: Arc::new(RwLock::new(HashMap::from([(
                service_key.clone(),
                FaultDetectRulesCacheItem::new(),
            )]))),
            lane_rules: Arc::new(RwLock::new(HashMap::from([(
                service_key.clone(),
                LaneRulesCacheItem::new(),
            )]))),
            lossless_rules: Arc::new(RwLock::new(HashMap::from([(
                service_key.clone(),
                LosslessRulesCacheItem::new(),
            )]))),
            traffic_security_rules: Arc::new(RwLock::new(HashMap::from([(
                service_key.clone(),
                TrafficSecurityRulesCacheItem::new(),
            )]))),
            traffic_mirror_rules: Arc::new(RwLock::new(HashMap::from([(
                service_key.clone(),
                TrafficMirrorRulesCacheItem::new(),
            )]))),
            traffic_mock_rules: Arc::new(RwLock::new(HashMap::from([(
                service_key,
                TrafficMockRulesCacheItem::new(),
            )]))),
            config_groups: Arc::new(RwLock::new(HashMap::new())),
            config_files: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    fn event_key(event_type: EventType) -> ResourceEventKey {
        ResourceEventKey {
            namespace: "default".to_string(),
            event_type,
            filter: HashMap::from([("service".to_string(), "svc-a".to_string())]),
        }
    }

    fn service() -> Service {
        Service {
            namespace: "default".to_string(),
            name: "svc-a".to_string(),
            revision: "rev-2".to_string(),
            ..Service::default()
        }
    }

    async fn emit(
        handler: Arc<MemoryResourceHandler>,
        event_type: EventType,
        resp: DiscoverResponse,
    ) {
        MemoryCache::on_spec_event(
            handler,
            RemoteData {
                event_key: event_key(event_type),
                discover_value: Some(resp),
                config_value: None,
            },
        )
        .await;
    }

    #[tokio::test]
    async fn on_spec_event_updates_new_service_rule_caches() {
        let handler = handler_for_rule_tests();

        emit(
            handler.clone(),
            EventType::LaneRule,
            DiscoverResponse {
                code: Code::ExecuteSuccess as u32,
                r#type: DiscoverResponseType::Lane.into(),
                service: Some(service()),
                lanes: vec![LaneGroup {
                    name: "lane-a".to_string(),
                    ..LaneGroup::default()
                }],
                ..DiscoverResponse::default()
            },
        )
        .await;

        emit(
            handler.clone(),
            EventType::LosslessRule,
            DiscoverResponse {
                code: Code::ExecuteSuccess as u32,
                r#type: DiscoverResponseType::Lossless.into(),
                service: Some(service()),
                lossless_rules: vec![LosslessRule {
                    id: "lossless-a".to_string(),
                    ..LosslessRule::default()
                }],
                ..DiscoverResponse::default()
            },
        )
        .await;

        emit(
            handler.clone(),
            EventType::FaultDetectRule,
            DiscoverResponse {
                code: Code::ExecuteSuccess as u32,
                r#type: DiscoverResponseType::FaultDetector.into(),
                service: Some(service()),
                fault_detect_rules: vec![FaultDetectRule {
                    name: "fault-detect-a".to_string(),
                    ..FaultDetectRule::default()
                }],
                ..DiscoverResponse::default()
            },
        )
        .await;

        emit(
            handler.clone(),
            EventType::TrafficSecurityRule,
            DiscoverResponse {
                code: Code::ExecuteSuccess as u32,
                r#type: DiscoverResponseType::TrafficSecurityRule.into(),
                service: Some(service()),
                traffic_security_rules: vec![TrafficSecurityRule {
                    id: "security-a".to_string(),
                    ..TrafficSecurityRule::default()
                }],
                ..DiscoverResponse::default()
            },
        )
        .await;

        emit(
            handler.clone(),
            EventType::TrafficMirrorRule,
            DiscoverResponse {
                code: Code::ExecuteSuccess as u32,
                r#type: DiscoverResponseType::TrafficMirrorRule.into(),
                service: Some(service()),
                traffic_mirror_rules: vec![TrafficMirror {
                    id: "mirror-a".to_string(),
                    ..TrafficMirror::default()
                }],
                ..DiscoverResponse::default()
            },
        )
        .await;

        emit(
            handler.clone(),
            EventType::TrafficMockRule,
            DiscoverResponse {
                code: Code::ExecuteSuccess as u32,
                r#type: DiscoverResponseType::TrafficMockRule.into(),
                service: Some(service()),
                traffic_mock_rules: vec![TrafficMock {
                    id: "mock-a".to_string(),
                    ..TrafficMock::default()
                }],
                ..DiscoverResponse::default()
            },
        )
        .await;

        let lane_rules = handler.lane_rules.read().await;
        let lane_item = lane_rules.get("default#svc-a").unwrap();
        assert!(lane_item.is_initialized());
        assert_eq!(lane_item.revision(), "rev-2");
        assert_eq!(lane_item.value[0].name, "lane-a");

        let lossless_rules = handler.lossless_rules.read().await;
        let lossless_item = lossless_rules.get("default#svc-a").unwrap();
        assert!(lossless_item.is_initialized());
        assert_eq!(lossless_item.revision(), "rev-2");
        assert_eq!(lossless_item.value[0].id, "lossless-a");

        let faultdetect_rules = handler.faultdetect_rules.read().await;
        let faultdetect_item = faultdetect_rules.get("default#svc-a").unwrap();
        assert!(faultdetect_item.is_initialized());
        assert_eq!(faultdetect_item.revision(), "rev-2");
        assert_eq!(faultdetect_item.value[0].name, "fault-detect-a");

        let security_rules = handler.traffic_security_rules.read().await;
        let security_item = security_rules.get("default#svc-a").unwrap();
        assert!(security_item.is_initialized());
        assert_eq!(security_item.revision(), "rev-2");
        assert_eq!(security_item.value[0].id, "security-a");

        let mirror_rules = handler.traffic_mirror_rules.read().await;
        let mirror_item = mirror_rules.get("default#svc-a").unwrap();
        assert!(mirror_item.is_initialized());
        assert_eq!(mirror_item.revision(), "rev-2");
        assert_eq!(mirror_item.value[0].id, "mirror-a");

        let mock_rules = handler.traffic_mock_rules.read().await;
        let mock_item = mock_rules.get("default#svc-a").unwrap();
        assert!(mock_item.is_initialized());
        assert_eq!(mock_item.revision(), "rev-2");
        assert_eq!(mock_item.value[0].id, "mock-a");
    }

    #[tokio::test]
    async fn on_spec_event_updates_available_instance_cache() {
        let handler = handler_for_rule_tests();

        emit(
            handler.clone(),
            EventType::Instance,
            DiscoverResponse {
                code: Code::ExecuteSuccess as u32,
                r#type: DiscoverResponseType::Instance.into(),
                service: Some(service()),
                instances: vec![
                    pole_specification::v1::Instance {
                        namespace: "default".to_string(),
                        service: "svc-a".to_string(),
                        host: "127.0.0.1".to_string(),
                        port: 8080,
                        weight: 100,
                        healthy: true,
                        isolate: false,
                        ..pole_specification::v1::Instance::default()
                    },
                    pole_specification::v1::Instance {
                        namespace: "default".to_string(),
                        service: "svc-a".to_string(),
                        host: "127.0.0.2".to_string(),
                        port: 8080,
                        weight: 0,
                        healthy: true,
                        isolate: false,
                        ..pole_specification::v1::Instance::default()
                    },
                ],
                ..DiscoverResponse::default()
            },
        )
        .await;

        let instances = handler.instances.read().await;
        let item = instances.get("default#svc-a").unwrap();
        assert!(item.is_initialized());
        assert_eq!(item.list_instances(false).await.len(), 2);

        let available = item.list_instances(true).await;
        assert_eq!(available.len(), 1);
        assert_eq!(available[0].ip, "127.0.0.1");
        assert_eq!(available[0].weight, 100);
    }

    #[test]
    fn load_service_rule_recovers_every_governance_type_from_configured_failover() {
        let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
        runtime.block_on(async {
            let event_types = [
                EventType::RouterRule,
                EventType::RateLimitRule,
                EventType::CircuitBreakerRule,
                EventType::FaultDetectRule,
                EventType::LaneRule,
                EventType::LosslessRule,
                EventType::TrafficSecurityRule,
                EventType::TrafficMirrorRule,
                EventType::TrafficMockRule,
            ];

            for event_type in event_types {
                let mut cache = memory_cache_for_failover_test(runtime.clone());
                let failover = Arc::new(RecordingFailover::default());
                cache.set_failover_provider(failover.clone());

                let first = cache
                    .load_service_rule(governance_filter(event_type))
                    .await
                    .unwrap();
                assert!(first.initialized, "{event_type:?}");
                assert_eq!(first.revision, "failover-rev", "{event_type:?}");
                assert_eq!(first.rules.len(), 1, "{event_type:?}");

                let second = cache
                    .load_service_rule(governance_filter(event_type))
                    .await
                    .unwrap();
                assert_eq!(second.revision, "failover-rev", "{event_type:?}");
                assert_eq!(failover.load_calls.load(Ordering::SeqCst), 1);
                assert_eq!(failover.save_calls.load(Ordering::SeqCst), 1);
            }
        });
    }

    #[test]
    fn load_service_rule_does_not_use_failover_when_cache_is_excluded() {
        let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
        runtime.block_on(async {
            let mut cache = memory_cache_for_failover_test(runtime.clone());
            let failover = Arc::new(RecordingFailover::default());
            cache.set_failover_provider(failover.clone());
            let mut filter = governance_filter(EventType::RouterRule);
            filter.include_cache = false;

            assert!(cache.load_service_rule(filter).await.is_err());
            assert_eq!(failover.load_calls.load(Ordering::SeqCst), 0);
            assert_eq!(failover.save_calls.load(Ordering::SeqCst), 0);
        });
    }
}
