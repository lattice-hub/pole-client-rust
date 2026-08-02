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

use std::time::Duration;

use crate::core::model::{
    error::{ErrorCode, PoleError},
    ArgumentType,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum QuotaResource {
    Qps,
    Token,
    Concurrency,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QuotaAmount {
    pub resource: QuotaResource,
    pub amount: u32,
}

/// QuotaRequest 获取请求配额
#[derive(Clone, Debug)]
pub struct QuotaRequest {
    pub flow_id: String,
    pub timeout: Duration,
    // service 服务名
    pub service: String,
    // namespace 命名空间
    pub namespace: String,
    // method 方法名
    pub method: String,
    // traffic_label_provider 流量标签提供者
    pub traffic_label_provider: fn(ArgumentType, &str) -> Option<String>,
    // quotas 本次请求按逻辑资源预留的最大配额
    pub quotas: Vec<QuotaAmount>,
    // lease_ttl 租约最长存活时间；调用方应在此时间内完成结算
    pub lease_ttl: Duration,
}

impl QuotaRequest {
    pub fn check_valid(&self) -> Result<(), PoleError> {
        if self.service.is_empty() {
            return Err(PoleError::new(
                ErrorCode::ApiInvalidArgument,
                "service is empty".to_string(),
            ));
        }

        if self.namespace.is_empty() {
            return Err(PoleError::new(
                ErrorCode::ApiInvalidArgument,
                "namespace is empty".to_string(),
            ));
        }
        if self.quotas.is_empty() {
            return Err(PoleError::new(
                ErrorCode::ApiInvalidArgument,
                "quota request must contain at least one resource".to_string(),
            ));
        }
        let mut resources = std::collections::HashSet::new();
        for quota in &self.quotas {
            if quota.amount == 0 {
                return Err(PoleError::new(
                    ErrorCode::ApiInvalidArgument,
                    "quota amount must be greater than zero".to_string(),
                ));
            }
            if !resources.insert(quota.resource) {
                return Err(PoleError::new(
                    ErrorCode::ApiInvalidArgument,
                    format!("duplicate quota resource {:?}", quota.resource),
                ));
            }
        }
        if self.lease_ttl.is_zero() {
            return Err(PoleError::new(
                ErrorCode::ApiInvalidArgument,
                "quota lease ttl must be greater than zero".to_string(),
            ));
        }
        if self.lease_ttl.as_secs() > u64::from(u32::MAX)
            || (self.lease_ttl.as_secs() == u64::from(u32::MAX)
                && self.lease_ttl.subsec_nanos() > 0)
        {
            return Err(PoleError::new(
                ErrorCode::ApiInvalidArgument,
                "quota lease ttl exceeds the protocol limit".to_string(),
            ));
        }
        Ok(())
    }

    pub(crate) fn amount_for(&self, resource: QuotaResource) -> Option<u32> {
        self.quotas
            .iter()
            .find(|quota| quota.resource == resource)
            .map(|quota| quota.amount)
    }
}
