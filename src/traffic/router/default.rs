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

use std::{sync::Arc, time::Duration};
use tokio::task::JoinHandle;

use super::{api::RouterAPI, req::ProcessRouteResponse};
use crate::core::{
    context::SDKContext,
    flow::RouterFlow,
    model::{
        error::{ErrorCode, PoleError},
        naming::ServiceInstances,
    },
    plugin::router::RouteContext,
};
use crate::debug;
use crate::traffic::policy::{
    api::MirrorSender,
    default::{dispatch_mirror_requests, evaluate_traffic_governance_from_cache, HttpMirrorSender},
    req::{MirrorRequest, TrafficGovernanceResult},
};

pub struct DefaultRouterAPI {
    manage_sdk: bool,
    context: Arc<SDKContext>,
    flow: Arc<RouterFlow>,
    mirror_sender: Option<Arc<dyn MirrorSender>>,
}

impl DefaultRouterAPI {
    pub fn new_raw(context: SDKContext) -> Self {
        let ctx = Arc::new(context);
        let extensions = ctx.get_engine().get_extensions();
        let mirror_sender = Some(Arc::new(HttpMirrorSender::with_resource_cache(
            extensions.get_resource_cache(),
            Duration::from_secs(1),
        )) as Arc<dyn MirrorSender>);
        Self {
            manage_sdk: true,
            context: ctx,
            flow: Arc::new(RouterFlow::new(extensions)),
            mirror_sender,
        }
    }

    pub fn new(context: Arc<SDKContext>) -> Self {
        let extensions = context.get_engine().get_extensions();
        let mirror_sender = Some(Arc::new(HttpMirrorSender::with_resource_cache(
            extensions.get_resource_cache(),
            Duration::from_secs(1),
        )) as Arc<dyn MirrorSender>);
        Self {
            manage_sdk: false,
            context: context,
            flow: Arc::new(RouterFlow::new(extensions)),
            mirror_sender,
        }
    }
}

impl Drop for DefaultRouterAPI {
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

fn reject_denied_traffic(governance: &TrafficGovernanceResult) -> Result<(), PoleError> {
    if governance.security.allowed {
        return Ok(());
    }

    let message = governance
        .security
        .reject_effect
        .as_ref()
        .map(|effect| {
            format!(
                "traffic security denied: code={} message={}",
                effect.code, effect.message
            )
        })
        .unwrap_or_else(|| "traffic security denied".to_string());

    Err(PoleError::new(ErrorCode::UNAUTHORIZED, message))
}

fn mock_route_response(
    governance: &TrafficGovernanceResult,
    service_instances: &ServiceInstances,
) -> Option<ProcessRouteResponse> {
    if governance.mock.is_none() {
        return None;
    }

    Some(ProcessRouteResponse {
        service_instances: service_instances.clone(),
        traffic_governance: governance.clone(),
    })
}

fn dispatch_route_mirrors(
    sender: Option<Arc<dyn MirrorSender>>,
    governance: &TrafficGovernanceResult,
    mirror_request: Option<MirrorRequest>,
) -> Vec<JoinHandle<()>> {
    if governance.mirrors.is_empty() {
        return Vec::new();
    }

    let Some(sender) = sender else {
        crate::warn!("[pole][router_api] mirror destinations matched but sender is missing");
        return Vec::new();
    };
    let Some(request) = mirror_request else {
        crate::warn!("[pole][router_api] mirror destinations matched but request is missing");
        return Vec::new();
    };

    dispatch_mirror_requests(sender, governance.mirrors.clone(), request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traffic::policy::{api::MirrorSender, req::MirrorRequest};
    use pole_specification::v1::MirrorDestination;
    use pole_specification::v1::MockResponse;
    use pole_specification::v1::TrafficSecurityRejectEffect;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tokio::sync::mpsc;

    struct RecordingMirrorSender {
        seen: Arc<Mutex<Vec<(MirrorDestination, MirrorRequest)>>>,
        started: mpsc::UnboundedSender<()>,
    }

    #[async_trait::async_trait]
    impl MirrorSender for RecordingMirrorSender {
        async fn send(
            &self,
            destination: MirrorDestination,
            request: MirrorRequest,
        ) -> Result<(), PoleError> {
            self.seen.lock().unwrap().push((destination, request));
            let _ = self.started.send(());
            Ok(())
        }
    }

    #[test]
    fn traffic_security_deny_is_converted_to_router_error() {
        let governance = TrafficGovernanceResult {
            security: crate::traffic::policy::req::TrafficSecurityDecision {
                allowed: false,
                reject_effect: Some(TrafficSecurityRejectEffect {
                    code: "DENIED".to_string(),
                    message: "blocked".to_string(),
                }),
            },
            ..TrafficGovernanceResult::default()
        };

        let err = reject_denied_traffic(&governance).unwrap_err();

        assert!(err.to_string().contains("UNAUTHORIZED"));
        assert!(err.to_string().contains("DENIED"));
        assert!(err.to_string().contains("blocked"));
    }

    #[test]
    fn mock_governance_short_circuits_route_response() {
        let governance = TrafficGovernanceResult {
            mock: Some(MockResponse {
                code: "OK".to_string(),
                body: "{\"ok\":true}".to_string(),
                ..MockResponse::default()
            }),
            ..TrafficGovernanceResult::default()
        };

        let response =
            mock_route_response(&governance, &ServiceInstances::default()).expect("mock matched");

        assert_eq!(response.traffic_governance.mock.unwrap().code, "OK");
        assert!(response.service_instances.instances.is_empty());
    }

    #[tokio::test]
    async fn route_mirror_dispatch_uses_process_route_request_payload() {
        let (started_tx, mut started_rx) = mpsc::unbounded_channel();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sender = Arc::new(RecordingMirrorSender {
            seen: seen.clone(),
            started: started_tx,
        });
        let request = MirrorRequest {
            method: "POST".to_string(),
            path: "/orders/42".to_string(),
            headers: HashMap::from([("x-debug".to_string(), "true".to_string())]),
            body: b"{\"id\":42}".to_vec(),
        };
        let governance = TrafficGovernanceResult {
            mirrors: vec![MirrorDestination {
                namespace: "shadow".to_string(),
                service: "orders-shadow".to_string(),
                labels: HashMap::new(),
            }],
            ..TrafficGovernanceResult::default()
        };

        let handles = dispatch_route_mirrors(Some(sender), &governance, Some(request.clone()));

        assert_eq!(handles.len(), 1);
        started_rx.recv().await.unwrap();
        for handle in handles {
            handle.await.unwrap();
        }
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0.service, "orders-shadow");
        assert_eq!(seen[0].1, request);
    }
}

#[async_trait::async_trait]
impl RouterAPI for DefaultRouterAPI {
    async fn router(
        &self,
        req: super::req::ProcessRouteRequest,
    ) -> Result<super::req::ProcessRouteResponse, PoleError> {
        debug!("[pole][router_api] route request {:?}", req);
        let extensions = self.context.get_engine().get_extensions();
        let route_ctx = RouteContext {
            route_info: req.route_info.clone(),
            extensions: Some(extensions.clone()),
        };
        let traffic_governance = evaluate_traffic_governance_from_cache(
            &route_ctx,
            extensions.get_resource_cache(),
            Duration::from_secs(1),
        )
        .await
        .unwrap_or_else(|err| {
            crate::error!(
                "[pole][router_api] evaluate traffic governance failed: {}",
                err.to_string()
            );
            TrafficGovernanceResult::default()
        });
        reject_denied_traffic(&traffic_governance)?;
        let mirror_handles = dispatch_route_mirrors(
            self.mirror_sender.clone(),
            &traffic_governance,
            req.mirror_request.clone(),
        );
        drop(mirror_handles);

        if let Some(response) = mock_route_response(&traffic_governance, &req.service_instances) {
            return Ok(response);
        }

        let ret = self
            .flow
            .choose_instances(req.route_info.clone(), req.service_instances)
            .await;

        match ret {
            Ok(result) => Ok(ProcessRouteResponse {
                service_instances: result,
                traffic_governance,
            }),
            Err(e) => Err(e),
        }
    }

    async fn load_balance(
        &self,
        req: super::req::ProcessLoadBalanceRequest,
    ) -> Result<super::req::ProcessLoadBalanceResponse, PoleError> {
        debug!("[pole][router_api] load_balance request {:?}", req);

        let criteria = req.criteria.clone();
        let mut lb_policy = criteria.policy.clone();

        if lb_policy.is_empty() {
            lb_policy.clone_from(&self.context.conf.consumer.load_balancer.default_policy);
        }

        let lb = self.flow.lookup_loadbalancer(&lb_policy).await;

        if lb.is_none() {
            crate::error!("[pole][router_api] load balancer {} not found", lb_policy);
            return Err(PoleError::new(
                ErrorCode::PluginError,
                format!("load balancer {} not found", lb_policy),
            ));
        }

        let lb = lb.unwrap();
        let result = lb.choose_instance(req.criteria, req.service_instances);

        match result {
            Ok(instance) => Ok(super::req::ProcessLoadBalanceResponse { instance }),
            Err(e) => Err(e),
        }
    }
}
