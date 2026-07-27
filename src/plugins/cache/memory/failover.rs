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

use pole_specification::v1::{
    config_discover_response::ConfigDiscoverResponseType, discover_response::DiscoverResponseType,
    ConfigDiscoverResponse, DiscoverResponse,
};
use prost::Message;
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncWriteExt},
};

use crate::core::{
    config::global::LocalCacheConfig,
    model::{
        cache::EventType,
        error::{ErrorCode, PoleError},
    },
    plugin::cache::{Filter, ResourceCacheFailover},
};

pub struct DiskCacheFailover {
    conf: LocalCacheConfig,
}

impl DiskCacheFailover {
    pub fn new(conf: LocalCacheConfig) -> Self {
        Self { conf }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pole_specification::v1::{
        config_discover_response::ConfigDiscoverResponseType,
        discover_response::DiscoverResponseType, ConfigDiscoverResponse, ConfigFileRelease,
        DiscoverResponse, LosslessRule, Service, TrafficMirror, TrafficMock, TrafficSecurityRule,
    };
    use std::collections::HashMap;
    use std::fs;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn temp_cache_config() -> LocalCacheConfig {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let persist_dir = std::env::temp_dir().join(format!("pole-client-failover-{suffix}"));
        fs::create_dir_all(&persist_dir).unwrap();

        LocalCacheConfig {
            name: "memory".to_string(),
            service_expire_enable: false,
            service_expire_time: Duration::from_secs(0),
            service_refresh_interval: Duration::from_secs(0),
            service_list_refresh_interval: Duration::from_secs(0),
            persist_enable: true,
            persist_dir: persist_dir.to_string_lossy().to_string(),
        }
    }

    fn service() -> Service {
        Service {
            namespace: "default".to_string(),
            name: "svc-a".to_string(),
            revision: "rev-4".to_string(),
            ..Service::default()
        }
    }

    fn filter(event_type: EventType) -> Filter {
        Filter {
            resource_key: crate::core::model::cache::ResourceEventKey {
                namespace: "default".to_string(),
                event_type,
                filter: HashMap::from([("service".to_string(), "svc-a".to_string())]),
            },
            internal_request: false,
            include_cache: true,
            timeout: Duration::from_secs(1),
        }
    }

    #[tokio::test]
    async fn save_and_load_naming_failover_supports_new_service_rule_types() {
        let conf = temp_cache_config();
        let failover = DiskCacheFailover::new(conf.clone());

        let cases = [
            (
                EventType::LosslessRule,
                DiscoverResponse {
                    r#type: DiscoverResponseType::Lossless.into(),
                    service: Some(service()),
                    lossless_rules: vec![LosslessRule {
                        id: "lossless-a".to_string(),
                        ..LosslessRule::default()
                    }],
                    ..DiscoverResponse::default()
                },
            ),
            (
                EventType::TrafficSecurityRule,
                DiscoverResponse {
                    r#type: DiscoverResponseType::TrafficSecurityRule.into(),
                    service: Some(service()),
                    traffic_security_rules: vec![TrafficSecurityRule {
                        id: "security-a".to_string(),
                        ..TrafficSecurityRule::default()
                    }],
                    ..DiscoverResponse::default()
                },
            ),
            (
                EventType::TrafficMirrorRule,
                DiscoverResponse {
                    r#type: DiscoverResponseType::TrafficMirrorRule.into(),
                    service: Some(service()),
                    traffic_mirror_rules: vec![TrafficMirror {
                        id: "mirror-a".to_string(),
                        ..TrafficMirror::default()
                    }],
                    ..DiscoverResponse::default()
                },
            ),
            (
                EventType::TrafficMockRule,
                DiscoverResponse {
                    r#type: DiscoverResponseType::TrafficMockRule.into(),
                    service: Some(service()),
                    traffic_mock_rules: vec![TrafficMock {
                        id: "mock-a".to_string(),
                        ..TrafficMock::default()
                    }],
                    ..DiscoverResponse::default()
                },
            ),
        ];

        for (event_type, response) in cases {
            failover
                .save_naming_failover(response.clone())
                .await
                .unwrap();
            let loaded = failover
                .failover_naming_load(filter(event_type))
                .await
                .unwrap();

            assert_eq!(loaded.r#type(), response.r#type());
            assert_eq!(loaded.service.unwrap().revision, "rev-4");
        }

        let _ = fs::remove_dir_all(conf.persist_dir);
    }

    #[tokio::test]
    async fn disabled_persistence_does_not_read_or_write_failover_files() {
        let mut conf = temp_cache_config();
        let persist_dir = conf.persist_dir.clone();
        fs::remove_dir_all(&persist_dir).unwrap();
        conf.persist_enable = false;
        let failover = DiskCacheFailover::new(conf);

        failover
            .save_naming_failover(DiscoverResponse {
                r#type: DiscoverResponseType::Lossless.into(),
                service: Some(service()),
                lossless_rules: vec![LosslessRule::default()],
                ..DiscoverResponse::default()
            })
            .await
            .unwrap();
        failover
            .save_config_failover(ConfigDiscoverResponse {
                r#type: ConfigDiscoverResponseType::ConfigFile.into(),
                file: Some(ConfigFileRelease {
                    namespace: "default".to_string(),
                    group: "group-a".to_string(),
                    file_name: "file-a".to_string(),
                    ..ConfigFileRelease::default()
                }),
                ..ConfigDiscoverResponse::default()
            })
            .await
            .unwrap();

        assert!(!std::path::Path::new(&persist_dir).exists());
        let naming_error = failover
            .failover_naming_load(filter(EventType::LosslessRule))
            .await
            .unwrap_err();
        assert!(naming_error
            .to_string()
            .contains("local cache persistence is disabled"));
        let config_error = failover
            .failover_config_load(Filter {
                resource_key: crate::core::model::cache::ResourceEventKey {
                    namespace: "default".to_string(),
                    event_type: EventType::ConfigFile,
                    filter: HashMap::from([
                        ("group".to_string(), "group-a".to_string()),
                        ("file".to_string(), "file-a".to_string()),
                    ]),
                },
                ..Filter::default()
            })
            .await
            .unwrap_err();
        assert!(config_error
            .to_string()
            .contains("local cache persistence is disabled"));
    }
}

#[async_trait::async_trait]
impl ResourceCacheFailover for DiskCacheFailover {
    // failover_naming_load 兜底加载
    async fn failover_naming_load(&self, filter: Filter) -> Result<DiscoverResponse, PoleError> {
        if !self.conf.persist_enable {
            return Err(PoleError::new(
                ErrorCode::InternalError,
                "local cache persistence is disabled".to_string(),
            ));
        }
        let mut persist_file = self.conf.persist_dir.clone();
        let event_type = filter.get_event_type();
        let resource_key = filter.resource_key;
        match event_type {
            EventType::Service => {
                persist_file = format!(
                    "{}/svc#{}#services.data",
                    persist_file,
                    resource_key.namespace.clone(),
                );
            }
            EventType::Instance
            | EventType::RouterRule
            | EventType::LaneRule
            | EventType::LosslessRule
            | EventType::TrafficSecurityRule
            | EventType::TrafficMirrorRule
            | EventType::TrafficMockRule
            | EventType::CircuitBreakerRule
            | EventType::FaultDetectRule
            | EventType::RateLimitRule => {
                persist_file = format!(
                    "{}/svc#{}#{}#{}",
                    persist_file,
                    resource_key.namespace.clone(),
                    resource_key.filter["service"].clone(),
                    event_type.to_persist_file(),
                );
            }
            _ => {
                return Err(PoleError::new(
                    ErrorCode::InternalError,
                    format!("unsupported event type"),
                ));
            }
        }

        let ret = File::open(persist_file.clone()).await;
        match ret {
            Ok(mut file) => {
                let mut buf = Vec::new();
                let ret = file.read_to_end(&mut buf).await;
                if let Err(e) = ret {
                    return Err(PoleError::new(
                        ErrorCode::InternalError,
                        format!("read file:{:?} error: {:?}", persist_file, e),
                    ));
                }
                let ret = DiscoverResponse::decode(buf.as_slice());
                match ret {
                    Ok(value) => Ok(value),
                    Err(e) => Err(PoleError::new(
                        ErrorCode::InternalError,
                        format!("decode file:{:?} error: {:?}", persist_file, e),
                    )),
                }
            }
            Err(e) => Err(PoleError::new(
                ErrorCode::InternalError,
                format!("open file:{:?} error: {:?}", persist_file, e),
            )),
        }
    }

    // save_failover 保存容灾数据
    async fn save_naming_failover(&self, value: DiscoverResponse) -> Result<(), PoleError> {
        if !self.conf.persist_enable {
            return Ok(());
        }
        let mut buf = Vec::new();
        let mut persist_file = self.conf.persist_dir.clone();
        let svc = value.service.clone().unwrap();

        match value.r#type().clone() {
            pole_specification::v1::discover_response::DiscoverResponseType::Services => {
                persist_file = format!(
                    "{}/svc#{}#services.data",
                    persist_file,
                    svc.namespace.clone()
                );
            }
            DiscoverResponseType::Instance
            | DiscoverResponseType::CustomRouteRule
            | DiscoverResponseType::RateLimit
            | DiscoverResponseType::CircuitBreaker
            | DiscoverResponseType::FaultDetector
            | DiscoverResponseType::Lane
            | DiscoverResponseType::Lossless
            | DiscoverResponseType::TrafficSecurityRule
            | DiscoverResponseType::TrafficMirrorRule
            | DiscoverResponseType::TrafficMockRule => {
                persist_file = format!(
                    "{}/svc#{}#{}#{}",
                    persist_file,
                    svc.namespace.clone(),
                    svc.name.clone(),
                    EventType::naming_spec_to_persist_file(value.r#type().clone())
                );
            }
            _ => {
                return Err(PoleError::new(
                    ErrorCode::InternalError,
                    format!("unsupported discover response type"),
                ));
            }
        }

        let ret = DiscoverResponse::encode(&value, &mut buf);
        if let Err(e) = ret {
            return Err(PoleError::new(
                ErrorCode::InternalError,
                format!("encode discover response error: {:?}", e),
            ));
        }

        match File::create(persist_file).await {
            Ok(mut file) => {
                let ret = file.write_all(buf.as_slice()).await;
                if let Err(e) = ret {
                    return Err(PoleError::new(
                        ErrorCode::InternalError,
                        format!("write file error: {:?}", e),
                    ));
                }
                Ok(())
            }
            Err(e) => Err(PoleError::new(
                ErrorCode::InternalError,
                format!("create file error: {:?}", e),
            )),
        }
    }

    // failover_config_load 兜底加载
    async fn failover_config_load(
        &self,
        filter: Filter,
    ) -> Result<ConfigDiscoverResponse, PoleError> {
        if !self.conf.persist_enable {
            return Err(PoleError::new(
                ErrorCode::InternalError,
                "local cache persistence is disabled".to_string(),
            ));
        }
        let mut persist_file = self.conf.persist_dir.clone();
        let event_type = filter.get_event_type();
        let resource_key = filter.resource_key;
        match event_type {
            EventType::ConfigFile => {
                persist_file = format!(
                    "{}/config#{}#{}#{}#config_file.data",
                    persist_file,
                    resource_key.namespace.clone(),
                    resource_key.filter["group"].clone(),
                    resource_key.filter["file"].clone(),
                );
            }
            EventType::ConfigGroup => {
                persist_file = format!(
                    "{}/config#{}#{}#config_group.data",
                    persist_file,
                    resource_key.namespace.clone(),
                    resource_key.filter["group"].clone(),
                );
            }
            _ => {
                return Err(PoleError::new(
                    ErrorCode::InternalError,
                    format!("unsupported event type"),
                ));
            }
        }

        let ret = File::open(persist_file.clone()).await;
        match ret {
            Ok(mut file) => {
                let mut buf = Vec::new();
                let ret = file.read_to_end(&mut buf).await;
                if let Err(e) = ret {
                    return Err(PoleError::new(
                        ErrorCode::InternalError,
                        format!("read file:{:?} error: {:?}", persist_file, e),
                    ));
                }
                let ret = ConfigDiscoverResponse::decode(buf.as_slice());
                match ret {
                    Ok(value) => Ok(value),
                    Err(e) => Err(PoleError::new(
                        ErrorCode::InternalError,
                        format!("decode file:{:?} error: {:?}", persist_file, e),
                    )),
                }
            }
            Err(e) => Err(PoleError::new(
                ErrorCode::InternalError,
                format!("open file:{:?} error: {:?}", persist_file, e),
            )),
        }
    }

    // save_config_failover 保存容灾数据
    async fn save_config_failover(&self, value: ConfigDiscoverResponse) -> Result<(), PoleError> {
        if !self.conf.persist_enable {
            return Ok(());
        }
        let mut buf = Vec::new();
        let mut persist_file = self.conf.persist_dir.clone();
        let conf = value
            .file
            .clone()
            .or_else(|| value.file_names.first().cloned())
            .unwrap_or_default();

        match value.r#type().clone() {
            ConfigDiscoverResponseType::ConfigFile => {
                persist_file = format!(
                    "{}/config#{}#{}#{}#config_file.data",
                    persist_file,
                    conf.namespace.clone(),
                    conf.group.clone(),
                    conf.file_name.clone(),
                );
            }
            ConfigDiscoverResponseType::ConfigFileNames => {
                persist_file = format!(
                    "{}/config#{}#{}#config_group.data",
                    persist_file,
                    conf.namespace.clone(),
                    conf.group.clone(),
                );
            }
            _ => {
                return Err(PoleError::new(
                    ErrorCode::InternalError,
                    format!("unsupported config response type"),
                ));
            }
        }

        let ret = ConfigDiscoverResponse::encode(&value, &mut buf);
        if let Err(e) = ret {
            return Err(PoleError::new(
                ErrorCode::InternalError,
                format!("encode config response error: {:?}", e),
            ));
        }

        match File::create(persist_file).await {
            Ok(mut file) => {
                let ret = file.write_all(buf.as_slice()).await;
                if let Err(e) = ret {
                    return Err(PoleError::new(
                        ErrorCode::InternalError,
                        format!("write file error: {:?}", e),
                    ));
                }
                Ok(())
            }
            Err(e) => Err(PoleError::new(
                ErrorCode::InternalError,
                format!("create file error: {:?}", e),
            )),
        }
    }
}
