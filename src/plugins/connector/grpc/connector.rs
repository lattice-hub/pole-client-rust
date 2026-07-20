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

use crate::core::config::global::ServerConnectorConfig;
use crate::core::model::cache::{EventType, RemoteData};
use crate::core::model::config::{ConfigFileRequest, ConfigPublishRequest, ConfigReleaseRequest};
use crate::core::model::error::ErrorCode::{ServerError, ServiceNotFound};
use crate::core::model::error::PoleError;
use crate::core::model::naming::{
    InstanceRequest, InstanceResponse, ServiceContract, ServiceContractRequest,
};
use crate::core::model::ReportClientRequest;
use crate::core::plugin::connector::{Connector, InitConnectorOption, ResourceHandler};
use crate::core::plugin::plugins::Plugin;
use crate::{debug, error, info};
use pole_specification::v1::config_grpc_client::ConfigGrpcClient;
use pole_specification::v1::discover_grpc_client::DiscoverGrpcClient;
use pole_specification::v1::Code::{ExecuteSuccess, ExistedResource, NotFoundResource};
use pole_specification::v1::{
    discover_request::DiscoverRequestType, Code, ConfigDiscoverRequest, ConfigDiscoverResponse,
    DiscoverRequest, DiscoverResponse, HeartbeatsRequest, InstanceHeartbeat, Service,
};
use std::cmp::PartialEq;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::RwLock;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::StreamExt;
use tonic::metadata::{AsciiMetadataKey, MetadataValue};
use tonic::service::interceptor::InterceptedService;
use tonic::service::Interceptor;
use tonic::transport::{Channel, Endpoint};
use tonic::Streaming;
use tracing::Instrument;

fn response_code(code: u32) -> Code {
    Code::try_from(code as i32).unwrap_or(Code::Unknown)
}

fn service_rule_watch_key(resp: &DiscoverResponse, event_type: EventType) -> Option<String> {
    let svc = resp.service.as_ref()?;
    Some(format!(
        "{:?}#{}#{}",
        event_type,
        svc.namespace.clone(),
        svc.name.clone()
    ))
}

fn discover_response_watch_key(resp: &DiscoverResponse) -> Option<String> {
    match resp.r#type() {
        pole_specification::v1::discover_response::DiscoverResponseType::Services => {
            resp.service.as_ref().map(|svc| svc.namespace.clone())
        }
        pole_specification::v1::discover_response::DiscoverResponseType::Instance => {
            service_rule_watch_key(resp, EventType::Instance)
        }
        pole_specification::v1::discover_response::DiscoverResponseType::CustomRouteRule => {
            service_rule_watch_key(resp, EventType::RouterRule)
        }
        pole_specification::v1::discover_response::DiscoverResponseType::RateLimit => {
            service_rule_watch_key(resp, EventType::RateLimitRule)
        }
        pole_specification::v1::discover_response::DiscoverResponseType::CircuitBreaker => {
            service_rule_watch_key(resp, EventType::CircuitBreakerRule)
        }
        pole_specification::v1::discover_response::DiscoverResponseType::FaultDetector => {
            service_rule_watch_key(resp, EventType::FaultDetectRule)
        }
        pole_specification::v1::discover_response::DiscoverResponseType::Lane => {
            service_rule_watch_key(resp, EventType::LaneRule)
        }
        pole_specification::v1::discover_response::DiscoverResponseType::Lossless => {
            service_rule_watch_key(resp, EventType::LosslessRule)
        }
        pole_specification::v1::discover_response::DiscoverResponseType::TrafficSecurityRule => {
            service_rule_watch_key(resp, EventType::TrafficSecurityRule)
        }
        pole_specification::v1::discover_response::DiscoverResponseType::TrafficMirrorRule => {
            service_rule_watch_key(resp, EventType::TrafficMirrorRule)
        }
        pole_specification::v1::discover_response::DiscoverResponseType::TrafficMockRule => {
            service_rule_watch_key(resp, EventType::TrafficMockRule)
        }
        _ => None,
    }
}

fn service_contract_discover_request(req: &ServiceContractRequest) -> DiscoverRequest {
    DiscoverRequest {
        r#type: DiscoverRequestType::ServiceContracts.into(),
        service: Some(Service {
            namespace: req.contract.namespace.clone(),
            name: req.contract.service.clone(),
            ..Service::default()
        }),
        filter: None,
        ..DiscoverRequest::default()
    }
}

fn match_optional(expected: &str, actual: &str) -> bool {
    expected.is_empty() || expected == actual
}

fn select_service_contract(
    req: &ServiceContractRequest,
    resp: DiscoverResponse,
) -> Option<ServiceContract> {
    resp.service_contracts
        .into_iter()
        .find(|contract| {
            contract.namespace == req.contract.namespace
                && contract.service == req.contract.service
                && match_optional(&req.contract.name, &contract.name)
                && match_optional(&req.contract.version, &contract.version)
                && match_optional(&req.contract.protocol, &contract.protocol)
        })
        .map(ServiceContract::parse_from_spec)
}

struct ResourceHandlerWrapper {
    handler: Box<dyn ResourceHandler>,
    revision: String,
}

static PLUGIN_NAME: &str = "grpc";

#[derive(Clone)]
pub struct GrpcConnector {
    opt: InitConnectorOption,
    discover_channel: Channel,
    config_channel: Channel,

    discover_spec_sender: Arc<UnboundedSender<DiscoverRequest>>,
    config_spec_sender: Arc<UnboundedSender<ConfigDiscoverRequest>>,

    watch_resources: Arc<RwLock<HashMap<String, ResourceHandlerWrapper>>>,
}

fn new_connector(opt: InitConnectorOption) -> Box<dyn Connector> {
    let conf = &opt.conf.global.server_connectors.clone();
    let (discover_channel, config_channel) = create_channel(conf);

    let (discover_sender, mut discover_reciver) =
        run_discover_spec_stream(discover_channel.clone(), opt.runtime.clone()).unwrap();

    let (config_sender, mut config_reciver) =
        run_config_spec_stream(config_channel.clone(), opt.runtime.clone()).unwrap();

    let c = GrpcConnector {
        opt,
        discover_channel: discover_channel.clone(),
        config_channel: config_channel.clone(),

        discover_spec_sender: Arc::new(discover_sender),
        config_spec_sender: Arc::new(config_sender),

        watch_resources: Arc::new(RwLock::new(HashMap::new())),
    };

    let receive_c = c.clone();
    // 创建一个新的线程，用于处理grpc的消息
    c.opt.runtime.spawn(async move {
        loop {
            tokio::select! {
                discover_ret = discover_reciver.recv() => {
                    if discover_ret.is_some() {
                        receive_c.receive_discover_response(discover_ret.unwrap()).await;
                    }
                }
                config_ret = config_reciver.recv() => {
                    if config_ret.is_some() {
                        receive_c.receive_config_response(config_ret.unwrap()).await;
                    }
                }
            }
        }
    });

    let send_c = c.clone();
    // 开启一个异步任务，定期发送请求到服务端
    c.opt.runtime.spawn(async move {
        loop {
            {
                // 额外一个方法块，减少 lock 的占用时间
                let watch_resources = send_c.watch_resources.read().await;
                watch_resources.iter().for_each(|(_key, handler)| {
                    let key = handler.handler.interest_resource();
                    let filter = key.clone().filter;
                    debug!(
                        "[pole][discovery][connector] send discover request: {:?} filter: {:?}",
                        key.clone(),
                        filter.clone()
                    );

                    let discover_request = key.to_discover_request(handler.revision.clone());
                    let config_request = key.to_config_request(handler.revision.clone());

                    if discover_request.is_some() {
                        send_c.send_naming_discover_request(discover_request.unwrap());
                    } else {
                        send_c.send_config_discover_request(config_request.unwrap());
                    }
                });
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });

    Box::new(c) as Box<dyn Connector + 'static>
}

fn create_channel(conf: &ServerConnectorConfig) -> (Channel, Channel) {
    let addresses = conf.addresses.clone();
    let mut discover_address: Vec<String> = Vec::new();
    let mut config_address: Vec<String> = Vec::new();

    for ele in addresses {
        if ele.starts_with("discover://") {
            discover_address.push(format!("http://{}", ele.trim_start_matches("discover://")));
        } else if ele.starts_with("config://") {
            config_address.push(format!("http://{}", ele.trim_start_matches("config://")));
        }
    }

    info!(
        "[pole][server_connector] discover_address: {:?} config_address: {:?}",
        discover_address, config_address
    );

    let connect_timeout = conf.connect_timeout;

    let discover_endpoints = discover_address.iter().map(|item| {
        Endpoint::from_shared(item.to_string())
            .unwrap()
            .connect_timeout(connect_timeout)
    });
    let config_endpoints = config_address.iter().map(|item| {
        Endpoint::from_shared(item.to_string())
            .unwrap()
            .connect_timeout(connect_timeout)
    });

    let discover_channel = Channel::balance_list(discover_endpoints);
    let config_channel = Channel::balance_list(config_endpoints);

    (discover_channel, config_channel)
}

impl Plugin for GrpcConnector {
    fn init(&mut self) {}

    fn destroy(&self) {}

    fn name(&self) -> String {
        PLUGIN_NAME.to_string()
    }
}

impl GrpcConnector {
    pub fn builder() -> (fn(opt: InitConnectorOption) -> Box<dyn Connector>, String) {
        (new_connector, PLUGIN_NAME.to_string())
    }

    fn create_discover_grpc_stub(
        &self,
        flow: String,
    ) -> DiscoverGrpcClient<InterceptedService<Channel, GrpcConnectorInterceptor>> {
        let interceptor = GrpcConnectorInterceptor {
            metadata: {
                let mut metadata = HashMap::new();
                metadata.insert("request-id".to_string(), flow.to_string());
                metadata
            },
        };
        DiscoverGrpcClient::with_interceptor(self.discover_channel.clone(), interceptor)
    }

    fn create_config_grpc_stub(
        &self,
        flow: String,
    ) -> ConfigGrpcClient<InterceptedService<Channel, GrpcConnectorInterceptor>> {
        let interceptor = GrpcConnectorInterceptor {
            metadata: {
                let mut metadata = HashMap::new();
                metadata.insert("request-id".to_string(), flow.to_string());
                metadata
            },
        };
        ConfigGrpcClient::with_interceptor(self.config_channel.clone(), interceptor)
    }

    async fn send_create_config_file(
        &self,
        req: &ConfigFileRequest,
    ) -> Result<(Code, String), PoleError> {
        let mut client = self.create_config_grpc_stub(req.flow_id.clone());
        let ret = client
            .create_config_file(tonic::Request::new(req.convert_spec()))
            .in_current_span()
            .await;
        match ret {
            Ok(rsp) => {
                let rsp = rsp.into_inner();
                Ok((response_code(rsp.code), rsp.info))
            }
            Err(err) => {
                error!(
                    "[pole][config][connector] send create config_file request to server fail: {}",
                    err
                );
                Err(PoleError::new(ServerError, err.to_string()))
            }
        }
    }

    async fn send_update_config_file(
        &self,
        req: &ConfigFileRequest,
    ) -> Result<(Code, String), PoleError> {
        let mut client = self.create_config_grpc_stub(req.flow_id.clone());
        let ret = client
            .update_config_file(tonic::Request::new(req.convert_spec()))
            .in_current_span()
            .await;
        match ret {
            Ok(rsp) => {
                let rsp = rsp.into_inner();
                Ok((response_code(rsp.code), rsp.info))
            }
            Err(err) => {
                error!(
                    "[pole][config][connector] send update config_file request to server fail: {}",
                    err
                );
                Err(PoleError::new(ServerError, err.to_string()))
            }
        }
    }

    async fn send_publish_config_file(
        &self,
        req: &ConfigReleaseRequest,
    ) -> Result<(Code, String), PoleError> {
        let mut client = self.create_config_grpc_stub(req.flow_id.clone());
        let ret = client
            .publish_config_file(tonic::Request::new(req.convert_spec()))
            .in_current_span()
            .await;
        match ret {
            Ok(rsp) => {
                let rsp = rsp.into_inner();
                Ok((response_code(rsp.code), rsp.info))
            }
            Err(err) => {
                error!(
                    "[pole][config][connector] send publish config_file request to server fail: {}",
                    err
                );
                Err(PoleError::new(ServerError, err.to_string()))
            }
        }
    }

    async fn ensure_config_file_written(&self, req: &ConfigFileRequest) -> Result<(), PoleError> {
        let (update_code, update_info) = self.send_update_config_file(req).await?;
        if ExecuteSuccess.eq(&update_code) {
            return Ok(());
        }
        if !NotFoundResource.eq(&update_code) {
            error!(
                "[pole][config][connector] send update config_file request to server receive fail: code={} info={}",
                update_code as i32,
                update_info.clone(),
            );
            return Err(PoleError::new(ServerError, update_info));
        }

        let (create_code, create_info) = self.send_create_config_file(req).await?;
        if ExecuteSuccess.eq(&create_code) {
            return Ok(());
        }
        if ExistedResource.eq(&create_code) {
            let (retry_update_code, retry_update_info) = self.send_update_config_file(req).await?;
            if ExecuteSuccess.eq(&retry_update_code) {
                return Ok(());
            }
            error!(
                "[pole][config][connector] retry update config_file request to server receive fail: code={} info={}",
                retry_update_code as i32,
                retry_update_info.clone(),
            );
            return Err(PoleError::new(ServerError, retry_update_info));
        }

        error!(
            "[pole][config][connector] send create config_file request to server receive fail: code={} info={}",
            create_code as i32,
            create_info.clone(),
        );
        Err(PoleError::new(ServerError, create_info))
    }

    async fn receive_discover_response(&self, resp: DiscoverResponse) {
        let remote_rsp = resp.clone();
        if remote_rsp.code == Code::DataNoChange as u32 {
            debug!(
                "[pole][discovery][connector] receive naming_discover no_change response: {:?}",
                resp
            );
            return;
        }

        if remote_rsp.code != Code::ExecuteSuccess as u32 {
            error!(
                "[pole][discovery][connector] receive naming_discover failure response: {:?}",
                resp
            );
            return;
        }

        let Some(watch_key) = discover_response_watch_key(&resp) else {
            return;
        };
        let mut handlers = self.watch_resources.write().await;
        if let Some(handle) = handlers.get_mut(watch_key.as_str()) {
            handle.revision = remote_rsp.service.clone().unwrap().revision.clone();
            handle.handler.handle_event(RemoteData {
                event_key: handle.handler.interest_resource(),
                discover_value: Some(remote_rsp),
                config_value: None,
            });
        }
    }

    async fn receive_config_response(&self, mut resp: ConfigDiscoverResponse) {
        info!(
            "[pole][config][connector] receive config_discover response: {:?}",
            resp
        );

        let filters = self.opt.config_filters.clone();
        for (_i, filter) in filters.iter().enumerate() {
            let ret = filter.response_process(
                crate::core::model::DiscoverResponseInfo::Configuration(resp.clone()),
            );
            match ret {
                Ok(rsp) => {
                    resp = rsp.to_config_response();
                }
                Err(err) => {
                    error!(
                        "[pole][config][connector] filter response_process fail: {}",
                        err.to_string()
                    );
                    return;
                }
            }
        }

        let remote_rsp = resp.clone();

        let mut watch_key = "".to_string();
        match resp.r#type() {
            pole_specification::v1::config_discover_response::ConfigDiscoverResponseType::ConfigFile => {
                let ret = resp.file.clone().unwrap_or_default();
                watch_key = format!("{:?}#{}#{}#{}", EventType::ConfigFile, ret.namespace.clone(), 
                ret.group.clone(),  ret.file_name.clone());
            },
            pole_specification::v1::config_discover_response::ConfigDiscoverResponseType::ConfigFileNames => {
                if let Some(ret) = resp.file_names.first() {
                    watch_key = format!("{:?}#{}#{}", EventType::ConfigGroup, ret.namespace.clone(), 
                    ret.group.clone());
                }
            },
            pole_specification::v1::config_discover_response::ConfigDiscoverResponseType::ConfigFileGroups => {
                error!("[pole][config][connector] not support ConfigFileGroups");
            },
            _ => {}
        }

        let mut handlers = self.watch_resources.write().await;
        if let Some(handle) = handlers.get_mut(watch_key.as_str()) {
            handle.revision = remote_rsp.revision.clone();
        }
    }

    fn send_naming_discover_request(&self, req: DiscoverRequest) {
        let _ = self.discover_spec_sender.send(req);
    }

    fn send_config_discover_request(&self, mut req: ConfigDiscoverRequest) {
        let filters = self.opt.config_filters.clone();
        for (_i, filter) in filters.iter().enumerate() {
            let ret = filter.request_process(
                crate::core::model::DiscoverRequestInfo::Configuration(req.clone()),
            );
            match ret {
                Ok(rsp) => {
                    req = rsp.to_config_request();
                }
                Err(err) => {
                    error!(
                        "[pole][config][connector] filter request_process fail: {}",
                        err.to_string()
                    );
                    return;
                }
            }
        }

        if let Some(file) = req.file.as_mut() {
            file.labels.clone_from(&self.opt.client_ctx.labels);
        }

        let _ = self.config_spec_sender.send(req);
    }
}

#[async_trait::async_trait]
impl Connector for GrpcConnector {
    async fn register_resource_handler(
        &self,
        handler: Box<dyn ResourceHandler>,
    ) -> Result<bool, PoleError> {
        let watch_key = handler.interest_resource();
        let watch_key_str = watch_key.to_string();

        let mut handlers = self.watch_resources.write().await;
        if handlers.contains_key(watch_key_str.as_str()) {
            return Err(PoleError::new(
                ServerError,
                format!(
                    "[pole][discovery][connector] resource handler already exist: {}",
                    watch_key_str
                ),
            ));
        }

        handlers.insert(
            watch_key_str.clone(),
            ResourceHandlerWrapper {
                handler,
                revision: String::new(),
            },
        );

        // 立即发送一个数据通知

        let discover_request = watch_key.to_discover_request("".to_string());
        let config_request = watch_key.to_config_request("".to_string());

        if discover_request.is_some() {
            self.send_naming_discover_request(discover_request.unwrap());
        } else {
            self.send_config_discover_request(config_request.unwrap());
        }

        info!(
            "[pole][discovery][connector] register resource handler: {}",
            watch_key_str.clone()
        );
        Ok(true)
    }

    async fn register_instance(&self, req: InstanceRequest) -> Result<InstanceResponse, PoleError> {
        debug!("[pole][discovery][connector] send register instance request={req:?}");

        let mut client = self.create_discover_grpc_stub(req.flow_id.clone());
        let ret = client
            .register_instance(tonic::Request::new(req.convert_spec()))
            .in_current_span()
            .await;
        return match ret {
            Ok(rsp) => {
                let rsp = rsp.into_inner();
                let recv_code = response_code(rsp.code);
                if ExecuteSuccess.eq(&recv_code) {
                    info!("[pole][discovery][connector] register instance to server success");
                    return Ok(InstanceResponse::default());
                }
                if ExistedResource.eq(&recv_code) {
                    return Ok(InstanceResponse::exist_resource());
                }
                Err(PoleError::new(ServerError, rsp.info))
            }
            Err(err) => {
                error!(
                    "[pole][discovery][connector] send register request to server fail: {}",
                    err
                );
                Err(PoleError::new(ServerError, err.to_string()))
            }
        };
    }

    async fn deregister_instance(&self, req: InstanceRequest) -> Result<bool, PoleError> {
        debug!("[pole][discovery][connector] send deregister instance request={req:?}");

        let mut client = self.create_discover_grpc_stub(req.flow_id.clone());
        let ret = client
            .deregister_instance(tonic::Request::new(req.convert_spec()))
            .in_current_span()
            .await;
        return match ret {
            Ok(rsp) => {
                let rsp = rsp.into_inner();
                let recv_code = response_code(rsp.code);
                if ExecuteSuccess.eq(&recv_code) {
                    return Ok(true);
                }
                error!(
                    "[pole][discovery][connector] send deregister request to server receive fail: code={} info={}",
                    rsp.code,
                    rsp.info.clone(),
                );
                Err(PoleError::new(ServerError, rsp.info))
            }
            Err(err) => {
                error!(
                    "[pole][discovery][connector] send deregister request to server fail: {}",
                    err
                );
                Err(PoleError::new(ServerError, err.to_string()))
            }
        };
    }

    async fn heartbeat_instance(&self, req: InstanceRequest) -> Result<bool, PoleError> {
        debug!(
            "[pole][discovery][connector] send heartbeat instance request={:?}",
            req.convert_beat_spec()
        );

        let mut client = self.create_discover_grpc_stub(req.flow_id.clone());
        let heartbeat = InstanceHeartbeat {
            instance_id: req.instance.id.clone(),
            service: req.instance.service.clone(),
            namespace: req.instance.namespace.clone(),
            host: req.instance.ip.clone(),
            port: req.instance.port,
        };
        let ret = client
            .heartbeat(tonic::Request::new(tokio_stream::iter(vec![
                HeartbeatsRequest {
                    heartbeats: vec![heartbeat],
                },
            ])))
            .in_current_span()
            .await;
        return match ret {
            Ok(rsp) => {
                let _rsp = rsp.into_inner();
                Ok(true)
            }
            Err(err) => {
                error!(
                    "[pole][discovery][connector] send heartbeat request to server fail: {}",
                    err
                );
                Err(PoleError::new(ServerError, err.to_string()))
            }
        };
    }

    async fn report_client(&self, req: ReportClientRequest) -> Result<bool, PoleError> {
        debug!("[pole][discovery][connector] send report client request");

        let mut client = self.create_discover_grpc_stub(uuid::Uuid::new_v4().to_string());
        let ret = client
            .report_client(tonic::Request::new(req.convert_spec()))
            .in_current_span()
            .await;
        match ret {
            Ok(rsp) => {
                let rsp = rsp.into_inner();
                let recv_code = response_code(rsp.code);
                if ExecuteSuccess.eq(&recv_code) {
                    return Ok(true);
                }
                error!(
                    "[pole][discovery][connector] send report client request to server receive fail: code={} info={}",
                    rsp.code,
                    rsp.info.clone(),
                );
                Err(PoleError::new(ServerError, rsp.info))
            }
            Err(err) => {
                error!(
                    "[pole][discovery][connector] send report client request to server fail: {}",
                    err
                );
                Err(PoleError::new(ServerError, err.to_string()))
            }
        }
    }

    async fn report_service_contract(
        &self,
        req: ServiceContractRequest,
    ) -> Result<bool, PoleError> {
        debug!("[pole][discovery][connector] send report service_contract request={req:?}");

        let interceptor = GrpcConnectorInterceptor {
            metadata: {
                let mut metadata = HashMap::new();
                metadata.insert("request-id".to_string(), req.flow_id.to_string());
                metadata
            },
        };

        let mut client =
            DiscoverGrpcClient::with_interceptor(self.discover_channel.clone(), interceptor);
        let ret = client
            .report_service_contract(tonic::Request::new(req.contract.convert_spec()))
            .in_current_span()
            .await;
        return match ret {
            Ok(rsp) => {
                let rsp = rsp.into_inner();
                let recv_code = response_code(rsp.code);
                if ExecuteSuccess.eq(&recv_code) {
                    return Ok(true);
                }
                error!(
                    "[pole][discovery][connector] send report service_contract request to server receive fail: code={} info={}",
                    rsp.code,
                    rsp.info.clone(),
                );
                Err(PoleError::new(ServerError, rsp.info))
            }
            Err(err) => {
                error!(
                    "[pole][discovery][connector] send report service_contract request to server fail: {}",
                    err
                );
                Err(PoleError::new(ServerError, err.to_string()))
            }
        };
    }

    async fn get_service_contract(
        &self,
        req: ServiceContractRequest,
    ) -> Result<ServiceContract, PoleError> {
        debug!("[pole][discovery][connector] send get service_contract request={req:?}");

        let mut client = self.create_discover_grpc_stub(req.flow_id.clone());
        let ret = client
            .discover(tonic::Request::new(tokio_stream::iter(vec![
                service_contract_discover_request(&req),
            ])))
            .in_current_span()
            .await;

        let mut stream = match ret {
            Ok(rsp) => rsp.into_inner(),
            Err(err) => {
                error!(
                    "[pole][discovery][connector] send get service_contract request to server fail: {}",
                    err
                );
                return Err(PoleError::new(ServerError, err.to_string()));
            }
        };

        let timeout = self.opt.conf.global.server_connectors.message_timeout;
        loop {
            let received = tokio::time::timeout(timeout, stream.next()).await;
            let Some(item) = (match received {
                Ok(item) => item,
                Err(_) => {
                    return Err(PoleError::new(
                        ServerError,
                        "get service_contract request timeout".to_string(),
                    ));
                }
            }) else {
                return Err(PoleError::new(
                    ServiceNotFound,
                    "service_contract response stream closed".to_string(),
                ));
            };

            let resp = match item {
                Ok(resp) => resp,
                Err(err) => {
                    error!(
                        "[pole][discovery][connector] get service_contract stream receive err: {}",
                        err
                    );
                    return Err(PoleError::new(ServerError, err.to_string()));
                }
            };

            let recv_code = response_code(resp.code);
            if Code::DataNoChange.eq(&recv_code) {
                continue;
            }
            if !ExecuteSuccess.eq(&recv_code) {
                error!(
                    "[pole][discovery][connector] get service_contract request receive fail: code={} info={}",
                    resp.code,
                    resp.info.clone(),
                );
                return Err(PoleError::new(ServerError, resp.info));
            }

            if resp.r#type()
                != pole_specification::v1::discover_response::DiscoverResponseType::ServiceContracts
            {
                continue;
            }

            return select_service_contract(&req, resp).ok_or_else(|| {
                PoleError::new(
                    ServiceNotFound,
                    format!(
                        "service_contract not found: namespace={} service={} name={} version={} protocol={}",
                        req.contract.namespace,
                        req.contract.service,
                        req.contract.name,
                        req.contract.version,
                        req.contract.protocol
                    ),
                )
            });
        }
    }

    async fn create_config_file(&self, req: ConfigFileRequest) -> Result<bool, PoleError> {
        debug!("[pole][config][connector] send create config_file request={req:?}");

        let (code, info) = self.send_create_config_file(&req).await?;
        if ExecuteSuccess.eq(&code) {
            return Ok(true);
        }
        error!(
            "[pole][config][connector] send create config_file request to server receive fail: code={} info={}",
            code as i32,
            info.clone(),
        );
        Err(PoleError::new(ServerError, info))
    }

    async fn update_config_file(&self, req: ConfigFileRequest) -> Result<bool, PoleError> {
        debug!("[pole][config][connector] send update config_file request={req:?}");

        let (code, info) = self.send_update_config_file(&req).await?;
        if ExecuteSuccess.eq(&code) {
            return Ok(true);
        }
        error!(
            "[pole][config][connector] send update config_file request to server receive fail: code={} info={}",
            code as i32,
            info.clone(),
        );
        Err(PoleError::new(ServerError, info))
    }

    async fn release_config_file(&self, req: ConfigReleaseRequest) -> Result<bool, PoleError> {
        debug!("[pole][config][connector] send publish config_file request={req:?}");

        let (code, info) = self.send_publish_config_file(&req).await?;
        if ExecuteSuccess.eq(&code) {
            return Ok(true);
        }
        error!(
            "[pole][config][connector] send publish config_file request to server receive fail: code={} info={}",
            code as i32,
            info.clone(),
        );
        Err(PoleError::new(ServerError, info))
    }

    async fn upsert_publish_config_file(
        &self,
        req: ConfigPublishRequest,
    ) -> Result<bool, PoleError> {
        debug!("[pole][config][connector] send upsert and publish config_file request={req:?}");

        let file_req = req.to_config_file_request();
        self.ensure_config_file_written(&file_req).await?;

        let release_req = req.to_config_release_request();
        let (code, info) = self.send_publish_config_file(&release_req).await?;
        if ExecuteSuccess.eq(&code) {
            return Ok(true);
        }
        error!(
            "[pole][config][connector] send upsert publish config_file release request to server receive fail: code={} info={}",
            code as i32,
            info.clone(),
        );
        Err(PoleError::new(ServerError, info))
    }
}

fn run_discover_spec_stream(
    channel: Channel,
    executor: Arc<Runtime>,
) -> Result<
    (
        UnboundedSender<DiscoverRequest>,
        UnboundedReceiver<DiscoverResponse>,
    ),
    PoleError,
> {
    let (discover_sender, rx) = mpsc::unbounded_channel::<DiscoverRequest>();
    let (rsp_sender, rsp_recv) = mpsc::unbounded_channel::<DiscoverResponse>();
    _ = executor.spawn(async move {
        info!("[pole][discovery][connector] start naming_discover grpc stream");
        let reciver = UnboundedReceiverStream::new(rx);

        let mut client = DiscoverGrpcClient::new(channel);

        let discover_future = client.discover(tonic::Request::new(reciver));

        let discover_rt = discover_future.await;
        if discover_rt.is_err() {
            let stream_err = discover_rt.err().unwrap();
            error!(
                "[pole][discovery][connector] naming_discover stream receive err: {}",
                stream_err.clone().to_string()
            );
            return Err(PoleError::new(
                crate::core::model::error::ErrorCode::PluginError,
                stream_err.clone().to_string(),
            ));
        }

        let mut stream_recv = discover_rt.unwrap().into_inner();
        while let Some(received) = stream_recv.next().await {
            match received {
                Ok(rsp) => match rsp_sender.send(rsp) {
                    Ok(_) => {}
                    Err(err) => {
                        error!(
                            "[pole][discovery][connector] send discover request receive fail: {}",
                            err.to_string()
                        );
                    }
                },
                Err(err) => {
                    error!(
                        "[pole][discovery][connector] naming_discover stream receive err: {}",
                        err.to_string()
                    );
                }
            }
        }
        Ok(())
    });

    Ok((discover_sender, rsp_recv))
}

fn run_config_spec_stream(
    channel: Channel,
    executor: Arc<Runtime>,
) -> Result<
    (
        UnboundedSender<ConfigDiscoverRequest>,
        UnboundedReceiver<ConfigDiscoverResponse>,
    ),
    PoleError,
> {
    info!("[pole][config][connector] start config_discover grpc stream");
    let (config_sender, config_reciver) = mpsc::unbounded_channel::<ConfigDiscoverRequest>();
    let (rsp_sender, rsp_recv) = mpsc::unbounded_channel::<ConfigDiscoverResponse>();
    _ = executor.spawn(async move {
        let reciver = UnboundedReceiverStream::new(config_reciver);
        let mut client = ConfigGrpcClient::new(channel);

        let discover_future = client.discover(tonic::Request::new(reciver));

        let discover_rt = discover_future.await;
        if discover_rt.is_err() {
            let stream_err = discover_rt.err().unwrap();
            error!(
                "[pole][config][connector] config_discover stream receive err: {}",
                stream_err.clone().to_string()
            );
            return Err(PoleError::new(
                crate::core::model::error::ErrorCode::PluginError,
                stream_err.clone().to_string(),
            ));
        }

        let mut stream_recv: Streaming<ConfigDiscoverResponse> = discover_rt.unwrap().into_inner();
        while let Some(received) = stream_recv.next().await {
            match received {
                Ok(rsp) => match rsp_sender.send(rsp) {
                    Ok(_) => {}
                    Err(err) => {
                        error!(
                            "[pole][config][connector] send config request receive fail: {}",
                            err.to_string()
                        );
                    }
                },
                Err(err) => {
                    error!(
                        "[pole][config][connector] config_discover stream receive err: {}",
                        err.to_string()
                    );
                }
            }
        }
        Ok(())
    });

    Ok((config_sender, rsp_recv))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pole_specification::v1::{
        discover_response::DiscoverResponseType, ServiceContract as SpecServiceContract,
    };

    fn response(response_type: DiscoverResponseType) -> DiscoverResponse {
        DiscoverResponse {
            r#type: response_type.into(),
            service: Some(Service {
                namespace: "default".to_string(),
                name: "svc-a".to_string(),
                revision: "rev-3".to_string(),
                ..Service::default()
            }),
            ..DiscoverResponse::default()
        }
    }

    #[test]
    fn discover_response_watch_key_maps_new_service_rule_response_types() {
        let cases = [
            (DiscoverResponseType::Lossless, "LosslessRule#default#svc-a"),
            (
                DiscoverResponseType::TrafficSecurityRule,
                "TrafficSecurityRule#default#svc-a",
            ),
            (
                DiscoverResponseType::TrafficMirrorRule,
                "TrafficMirrorRule#default#svc-a",
            ),
            (
                DiscoverResponseType::TrafficMockRule,
                "TrafficMockRule#default#svc-a",
            ),
        ];

        for (response_type, expected) in cases {
            assert_eq!(
                discover_response_watch_key(&response(response_type)).as_deref(),
                Some(expected)
            );
        }
    }

    fn contract_request(
        name: &str,
        service: &str,
        version: &str,
        protocol: &str,
    ) -> ServiceContractRequest {
        ServiceContractRequest {
            flow_id: "flow".to_string(),
            contract: ServiceContract {
                name: name.to_string(),
                namespace: "default".to_string(),
                service: service.to_string(),
                version: version.to_string(),
                protocol: protocol.to_string(),
                content: String::new(),
                interfaces: Vec::new(),
                metadata: HashMap::new(),
            },
        }
    }

    #[test]
    fn service_contract_discover_request_uses_service_contracts_type() {
        let req = contract_request("api", "svc-a", "v1", "http");

        let discover_req = service_contract_discover_request(&req);

        assert_eq!(discover_req.r#type(), DiscoverRequestType::ServiceContracts);
        let service = discover_req.service.expect("service selector is required");
        assert_eq!(service.namespace, "default");
        assert_eq!(service.name, "svc-a");
    }

    #[test]
    fn select_service_contract_matches_requested_identity() {
        let req = contract_request("api", "svc-a", "v2", "grpc");
        let resp = DiscoverResponse {
            r#type: DiscoverResponseType::ServiceContracts.into(),
            service_contracts: vec![
                SpecServiceContract {
                    name: "api".to_string(),
                    namespace: "default".to_string(),
                    service: "svc-a".to_string(),
                    version: "v1".to_string(),
                    protocol: "grpc".to_string(),
                    ..SpecServiceContract::default()
                },
                SpecServiceContract {
                    name: "api".to_string(),
                    namespace: "default".to_string(),
                    service: "svc-a".to_string(),
                    version: "v2".to_string(),
                    protocol: "grpc".to_string(),
                    ..SpecServiceContract::default()
                },
            ],
            ..DiscoverResponse::default()
        };

        let contract = select_service_contract(&req, resp).expect("contract should match");

        assert_eq!(contract.name, "api");
        assert_eq!(contract.namespace, "default");
        assert_eq!(contract.service, "svc-a");
        assert_eq!(contract.version, "v2");
        assert_eq!(contract.protocol, "grpc");
    }

    #[test]
    fn select_service_contract_returns_none_when_identity_does_not_match() {
        let req = contract_request("api", "svc-a", "v2", "grpc");
        let resp = DiscoverResponse {
            r#type: DiscoverResponseType::ServiceContracts.into(),
            service_contracts: vec![SpecServiceContract {
                name: "api".to_string(),
                namespace: "default".to_string(),
                service: "svc-b".to_string(),
                version: "v2".to_string(),
                protocol: "grpc".to_string(),
                ..SpecServiceContract::default()
            }],
            ..DiscoverResponse::default()
        };

        assert!(select_service_contract(&req, resp).is_none());
    }
}

struct GrpcConnectorInterceptor {
    metadata: HashMap<String, String>,
}

impl Interceptor for GrpcConnectorInterceptor {
    fn call(
        &mut self,
        mut request: tonic::Request<()>,
    ) -> Result<tonic::Request<()>, tonic::Status> {
        let metadata = self.metadata.clone();
        for ele in metadata {
            let meta_key = AsciiMetadataKey::from_str(ele.0.to_string().as_str());
            if meta_key.is_err() {
                return Err(tonic::Status::internal("invalid metadata key"));
            }
            let meta_val: Result<MetadataValue<_>, _> = ele.1.to_string().parse();
            if meta_val.is_err() {
                return Err(tonic::Status::internal("invalid metadata value"));
            }
            request
                .metadata_mut()
                .insert(meta_key.unwrap(), meta_val.unwrap());
        }
        Ok(request)
    }
}
