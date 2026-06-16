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

use crate::core::{
    context::SDKContext,
    flow::CircuitBreakerFlow,
    model::{
        circuitbreaker::{CheckResult, Resource, ResourceStat},
        error::{ErrorCode, PoleError},
        naming::ServiceKey,
    },
};
use crate::discovery::req::{GetServiceRuleRequest, ServiceRuleType};
use pole_specification::v1::CircuitBreakerRule;

use super::{
    api::{CircuitBreakerAPI, CircuitBreakerRuleRefresher, InvokeHandler},
    req::RequestContext,
};

/// DefaultCircuitBreakerAPI .
pub struct DefaultCircuitBreakerAPI {
    context: Arc<SDKContext>,
    // manage_sdk: 是否管理 sdk_context 的生命周期
    manage_sdk: bool,
    // flow: 熔断器流程
    flow: Arc<CircuitBreakerFlow>,
}

impl DefaultCircuitBreakerAPI {
    pub fn new_raw(context: SDKContext) -> Self {
        let ctx = Arc::new(context);
        let extensions = ctx.get_engine().get_extensions();
        Self {
            context: ctx,
            manage_sdk: true,
            flow: Arc::new(CircuitBreakerFlow::new(extensions)),
        }
    }

    pub fn new(context: Arc<SDKContext>) -> Self {
        let extensions = context.get_engine().get_extensions();
        Self {
            context,
            manage_sdk: false,
            flow: Arc::new(CircuitBreakerFlow::new(extensions)),
        }
    }

    pub(crate) async fn load_circuit_breaker_rules(
        &self,
        namespace: String,
        service: String,
        timeout: std::time::Duration,
    ) -> Result<Vec<CircuitBreakerRule>, PoleError> {
        let service_rule = self
            .context
            .get_engine()
            .get_service_rule(GetServiceRuleRequest {
                namespace,
                service,
                rule_type: ServiceRuleType::CircuitBreaker,
                timeout,
            })
            .await?;
        circuit_breaker_rules_from_service_rules(service_rule.rules)
    }

    async fn refresh_rules_for_resource(&self, resource: &Resource) -> Result<(), PoleError> {
        let Some(service_key) = circuit_breaker_rule_service_key(resource) else {
            return Ok(());
        };
        let rules = self
            .load_circuit_breaker_rules(
                service_key.namespace,
                service_key.name,
                self.context.conf.global.api.timeout,
            )
            .await?;
        self.flow.update_rules(rules);
        Ok(())
    }

    fn rule_refresher(&self) -> Arc<dyn CircuitBreakerRuleRefresher> {
        Arc::new(DefaultCircuitBreakerRuleRefresher {
            context: self.context.clone(),
            flow: self.flow.clone(),
        })
    }
}

struct DefaultCircuitBreakerRuleRefresher {
    context: Arc<SDKContext>,
    flow: Arc<CircuitBreakerFlow>,
}

#[async_trait::async_trait]
impl CircuitBreakerRuleRefresher for DefaultCircuitBreakerRuleRefresher {
    async fn refresh_rules_for_resource(&self, resource: &Resource) -> Result<(), PoleError> {
        let Some(service_key) = circuit_breaker_rule_service_key(resource) else {
            return Ok(());
        };
        let service_rule = self
            .context
            .get_engine()
            .get_service_rule(GetServiceRuleRequest {
                namespace: service_key.namespace,
                service: service_key.name,
                rule_type: ServiceRuleType::CircuitBreaker,
                timeout: self.context.conf.global.api.timeout,
            })
            .await?;
        self.flow
            .update_rules(circuit_breaker_rules_from_service_rules(
                service_rule.rules,
            )?);
        Ok(())
    }
}

fn circuit_breaker_rules_from_service_rules(
    rules: Vec<Box<dyn std::any::Any + Send>>,
) -> Result<Vec<CircuitBreakerRule>, PoleError> {
    let mut circuit_breaker_rules = Vec::with_capacity(rules.len());
    for rule in rules {
        let type_id = rule.type_id();
        match rule.downcast::<CircuitBreakerRule>() {
            Ok(rule) => circuit_breaker_rules.push(*rule),
            Err(_) => {
                return Err(PoleError::new(
                    ErrorCode::InvalidRule,
                    format!(
                        "rule type error, expect CircuitBreakerRule, but got {:?}",
                        type_id
                    ),
                ));
            }
        }
    }
    Ok(circuit_breaker_rules)
}

fn circuit_breaker_rule_service_key(resource: &Resource) -> Option<ServiceKey> {
    match resource {
        Resource::ServiceResource(resource) => Some(resource.callee.clone()),
        Resource::MethodResource(resource) => Some(resource.callee.clone()),
        Resource::InstanceResource(_) => None,
    }
}

impl Drop for DefaultCircuitBreakerAPI {
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
impl CircuitBreakerAPI for DefaultCircuitBreakerAPI {
    async fn check_resource(&self, resource: Resource) -> Result<CheckResult, PoleError> {
        self.refresh_rules_for_resource(&resource).await?;
        self.flow.check_resource(resource).await
    }

    async fn report_stat(&self, stat: ResourceStat) -> Result<(), PoleError> {
        self.refresh_rules_for_resource(&stat.resource).await?;
        self.flow.report_stat(stat).await
    }

    async fn make_invoke_handler(
        &self,
        req: RequestContext,
    ) -> Result<Arc<InvokeHandler>, PoleError> {
        Ok(Arc::new(InvokeHandler::new_with_refresher(
            req,
            self.flow.clone(),
            self.rule_refresher(),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pole_specification::v1::{CircuitBreakerRule, RateLimit};

    #[test]
    fn circuit_breaker_rules_from_service_rules_downcasts_expected_type() {
        let rules = circuit_breaker_rules_from_service_rules(vec![Box::new(CircuitBreakerRule {
            id: "cb-1".to_string(),
            ..CircuitBreakerRule::default()
        })])
        .unwrap();

        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].id, "cb-1");
    }

    #[test]
    fn circuit_breaker_rules_from_service_rules_rejects_wrong_rule_type() {
        let err = circuit_breaker_rules_from_service_rules(vec![Box::new(RateLimit::default())])
            .unwrap_err();

        assert!(err.to_string().contains("expect CircuitBreakerRule"));
    }

    #[test]
    fn circuit_breaker_rule_service_key_uses_callee_service_resource() {
        let key = ServiceKey {
            namespace: "default".to_string(),
            name: "orders".to_string(),
        };
        let resource = Resource::ServiceResource(
            crate::core::model::circuitbreaker::ServiceResource::new(key.clone()),
        );

        assert_eq!(circuit_breaker_rule_service_key(&resource), Some(key));
    }
}
