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

use crate::core::{
    context::SDKContext,
    model::{error::PoleError, ArgumentType},
};
use crate::discovery::req::{GetServiceRuleRequest, ServiceRuleType};
use crate::plugins::router::rule::helper::match_label_value;
use crate::traffic::matcher::{ApiMatchIndex, ApiMatchInput};
use pole_specification::v1::{
    limit_trigger, match_argument, match_string, rate_limit, LimitTrigger, MatchArgument, RateLimit,
};

use super::{
    api::RateLimitAPI,
    req::{QuotaRequest, QuotaResponse},
};

pub struct DefaultRateLimitAPI {
    manage_sdk: bool,
    context: Arc<SDKContext>,
    local_counter: LocalQuotaCounter,
}

impl DefaultRateLimitAPI {
    pub fn new_raw(context: SDKContext) -> Self {
        let ctx = Arc::new(context);
        Self {
            manage_sdk: true,
            context: ctx,
            local_counter: LocalQuotaCounter::new(),
        }
    }

    pub fn new(context: Arc<SDKContext>) -> Self {
        Self {
            manage_sdk: false,
            context: context,
            local_counter: LocalQuotaCounter::new(),
        }
    }

    async fn load_rate_limit_rules(&self, req: &QuotaRequest) -> Result<Vec<RateLimit>, PoleError> {
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

        Ok(check_quota(&self.local_counter, &req, &rules))
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
            if rule.r#type() == rate_limit::Type::Global
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
                    for amount in trigger.amounts.iter() {
                        if amount.max_amount == 0 {
                            continue;
                        }
                        let duration = amount_duration(amount);
                        let key = quota_counter_key(req, rule, trigger, duration);
                        let mut windows = self.windows.lock().unwrap();
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
                        window.used += 1;
                        return QuotaResponse {
                            allowed: true,
                            message: String::new(),
                        };
                    }
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

fn check_quota(
    local_counter: &LocalQuotaCounter,
    req: &QuotaRequest,
    rules: &[RateLimit],
) -> QuotaResponse {
    for entry in matching_quota_triggers(req, rules) {
        if entry.rule.r#type() == rate_limit::Type::Global {
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
            if entry.trigger.failover() == limit_trigger::FailoverType::FailoverPass {
                return;
            }
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
    rule.r#type() == rate_limit::Type::Local
        || (rule.r#type() == rate_limit::Type::Global
            && trigger.failover() == limit_trigger::FailoverType::FailoverLocal)
}

fn matching_quota_triggers<'a>(
    req: &QuotaRequest,
    rules: &'a [RateLimit],
) -> Vec<QuotaTriggerEntry<'a>> {
    let mut rules = rules.iter().collect::<Vec<_>>();
    rules.sort_by(|a, b| a.priority.cmp(&b.priority));

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
        "{}#{}#{}#{}#{}#{}",
        req.namespace,
        req.service,
        req.method,
        rule.id,
        trigger.name,
        duration.as_secs()
    )
}

fn concurrency_counter_key(req: &QuotaRequest, rule: &RateLimit, trigger: &LimitTrigger) -> String {
    format!(
        "{}#{}#{}#{}#{}#concurrency",
        req.namespace, req.service, req.method, rule.id, trigger.name
    )
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

    fn local_concurrency_rule(max_amount: u32) -> RateLimit {
        let mut rule = local_qps_rule(1);
        rule.id = "concurrency-rule".to_string();
        rule.rules[0].resource = limit_trigger::Resource::Concurrency.into();
        rule.rules[0].amounts.clear();
        rule.rules[0].concurrency_amount =
            Some(pole_specification::v1::ConcurrencyAmount { max_amount });
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
    fn unknown_argument_value_type_fails_closed() {
        let mut req = quota_request();
        req.traffic_label_provider = header_traffic_label;
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
