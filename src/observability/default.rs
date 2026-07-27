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

use std::collections::BTreeMap;

use crate::core::config::global::GlobalConfig;

use super::req::{
    ExporterConfig, ObservabilityConfig, OtlpProtocol, ResourceAttributeInputs, ResourceAttributes,
    SignalConfig, TelemetryEndpoint, TelemetryEndpointSource, TelemetrySignal,
    ENV_OTEL_EXPORTER_OTLP_ENDPOINT, ENV_OTEL_LOGS_EXPORTER, ENV_OTEL_METRICS_EXPORTER,
    ENV_OTEL_TRACES_EXPORTER,
};

#[derive(Clone, Debug)]
pub struct DefaultObservability {
    pub config: ObservabilityConfig,
    pub endpoints: Vec<TelemetryEndpoint>,
}

impl DefaultObservability {
    pub fn from_env_map<I, K, V>(inputs: ResourceAttributeInputs, env: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let env = env
            .into_iter()
            .map(|(key, value)| (key.as_ref().to_string(), value.as_ref().to_string()))
            .collect::<BTreeMap<_, _>>();
        let signals = signals_from_env(&env);
        let endpoint = env.get(ENV_OTEL_EXPORTER_OTLP_ENDPOINT).cloned();
        let mut endpoints = Vec::new();
        if let Some(endpoint) = endpoint.clone() {
            if !endpoint.is_empty() {
                endpoints.push(TelemetryEndpoint {
                    protocol: protocol_from_endpoint(&endpoint),
                    endpoint,
                    source: TelemetryEndpointSource::Env,
                    signals: signals.to_list(),
                });
            }
        }

        Self {
            config: ObservabilityConfig {
                enabled: !signals.disabled(),
                resource: ResourceAttributes::from_inputs_and_env(inputs, env.iter()),
                exporter: ExporterConfig {
                    endpoint,
                    protocol: endpoints
                        .first()
                        .map(|endpoint| endpoint.protocol.clone())
                        .unwrap_or_default(),
                    ..ExporterConfig::default()
                },
                signals,
            },
            endpoints,
        }
    }

    pub fn from_global_config<I, K, V>(global: &GlobalConfig, env: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let inputs = ResourceAttributeInputs {
            service_name: global.observability.service_name.clone(),
            service_version: global.observability.service_version.clone(),
            service_instance_id: global
                .observability
                .service_instance_id
                .clone()
                .or_else(|| Some(global.client.id.clone())),
            namespace: global.client.labels.get("pole.io/namespace").cloned(),
            pole_service_name: global.client.labels.get("pole.io/service").cloned(),
            pole_instance_id: global.client.labels.get("pole.io/instance-id").cloned(),
            labels: global
                .client
                .labels
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        };
        let mut observability = Self::from_env_map(inputs, env);
        observability.config.enabled = global.observability.enable && observability.config.enabled;

        if observability.config.exporter.endpoint.is_none() {
            if let Some(connector) = global.server_connectors.observability.as_ref() {
                let protocol = protocol_from_kind(&connector.protocol);
                observability.endpoints = connector
                    .addresses
                    .iter()
                    .filter(|address| !address.is_empty())
                    .map(|address| TelemetryEndpoint {
                        endpoint: format!("http://{address}"),
                        protocol: protocol.clone(),
                        source: TelemetryEndpointSource::LocalConfig,
                        signals: observability.config.signals.to_list(),
                    })
                    .collect();
                if let Some(endpoint) = observability.endpoints.first() {
                    observability.config.exporter.endpoint = Some(endpoint.endpoint.clone());
                    observability.config.exporter.protocol = endpoint.protocol.clone();
                    observability.config.exporter.timeout = connector.message_timeout;
                }
            }
        }

        observability
    }
}

impl Default for DefaultObservability {
    fn default() -> Self {
        Self {
            config: ObservabilityConfig::default(),
            endpoints: Vec::new(),
        }
    }
}

fn signals_from_env(env: &BTreeMap<String, String>) -> SignalConfig {
    SignalConfig {
        traces: exporter_enabled(env.get(ENV_OTEL_TRACES_EXPORTER).map(String::as_str)),
        metrics: exporter_enabled(env.get(ENV_OTEL_METRICS_EXPORTER).map(String::as_str)),
        logs: exporter_enabled(env.get(ENV_OTEL_LOGS_EXPORTER).map(String::as_str)),
    }
}

fn exporter_enabled(value: Option<&str>) -> bool {
    !matches!(
        value.map(|value| value.trim().to_ascii_lowercase()),
        Some(value) if value == "none"
    )
}

fn protocol_from_endpoint(endpoint: &str) -> OtlpProtocol {
    if endpoint.contains("/v1/") || endpoint.ends_with("/v1") {
        OtlpProtocol::Http
    } else {
        OtlpProtocol::Grpc
    }
}

fn protocol_from_kind(kind: &str) -> OtlpProtocol {
    let kind = kind.to_ascii_lowercase();
    if kind.contains("http") {
        OtlpProtocol::Http
    } else {
        OtlpProtocol::Grpc
    }
}

trait SignalConfigExt {
    fn to_list(&self) -> Vec<TelemetrySignal>;
}

impl SignalConfigExt for SignalConfig {
    fn to_list(&self) -> Vec<TelemetrySignal> {
        let mut signals = Vec::new();
        if self.traces {
            signals.push(TelemetrySignal::Traces);
        }
        if self.metrics {
            signals.push(TelemetrySignal::Metrics);
        }
        if self.logs {
            signals.push(TelemetrySignal::Logs);
        }
        signals
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::observability::req::{ATTR_SERVICE_NAME, ENV_OTEL_SERVICE_NAME};

    #[test]
    fn env_config_disables_only_none_exporters_and_keeps_endpoint() {
        let obs = DefaultObservability::from_env_map(
            ResourceAttributeInputs::default(),
            [
                (ENV_OTEL_EXPORTER_OTLP_ENDPOINT, "http://collector:4318/v1"),
                (ENV_OTEL_TRACES_EXPORTER, "otlp"),
                (ENV_OTEL_METRICS_EXPORTER, "none"),
                (ENV_OTEL_LOGS_EXPORTER, "none"),
                (ENV_OTEL_SERVICE_NAME, "checkout"),
            ],
        );

        assert!(obs.config.enabled);
        assert!(obs.config.signals.traces);
        assert!(!obs.config.signals.metrics);
        assert!(!obs.config.signals.logs);
        assert_eq!(
            obs.config.exporter.endpoint.as_deref(),
            Some("http://collector:4318/v1")
        );
        assert_eq!(obs.config.exporter.protocol, OtlpProtocol::Http);
        assert_eq!(obs.endpoints.len(), 1);
        assert_eq!(obs.config.resource.get(ATTR_SERVICE_NAME), Some("checkout"));
    }

    #[test]
    fn global_config_builds_resource_and_local_endpoint() {
        let global: GlobalConfig = serde_yaml::from_str(
            r#"
api:
  timeout: 1s
  maxRetryTimes: 1
  retryInterval: 500ms
  reportInterval: 10m
serverConnectors:
  discover:
    addresses: [127.0.0.1:8091]
    protocol: grpc
    connectTimeout: 500ms
    messageTimeout: 5s
  config:
    addresses: [127.0.0.1:8093]
    protocol: grpc
    connectTimeout: 500ms
    messageTimeout: 5s
  observability:
    addresses: [collector:4318]
    protocol: otlpHttp
    connectTimeout: 500ms
    messageTimeout: 3s
location:
  providers: []
client:
  id: client-a
  labels:
    pole.io/namespace: default
    pole.io/service: checkout
    pole.io/instance-id: pole-instance-a
observability:
  serviceName: checkout-http
"#,
        )
        .unwrap();

        let obs =
            DefaultObservability::from_global_config(&global, std::iter::empty::<(&str, &str)>());

        assert_eq!(
            obs.config.resource.get(ATTR_SERVICE_NAME),
            Some("checkout-http")
        );
        assert_eq!(obs.endpoints.len(), 1);
        assert_eq!(obs.endpoints[0].endpoint, "http://collector:4318");
        assert_eq!(
            obs.endpoints[0].source,
            TelemetryEndpointSource::LocalConfig
        );
        assert_eq!(obs.endpoints[0].protocol, OtlpProtocol::Http);
        assert_eq!(obs.config.exporter.timeout, Duration::from_secs(3));
    }
}
