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

use std::sync::Arc;

use tokio::runtime::Runtime;

use crate::core::config::config::Configuration;
use crate::core::model::cache::{RemoteData, ResourceEventKey};
use crate::core::model::config::{ConfigFileRequest, ConfigPublishRequest, ConfigReleaseRequest};
use crate::core::model::error::{ErrorCode, PoleError};
use crate::core::model::naming::{
    InstanceRequest, InstanceResponse, ServiceContract, ServiceContractRequest,
};
use crate::core::model::{ClientContext, ReportClientRequest};
use crate::core::plugin::plugins::Plugin;

use super::filter::DiscoverFilter;

pub trait ResourceHandler: Send + Sync {
    // handle_event 处理资源事件
    fn handle_event(&self, event: RemoteData);
    /// interest_resource 获取感兴趣的资源
    fn interest_resource(&self) -> ResourceEventKey;
}

/// InitConnectorOption
#[derive(Clone)]
pub struct InitConnectorOption {
    pub runtime: Arc<Runtime>,
    pub conf: Arc<Configuration>,
    // config_filters
    pub config_filters: Arc<Vec<Box<dyn DiscoverFilter>>>,
    // client_ctx: 客户端数据上下文
    pub client_ctx: Arc<ClientContext>,
}

#[async_trait::async_trait]
pub trait Connector: Plugin {
    /// register_resource_handler 注册资源处理器
    async fn register_resource_handler(
        &self,
        handler: Box<dyn ResourceHandler>,
    ) -> Result<bool, PoleError>;

    /// register_instance: 实例注册回调函数
    async fn register_instance(&self, req: InstanceRequest) -> Result<InstanceResponse, PoleError>;

    /// deregister_instance 实例注销回调函数
    async fn deregister_instance(&self, req: InstanceRequest) -> Result<bool, PoleError>;

    /// heartbeat_instance 实例心跳回调函数
    async fn heartbeat_instance(&self, req: InstanceRequest) -> Result<bool, PoleError>;

    /// report_client 上报客户端信息
    async fn report_client(&self, req: ReportClientRequest) -> Result<bool, PoleError>;

    /// report_service_contract 上报服务契约
    async fn report_service_contract(&self, req: ServiceContractRequest)
        -> Result<bool, PoleError>;

    /// get_service_contract 获取服务契约
    async fn get_service_contract(
        &self,
        req: ServiceContractRequest,
    ) -> Result<ServiceContract, PoleError>;

    /// create_config_file 创建配置文件
    async fn create_config_file(&self, req: ConfigFileRequest) -> Result<bool, PoleError>;

    /// update_config_file 更新配置文件
    async fn update_config_file(&self, req: ConfigFileRequest) -> Result<bool, PoleError>;

    /// release_config_file 删除配置文件
    async fn release_config_file(&self, req: ConfigReleaseRequest) -> Result<bool, PoleError>;

    /// upsert_publish_config_file 更新发布配置文件
    async fn upsert_publish_config_file(
        &self,
        req: ConfigPublishRequest,
    ) -> Result<bool, PoleError>;
}

#[derive(Default)]
pub struct NoopConnector {}

impl Plugin for NoopConnector {
    fn init(&mut self) {}

    fn destroy(&self) {}

    fn name(&self) -> String {
        "noopConnector".to_string()
    }
}

fn unsupported_noop_connector(operation: &str) -> PoleError {
    PoleError::new(
        ErrorCode::NotSupport,
        format!("noop connector does not support {operation}"),
    )
}

#[async_trait::async_trait]
impl Connector for NoopConnector {
    async fn register_resource_handler(
        &self,
        _handler: Box<dyn ResourceHandler>,
    ) -> Result<bool, PoleError> {
        Err(unsupported_noop_connector("register_resource_handler"))
    }

    async fn register_instance(
        &self,
        _req: InstanceRequest,
    ) -> Result<InstanceResponse, PoleError> {
        Err(unsupported_noop_connector("register_instance"))
    }

    async fn deregister_instance(&self, _req: InstanceRequest) -> Result<bool, PoleError> {
        Err(unsupported_noop_connector("deregister_instance"))
    }

    async fn heartbeat_instance(&self, _req: InstanceRequest) -> Result<bool, PoleError> {
        Err(unsupported_noop_connector("heartbeat_instance"))
    }

    async fn report_client(&self, _req: ReportClientRequest) -> Result<bool, PoleError> {
        Err(unsupported_noop_connector("report_client"))
    }

    async fn report_service_contract(
        &self,
        _req: ServiceContractRequest,
    ) -> Result<bool, PoleError> {
        Err(unsupported_noop_connector("report_service_contract"))
    }

    async fn get_service_contract(
        &self,
        _req: ServiceContractRequest,
    ) -> Result<ServiceContract, PoleError> {
        Err(unsupported_noop_connector("get_service_contract"))
    }

    async fn create_config_file(&self, _req: ConfigFileRequest) -> Result<bool, PoleError> {
        Err(unsupported_noop_connector("create_config_file"))
    }

    async fn update_config_file(&self, _req: ConfigFileRequest) -> Result<bool, PoleError> {
        Err(unsupported_noop_connector("update_config_file"))
    }

    async fn release_config_file(&self, _req: ConfigReleaseRequest) -> Result<bool, PoleError> {
        Err(unsupported_noop_connector("release_config_file"))
    }

    async fn upsert_publish_config_file(
        &self,
        _req: ConfigPublishRequest,
    ) -> Result<bool, PoleError> {
        Err(unsupported_noop_connector("upsert_publish_config_file"))
    }
}

#[cfg(test)]
mod noop_tests {
    use super::*;
    use crate::core::model::cache::{EventType, RemoteData};
    use crate::core::model::config::{ConfigFile, ConfigFileRelease};
    use crate::core::model::naming::{Instance, Location};
    use std::collections::HashMap;

    struct DummyResourceHandler;

    impl ResourceHandler for DummyResourceHandler {
        fn handle_event(&self, _event: RemoteData) {}

        fn interest_resource(&self) -> ResourceEventKey {
            ResourceEventKey {
                namespace: "default".to_string(),
                event_type: EventType::Service,
                filter: HashMap::new(),
            }
        }
    }

    fn service_contract_request() -> ServiceContractRequest {
        ServiceContractRequest {
            flow_id: "flow".to_string(),
            contract: ServiceContract {
                name: "contract".to_string(),
                namespace: "default".to_string(),
                service: "svc".to_string(),
                version: "v1".to_string(),
                protocol: "http".to_string(),
                content: String::new(),
                interfaces: Vec::new(),
                metadata: HashMap::new(),
            },
        }
    }

    fn config_file_request() -> ConfigFileRequest {
        ConfigFileRequest {
            flow_id: "flow".to_string(),
            config_file: ConfigFile::default(),
        }
    }

    fn config_release_request() -> ConfigReleaseRequest {
        ConfigReleaseRequest {
            flow_id: "flow".to_string(),
            config_file: ConfigFileRelease {
                namespace: "default".to_string(),
                group: "group".to_string(),
                file_name: "file".to_string(),
                release_name: "release".to_string(),
                md5: String::new(),
            },
        }
    }

    fn config_publish_request() -> ConfigPublishRequest {
        ConfigPublishRequest {
            flow_id: "flow".to_string(),
            md5: String::new(),
            release_name: "release".to_string(),
            config_file: ConfigFile::default(),
        }
    }

    fn assert_not_support<T>(ret: Result<T, PoleError>) {
        match ret {
            Ok(_) => panic!("noop connector should reject capability calls"),
            Err(err) => assert!(
                err.to_string().contains("NotSupport"),
                "unexpected error: {err}"
            ),
        }
    }

    #[tokio::test]
    async fn noop_connector_methods_do_not_panic() {
        let mut connector = NoopConnector::default();
        connector.init();
        assert_eq!(connector.name(), "noopConnector");

        assert_not_support(
            connector
                .register_resource_handler(Box::new(DummyResourceHandler))
                .await,
        );
        assert_not_support(
            connector
                .register_instance(InstanceRequest {
                    flow_id: "flow".to_string(),
                    ttl: 0,
                    instance: Instance::default(),
                })
                .await,
        );
        assert_not_support(
            connector
                .deregister_instance(InstanceRequest {
                    flow_id: "flow".to_string(),
                    ttl: 0,
                    instance: Instance::default(),
                })
                .await,
        );
        assert_not_support(
            connector
                .heartbeat_instance(InstanceRequest {
                    flow_id: "flow".to_string(),
                    ttl: 0,
                    instance: Instance::default(),
                })
                .await,
        );
        assert_not_support(
            connector
                .report_client(ReportClientRequest {
                    client_id: "client".to_string(),
                    host: "127.0.0.1".to_string(),
                    version: "test".to_string(),
                    location: Location::default(),
                })
                .await,
        );
        assert_not_support(
            connector
                .report_service_contract(service_contract_request())
                .await,
        );
        assert_not_support(
            connector
                .get_service_contract(service_contract_request())
                .await,
        );
        assert_not_support(connector.create_config_file(config_file_request()).await);
        assert_not_support(connector.update_config_file(config_file_request()).await);
        assert_not_support(
            connector
                .release_config_file(config_release_request())
                .await,
        );
        assert_not_support(
            connector
                .upsert_publish_config_file(config_publish_request())
                .await,
        );

        connector.destroy();
    }
}
