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

use crate::core::config::global::LocalCacheConfig;
use crate::core::model::cache::{
    EventType, ResourceEventKey, ServerEvent, ServiceInstancesCacheItem,
};
use crate::core::model::config::{ConfigFile, ConfigGroup};
use crate::core::model::error::{ErrorCode, PoleError};
use crate::core::model::naming::{ServiceRule, Services};
use crate::core::plugin::plugins::Plugin;
use pole_specification::v1::{ConfigDiscoverResponse, DiscoverResponse};
use std::sync::Arc;
use std::time::Duration;

use super::connector::Connector;

#[derive(Clone, Default)]
pub struct Filter {
    pub resource_key: ResourceEventKey,
    pub internal_request: bool,
    pub include_cache: bool,
    pub timeout: Duration,
}

impl Filter {
    pub fn get_event_type(&self) -> EventType {
        self.resource_key.event_type
    }
}

pub enum Action {
    Add,
    Update,
    Delete,
}

#[async_trait::async_trait]
pub trait ResourceListener: Send + Sync {
    // 处理事件
    async fn on_event(&self, action: Action, val: ServerEvent);
    // 获取监听的key
    fn watch_key(&self) -> EventType;
}

pub struct InitResourceCacheOption {
    pub conf: LocalCacheConfig,
    pub runtime: Arc<tokio::runtime::Runtime>,
    pub server_connector: Arc<Box<dyn Connector>>,
}

/// 资源缓存
#[async_trait::async_trait]
pub trait ResourceCache: Plugin {
    fn set_failover_provider(&mut self, failover: Arc<dyn ResourceCacheFailover>);
    // 加载服务规则
    async fn load_service_rule(&self, filter: Filter) -> Result<ServiceRule, PoleError>;
    // 加载服务
    async fn load_services(&self, filter: Filter) -> Result<Services, PoleError>;
    // 加载服务实例
    async fn load_service_instances(
        &self,
        filter: Filter,
    ) -> Result<ServiceInstancesCacheItem, PoleError>;
    // 加载配置文件
    async fn load_config_file(&self, filter: Filter) -> Result<ConfigFile, PoleError>;
    // 加载配置文件组
    async fn load_config_group_files(&self, filter: Filter) -> Result<ConfigGroup, PoleError>;
    // 注册资源监听器
    async fn register_resource_listener(&self, listener: Arc<dyn ResourceListener>);
}

#[async_trait::async_trait]
pub trait ResourceCacheFailover: Send + Sync {
    // failover_naming_load 兜底加载
    async fn failover_naming_load(&self, filter: Filter) -> Result<DiscoverResponse, PoleError>;
    // save_naming_failover 保存容灾数据
    async fn save_naming_failover(&self, value: DiscoverResponse) -> Result<(), PoleError>;

    // failover_config_load 兜底加载
    async fn failover_config_load(
        &self,
        filter: Filter,
    ) -> Result<ConfigDiscoverResponse, PoleError>;
    // save_config_failover 保存容灾数据
    async fn save_config_failover(&self, value: ConfigDiscoverResponse) -> Result<(), PoleError>;
}

#[derive(Default)]
pub struct NoopResourceCache {}

impl Plugin for NoopResourceCache {
    fn init(&mut self) {}

    fn destroy(&self) {}

    fn name(&self) -> String {
        "noopResourceCache".to_string()
    }
}

fn unsupported_noop_cache(operation: &str) -> PoleError {
    PoleError::new(
        ErrorCode::NotSupport,
        format!("noop resource cache does not support {operation}"),
    )
}

#[async_trait::async_trait]
impl ResourceCache for NoopResourceCache {
    fn set_failover_provider(&mut self, _failover: Arc<dyn ResourceCacheFailover>) {}

    async fn load_service_rule(&self, _filter: Filter) -> Result<ServiceRule, PoleError> {
        Err(unsupported_noop_cache("load_service_rule"))
    }

    async fn load_services(&self, _filter: Filter) -> Result<Services, PoleError> {
        Err(unsupported_noop_cache("load_services"))
    }

    async fn load_service_instances(
        &self,
        _filter: Filter,
    ) -> Result<ServiceInstancesCacheItem, PoleError> {
        Err(unsupported_noop_cache("load_service_instances"))
    }

    async fn load_config_file(&self, _filter: Filter) -> Result<ConfigFile, PoleError> {
        Err(unsupported_noop_cache("load_config_file"))
    }

    async fn load_config_group_files(&self, _filter: Filter) -> Result<ConfigGroup, PoleError> {
        Err(unsupported_noop_cache("load_config_group_files"))
    }

    async fn register_resource_listener(&self, _listener: Arc<dyn ResourceListener>) {}
}

#[cfg(test)]
mod noop_tests {
    use super::*;
    use crate::core::model::cache::{CacheItemType, ServerEvent};

    struct DummyFailover;

    #[async_trait::async_trait]
    impl ResourceCacheFailover for DummyFailover {
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

    struct DummyListener;

    #[async_trait::async_trait]
    impl ResourceListener for DummyListener {
        async fn on_event(&self, _action: Action, _val: ServerEvent) {}

        fn watch_key(&self) -> EventType {
            EventType::Service
        }
    }

    fn assert_not_support<T>(ret: Result<T, PoleError>) {
        match ret {
            Ok(_) => panic!("noop resource cache should reject capability calls"),
            Err(err) => assert!(
                err.to_string().contains("NotSupport"),
                "unexpected error: {err}"
            ),
        }
    }

    #[tokio::test]
    async fn noop_resource_cache_methods_do_not_panic() {
        let mut cache = NoopResourceCache::default();
        cache.init();
        assert_eq!(cache.name(), "noopResourceCache");
        cache.set_failover_provider(Arc::new(DummyFailover));

        let filter = Filter::default();
        assert_not_support(cache.load_service_rule(filter.clone()).await);
        assert_not_support(cache.load_services(filter.clone()).await);
        assert_not_support(cache.load_service_instances(filter.clone()).await);
        assert_not_support(cache.load_config_file(filter.clone()).await);
        assert_not_support(cache.load_config_group_files(filter).await);

        cache
            .register_resource_listener(Arc::new(DummyListener))
            .await;
        cache.destroy();

        let _ = CacheItemType::Unknown;
    }
}
