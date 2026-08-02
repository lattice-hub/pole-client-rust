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

use crate::core::{context::SDKContext, model::error::PoleError};

use super::{
    default::DefaultRateLimitAPI,
    req::{QuotaAmount, QuotaRequest, QuotaResource},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QuotaConsumption {
    pub resource: QuotaResource,
    pub consumed_total: u32,
}

#[async_trait::async_trait]
pub(crate) trait QuotaLeaseBackend: Send + Sync {
    async fn update(
        &self,
        consumptions: Vec<QuotaConsumption>,
        sequence: u64,
    ) -> Result<(), PoleError>;

    async fn finish(
        &self,
        consumptions: Vec<QuotaConsumption>,
        sequence: u64,
    ) -> Result<(), PoleError>;

    fn abandon(&self, consumptions: Vec<QuotaConsumption>, sequence: u64);
}

struct QuotaLeaseState {
    consumptions: Vec<QuotaConsumption>,
    sequence: u64,
    finished: bool,
}

/// 一次成功预留的配额租约。
///
/// 单资源租约可使用 `update` 和 `finish`；同时预留 RPM、TPM 和并发等多个
/// 逻辑资源时，使用 `update_consumptions` 和 `finish_consumptions` 分别提交各资源
/// 的累计消费量。SDK 会把逻辑资源展开到关联的远端 counter，并隐藏 lease id、
/// sequence 和消息排序。
///
/// 更新、结算或对应 Future 被取消时，租约保留最后一次成功确认的状态，调用方可以
/// 用相同累计值重试而不会重复扣减。Drop 会以最后一次成功确认的累计值结算本地租约，
/// 并异步尝试结算分布式租约；若远端不可达，服务端 TTL 是最终回收边界。
pub struct QuotaLease {
    reservations: Vec<QuotaAmount>,
    backend: Arc<dyn QuotaLeaseBackend>,
    state: tokio::sync::Mutex<QuotaLeaseState>,
}

impl QuotaLease {
    pub(crate) fn new(
        reservations: Vec<QuotaAmount>,
        consumptions: Vec<QuotaConsumption>,
        backend: Arc<dyn QuotaLeaseBackend>,
    ) -> Self {
        Self {
            reservations,
            backend,
            state: tokio::sync::Mutex::new(QuotaLeaseState {
                consumptions,
                sequence: 0,
                finished: false,
            }),
        }
    }

    /// 更新租约内单个逻辑指标的累计消费量。
    pub async fn update(&self, consumed_total: u32) -> Result<(), PoleError> {
        let resource = self.single_resource()?;
        self.update_consumptions(&[QuotaConsumption {
            resource,
            consumed_total,
        }])
        .await
    }

    /// 按逻辑资源更新累计消费量；SDK 会展开到每个关联 counter。
    pub async fn update_consumptions(&self, updates: &[QuotaConsumption]) -> Result<(), PoleError> {
        let mut state = self.state.lock().await;
        let next = apply_consumption_updates(
            &self.reservations,
            &state.consumptions,
            updates,
            state.finished,
        )?;
        if next == state.consumptions {
            return Ok(());
        }
        let sequence = next_sequence(state.sequence)?;
        self.backend.update(next.clone(), sequence).await?;
        state.consumptions = next;
        state.sequence = sequence;
        Ok(())
    }

    /// 结算租约并归还未使用的预留配额。
    ///
    /// 失败时租约保持可重试状态；使用相同累计值重试不会重复扣减。
    pub async fn finish(&self, consumed_total: u32) -> Result<(), PoleError> {
        let resource = self.single_resource()?;
        self.finish_consumptions(&[QuotaConsumption {
            resource,
            consumed_total,
        }])
        .await
    }

    /// 按逻辑资源结算租约；未列出的资源沿用最后确认的累计值。
    pub async fn finish_consumptions(&self, updates: &[QuotaConsumption]) -> Result<(), PoleError> {
        let mut state = self.state.lock().await;
        if state.finished {
            let next =
                apply_consumption_updates(&self.reservations, &state.consumptions, updates, false)?;
            if next == state.consumptions {
                return Ok(());
            }
            return Err(PoleError::new(
                crate::core::model::error::ErrorCode::InvalidState,
                "quota lease is already finished".to_string(),
            ));
        }
        let next =
            apply_consumption_updates(&self.reservations, &state.consumptions, updates, false)?;
        let sequence = next_sequence(state.sequence)?;
        self.backend.finish(next.clone(), sequence).await?;
        state.consumptions = next;
        state.sequence = sequence;
        state.finished = true;
        Ok(())
    }

    fn single_resource(&self) -> Result<QuotaResource, PoleError> {
        if self.reservations.len() == 1 {
            return Ok(self.reservations[0].resource);
        }
        Err(PoleError::new(
            crate::core::model::error::ErrorCode::ApiInvalidArgument,
            "multi-resource quota lease requires the per-resource API".to_string(),
        ))
    }
}

impl Drop for QuotaLease {
    fn drop(&mut self) {
        let Ok(state) = self.state.try_lock() else {
            return;
        };
        if state.finished {
            return;
        }
        if let Ok(sequence) = next_sequence(state.sequence) {
            self.backend.abandon(state.consumptions.clone(), sequence);
        }
    }
}

fn apply_consumption_updates(
    reservations: &[QuotaAmount],
    current: &[QuotaConsumption],
    updates: &[QuotaConsumption],
    finished: bool,
) -> Result<Vec<QuotaConsumption>, PoleError> {
    use crate::core::model::error::ErrorCode;

    if finished {
        return Err(PoleError::new(
            ErrorCode::InvalidState,
            "quota lease is already finished".to_string(),
        ));
    }
    let mut next = current.to_vec();
    let mut seen = std::collections::HashSet::new();
    for update in updates {
        if !seen.insert(update.resource) {
            return Err(PoleError::new(
                ErrorCode::ApiInvalidArgument,
                format!("duplicate quota consumption for {:?}", update.resource),
            ));
        }
        let reserved = reservations
            .iter()
            .find(|reservation| reservation.resource == update.resource)
            .map(|reservation| reservation.amount)
            .ok_or_else(|| {
                PoleError::new(
                    ErrorCode::ApiInvalidArgument,
                    format!("quota resource {:?} was not reserved", update.resource),
                )
            })?;
        let consumption = next
            .iter_mut()
            .find(|consumption| consumption.resource == update.resource)
            .expect("every reservation has an initial consumption");
        if update.consumed_total < consumption.consumed_total {
            return Err(PoleError::new(
                ErrorCode::ApiInvalidArgument,
                format!(
                    "quota consumed total cannot decrease from {} to {}",
                    consumption.consumed_total, update.consumed_total
                ),
            ));
        }
        if update.consumed_total > reserved {
            return Err(PoleError::new(
                ErrorCode::ApiInvalidArgument,
                format!(
                    "quota consumed total {} exceeds reservation {reserved}",
                    update.consumed_total
                ),
            ));
        }
        consumption.consumed_total = update.consumed_total;
    }
    Ok(next)
}

fn next_sequence(sequence: u64) -> Result<u64, PoleError> {
    sequence.checked_add(1).ok_or_else(|| {
        PoleError::new(
            crate::core::model::error::ErrorCode::InvalidState,
            "quota lease sequence is exhausted".to_string(),
        )
    })
}

/// new_ratelimit_api
pub fn new_ratelimit_api() -> Result<impl RateLimitAPI, PoleError> {
    let context_ret = SDKContext::default();
    if context_ret.is_err() {
        return Err(context_ret.err().unwrap());
    }

    Ok(DefaultRateLimitAPI::new_raw(context_ret.unwrap()))
}

/// new_ratelimit_api_by_context
pub fn new_ratelimit_api_by_context(
    context: Arc<SDKContext>,
) -> Result<impl RateLimitAPI, PoleError> {
    Ok(DefaultRateLimitAPI::new(context))
}

#[async_trait::async_trait]
pub trait RateLimitAPI
where
    Self: Send + Sync,
{
    /// 在业务请求开始前原子预留最大配额；限流拒绝通过 `RequestLimit` 返回。
    ///
    /// 请求超时或 Future 被取消时，服务端可能已经完成预留但客户端尚未收到 lease id；
    /// 该未知预留不会计为消费，并由 `lease_ttl` 到期回收。
    async fn reserve_quota(&self, req: QuotaRequest) -> Result<QuotaLease, PoleError>;
}
