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
    api::RateLimitAPI,
    distributed::DistributedQuotaManager,
    req::{QuotaRequest, QuotaResponse},
};

pub struct DefaultRateLimitAPI {
    manage_sdk: bool,
    context: Arc<SDKContext>,
    local_counter: LocalQuotaCounter,
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
            local_counter: LocalQuotaCounter::new(),
            distributed_quota: DistributedQuotaManager::default(),
            #[cfg(test)]
            test_inputs: None,
        }
    }

    pub fn new(context: Arc<SDKContext>) -> Self {
        Self {
            manage_sdk: false,
            context,
            local_counter: LocalQuotaCounter::new(),
            distributed_quota: DistributedQuotaManager::default(),
            #[cfg(test)]
            test_inputs: None,
        }
    }

    #[cfg(test)]
    pub(super) fn new_with_test_inputs(
        context: Arc<SDKContext>,
        rules: Vec<RateLimit>,
        instances: Vec<crate::core::model::naming::Instance>,
    ) -> Self {
        Self {
            manage_sdk: false,
            context,
            local_counter: LocalQuotaCounter::new(),
            distributed_quota: DistributedQuotaManager::default(),
            test_inputs: Some((rules, instances)),
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

    async fn check_quota(&self, req: &QuotaRequest, rules: &[RateLimit]) -> QuotaResponse {
        for entry in matching_quota_triggers(req, rules) {
            if uses_distributed_quota(entry.rule, entry.trigger) {
                match self
                    .check_distributed_quota(req, entry.rule, entry.trigger)
                    .await
                {
                    Ok(response) => return response,
                    Err(_) => {
                        return global_quota_failover(
                            &self.local_counter,
                            req,
                            entry.rule,
                            entry.trigger,
                        );
                    }
                }
            }

            if uses_local_counter(entry.rule, entry.trigger) {
                return check_local_trigger(&self.local_counter, req, entry.rule, entry.trigger);
            }
        }

        QuotaResponse {
            allowed: true,
            message: String::new(),
        }
    }

    async fn check_distributed_quota(
        &self,
        req: &QuotaRequest,
        rule: &RateLimit,
        trigger: &LimitTrigger,
    ) -> Result<QuotaResponse, PoleError> {
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
            .check(req, rule, trigger, &instances, &client_context.client_id)
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
    async fn get_quota(&self, req: QuotaRequest) -> Result<QuotaResponse, PoleError> {
        let check_ret = req.check_valid();
        check_ret?;
        let rules = self.load_rate_limit_rules(&req).await?;

        Ok(self.check_quota(&req, &rules).await)
    }

    async fn return_quota(&self, req: QuotaRequest) -> Result<(), PoleError> {
        let check_ret = req.check_valid();
        check_ret?;
        let rules = self.load_rate_limit_rules(&req).await?;
        return_quota(&self.local_counter, &req, &rules);
        Ok(())
    }
}

struct LocalQuotaCounter {
    windows: Mutex<HashMap<String, QuotaWindow>>,
    concurrency: Mutex<HashMap<String, u32>>,
}

struct QuotaWindow {
    started_at: Instant,
    used: u32,
}

#[derive(Clone, Copy)]
struct QuotaTriggerEntry<'a> {
    rule: &'a RateLimit,
    trigger: &'a LimitTrigger,
}

impl LocalQuotaCounter {
    fn new() -> Self {
        Self {
            windows: Mutex::new(HashMap::new()),
            concurrency: Mutex::new(HashMap::new()),
        }
    }

    fn check_quota(&self, req: &QuotaRequest, rules: &[RateLimit]) -> QuotaResponse {
        for entry in matching_quota_triggers(req, rules) {
            let rule = entry.rule;
            let trigger = entry.trigger;
            if uses_distributed_quota(rule, trigger)
                && trigger.failover() == limit_trigger::FailoverType::FailoverPass
            {
                return QuotaResponse {
                    allowed: true,
                    message: String::new(),
                };
            }
            if !uses_local_counter(rule, trigger) {
                continue;
            }
            match trigger.resource() {
                limit_trigger::Resource::Qps => {
                    let amounts = trigger
                        .amounts
                        .iter()
                        .filter(|amount| amount.max_amount > 0)
                        .collect::<Vec<_>>();
                    if amounts.is_empty() {
                        continue;
                    }

                    // 同一个 trigger 的多个窗口是叠加约束。先完成全部窗口的可用性
                    // 判断，再统一扣减，避免后续窗口拒绝时前面的窗口已被部分消费。
                    let mut windows = self.windows.lock().unwrap();
                    for amount in &amounts {
                        let duration = amount_duration(amount);
                        let key = quota_counter_key(req, rule, trigger, duration);
                        let window = windows.entry(key).or_insert_with(|| QuotaWindow {
                            started_at: Instant::now(),
                            used: 0,
                        });
                        if window.started_at.elapsed() >= duration {
                            window.started_at = Instant::now();
                            window.used = 0;
                        }
                        if window.used >= amount.max_amount {
                            return QuotaResponse {
                                allowed: false,
                                message: "rate limit quota exhausted".to_string(),
                            };
                        }
                    }

                    for amount in amounts {
                        let duration = amount_duration(amount);
                        let key = quota_counter_key(req, rule, trigger, duration);
                        let window = windows
                            .get_mut(&key)
                            .expect("quota window must exist after availability check");
                        window.used += 1;
                    }
                    return QuotaResponse {
                        allowed: true,
                        message: String::new(),
                    };
                }
                limit_trigger::Resource::Concurrency => {
                    let Some(amount) = trigger.concurrency_amount.as_ref() else {
                        continue;
                    };
                    if amount.max_amount == 0 {
                        continue;
                    }
                    let key = concurrency_counter_key(req, rule, trigger);
                    let mut concurrency = self.concurrency.lock().unwrap();
                    let used = concurrency.entry(key).or_insert(0);
                    if *used >= amount.max_amount {
                        return QuotaResponse {
                            allowed: false,
                            message: "rate limit concurrency exhausted".to_string(),
                        };
                    }
                    *used += 1;
                    return QuotaResponse {
                        allowed: true,
                        message: String::new(),
                    };
                }
                _ => {}
            }
        }

        QuotaResponse {
            allowed: true,
            message: String::new(),
        }
    }

    fn return_quota(&self, req: &QuotaRequest, rules: &[RateLimit]) -> bool {
        for entry in matching_quota_triggers(req, rules) {
            let rule = entry.rule;
            let trigger = entry.trigger;
            if trigger.resource() != limit_trigger::Resource::Concurrency
                || !uses_local_counter(rule, trigger)
            {
                continue;
            }
            let Some(amount) = trigger.concurrency_amount.as_ref() else {
                continue;
            };
            if amount.max_amount == 0 {
                continue;
            }
            let key = concurrency_counter_key(req, rule, trigger);
            let mut concurrency = self.concurrency.lock().unwrap();
            let Some(used) = concurrency.get_mut(&key) else {
                return false;
            };
            if *used == 0 {
                return false;
            }
            *used -= 1;
            if *used == 0 {
                concurrency.remove(&key);
            }
            return true;
        }

        false
    }
}

#[cfg(test)]
fn check_quota(
    local_counter: &LocalQuotaCounter,
    req: &QuotaRequest,
    rules: &[RateLimit],
) -> QuotaResponse {
    for entry in matching_quota_triggers(req, rules) {
        if uses_distributed_quota(entry.rule, entry.trigger) {
            return global_quota_failover(local_counter, req, entry.rule, entry.trigger);
        }

        if uses_local_counter(entry.rule, entry.trigger) {
            return check_local_trigger(local_counter, req, entry.rule, entry.trigger);
        }
    }

    QuotaResponse {
        allowed: true,
        message: String::new(),
    }
}

fn return_quota(local_counter: &LocalQuotaCounter, req: &QuotaRequest, rules: &[RateLimit]) {
    for entry in matching_quota_triggers(req, rules) {
        if entry.trigger.resource() != limit_trigger::Resource::Concurrency {
            continue;
        }

        if entry.rule.r#type() == rate_limit::Type::Global {
            let local_rule = rule_with_single_trigger(entry.rule, entry.trigger);
            local_counter.return_quota(req, &[local_rule]);
            return;
        }

        if uses_local_counter(entry.rule, entry.trigger) {
            let local_rule = rule_with_single_trigger(entry.rule, entry.trigger);
            local_counter.return_quota(req, &[local_rule]);
            return;
        }
    }
}

fn global_quota_failover(
    local_counter: &LocalQuotaCounter,
    req: &QuotaRequest,
    rule: &RateLimit,
    trigger: &LimitTrigger,
) -> QuotaResponse {
    match trigger.failover() {
        limit_trigger::FailoverType::FailoverPass => QuotaResponse {
            allowed: true,
            message: String::new(),
        },
        limit_trigger::FailoverType::FailoverLocal => {
            check_local_trigger(local_counter, req, rule, trigger)
        }
    }
}

fn check_local_trigger(
    local_counter: &LocalQuotaCounter,
    req: &QuotaRequest,
    rule: &RateLimit,
    trigger: &LimitTrigger,
) -> QuotaResponse {
    let local_rule = rule_with_single_trigger(rule, trigger);
    local_counter.check_quota(req, &[local_rule])
}

fn rule_with_single_trigger(rule: &RateLimit, trigger: &LimitTrigger) -> RateLimit {
    let mut local_rule = rule.clone();
    local_rule.rules = vec![trigger.clone()];
    local_rule
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
    // 并发数需要在请求完成时由 return_quota 归还，远端限流协议只有按时间窗口
    // 汇总的 quota report，因此 GLOBAL 并发规则也必须保持本地计数语义。
    trigger.resource() == limit_trigger::Resource::Concurrency
        || rule.r#type() == rate_limit::Type::Local
        || (rule.r#type() == rate_limit::Type::Global
            && trigger.failover() == limit_trigger::FailoverType::FailoverLocal)
}

fn uses_distributed_quota(rule: &RateLimit, trigger: &LimitTrigger) -> bool {
    rule.r#type() == rate_limit::Type::Global && trigger.resource() == limit_trigger::Resource::Qps
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
            limit_trigger::Resource::Qps | limit_trigger::Resource::Concurrency
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
    format!("{}|{}", req.method, arguments.join("|"))
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
        limit_trigger, match_argument, match_string, rate_limit, Amount, LimitTrigger,
        MatchArgument, MatchString, RateLimit,
    };
    use prost_types::Duration as ProstDuration;
    use std::time::Duration;

    fn no_traffic_label(_: ArgumentType, _: &str) -> Option<String> {
        None
    }

    fn order_99_traffic_label(arg_type: ArgumentType, _: &str) -> Option<String> {
        match arg_type {
            ArgumentType::Path => Some("/orders/99".to_string()),
            _ => None,
        }
    }

    fn quota_request() -> QuotaRequest {
        QuotaRequest {
            flow_id: "flow-1".to_string(),
            timeout: Duration::from_secs(1),
            service: "svc-a".to_string(),
            namespace: "default".to_string(),
            method: "GET /orders".to_string(),
            traffic_label_provider: no_traffic_label,
        }
    }

    fn get_order_99_request() -> QuotaRequest {
        QuotaRequest {
            method: "GET".to_string(),
            traffic_label_provider: order_99_traffic_label,
            ..quota_request()
        }
    }

    fn local_qps_rule(max_amount: u32) -> RateLimit {
        RateLimit {
            id: "rule-1".to_string(),
            priority: 0,
            r#type: rate_limit::Type::Local.into(),
            rules: vec![LimitTrigger {
                name: "trigger-1".to_string(),
                apis: vec![pole_specification::v1::Api {
                    method: "GET /orders".to_string(),
                    ..pole_specification::v1::Api::default()
                }],
                resource: limit_trigger::Resource::Qps.into(),
                amounts: vec![Amount {
                    max_amount,
                    valid_duration: Some(ProstDuration {
                        seconds: 60,
                        nanos: 0,
                    }),
                    ..Amount::default()
                }],
                ..LimitTrigger::default()
            }],
            ..RateLimit::default()
        }
    }

    fn local_qps_rule_for_path(path: &str, max_amount: u32) -> RateLimit {
        RateLimit {
            id: "rule-path".to_string(),
            priority: 0,
            r#type: rate_limit::Type::Local.into(),
            rules: vec![LimitTrigger {
                name: "trigger-path".to_string(),
                apis: vec![pole_specification::v1::Api {
                    method: "GET".to_string(),
                    path: Some(MatchString {
                        r#type: match_string::MatchStringType::Exact.into(),
                        value: path.to_string(),
                        value_type: match_string::ValueType::Text.into(),
                    }),
                    ..pole_specification::v1::Api::default()
                }],
                resource: limit_trigger::Resource::Qps.into(),
                amounts: vec![Amount {
                    max_amount,
                    valid_duration: Some(ProstDuration {
                        seconds: 60,
                        nanos: 0,
                    }),
                    ..Amount::default()
                }],
                ..LimitTrigger::default()
            }],
            ..RateLimit::default()
        }
    }

    fn header_traffic_label(arg_type: ArgumentType, key: &str) -> Option<String> {
        match (arg_type, key) {
            (ArgumentType::Header, "x-user") => Some("alice".to_string()),
            _ => None,
        }
    }

    fn local_qps_rule_with_header(max_amount: u32, header_value: &str) -> RateLimit {
        let mut rule = local_qps_rule(max_amount);
        rule.rules[0].arguments = vec![MatchArgument {
            r#type: match_argument::Type::Header.into(),
            key: "x-user".to_string(),
            value: Some(MatchString {
                r#type: match_string::MatchStringType::Exact.into(),
                value: header_value.to_string(),
                value_type: match_string::ValueType::Text.into(),
            }),
        }];
        rule
    }

    fn local_qps_rule_with_dynamic_header(max_amount: u32) -> RateLimit {
        let mut rule = local_qps_rule(max_amount);
        rule.rules[0].arguments = vec![MatchArgument {
            r#type: match_argument::Type::Header.into(),
            key: "x-user".to_string(),
            value: Some(MatchString {
                r#type: match_string::MatchStringType::Exact.into(),
                value: String::new(),
                value_type: match_string::ValueType::Parameter.into(),
            }),
        }];
        rule
    }

    #[test]
    fn unknown_argument_value_type_fails_closed() {
        let req = QuotaRequest {
            traffic_label_provider: header_traffic_label,
            ..quota_request()
        };
        let argument = MatchArgument {
            r#type: match_argument::Type::Header.into(),
            key: "x-user".to_string(),
            value: Some(MatchString {
                r#type: match_string::MatchStringType::Exact.into(),
                value: "alice".to_string(),
                value_type: 2,
            }),
        };

        assert!(!match_argument_matches(&req, &argument));
    }

    fn local_concurrency_rule(max_amount: u32) -> RateLimit {
        let mut rule = local_qps_rule(1);
        rule.id = "concurrency-rule".to_string();
        rule.rules[0].resource = limit_trigger::Resource::Concurrency.into();
        rule.rules[0].amounts.clear();
        rule.rules[0].concurrency_amount =
            Some(pole_specification::v1::ConcurrencyAmount { max_amount });
        rule
    }

    fn local_qps_rule_with_multiple_windows() -> RateLimit {
        let mut rule = local_qps_rule(2);
        rule.rules[0].amounts.push(Amount {
            max_amount: 1,
            valid_duration: Some(ProstDuration {
                seconds: 60,
                nanos: 0,
            }),
            ..Amount::default()
        });
        rule
    }

    fn global_qps_rule_with_failover(
        max_amount: u32,
        failover: limit_trigger::FailoverType,
    ) -> RateLimit {
        let mut rule = local_qps_rule(max_amount);
        rule.id = format!("global-{:?}", failover);
        rule.r#type = rate_limit::Type::Global.into();
        rule.rules[0].failover = failover.into();
        rule
    }

    #[test]
    fn local_quota_counter_rejects_when_window_amount_is_exhausted() {
        let counter = LocalQuotaCounter::new();
        let req = quota_request();
        let rules = vec![local_qps_rule(1)];

        let first = counter.check_quota(&req, &rules);
        let second = counter.check_quota(&req, &rules);

        assert!(first.allowed);
        assert!(!second.allowed);
    }

    #[test]
    fn local_quota_counter_only_consumes_quota_when_arguments_match() {
        let counter = LocalQuotaCounter::new();
        let mut req = quota_request();
        req.traffic_label_provider = header_traffic_label;

        let not_matched = counter.check_quota(&req, &[local_qps_rule_with_header(1, "bob")]);
        let first_matched = counter.check_quota(&req, &[local_qps_rule_with_header(1, "alice")]);
        let second_matched = counter.check_quota(&req, &[local_qps_rule_with_header(1, "alice")]);

        assert!(not_matched.allowed);
        assert!(first_matched.allowed);
        assert!(!second_matched.allowed);
    }

    #[test]
    fn local_quota_counter_partitions_by_captured_request_parameter() {
        fn alice(arg_type: ArgumentType, key: &str) -> Option<String> {
            header_traffic_label(arg_type, key)
        }
        fn bob(arg_type: ArgumentType, key: &str) -> Option<String> {
            match (arg_type, key) {
                (ArgumentType::Header, "x-user") => Some("bob".to_string()),
                _ => None,
            }
        }

        let counter = LocalQuotaCounter::new();
        let rule = local_qps_rule_with_dynamic_header(1);
        let mut alice_req = quota_request();
        alice_req.traffic_label_provider = alice;
        let mut bob_req = quota_request();
        bob_req.traffic_label_provider = bob;

        assert!(counter.check_quota(&alice_req, &[rule.clone()]).allowed);
        assert!(!counter.check_quota(&alice_req, &[rule.clone()]).allowed);
        assert!(counter.check_quota(&bob_req, &[rule]).allowed);
    }

    #[test]
    fn local_quota_counter_does_not_consume_quota_when_api_path_misses() {
        let counter = LocalQuotaCounter::new();
        let req = get_order_99_request();
        let rules = vec![local_qps_rule_for_path("/orders/42", 1)];

        let first = counter.check_quota(&req, &rules);
        let second = counter.check_quota(&req, &rules);

        assert!(first.allowed);
        assert!(second.allowed);
    }

    #[test]
    fn local_quota_counter_rejects_when_concurrency_amount_is_exhausted() {
        let counter = LocalQuotaCounter::new();
        let req = quota_request();
        let rules = vec![local_concurrency_rule(1)];

        let first = counter.check_quota(&req, &rules);
        let second = counter.check_quota(&req, &rules);

        assert!(first.allowed);
        assert!(!second.allowed);
    }

    #[test]
    fn local_quota_counter_checks_all_qps_windows_before_consuming() {
        let counter = LocalQuotaCounter::new();
        let req = quota_request();
        let rules = vec![local_qps_rule_with_multiple_windows()];

        let first = counter.check_quota(&req, &rules);
        let second = counter.check_quota(&req, &rules);

        assert!(first.allowed);
        assert!(!second.allowed);
    }

    #[test]
    fn local_quota_counter_releases_concurrency_amount() {
        let counter = LocalQuotaCounter::new();
        let req = quota_request();
        let rules = vec![local_concurrency_rule(1)];

        let first = counter.check_quota(&req, &rules);
        let exhausted = counter.check_quota(&req, &rules);
        let returned = counter.return_quota(&req, &rules);
        let after_return = counter.check_quota(&req, &rules);

        assert!(first.allowed);
        assert!(!exhausted.allowed);
        assert!(returned);
        assert!(after_return.allowed);
    }

    #[test]
    fn global_quota_counter_falls_back_to_local_when_failover_is_local() {
        let counter = LocalQuotaCounter::new();
        let req = quota_request();
        let rules = vec![global_qps_rule_with_failover(
            1,
            limit_trigger::FailoverType::FailoverLocal,
        )];

        let first = counter.check_quota(&req, &rules);
        let second = counter.check_quota(&req, &rules);

        assert!(first.allowed);
        assert!(!second.allowed);
    }

    #[test]
    fn global_concurrency_rule_keeps_local_counter_semantics() {
        let mut rule = local_concurrency_rule(1);
        rule.r#type = rate_limit::Type::Global.into();
        rule.rules[0].failover = limit_trigger::FailoverType::FailoverPass.into();
        let trigger = &rule.rules[0];

        assert!(uses_local_counter(&rule, trigger));
        assert!(!uses_distributed_quota(&rule, trigger));

        let counter = LocalQuotaCounter::new();
        let req = quota_request();
        let first = check_quota(&counter, &req, &[rule.clone()]);
        let second = check_quota(&counter, &req, &[rule.clone()]);
        return_quota(&counter, &req, &[rule.clone()]);
        let after_return = check_quota(&counter, &req, &[rule]);

        assert!(first.allowed);
        assert!(!second.allowed);
        assert!(after_return.allowed);
    }

    #[test]
    fn global_quota_counter_passes_when_failover_is_pass() {
        let counter = LocalQuotaCounter::new();
        let req = quota_request();
        let mut local_rule = local_qps_rule(1);
        local_rule.priority = 1;
        let rules = vec![
            global_qps_rule_with_failover(1, limit_trigger::FailoverType::FailoverPass),
            local_rule,
        ];

        let first = counter.check_quota(&req, &rules);
        let second = counter.check_quota(&req, &rules);

        assert!(first.allowed);
        assert!(second.allowed);
    }

    #[test]
    fn global_quota_falls_back_to_local_when_configured() {
        let counter = LocalQuotaCounter::new();
        let req = quota_request();
        let rules = vec![global_qps_rule_with_failover(
            1,
            limit_trigger::FailoverType::FailoverLocal,
        )];

        let first = check_quota(&counter, &req, &rules);
        let second = check_quota(&counter, &req, &rules);

        assert!(first.allowed);
        assert!(!second.allowed);
    }

    #[test]
    fn global_quota_passes_when_configured() {
        let counter = LocalQuotaCounter::new();
        let req = quota_request();
        let rules = vec![global_qps_rule_with_failover(
            1,
            limit_trigger::FailoverType::FailoverPass,
        )];

        let first = check_quota(&counter, &req, &rules);
        let second = check_quota(&counter, &req, &rules);

        assert!(first.allowed);
        assert!(second.allowed);
    }
}
