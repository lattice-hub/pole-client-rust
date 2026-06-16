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

use crate::core::{
    flow::CircuitBreakerFlow,
    model::{
        circuitbreaker::{
            CallAbortedError, CheckResult, MethodResource, Resource, ResourceStat, RetStatus,
            ServiceResource,
        },
        error::PoleError,
    },
};

use super::req::{RequestContext, ResponseContext};

/// CircuitBreakerAPI .
#[async_trait::async_trait]
pub trait CircuitBreakerAPI
where
    Self: Send + Sync,
{
    /// check_resource .
    async fn check_resource(&self, resource: Resource) -> Result<CheckResult, PoleError>;
    /// report_stat .
    async fn report_stat(&self, stat: ResourceStat) -> Result<(), PoleError>;
    /// make_invoke_handler .
    async fn make_invoke_handler(
        &self,
        req: RequestContext,
    ) -> Result<Arc<InvokeHandler>, PoleError>;
}

/// InvokeHandler .
pub struct InvokeHandler {
    // req_ctx: 请求上下文
    req_ctx: RequestContext,
    // flow: 熔断器流程
    flow: Arc<CircuitBreakerFlow>,
    rule_refresher: Option<Arc<dyn CircuitBreakerRuleRefresher>>,
}

impl InvokeHandler {
    pub fn new(req_ctx: RequestContext, flow: Arc<CircuitBreakerFlow>) -> Self {
        InvokeHandler {
            req_ctx,
            flow,
            rule_refresher: None,
        }
    }

    pub(crate) fn new_with_refresher(
        req_ctx: RequestContext,
        flow: Arc<CircuitBreakerFlow>,
        rule_refresher: Arc<dyn CircuitBreakerRuleRefresher>,
    ) -> Self {
        InvokeHandler {
            req_ctx,
            flow,
            rule_refresher: Some(rule_refresher),
        }
    }

    /// acquire_permission 检查当前请求是否可放通
    pub async fn acquire_permission(&self) -> Result<(), CallAbortedError> {
        if let Some(refresher) = &self.rule_refresher {
            if let Err(err) = refresh_invoke_service_rules(&self.req_ctx, refresher.as_ref()).await
            {
                crate::error!(
                    "[circuitbreaker][invoke] refresh circuit breaker rules failed: {:?}",
                    err
                );
            }
        }
        let svc_res = ServiceResource::new_waith_caller(
            self.req_ctx.caller_service.clone(),
            self.req_ctx.callee_service.clone(),
        );

        match self
            .flow
            .check_resource(Resource::ServiceResource(svc_res))
            .await
        {
            Ok(ret) => {
                if ret.pass {
                    Ok(())
                } else {
                    Err(CallAbortedError::new(ret.rule_name, ret.fallback_info))
                }
            }
            Err(e) => {
                // 内部异常，不触发垄断，但是需要记录
                crate::error!("[circuitbreaker][invoke] check resource failed: {:?}", e);
                Ok(())
            }
        }
    }

    pub async fn on_success(&self, rsp: ResponseContext) -> Result<(), PoleError> {
        let cost = rsp.duration.clone();
        let mut code = -1 as i32;
        let mut status = RetStatus::RetSuccess;

        if let Some(r) = &self.req_ctx.result_to_code {
            code = r.on_success(rsp.result.unwrap());
        }
        if let Some(e) = rsp.error {
            let ret = e.downcast::<CallAbortedError>();
            if ret.is_ok() {
                status = RetStatus::RetReject;
            }
        }
        self.common_report(cost, code, status).await
    }

    pub async fn on_error(&self, rsp: ResponseContext) -> Result<(), PoleError> {
        let cost = rsp.duration.clone();
        let mut code = 0 as i32;
        let status = RetStatus::RetUnknown;

        if let Some(r) = &self.req_ctx.result_to_code {
            if let Some(err) = rsp.error {
                code = r.on_error(err);
            }
        }
        self.common_report(cost, code, status).await
    }

    async fn common_report(
        &self,
        cost: Duration,
        code: i32,
        status: RetStatus,
    ) -> Result<(), PoleError> {
        if let Some(refresher) = &self.rule_refresher {
            refresh_invoke_service_rules(&self.req_ctx, refresher.as_ref()).await?;
        }
        let stat = ResourceStat {
            resource: Resource::ServiceResource(ServiceResource::new_waith_caller(
                self.req_ctx.caller_service.clone(),
                self.req_ctx.callee_service.clone(),
            )),
            ret_code: code.to_string(),
            delay: cost,
            status: status.clone(),
        };

        let ret = self.flow.report_stat(stat).await;
        if ret.is_err() {
            crate::error!("[circuitbreaker][invoke] report stat failed");
            return ret;
        }

        if self.req_ctx.path.is_empty() {
            return Ok(());
        }

        // 补充一个接口级别的数据上报
        let stat = ResourceStat {
            resource: Resource::MethodResource(MethodResource::new_waith_caller(
                self.req_ctx.caller_service.clone(),
                self.req_ctx.callee_service.clone(),
                self.req_ctx.protocol.clone(),
                self.req_ctx.method.clone(),
                self.req_ctx.path.clone(),
            )),
            ret_code: code.to_string(),
            delay: cost,
            status,
        };
        self.flow.report_stat(stat).await
    }
}

#[async_trait::async_trait]
pub(crate) trait CircuitBreakerRuleRefresher: Send + Sync {
    async fn refresh_rules_for_resource(&self, resource: &Resource) -> Result<(), PoleError>;
}

pub(crate) async fn refresh_invoke_service_rules(
    req_ctx: &RequestContext,
    refresher: &dyn CircuitBreakerRuleRefresher,
) -> Result<(), PoleError> {
    let svc_res = ServiceResource::new_waith_caller(
        req_ctx.caller_service.clone(),
        req_ctx.callee_service.clone(),
    );
    refresher
        .refresh_rules_for_resource(&Resource::ServiceResource(svc_res))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::naming::ServiceKey;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct RecordingRuleRefresher {
        services: Mutex<Vec<ServiceKey>>,
    }

    #[async_trait::async_trait]
    impl CircuitBreakerRuleRefresher for RecordingRuleRefresher {
        async fn refresh_rules_for_resource(&self, resource: &Resource) -> Result<(), PoleError> {
            if let Resource::ServiceResource(resource) = resource {
                self.services.lock().unwrap().push(resource.callee.clone());
            }
            Ok(())
        }
    }

    fn request_context() -> RequestContext {
        RequestContext {
            caller_service: ServiceKey {
                namespace: "default".to_string(),
                name: "frontend".to_string(),
            },
            callee_service: ServiceKey {
                namespace: "default".to_string(),
                name: "orders".to_string(),
            },
            protocol: "http".to_string(),
            method: "GET".to_string(),
            path: "/orders".to_string(),
            result_to_code: None,
        }
    }

    fn test_flow() -> Arc<CircuitBreakerFlow> {
        Arc::new(CircuitBreakerFlow::new(
            crate::core::plugin::plugins::test_extensions_without_plugins(),
        ))
    }

    #[tokio::test]
    async fn invoke_handler_refreshes_callee_service_rules_before_checking() {
        let refresher = RecordingRuleRefresher::default();

        refresh_invoke_service_rules(&request_context(), &refresher)
            .await
            .unwrap();

        assert_eq!(
            refresher.services.lock().unwrap().as_slice(),
            &[ServiceKey {
                namespace: "default".to_string(),
                name: "orders".to_string(),
            }]
        );
    }

    #[test]
    fn invoke_handler_refreshes_before_acquire_and_report() {
        let flow = test_flow();
        let refresher = Arc::new(RecordingRuleRefresher::default());
        let handler = InvokeHandler::new_with_refresher(request_context(), flow, refresher.clone());

        tokio::runtime::Runtime::new().unwrap().block_on(async {
            assert!(handler.acquire_permission().await.is_ok());
            handler
                .on_success(ResponseContext {
                    duration: Duration::from_millis(7),
                    result: None,
                    error: None,
                })
                .await
                .unwrap();
        });

        let services = refresher.services.lock().unwrap();
        assert_eq!(services.len(), 2);
        assert!(services
            .iter()
            .all(|service| { service.namespace == "default" && service.name == "orders" }));
    }
}
