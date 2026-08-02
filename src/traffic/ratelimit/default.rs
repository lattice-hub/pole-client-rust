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

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use sha2::{Digest, Sha256};

use crate::core::{
    context::SDKContext,
    model::{error::PoleError, ArgumentType},
};
use crate::discovery::req::{GetAllInstanceRequest, GetServiceRuleRequest, ServiceRuleType};
use crate::plugins::router::rule::helper::match_label_value;
use crate::traffic::matcher::{ApiMatchIndex, ApiMatchInput};
use pole_specification::v1::{
    limit_trigger, match_argument, match_string, rate_limit, LimitTrigger, MatchArgument, RateLimit,
};

use super::{
    api::{QuotaConsumption, QuotaLease, QuotaLeaseBackend, RateLimitAPI},
    distributed::{DistributedQuotaManager, DistributedQuotaSpec, DistributedReserveResult},
    req::{QuotaAmount, QuotaRequest, QuotaResource},
};

pub struct DefaultRateLimitAPI {
    manage_sdk: bool,
    context: Arc<SDKContext>,
    local_counter: Arc<LocalQuotaCounter>,
    distributed_quota: DistributedQuotaManager,
    #[cfg(test)]
    test_inputs: Option<(Vec<RateLimit>, Vec<crate::core::model::naming::Instance>)>,
}

impl DefaultRateLimitAPI {
    pub fn new_raw(context: SDKContext) -> Self {
        let ctx = Arc::new(context);
        Self {
            manage_sdk: true,
            context: ctx,
            local_counter: Arc::new(LocalQuotaCounter::new()),
            distributed_quota: DistributedQuotaManager::default(),
            #[cfg(test)]
            test_inputs: None,
        }
    }

    pub fn new(context: Arc<SDKContext>) -> Self {
        Self {
            manage_sdk: false,
            context,
            local_counter: Arc::new(LocalQuotaCounter::new()),
            distributed_quota: DistributedQuotaManager::default(),
            #[cfg(test)]
            test_inputs: None,
        }
    }

    async fn load_rate_limit_rules(&self, req: &QuotaRequest) -> Result<Vec<RateLimit>, PoleError> {
        #[cfg(test)]
        if let Some((rules, _)) = &self.test_inputs {
            return Ok(rules.clone());
        }
        let service_rule = self
            .context
            .get_engine()
            .get_service_rule(GetServiceRuleRequest {
                namespace: req.namespace.clone(),
                service: req.service.clone(),
                rule_type: ServiceRuleType::RateLimit,
                timeout: req.timeout,
            })
            .await?;
        rate_limits_from_service_rules(service_rule.rules)
    }

    async fn reserve(
        &self,
        req: &QuotaRequest,
        rules: &[RateLimit],
    ) -> Result<QuotaLease, PoleError> {
        let mut selected = HashMap::new();
        for entry in matching_quota_triggers(req, rules) {
            let Some(resource) = quota_resource(entry.trigger.resource()) else {
                continue;
            };
            if req.amount_for(resource).is_some() {
                selected.entry(resource).or_insert(entry);
            }
        }
        let initial = initial_consumptions(&req.quotas);
        let mut backends: Vec<Arc<dyn QuotaLeaseBackend>> = Vec::new();
        let distributed = req
            .quotas
            .iter()
            .filter_map(|quota| {
                let entry = selected.get(&quota.resource).copied()?;
                uses_distributed_quota(entry.rule, entry.trigger).then_some(DistributedQuotaSpec {
                    rule: entry.rule,
                    trigger: entry.trigger,
                    amount: quota.amount,
                })
            })
            .collect::<Vec<_>>();
        if !distributed.is_empty() {
            match self.reserve_distributed_quota(req, &distributed).await {
                Ok(DistributedReserveResult::Reserved(backend)) => backends.push(backend),
                Ok(DistributedReserveResult::Rejected(message)) => {
                    return Err(quota_rejected(message));
                }
                Err(error) => {
                    for spec in &distributed {
                        match self.reserve_failover(req, spec.rule, spec.trigger, spec.amount) {
                            Ok(Some(backend)) => backends.push(backend),
                            Ok(None) => {}
                            Err(_) => {
                                rollback_reservations(&backends, &initial);
                                return Err(error);
                            }
                        }
                    }
                }
            }
        }
        for quota in &req.quotas {
            let Some(entry) = selected.get(&quota.resource).copied() else {
                continue;
            };
            if uses_distributed_quota(entry.rule, entry.trigger) {
                continue;
            }
            let backend = if uses_local_counter(entry.rule, entry.trigger) {
                match self.reserve_local(req, entry.rule, entry.trigger, quota.amount) {
                    Ok(backend) => backend,
                    Err(error) => {
                        rollback_reservations(&backends, &initial);
                        return Err(error);
                    }
                }
            } else {
                None
            };
            if let Some(backend) = backend {
                backends.push(backend);
            }
        }
        Ok(QuotaLease::new(
            req.quotas.clone(),
            initial,
            Arc::new(CompositeLeaseBackend { backends }),
        ))
    }

    fn reserve_failover(
        &self,
        req: &QuotaRequest,
        rule: &RateLimit,
        trigger: &LimitTrigger,
        amount: u32,
    ) -> Result<Option<Arc<dyn QuotaLeaseBackend>>, PoleError> {
        match trigger.failover() {
            limit_trigger::FailoverType::FailoverPass => Ok(None),
            limit_trigger::FailoverType::FailoverLocal => {
                self.reserve_local(req, rule, trigger, amount)
            }
        }
    }

    fn reserve_local(
        &self,
        req: &QuotaRequest,
        rule: &RateLimit,
        trigger: &LimitTrigger,
        amount: u32,
    ) -> Result<Option<Arc<dyn QuotaLeaseBackend>>, PoleError> {
        match self.local_counter.reserve(req, rule, trigger, amount)? {
            LocalReserveResult::Reserved(backend) => Ok(Some(backend)),
            LocalReserveResult::Rejected(message) => Err(quota_rejected(message)),
            LocalReserveResult::NotConfigured => Ok(None),
        }
    }

    async fn reserve_distributed_quota(
        &self,
        req: &QuotaRequest,
        specs: &[DistributedQuotaSpec<'_>],
    ) -> Result<DistributedReserveResult, PoleError> {
        let rule = specs
            .first()
            .expect("distributed quota group is non-empty")
            .rule;
        let cluster = rule.cluster.as_ref().ok_or_else(|| {
            PoleError::new(
                crate::core::model::error::ErrorCode::InvalidRule,
                format!(
                    "global rate limit rule {} does not declare cluster",
                    rule.id
                ),
            )
        })?;
        #[cfg(test)]
        let test_instances = self
            .test_inputs
            .as_ref()
            .map(|(_, instances)| instances.clone());
        #[cfg(not(test))]
        let test_instances: Option<Vec<crate::core::model::naming::Instance>> = None;
        let instances = if let Some(instances) = test_instances {
            instances
        } else {
            self.context
                .get_engine()
                .get_service_instances(
                    GetAllInstanceRequest {
                        flow_id: req.flow_id.clone(),
                        timeout: req.timeout,
                        namespace: cluster.namespace.clone(),
                        service: cluster.service.clone(),
                    },
                    true,
                )
                .await?
                .instances
                .instances
        };
        let client_context = self
            .context
            .get_engine()
            .get_extensions()
            .client_ctx
            .clone();
        self.distributed_quota
            .reserve_many(req, specs, &instances, &client_context.client_id)
            .await
    }
}

impl Drop for DefaultRateLimitAPI {
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
impl RateLimitAPI for DefaultRateLimitAPI {
    async fn reserve_quota(&self, req: QuotaRequest) -> Result<QuotaLease, PoleError> {
        req.check_valid()?;
        let rules = self.load_rate_limit_rules(&req).await?;
        self.reserve(&req, &rules).await
    }
}

struct LocalQuotaCounter {
    state: Mutex<LocalQuotaState>,
}

#[derive(Default)]
struct LocalQuotaState {
    windows: HashMap<String, QuotaWindow>,
    concurrency: HashMap<String, u32>,
    leases: HashMap<String, LocalLeaseState>,
    completed: HashMap<String, CompletedLocalLease>,
}

struct QuotaWindow {
    started_at: Instant,
    generation: u64,
    committed: u32,
    reserved: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LocalAccounting {
    Consumable,
    Occupancy,
}

struct LocalWindowReservation {
    key: String,
    generation: u64,
}

struct LocalLeaseState {
    accounting: LocalAccounting,
    reserved_amount: u32,
    consumed_total: u32,
    sequence: u64,
    expires_at: Instant,
    windows: Vec<LocalWindowReservation>,
    concurrency_key: Option<String>,
}

struct CompletedLocalLease {
    consumed_total: u32,
    sequence: u64,
    expires_at: Instant,
}

enum LocalReserveResult {
    Reserved(Arc<dyn QuotaLeaseBackend>),
    Rejected(String),
    NotConfigured,
}

#[derive(Clone, Copy)]
struct QuotaTriggerEntry<'a> {
    rule: &'a RateLimit,
    trigger: &'a LimitTrigger,
}

impl LocalQuotaCounter {
    fn new() -> Self {
        Self {
            state: Mutex::new(LocalQuotaState::default()),
        }
    }

    fn reserve(
        self: &Arc<Self>,
        req: &QuotaRequest,
        rule: &RateLimit,
        trigger: &LimitTrigger,
        reserved_amount: u32,
    ) -> Result<LocalReserveResult, PoleError> {
        let mut state = self.state.lock().unwrap();
        cleanup_expired_local_leases(&mut state);
        let lease_id = uuid::Uuid::new_v4().to_string();
        let expires_at = Instant::now().checked_add(req.lease_ttl).ok_or_else(|| {
            PoleError::new(
                crate::core::model::error::ErrorCode::ApiInvalidArgument,
                "quota lease ttl cannot be represented locally".to_string(),
            )
        })?;

        let lease = match trigger.resource() {
            limit_trigger::Resource::Qps | limit_trigger::Resource::Token => {
                let amounts = trigger
                    .amounts
                    .iter()
                    .filter(|amount| amount.max_amount > 0)
                    .collect::<Vec<_>>();
                if amounts.is_empty() {
                    return Ok(LocalReserveResult::NotConfigured);
                }
                let mut reservations = Vec::with_capacity(amounts.len());
                for amount in &amounts {
                    let duration = amount_duration(amount);
                    let key = quota_counter_key(req, rule, trigger, duration);
                    let window = state
                        .windows
                        .entry(key.clone())
                        .or_insert_with(|| QuotaWindow {
                            started_at: Instant::now(),
                            generation: 0,
                            committed: 0,
                            reserved: 0,
                        });
                    if window.started_at.elapsed() >= duration {
                        window.started_at = Instant::now();
                        window.generation = window.generation.wrapping_add(1);
                        window.committed = 0;
                        window.reserved = 0;
                    }
                    if window
                        .committed
                        .saturating_add(window.reserved)
                        .saturating_add(reserved_amount)
                        > amount.max_amount
                    {
                        return Ok(LocalReserveResult::Rejected(
                            "rate limit quota exhausted".to_string(),
                        ));
                    }
                    reservations.push(LocalWindowReservation {
                        key,
                        generation: window.generation,
                    });
                }
                for reservation in &reservations {
                    state
                        .windows
                        .get_mut(&reservation.key)
                        .expect("quota window exists after reservation check")
                        .reserved += reserved_amount;
                }
                LocalLeaseState {
                    accounting: LocalAccounting::Consumable,
                    reserved_amount,
                    consumed_total: initial_consumed_total(trigger),
                    sequence: 0,
                    expires_at,
                    windows: reservations,
                    concurrency_key: None,
                }
            }
            limit_trigger::Resource::Concurrency => {
                let Some(amount) = trigger.concurrency_amount.as_ref() else {
                    return Ok(LocalReserveResult::NotConfigured);
                };
                if amount.max_amount == 0 {
                    return Ok(LocalReserveResult::NotConfigured);
                }
                let key = concurrency_counter_key(req, rule, trigger);
                let occupied = state.concurrency.entry(key.clone()).or_insert(0);
                if occupied.saturating_add(reserved_amount) > amount.max_amount {
                    return Ok(LocalReserveResult::Rejected(
                        "rate limit concurrency exhausted".to_string(),
                    ));
                }
                *occupied += reserved_amount;
                LocalLeaseState {
                    accounting: LocalAccounting::Occupancy,
                    reserved_amount,
                    consumed_total: 0,
                    sequence: 0,
                    expires_at,
                    windows: Vec::new(),
                    concurrency_key: Some(key),
                }
            }
            _ => return Ok(LocalReserveResult::NotConfigured),
        };

        state.leases.insert(lease_id.clone(), lease);
        Ok(LocalReserveResult::Reserved(Arc::new(LocalLeaseBackend {
            counter: self.clone(),
            lease_id,
            resource: quota_resource(trigger.resource())
                .expect("supported trigger has a quota resource"),
        })))
    }

    fn update_lease(
        &self,
        lease_id: &str,
        consumed_total: u32,
        sequence: u64,
    ) -> Result<(), PoleError> {
        let mut state = self.state.lock().unwrap();
        cleanup_expired_local_leases(&mut state);
        let lease = state.leases.get_mut(lease_id).ok_or_else(|| {
            PoleError::new(
                crate::core::model::error::ErrorCode::InvalidState,
                "quota lease is expired or already settled".to_string(),
            )
        })?;
        if sequence == lease.sequence && consumed_total == lease.consumed_total {
            return Ok(());
        }
        validate_local_operation(lease, consumed_total, sequence)?;
        lease.consumed_total = consumed_total;
        lease.sequence = sequence;
        Ok(())
    }

    fn finish_lease(
        &self,
        lease_id: &str,
        consumed_total: u32,
        sequence: u64,
    ) -> Result<(), PoleError> {
        let mut state = self.state.lock().unwrap();
        cleanup_expired_local_leases(&mut state);
        if let Some(completed) = state.completed.get(lease_id) {
            if completed.sequence == sequence && completed.consumed_total == consumed_total {
                return Ok(());
            }
        }
        let lease = state.leases.get(lease_id).ok_or_else(|| {
            PoleError::new(
                crate::core::model::error::ErrorCode::InvalidState,
                "quota lease is expired or already settled".to_string(),
            )
        })?;
        validate_local_operation(lease, consumed_total, sequence)?;
        let mut lease = state
            .leases
            .remove(lease_id)
            .expect("validated local quota lease exists");
        lease.consumed_total = consumed_total;
        let completed = CompletedLocalLease {
            consumed_total,
            sequence,
            expires_at: Instant::now() + Duration::from_secs(60),
        };
        settle_local_lease(&mut state, lease);
        state.completed.insert(lease_id.to_string(), completed);
        Ok(())
    }
}

struct LocalLeaseBackend {
    counter: Arc<LocalQuotaCounter>,
    lease_id: String,
    resource: QuotaResource,
}

#[async_trait::async_trait]
impl QuotaLeaseBackend for LocalLeaseBackend {
    async fn update(
        &self,
        consumptions: Vec<QuotaConsumption>,
        sequence: u64,
    ) -> Result<(), PoleError> {
        self.counter.update_lease(
            &self.lease_id,
            consumption_for(&consumptions, self.resource)?,
            sequence,
        )
    }

    async fn finish(
        &self,
        consumptions: Vec<QuotaConsumption>,
        sequence: u64,
    ) -> Result<(), PoleError> {
        self.counter.finish_lease(
            &self.lease_id,
            consumption_for(&consumptions, self.resource)?,
            sequence,
        )
    }

    fn abandon(&self, consumptions: Vec<QuotaConsumption>, sequence: u64) {
        if let Ok(consumed_total) = consumption_for(&consumptions, self.resource) {
            let _ = self
                .counter
                .finish_lease(&self.lease_id, consumed_total, sequence);
        }
    }
}

struct CompositeLeaseBackend {
    backends: Vec<Arc<dyn QuotaLeaseBackend>>,
}

#[async_trait::async_trait]
impl QuotaLeaseBackend for CompositeLeaseBackend {
    async fn update(
        &self,
        consumptions: Vec<QuotaConsumption>,
        sequence: u64,
    ) -> Result<(), PoleError> {
        for backend in &self.backends {
            backend.update(consumptions.clone(), sequence).await?;
        }
        Ok(())
    }

    async fn finish(
        &self,
        consumptions: Vec<QuotaConsumption>,
        sequence: u64,
    ) -> Result<(), PoleError> {
        for backend in &self.backends {
            backend.finish(consumptions.clone(), sequence).await?;
        }
        Ok(())
    }

    fn abandon(&self, consumptions: Vec<QuotaConsumption>, sequence: u64) {
        for backend in &self.backends {
            backend.abandon(consumptions.clone(), sequence);
        }
    }
}

fn validate_local_operation(
    lease: &LocalLeaseState,
    consumed_total: u32,
    sequence: u64,
) -> Result<(), PoleError> {
    use crate::core::model::error::ErrorCode;

    if sequence != lease.sequence.saturating_add(1) {
        return Err(PoleError::new(
            ErrorCode::InvalidState,
            "quota lease operation sequence is out of order".to_string(),
        ));
    }
    if consumed_total < lease.consumed_total || consumed_total > lease.reserved_amount {
        return Err(PoleError::new(
            ErrorCode::ApiInvalidArgument,
            "quota lease cumulative consumption is outside its reservation".to_string(),
        ));
    }
    Ok(())
}

fn cleanup_expired_local_leases(state: &mut LocalQuotaState) {
    let now = Instant::now();
    let expired = state
        .leases
        .iter()
        .filter(|(_, lease)| lease.expires_at <= now)
        .map(|(lease_id, _)| lease_id.clone())
        .collect::<Vec<_>>();
    for lease_id in expired {
        if let Some(lease) = state.leases.remove(&lease_id) {
            settle_local_lease(state, lease);
        }
    }
    state
        .completed
        .retain(|_, completed| completed.expires_at > now);
}

fn settle_local_lease(state: &mut LocalQuotaState, lease: LocalLeaseState) {
    match lease.accounting {
        LocalAccounting::Consumable => {
            for reservation in lease.windows {
                let Some(window) = state.windows.get_mut(&reservation.key) else {
                    continue;
                };
                if window.generation != reservation.generation {
                    continue;
                }
                window.reserved = window.reserved.saturating_sub(lease.reserved_amount);
                window.committed = window.committed.saturating_add(lease.consumed_total);
            }
        }
        LocalAccounting::Occupancy => {
            let Some(key) = lease.concurrency_key else {
                return;
            };
            let Some(occupied) = state.concurrency.get_mut(&key) else {
                return;
            };
            *occupied = occupied.saturating_sub(lease.reserved_amount);
            if *occupied == 0 {
                state.concurrency.remove(&key);
            }
        }
    }
}

pub(super) fn initial_consumed_total(trigger: &LimitTrigger) -> u32 {
    match trigger.resource() {
        limit_trigger::Resource::Qps => 1,
        _ => 0,
    }
}

fn quota_resource(resource: limit_trigger::Resource) -> Option<QuotaResource> {
    match resource {
        limit_trigger::Resource::Qps => Some(QuotaResource::Qps),
        limit_trigger::Resource::Token => Some(QuotaResource::Token),
        limit_trigger::Resource::Concurrency => Some(QuotaResource::Concurrency),
        _ => None,
    }
}

fn initial_consumptions(quotas: &[QuotaAmount]) -> Vec<QuotaConsumption> {
    quotas
        .iter()
        .map(|quota| QuotaConsumption {
            resource: quota.resource,
            consumed_total: if quota.resource == QuotaResource::Qps {
                1
            } else {
                0
            },
        })
        .collect()
}

fn consumption_for(
    consumptions: &[QuotaConsumption],
    resource: QuotaResource,
) -> Result<u32, PoleError> {
    consumptions
        .iter()
        .find(|consumption| consumption.resource == resource)
        .map(|consumption| consumption.consumed_total)
        .ok_or_else(|| {
            PoleError::new(
                crate::core::model::error::ErrorCode::ApiInvalidArgument,
                format!("quota consumption for {resource:?} is missing"),
            )
        })
}

fn rollback_reservations(
    backends: &[Arc<dyn QuotaLeaseBackend>],
    consumptions: &[QuotaConsumption],
) {
    for backend in backends {
        backend.abandon(consumptions.to_vec(), 1);
    }
}

fn quota_rejected(message: String) -> PoleError {
    PoleError::new(crate::core::model::error::ErrorCode::RequestLimit, message)
}

fn rate_limits_from_service_rules(
    rules: Vec<Box<dyn std::any::Any + Send>>,
) -> Result<Vec<RateLimit>, PoleError> {
    let mut rate_limits = Vec::with_capacity(rules.len());
    for rule in rules {
        let type_id = rule.type_id();
        match rule.downcast::<RateLimit>() {
            Ok(rule) => rate_limits.push(*rule),
            Err(_) => {
                return Err(PoleError::new(
                    crate::core::model::error::ErrorCode::InvalidRule,
                    format!("rule type error, expect RateLimit, but got {:?}", type_id),
                ));
            }
        }
    }
    Ok(rate_limits)
}

fn quota_rule_matches(rule: &RateLimit) -> bool {
    !rule.disable
        && matches!(
            rule.r#type(),
            rate_limit::Type::Local | rate_limit::Type::Global
        )
}

fn uses_local_counter(rule: &RateLimit, trigger: &LimitTrigger) -> bool {
    rule.r#type() == rate_limit::Type::Local
        || (rule.r#type() == rate_limit::Type::Global
            && trigger.failover() == limit_trigger::FailoverType::FailoverLocal)
}

fn uses_distributed_quota(rule: &RateLimit, trigger: &LimitTrigger) -> bool {
    rule.r#type() == rate_limit::Type::Global
        && matches!(
            trigger.resource(),
            limit_trigger::Resource::Qps
                | limit_trigger::Resource::Token
                | limit_trigger::Resource::Concurrency
        )
}

fn matching_quota_triggers<'a>(
    req: &QuotaRequest,
    rules: &'a [RateLimit],
) -> Vec<QuotaTriggerEntry<'a>> {
    let mut rules = rules.iter().collect::<Vec<_>>();
    rules.sort_by(|a, b| a.priority.cmp(&b.priority));

    // 限流匹配拆成三段：先过滤禁用/资源类型这类廉价条件，再用 API
    // 索引收敛 method/path 候选，最后执行 header/query/custom 等参数匹配。
    // 这样既补齐 Api.path 语义，也避免大量无关 trigger 反复跑参数匹配。
    let entries = rules.into_iter().flat_map(|rule| {
        if !quota_rule_matches(rule) {
            return Vec::new();
        }
        rule.rules
            .iter()
            .filter(|trigger| trigger_base_matches(trigger))
            .map(|trigger| (QuotaTriggerEntry { rule, trigger }, trigger.apis.as_slice()))
            .collect::<Vec<_>>()
    });
    let input = quota_api_match_input(req);
    let index = ApiMatchIndex::new(entries);

    index
        .candidates(&input)
        .into_iter()
        .filter(|entry| trigger_arguments_match(req, entry.trigger))
        .collect()
}

fn trigger_base_matches(trigger: &LimitTrigger) -> bool {
    !trigger.disable
        && matches!(
            trigger.resource(),
            limit_trigger::Resource::Qps
                | limit_trigger::Resource::Token
                | limit_trigger::Resource::Concurrency
        )
}

fn trigger_arguments_match(req: &QuotaRequest, trigger: &LimitTrigger) -> bool {
    trigger
        .arguments
        .iter()
        .all(|argument| match_argument_matches(req, argument))
}

fn quota_api_match_input(req: &QuotaRequest) -> ApiMatchInput {
    // QuotaRequest.method 是限流 API 的稳定入口；path 仍从流量标签提供器取，
    // 兼容没有 path 维度的调用方。
    ApiMatchInput::new(
        Some(req.method.clone()),
        (req.traffic_label_provider)(ArgumentType::Path, ""),
    )
}

fn match_argument_matches(req: &QuotaRequest, argument: &MatchArgument) -> bool {
    let Some(rule_value) = &argument.value else {
        return false;
    };
    let Ok(value_type) = match_string::ValueType::try_from(rule_value.value_type) else {
        return false;
    };
    let arg_type = ratelimit_argument_type(argument.r#type());
    if value_type == match_string::ValueType::Parameter && rule_value.value.is_empty() {
        return (req.traffic_label_provider)(arg_type, argument.key.as_str()).is_some();
    }
    let actual = match value_type {
        match_string::ValueType::Text => {
            (req.traffic_label_provider)(arg_type, argument.key.as_str())
        }
        match_string::ValueType::Parameter => {
            (req.traffic_label_provider)(arg_type, rule_value.value.as_str())
        }
    };
    let Some(actual) = actual else {
        return false;
    };

    match_label_value(rule_value, actual)
}

fn ratelimit_argument_type(arg_type: match_argument::Type) -> ArgumentType {
    match arg_type {
        match_argument::Type::Method => ArgumentType::Method,
        match_argument::Type::Header => ArgumentType::Header,
        match_argument::Type::Query => ArgumentType::Query,
        match_argument::Type::CallerService => ArgumentType::CallerService,
        match_argument::Type::CallerIp => ArgumentType::CallerIP,
        match_argument::Type::CallerMetadata => ArgumentType::CallerService,
        match_argument::Type::Custom => ArgumentType::Custom,
    }
}

/// 按 Polaris SDK/limiter 约定生成远端 counter 的 canonical labels：
/// `method|TYPE:key:value`，参数项按字典序排列。limiter 将 labels 作为跨 SDK
/// 共享配额桶的 opaque key，因此这里不能混入 Rust 私有的 rule revision 或摘要格式。
pub(super) fn remote_quota_labels(req: &QuotaRequest, trigger: &LimitTrigger) -> String {
    let mut arguments = trigger
        .arguments
        .iter()
        .filter_map(|argument| {
            let value = argument.value.as_ref()?;
            let argument_type = argument.r#type();
            let provider_key = if value.value_type() == match_string::ValueType::Parameter
                && !value.value.is_empty()
            {
                value.value.as_str()
            } else {
                argument.key.as_str()
            };
            let actual = if argument_type == match_argument::Type::Method {
                req.method.clone()
            } else {
                (req.traffic_label_provider)(ratelimit_argument_type(argument_type), provider_key)
                    .unwrap_or_default()
            };
            let type_name = match argument_type {
                match_argument::Type::Custom => "CUSTOM",
                match_argument::Type::Method => "METHOD",
                match_argument::Type::Header => "HEADER",
                match_argument::Type::Query => "QUERY",
                match_argument::Type::CallerService => "CALLER_SERVICE",
                match_argument::Type::CallerIp => "CALLER_IP",
                match_argument::Type::CallerMetadata => "CALLER_METADATA",
            };
            Some(match argument_type {
                match_argument::Type::Method | match_argument::Type::CallerIp => {
                    format!("{type_name}:{actual}")
                }
                _ => format!("{type_name}:{}:{actual}", argument.key),
            })
        })
        .collect::<Vec<_>>();
    arguments.sort_unstable();
    let resource = match trigger.resource() {
        limit_trigger::Resource::Qps => "QPS",
        limit_trigger::Resource::Token => "TOKEN",
        limit_trigger::Resource::Concurrency => "CONCURRENCY",
        _ => "UNKNOWN",
    };
    format!(
        "{}|RESOURCE:{}|{}",
        req.method,
        resource,
        arguments.join("|")
    )
}

fn amount_duration(amount: &pole_specification::v1::Amount) -> Duration {
    amount
        .valid_duration
        .map(|duration| Duration::from_secs(duration.seconds.max(1) as u64))
        .unwrap_or_else(|| Duration::from_secs(1))
}

fn quota_counter_key(
    req: &QuotaRequest,
    rule: &RateLimit,
    trigger: &LimitTrigger,
    duration: Duration,
) -> String {
    format!(
        "{}#{}#{}#{}#{}#{}{}",
        req.namespace,
        req.service,
        req.method,
        rule.id,
        trigger.name,
        duration.as_secs(),
        request_parameter_dimension_suffix(req, trigger),
    )
}

fn concurrency_counter_key(req: &QuotaRequest, rule: &RateLimit, trigger: &LimitTrigger) -> String {
    format!(
        "{}#{}#{}#{}#{}#concurrency{}",
        req.namespace,
        req.service,
        req.method,
        rule.id,
        trigger.name,
        request_parameter_dimension_suffix(req, trigger),
    )
}

// PARAMETER + 空 value 采集当前请求参数并加入限流器 key。值只以 SHA-256
// 摘要进入内存 key，既避免泄露 Header/Query 明文，又让不同请求值拥有独立桶。
fn request_parameter_dimension_suffix(req: &QuotaRequest, trigger: &LimitTrigger) -> String {
    let mut dimensions = trigger
        .arguments
        .iter()
        .filter_map(|argument| {
            let value = argument.value.as_ref()?;
            if value.value_type() != match_string::ValueType::Parameter || !value.value.is_empty() {
                return None;
            }
            let actual = (req.traffic_label_provider)(
                ratelimit_argument_type(argument.r#type()),
                argument.key.as_str(),
            )?;
            let digest = Sha256::digest(actual.as_bytes());
            Some((
                format!("{:?}.{}", argument.r#type(), argument.key),
                format!("{digest:x}"),
            ))
        })
        .collect::<Vec<_>>();
    dimensions.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    dimensions
        .into_iter()
        .map(|(key, value)| format!("#{key}={value}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::ArgumentType;
    use pole_specification::v1::{
        limit_trigger, match_string, rate_limit, Amount, LimitTrigger, MatchString, RateLimit,
    };
    use prost_types::Duration as ProstDuration;

    fn no_traffic_label(_: ArgumentType, _: &str) -> Option<String> {
        None
    }

    fn quota_request(resource: QuotaResource, amount: u32) -> QuotaRequest {
        QuotaRequest {
            flow_id: "flow-1".to_string(),
            timeout: Duration::from_secs(1),
            service: "svc-a".to_string(),
            namespace: "default".to_string(),
            method: "GET /orders".to_string(),
            traffic_label_provider: no_traffic_label,
            quotas: vec![QuotaAmount { resource, amount }],
            lease_ttl: Duration::from_secs(30),
        }
    }

    fn local_consumable_rule(resource: limit_trigger::Resource, max_amount: u32) -> RateLimit {
        RateLimit {
            id: format!("{resource:?}-rule"),
            priority: 0,
            r#type: rate_limit::Type::Local.into(),
            rules: vec![LimitTrigger {
                name: format!("{resource:?}-trigger"),
                apis: vec![pole_specification::v1::Api {
                    method: "GET /orders".to_string(),
                    ..Default::default()
                }],
                resource: resource.into(),
                amounts: vec![Amount {
                    max_amount,
                    valid_duration: Some(ProstDuration {
                        seconds: 60,
                        nanos: 0,
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn local_concurrency_rule(max_amount: u32) -> RateLimit {
        let mut rule = local_consumable_rule(limit_trigger::Resource::Qps, 1);
        rule.id = "concurrency-rule".to_string();
        rule.rules[0].name = "concurrency-trigger".to_string();
        rule.rules[0].resource = limit_trigger::Resource::Concurrency.into();
        rule.rules[0].amounts.clear();
        rule.rules[0].concurrency_amount =
            Some(pole_specification::v1::ConcurrencyAmount { max_amount });
        rule
    }

    fn reserve_local(
        counter: &Arc<LocalQuotaCounter>,
        req: &QuotaRequest,
        rule: &RateLimit,
    ) -> Result<QuotaLease, PoleError> {
        let trigger = &rule.rules[0];
        let amount = req
            .amount_for(quota_resource(trigger.resource()).unwrap())
            .unwrap();
        match counter.reserve(req, rule, trigger, amount)? {
            LocalReserveResult::Reserved(backend) => Ok(QuotaLease::new(
                req.quotas.clone(),
                initial_consumptions(&req.quotas),
                backend,
            )),
            LocalReserveResult::Rejected(message) => Err(quota_rejected(message)),
            LocalReserveResult::NotConfigured => panic!("test rule must configure quota"),
        }
    }

    #[tokio::test]
    async fn rpm_reservation_commits_one_request_at_finish() {
        let counter = Arc::new(LocalQuotaCounter::new());
        let rule = local_consumable_rule(limit_trigger::Resource::Qps, 2);
        let req = quota_request(QuotaResource::Qps, 1);

        reserve_local(&counter, &req, &rule)
            .unwrap()
            .finish(1)
            .await
            .unwrap();
        reserve_local(&counter, &req, &rule)
            .unwrap()
            .finish(1)
            .await
            .unwrap();

        let rejected = reserve_local(&counter, &req, &rule).err().unwrap();
        assert!(rejected.to_string().contains("RequestLimit"));
    }

    #[tokio::test]
    async fn tpm_reserves_updates_and_returns_unused_quota() {
        let counter = Arc::new(LocalQuotaCounter::new());
        let rule = local_consumable_rule(limit_trigger::Resource::Token, 100);
        let lease =
            reserve_local(&counter, &quota_request(QuotaResource::Token, 80), &rule).unwrap();

        lease.update(40).await.unwrap();
        lease.finish(60).await.unwrap();

        let returned =
            reserve_local(&counter, &quota_request(QuotaResource::Token, 40), &rule).unwrap();
        returned.finish(0).await.unwrap();
        let rejected = reserve_local(&counter, &quota_request(QuotaResource::Token, 41), &rule)
            .err()
            .unwrap();
        assert!(rejected.to_string().contains("RequestLimit"));
    }

    #[tokio::test]
    async fn concurrency_occupancy_is_released_at_finish() {
        let counter = Arc::new(LocalQuotaCounter::new());
        let rule = local_concurrency_rule(1);
        let req = quota_request(QuotaResource::Concurrency, 1);
        let lease = reserve_local(&counter, &req, &rule).unwrap();

        assert!(reserve_local(&counter, &req, &rule).is_err());
        lease.finish(0).await.unwrap();
        reserve_local(&counter, &req, &rule).unwrap();
    }

    #[tokio::test]
    async fn expired_local_concurrency_is_reclaimed_before_next_reserve() {
        let counter = Arc::new(LocalQuotaCounter::new());
        let rule = local_concurrency_rule(1);
        let mut req = quota_request(QuotaResource::Concurrency, 1);
        req.lease_ttl = Duration::from_millis(1);
        let lease = reserve_local(&counter, &req, &rule).unwrap();

        tokio::time::sleep(Duration::from_millis(5)).await;
        reserve_local(&counter, &req, &rule).unwrap();
        drop(lease);
    }

    #[tokio::test]
    async fn cumulative_updates_are_idempotent_and_bounded() {
        let counter = Arc::new(LocalQuotaCounter::new());
        let rule = local_consumable_rule(limit_trigger::Resource::Token, 100);
        let lease =
            reserve_local(&counter, &quota_request(QuotaResource::Token, 100), &rule).unwrap();

        lease.update(40).await.unwrap();
        lease.update(40).await.unwrap();
        assert!(lease.update(39).await.is_err());
        assert!(lease.update(101).await.is_err());
        lease.finish(60).await.unwrap();
        lease.finish(60).await.unwrap();
    }

    #[tokio::test]
    async fn drop_settles_last_confirmed_consumption_without_double_charge() {
        let counter = Arc::new(LocalQuotaCounter::new());
        let rule = local_consumable_rule(limit_trigger::Resource::Token, 100);
        let lease =
            reserve_local(&counter, &quota_request(QuotaResource::Token, 80), &rule).unwrap();

        lease.update(30).await.unwrap();
        drop(lease);

        reserve_local(&counter, &quota_request(QuotaResource::Token, 70), &rule).unwrap();
    }

    #[tokio::test]
    async fn all_consumable_windows_are_reserved_atomically() {
        let counter = Arc::new(LocalQuotaCounter::new());
        let mut rule = local_consumable_rule(limit_trigger::Resource::Token, 100);
        rule.rules[0].amounts.push(Amount {
            max_amount: 50,
            valid_duration: Some(ProstDuration {
                seconds: 1,
                nanos: 0,
            }),
            ..Default::default()
        });

        assert!(reserve_local(&counter, &quota_request(QuotaResource::Token, 60), &rule).is_err());
        reserve_local(&counter, &quota_request(QuotaResource::Token, 50), &rule).unwrap();
    }

    #[tokio::test]
    async fn local_mixed_metrics_share_one_public_lease() {
        let counter = Arc::new(LocalQuotaCounter::new());
        let qps = local_consumable_rule(limit_trigger::Resource::Qps, 1);
        let token = local_consumable_rule(limit_trigger::Resource::Token, 100);
        let concurrency = local_concurrency_rule(1);
        let quotas = vec![
            QuotaAmount {
                resource: QuotaResource::Qps,
                amount: 1,
            },
            QuotaAmount {
                resource: QuotaResource::Token,
                amount: 80,
            },
            QuotaAmount {
                resource: QuotaResource::Concurrency,
                amount: 1,
            },
        ];
        let req = QuotaRequest {
            quotas: quotas.clone(),
            ..quota_request(QuotaResource::Token, 80)
        };
        let mut backends = Vec::new();
        for rule in [&qps, &token, &concurrency] {
            let trigger = &rule.rules[0];
            let resource = quota_resource(trigger.resource()).unwrap();
            let amount = req.amount_for(resource).unwrap();
            let LocalReserveResult::Reserved(backend) =
                counter.reserve(&req, rule, trigger, amount).unwrap()
            else {
                panic!("mixed local quota must reserve");
            };
            backends.push(backend);
        }
        let lease = QuotaLease::new(
            quotas,
            initial_consumptions(&req.quotas),
            Arc::new(CompositeLeaseBackend { backends }),
        );

        assert!(lease.update(40).await.is_err());
        lease
            .update_consumptions(&[QuotaConsumption {
                resource: QuotaResource::Token,
                consumed_total: 40,
            }])
            .await
            .unwrap();
        lease
            .finish_consumptions(&[QuotaConsumption {
                resource: QuotaResource::Token,
                consumed_total: 60,
            }])
            .await
            .unwrap();

        reserve_local(
            &counter,
            &quota_request(QuotaResource::Concurrency, 1),
            &concurrency,
        )
        .unwrap();
        reserve_local(&counter, &quota_request(QuotaResource::Token, 40), &token).unwrap();
        assert!(reserve_local(&counter, &quota_request(QuotaResource::Qps, 1), &qps,).is_err());
    }

    #[test]
    fn token_rules_participate_in_request_matching() {
        let rule = local_consumable_rule(limit_trigger::Resource::Token, 100);
        let req = quota_request(QuotaResource::Token, 10);

        let rules = [rule];
        let matches = matching_quota_triggers(&req, &rules);

        assert_eq!(matches.len(), 1);
        assert_eq!(
            matches[0].trigger.resource(),
            limit_trigger::Resource::Token
        );
    }

    #[test]
    fn api_path_mismatch_does_not_select_a_rule() {
        fn order_path(_: ArgumentType, _: &str) -> Option<String> {
            Some("/orders/99".to_string())
        }

        let mut rule = local_consumable_rule(limit_trigger::Resource::Qps, 1);
        rule.rules[0].apis[0].method = "GET".to_string();
        rule.rules[0].apis[0].path = Some(MatchString {
            r#type: match_string::MatchStringType::Exact.into(),
            value: "/orders/42".to_string(),
            value_type: match_string::ValueType::Text.into(),
        });
        let req = QuotaRequest {
            method: "GET".to_string(),
            traffic_label_provider: order_path,
            ..quota_request(QuotaResource::Qps, 1)
        };

        assert!(matching_quota_triggers(&req, &[rule]).is_empty());
    }
}
