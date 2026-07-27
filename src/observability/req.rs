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

use std::{collections::BTreeMap, time::Duration};

use uuid::Uuid;

pub const ENV_OTEL_SERVICE_NAME: &str = "OTEL_SERVICE_NAME";
pub const ENV_OTEL_RESOURCE_ATTRIBUTES: &str = "OTEL_RESOURCE_ATTRIBUTES";
pub const ENV_OTEL_EXPORTER_OTLP_ENDPOINT: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";
pub const ENV_OTEL_TRACES_EXPORTER: &str = "OTEL_TRACES_EXPORTER";
pub const ENV_OTEL_METRICS_EXPORTER: &str = "OTEL_METRICS_EXPORTER";
pub const ENV_OTEL_LOGS_EXPORTER: &str = "OTEL_LOGS_EXPORTER";

pub const ATTR_SERVICE_NAME: &str = "service.name";
pub const ATTR_SERVICE_VERSION: &str = "service.version";
pub const ATTR_SERVICE_INSTANCE_ID: &str = "service.instance.id";
pub const ATTR_TELEMETRY_SDK_LANGUAGE: &str = "telemetry.sdk.language";
pub const ATTR_POLE_RUNTIME_LANGUAGE: &str = "pole.runtime.language";
pub const ATTR_POLE_NAMESPACE: &str = "pole.namespace";
pub const ATTR_POLE_SERVICE_NAME: &str = "pole.service.name";
pub const ATTR_POLE_INSTANCE_ID: &str = "pole.service.instance.id";
pub const ATTR_GOVERNANCE_DECISION_ID: &str = "pole.governance.decision.id";
pub const ATTR_GOVERNANCE_RULE_TYPE: &str = "pole.governance.rule.type";
pub const ATTR_GOVERNANCE_RULE_ID: &str = "pole.governance.rule.id";
pub const ATTR_GOVERNANCE_RESULT: &str = "pole.governance.result";
pub const ATTR_CALLEE_SERVICE_NAME: &str = "pole.callee.service.name";

pub const META_SERVICE_NAME: &str = "pole.io/service";
pub const META_NAMESPACE: &str = "pole.io/namespace";
pub const META_INSTANCE_ID: &str = "pole.io/instance-id";

const DEFAULT_EXPORT_TIMEOUT: Duration = Duration::from_secs(10);

const HIGH_CARDINALITY_METRIC_KEYS: &[&str] = &[
    ATTR_GOVERNANCE_DECISION_ID,
    ATTR_GOVERNANCE_RULE_ID,
    ATTR_POLE_INSTANCE_ID,
    ATTR_SERVICE_INSTANCE_ID,
    "trace_id",
    "trace.id",
    "span_id",
    "span.id",
    "request_id",
    "request.id",
    "http.target",
    "http.url",
    "url.full",
    "url.path",
    "client.address",
    "client.ip",
    "net.peer.ip",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservabilityConfig {
    pub enabled: bool,
    pub resource: ResourceAttributes,
    pub exporter: ExporterConfig,
    pub signals: SignalConfig,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            resource: ResourceAttributes::default(),
            exporter: ExporterConfig::default(),
            signals: SignalConfig::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExporterConfig {
    pub endpoint: Option<String>,
    pub protocol: OtlpProtocol,
    pub timeout: Duration,
}

impl Default for ExporterConfig {
    fn default() -> Self {
        Self {
            endpoint: None,
            protocol: OtlpProtocol::Grpc,
            timeout: DEFAULT_EXPORT_TIMEOUT,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalConfig {
    pub traces: bool,
    pub metrics: bool,
    pub logs: bool,
}

impl SignalConfig {
    pub fn disabled(&self) -> bool {
        !self.traces && !self.metrics && !self.logs
    }
}

impl Default for SignalConfig {
    fn default() -> Self {
        Self {
            traces: true,
            metrics: true,
            logs: true,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResourceAttributeInputs {
    pub service_name: Option<String>,
    pub service_version: Option<String>,
    pub service_instance_id: Option<String>,
    pub namespace: Option<String>,
    pub pole_service_name: Option<String>,
    pub pole_instance_id: Option<String>,
    pub labels: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResourceAttributes {
    pub attributes: BTreeMap<String, String>,
}

impl ResourceAttributes {
    pub fn from_inputs_and_env<I, K, V>(inputs: ResourceAttributeInputs, env: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let env = env
            .into_iter()
            .map(|(key, value)| (key.as_ref().to_string(), value.as_ref().to_string()))
            .collect::<BTreeMap<_, _>>();
        let mut attributes = BTreeMap::new();

        // 先写入 SDK 固定身份，再按业务输入和 OTel 环境变量逐层覆盖。
        attributes.insert(ATTR_TELEMETRY_SDK_LANGUAGE.to_string(), "rust".to_string());
        attributes.insert(ATTR_POLE_RUNTIME_LANGUAGE.to_string(), "rust".to_string());

        insert_optional(
            &mut attributes,
            ATTR_SERVICE_NAME,
            inputs.service_name.as_deref(),
        );
        insert_optional(
            &mut attributes,
            ATTR_SERVICE_VERSION,
            inputs.service_version.as_deref(),
        );
        insert_optional(
            &mut attributes,
            ATTR_SERVICE_INSTANCE_ID,
            inputs.service_instance_id.as_deref(),
        );

        let namespace = inputs
            .namespace
            .as_deref()
            .or_else(|| inputs.labels.get(META_NAMESPACE).map(String::as_str));
        insert_optional(&mut attributes, ATTR_POLE_NAMESPACE, namespace);

        let service_name = inputs
            .pole_service_name
            .as_deref()
            .or_else(|| inputs.labels.get(META_SERVICE_NAME).map(String::as_str));
        insert_optional(&mut attributes, ATTR_POLE_SERVICE_NAME, service_name);

        let pole_instance_id = inputs
            .pole_instance_id
            .as_deref()
            .or_else(|| inputs.labels.get(META_INSTANCE_ID).map(String::as_str));
        insert_optional(&mut attributes, ATTR_POLE_INSTANCE_ID, pole_instance_id);

        if let Some(raw) = env.get(ENV_OTEL_RESOURCE_ATTRIBUTES) {
            for (key, value) in parse_key_value_list(raw) {
                attributes.insert(key, value);
            }
        }
        if let Some(service_name) = env.get(ENV_OTEL_SERVICE_NAME) {
            if !service_name.is_empty() {
                attributes.insert(ATTR_SERVICE_NAME.to_string(), service_name.to_string());
            }
        }

        Self { attributes }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.attributes.get(key).map(String::as_str)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OtlpProtocol {
    Grpc,
    Http,
}

impl Default for OtlpProtocol {
    fn default() -> Self {
        Self::Grpc
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TelemetryEndpointSource {
    LocalConfig,
    Env,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TelemetryEndpoint {
    pub endpoint: String,
    pub protocol: OtlpProtocol,
    pub source: TelemetryEndpointSource,
    pub signals: Vec<TelemetrySignal>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TelemetrySignal {
    Traces,
    Metrics,
    Logs,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetricRecord {
    pub name: String,
    pub attributes: BTreeMap<String, String>,
}

impl MetricRecord {
    pub fn new<I, K, V>(name: impl Into<String>, attributes: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        Self {
            name: name.into(),
            attributes: low_cardinality_metric_attributes(attributes),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservabilityEvent {
    pub name: String,
    pub attributes: BTreeMap<String, String>,
}

impl ObservabilityEvent {
    pub fn new<I, K, V>(name: impl Into<String>, attributes: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        Self {
            name: name.into(),
            attributes: to_attribute_map(attributes),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpanAttributes {
    pub attributes: BTreeMap<String, String>,
}

impl SpanAttributes {
    pub fn new<I, K, V>(attributes: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        Self {
            attributes: to_attribute_map(attributes),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GovernanceDecisionContext {
    pub decision_id: String,
    pub namespace: String,
    pub service_name: String,
    pub callee_service_name: String,
    pub rule_type: String,
    pub rule_id: Option<String>,
    pub result: String,
}

impl GovernanceDecisionContext {
    pub fn new(
        namespace: impl Into<String>,
        service_name: impl Into<String>,
        callee_service_name: impl Into<String>,
        rule_type: impl Into<String>,
        rule_id: Option<String>,
        result: impl Into<String>,
    ) -> Self {
        Self::with_decision_id(
            Uuid::new_v4().to_string(),
            namespace,
            service_name,
            callee_service_name,
            rule_type,
            rule_id,
            result,
        )
    }

    pub fn with_decision_id(
        decision_id: impl Into<String>,
        namespace: impl Into<String>,
        service_name: impl Into<String>,
        callee_service_name: impl Into<String>,
        rule_type: impl Into<String>,
        rule_id: Option<String>,
        result: impl Into<String>,
    ) -> Self {
        Self {
            decision_id: decision_id.into(),
            namespace: namespace.into(),
            service_name: service_name.into(),
            callee_service_name: callee_service_name.into(),
            rule_type: rule_type.into(),
            rule_id,
            result: result.into(),
        }
    }

    pub fn metric_attributes(&self) -> BTreeMap<String, String> {
        low_cardinality_metric_attributes([
            (ATTR_POLE_NAMESPACE, self.namespace.as_str()),
            (ATTR_POLE_SERVICE_NAME, self.service_name.as_str()),
            (ATTR_CALLEE_SERVICE_NAME, self.callee_service_name.as_str()),
            (ATTR_GOVERNANCE_RULE_TYPE, self.rule_type.as_str()),
            (ATTR_GOVERNANCE_RESULT, self.result.as_str()),
        ])
    }

    pub fn event_attributes(&self) -> BTreeMap<String, String> {
        let mut attributes = self.shared_correlation_attributes();
        if let Some(rule_id) = &self.rule_id {
            attributes.insert(ATTR_GOVERNANCE_RULE_ID.to_string(), rule_id.clone());
        }
        attributes
    }

    pub fn span_attributes(&self) -> SpanAttributes {
        SpanAttributes {
            attributes: self.shared_correlation_attributes(),
        }
    }

    fn shared_correlation_attributes(&self) -> BTreeMap<String, String> {
        let mut attributes = BTreeMap::new();
        attributes.insert(
            ATTR_GOVERNANCE_DECISION_ID.to_string(),
            self.decision_id.clone(),
        );
        attributes.insert(ATTR_POLE_NAMESPACE.to_string(), self.namespace.clone());
        attributes.insert(
            ATTR_POLE_SERVICE_NAME.to_string(),
            self.service_name.clone(),
        );
        attributes.insert(
            ATTR_CALLEE_SERVICE_NAME.to_string(),
            self.callee_service_name.clone(),
        );
        attributes.insert(
            ATTR_GOVERNANCE_RULE_TYPE.to_string(),
            self.rule_type.clone(),
        );
        attributes.insert(ATTR_GOVERNANCE_RESULT.to_string(), self.result.clone());
        attributes
    }
}

pub fn low_cardinality_metric_attributes<I, K, V>(attributes: I) -> BTreeMap<String, String>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: AsRef<str>,
{
    attributes
        .into_iter()
        .filter_map(|(key, value)| {
            let key = key.as_ref();
            if is_high_cardinality_metric_key(key) {
                return None;
            }
            Some((key.to_string(), value.as_ref().to_string()))
        })
        .collect()
}

pub fn parse_key_value_list(raw: &str) -> BTreeMap<String, String> {
    raw.split(',')
        .filter_map(|part| {
            let trimmed = part.trim();
            if trimmed.is_empty() {
                return None;
            }
            let (key, value) = trimmed.split_once('=')?;
            let key = key.trim();
            if key.is_empty() {
                return None;
            }
            Some((key.to_string(), value.trim().to_string()))
        })
        .collect()
}

fn is_high_cardinality_metric_key(key: &str) -> bool {
    HIGH_CARDINALITY_METRIC_KEYS.contains(&key)
}

fn insert_optional(attributes: &mut BTreeMap<String, String>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        if !value.is_empty() {
            attributes.insert(key.to_string(), value.to_string());
        }
    }
}

fn to_attribute_map<I, K, V>(attributes: I) -> BTreeMap<String, String>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: AsRef<str>,
{
    attributes
        .into_iter()
        .map(|(key, value)| (key.as_ref().to_string(), value.as_ref().to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_attributes_apply_otel_env_priority() {
        let inputs = ResourceAttributeInputs {
            service_name: Some("local-service".to_string()),
            service_version: Some("1.0.0".to_string()),
            service_instance_id: Some("instance-a".to_string()),
            namespace: Some("prod".to_string()),
            pole_service_name: Some("order".to_string()),
            pole_instance_id: Some("pole-instance-a".to_string()),
            labels: BTreeMap::new(),
        };

        let attrs = ResourceAttributes::from_inputs_and_env(
            inputs,
            [
                (ENV_OTEL_SERVICE_NAME, "env-service"),
                (
                    ENV_OTEL_RESOURCE_ATTRIBUTES,
                    "service.version=2.0.0,k8s.namespace.name=prod-ns",
                ),
            ],
        );

        assert_eq!(attrs.get(ATTR_SERVICE_NAME), Some("env-service"));
        assert_eq!(attrs.get(ATTR_SERVICE_VERSION), Some("2.0.0"));
        assert_eq!(attrs.get(ATTR_TELEMETRY_SDK_LANGUAGE), Some("rust"));
        assert_eq!(attrs.get(ATTR_POLE_RUNTIME_LANGUAGE), Some("rust"));
        assert_eq!(attrs.get(ATTR_POLE_NAMESPACE), Some("prod"));
        assert_eq!(attrs.get("k8s.namespace.name"), Some("prod-ns"));
    }

    #[test]
    fn metric_record_strips_high_cardinality_attributes() {
        let metric = MetricRecord::new(
            "pole.sdk.governance.decision.count",
            [
                (ATTR_GOVERNANCE_RULE_TYPE, "rate_limit"),
                (ATTR_GOVERNANCE_RULE_ID, "rule-1"),
                (ATTR_GOVERNANCE_DECISION_ID, "decision-1"),
                ("trace_id", "trace-1"),
                ("url.path", "/users/123/orders/456"),
            ],
        );

        assert_eq!(
            metric.attributes.get(ATTR_GOVERNANCE_RULE_TYPE),
            Some(&"rate_limit".to_string())
        );
        assert!(!metric.attributes.contains_key(ATTR_GOVERNANCE_RULE_ID));
        assert!(!metric.attributes.contains_key(ATTR_GOVERNANCE_DECISION_ID));
        assert!(!metric.attributes.contains_key("trace_id"));
        assert!(!metric.attributes.contains_key("url.path"));
    }

    #[test]
    fn governance_decision_keeps_correlation_out_of_metrics() {
        let decision = GovernanceDecisionContext::with_decision_id(
            "decision-1",
            "default",
            "consumer",
            "provider",
            "router",
            Some("rule-1".to_string()),
            "matched",
        );

        let metric_attrs = decision.metric_attributes();
        let event_attrs = decision.event_attributes();
        let span_attrs = decision.span_attributes();

        assert_eq!(
            metric_attrs.get(ATTR_GOVERNANCE_RULE_TYPE),
            Some(&"router".to_string())
        );
        assert!(!metric_attrs.contains_key(ATTR_GOVERNANCE_DECISION_ID));
        assert!(!metric_attrs.contains_key(ATTR_GOVERNANCE_RULE_ID));
        assert_eq!(
            event_attrs.get(ATTR_GOVERNANCE_DECISION_ID),
            Some(&"decision-1".to_string())
        );
        assert_eq!(
            event_attrs.get(ATTR_GOVERNANCE_RULE_ID),
            Some(&"rule-1".to_string())
        );
        assert_eq!(
            span_attrs.attributes.get(ATTR_GOVERNANCE_DECISION_ID),
            Some(&"decision-1".to_string())
        );
    }
}
