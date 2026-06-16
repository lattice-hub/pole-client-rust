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

use std::env::VarError;

use pole_specification::v1::{
    match_string::ValueType, source_match, traffic_match_rule, CustomRouteRule, MatchString,
    SourceMatch, TrafficMatchRule,
};

use crate::core::{model::ArgumentType, plugin::router::RouteContext};

static WILDCARD: &str = "*";

// route_traffic_match 匹配主、被调服务信息，以及匹配请求流量标签
pub fn route_traffic_match(ctx: &RouteContext, rule: &CustomRouteRule) -> bool {
    if !match_callee_caller(ctx, rule) {
        return false;
    }

    rule.arguments
        .as_ref()
        .map(|rule| traffic_match_rule_match(ctx, rule))
        .unwrap_or(true)
}

pub fn traffic_match_rule_match(ctx: &RouteContext, rule: &TrafficMatchRule) -> bool {
    if rule.random_percent == 0 {
        return false;
    }

    if rule.arguments.is_empty() {
        return true;
    }

    match rule.match_mode() {
        traffic_match_rule::TrafficMatchMode::Or => rule
            .arguments
            .iter()
            .any(|source_match| source_match_matches(ctx, source_match)),
        traffic_match_rule::TrafficMatchMode::And => rule
            .arguments
            .iter()
            .all(|source_match| source_match_matches(ctx, source_match)),
    }
}

fn source_match_matches(ctx: &RouteContext, source_match: &SourceMatch) -> bool {
    let Some(rule_value) = &source_match.value else {
        return false;
    };

    let Some(actual_val) = resolve_actual_value(ctx, source_match, rule_value) else {
        return false;
    };

    match_label_value(rule_value, actual_val)
}

fn resolve_actual_value(
    ctx: &RouteContext,
    source_match: &SourceMatch,
    rule_value: &MatchString,
) -> Option<String> {
    let traffic_provider = ctx.route_info.traffic_label_provider;
    let ext_provider = ctx.route_info.external_parameter_supplier;

    let mut match_key = source_match.key.as_str();
    match rule_value.value_type() {
        ValueType::Text => {
            let mut traffic_type = argument_type_from_source(source_match.r#type());
            if source_match.key.contains('.') {
                let parts: Vec<&str> = match_key.splitn(2, '.').collect();
                match_key = parts[1];
                let mut label_prefix = parts[0];
                if parts[0].starts_with('$') {
                    label_prefix = &parts[0][1..];
                }
                traffic_type = ArgumentType::parse_from_str(label_prefix);
            }
            traffic_provider(traffic_type, match_key)
        }
        ValueType::Parameter => traffic_provider(
            argument_type_from_source(source_match.r#type()),
            rule_value.value.as_str(),
        ),
        ValueType::Variable => match std::env::var(rule_value.value.as_str()) {
            Ok(v) => Some(v),
            Err(err) => {
                if err != VarError::NotPresent {
                    return None;
                }
                ext_provider(rule_value.value.as_str())
            }
        },
    }
}

/// match_label_value 匹配标签值
pub fn match_label_value(rule_value: &MatchString, actual_val: String) -> bool {
    let match_value = rule_value.value.clone();
    if is_match_all(&match_value) {
        return true;
    }

    match rule_value.r#type() {
        pole_specification::v1::match_string::MatchStringType::Exact => {
            return match_value == actual_val;
        }
        pole_specification::v1::match_string::MatchStringType::NotEquals => {
            return match_value != actual_val;
        }
        pole_specification::v1::match_string::MatchStringType::Regex => {
            return regex::Regex::new(&match_value)
                .map(|regex| regex.is_match(&actual_val))
                .unwrap_or(false);
        }
        pole_specification::v1::match_string::MatchStringType::In => {
            return match_value.split(',').any(|x| x == actual_val);
        }
        pole_specification::v1::match_string::MatchStringType::NotIn => {
            return !match_value.split(',').any(|x| x == actual_val);
        }
        pole_specification::v1::match_string::MatchStringType::Range => {
            let parts: Vec<&str> = match_value.split(',').collect();
            if parts.len() != 2 {
                return false;
            }
            let Ok(min) = parts[0].parse::<i64>() else {
                return false;
            };
            let Ok(max) = parts[1].parse::<i64>() else {
                return false;
            };
            let Ok(actual_val) = actual_val.parse::<i64>() else {
                return false;
            };
            return actual_val >= min && actual_val <= max;
        }
    }
}

/// match_callee_caller 匹配主被调服务信息
pub fn match_callee_caller(rctx: &RouteContext, rule: &CustomRouteRule) -> bool {
    let callee = &rctx.route_info.callee;

    for (_, ele) in rule.destinations.iter().enumerate() {
        let svc = ele.service.clone();
        let ns = ele.namespace.clone();
        if svc != callee.name && !is_match_all(&svc) {
            return false;
        }
        if ns != callee.namespace && !is_match_all(&ns) {
            return false;
        }
    }

    true
}

pub fn is_match_all(s: &str) -> bool {
    s == WILDCARD
}

fn argument_type_from_source(source_type: source_match::Type) -> ArgumentType {
    match source_type {
        source_match::Type::Method => ArgumentType::Method,
        source_match::Type::Header => ArgumentType::Header,
        source_match::Type::Query => ArgumentType::Query,
        source_match::Type::CallerIp => ArgumentType::CallerIP,
        source_match::Type::Path => ArgumentType::Path,
        source_match::Type::Cookie => ArgumentType::Cookie,
        source_match::Type::CallerMetadata => ArgumentType::CallerService,
        source_match::Type::Custom => ArgumentType::Custom,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pole_specification::v1::{
        match_string::{MatchStringType, ValueType},
        source_match, traffic_match_rule, MatchString, SourceMatch, TrafficMatchRule,
    };

    fn traffic_label_provider(arg_type: ArgumentType, key: &str) -> Option<String> {
        match (arg_type, key) {
            (ArgumentType::Header, "x-env") => Some("prod".to_string()),
            (ArgumentType::Query, "uid") => Some("42".to_string()),
            (ArgumentType::Path, "") => Some("/v1/orders/123".to_string()),
            (ArgumentType::Method, "") => Some("GET".to_string()),
            _ => None,
        }
    }

    fn route_ctx() -> RouteContext {
        let mut route_info = crate::core::model::router::RouteInfo::default();
        route_info.traffic_label_provider = traffic_label_provider;
        RouteContext {
            route_info,
            extensions: None,
        }
    }

    fn source(
        source_type: source_match::Type,
        key: &str,
        match_type: MatchStringType,
        value: &str,
    ) -> SourceMatch {
        SourceMatch {
            r#type: source_type.into(),
            key: key.to_string(),
            value: Some(MatchString {
                r#type: match_type.into(),
                value: value.to_string(),
                value_type: ValueType::Text.into(),
            }),
        }
    }

    #[test]
    fn traffic_match_rule_match_supports_and_mode() {
        let rule = TrafficMatchRule {
            arguments: vec![
                source(
                    source_match::Type::Header,
                    "x-env",
                    MatchStringType::Exact,
                    "prod",
                ),
                source(
                    source_match::Type::Path,
                    "",
                    MatchStringType::Regex,
                    r"^/v1/orders/\d+$",
                ),
            ],
            random_percent: 100,
            match_mode: traffic_match_rule::TrafficMatchMode::And.into(),
        };

        assert!(traffic_match_rule_match(&route_ctx(), &rule));
    }

    #[test]
    fn traffic_match_rule_match_supports_or_mode() {
        let rule = TrafficMatchRule {
            arguments: vec![
                source(
                    source_match::Type::Header,
                    "missing",
                    MatchStringType::Exact,
                    "prod",
                ),
                source(
                    source_match::Type::Query,
                    "uid",
                    MatchStringType::Range,
                    "40,50",
                ),
            ],
            random_percent: 100,
            match_mode: traffic_match_rule::TrafficMatchMode::Or.into(),
        };

        assert!(traffic_match_rule_match(&route_ctx(), &rule));
    }

    #[test]
    fn traffic_match_rule_match_returns_false_when_and_clause_missing() {
        let rule = TrafficMatchRule {
            arguments: vec![
                source(
                    source_match::Type::Header,
                    "x-env",
                    MatchStringType::Exact,
                    "prod",
                ),
                source(
                    source_match::Type::Method,
                    "",
                    MatchStringType::Exact,
                    "POST",
                ),
            ],
            random_percent: 100,
            match_mode: traffic_match_rule::TrafficMatchMode::And.into(),
        };

        assert!(!traffic_match_rule_match(&route_ctx(), &rule));
    }

    #[test]
    fn match_label_value_handles_invalid_range_without_panic() {
        let value = MatchString {
            r#type: MatchStringType::Range.into(),
            value: "bad,50".to_string(),
            value_type: ValueType::Text.into(),
        };

        assert!(!match_label_value(&value, "42".to_string()));
    }
}
