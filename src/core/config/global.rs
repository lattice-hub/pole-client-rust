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

use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GlobalConfig {
    pub api: APIConfig,
    pub server_connectors: ServerConnectorConfig,
    pub location: LocationConfig,
    pub client: ClientConfig,
    #[serde(default)]
    pub observability: ObservabilityConfig,
}

impl GlobalConfig {
    pub fn update_server_connector_address(&mut self, addresses: Vec<String>) {
        self.server_connectors.update_addresses(addresses);
    }
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct APIConfig {
    #[serde(with = "serde_duration_ext")]
    pub timeout: Duration,
    pub max_retry_times: u32,
    #[serde(with = "serde_duration_ext")]
    pub retry_interval: Duration,
    pub bind_if: Option<String>,
    pub bind_ip: Option<String>,
    #[serde(with = "serde_duration_ext")]
    pub report_interval: Duration,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerConnectorConfig {
    pub discover: ConnectorTargetConfig,
    pub config: ConnectorTargetConfig,
    pub observability: Option<ConnectorTargetConfig>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectorTargetConfig {
    pub addresses: Vec<String>,
    pub protocol: String,
    #[serde(with = "serde_duration_ext")]
    pub connect_timeout: Duration,
    #[serde(with = "serde_duration_ext")]
    pub message_timeout: Duration,
}

impl ServerConnectorConfig {
    pub fn get_protocol(&self) -> String {
        if self.discover.protocol.eq_ignore_ascii_case("grpcs") {
            // grpc/grpcs 共用同一个 connector 插件，差异仅在 channel scheme/TLS。
            "grpc".to_string()
        } else {
            self.discover.protocol.clone()
        }
    }

    pub fn update_addresses(&mut self, addresses: Vec<String>) {
        self.discover.addresses.clear();
        self.config.addresses.clear();
        for address in addresses {
            if let Some(endpoint) = address.strip_prefix("discover://") {
                self.discover.addresses.push(endpoint.to_string());
            } else if let Some(endpoint) = address.strip_prefix("config://") {
                self.config.addresses.push(endpoint.to_string());
            }
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocationConfig {
    pub providers: Option<Vec<LocationProviderConfig>>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocationProviderConfig {
    pub name: String,
    pub options: HashMap<String, String>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalCacheConfig {
    #[serde(default = "default_local_cache_name")]
    pub name: String,
    pub service_expire_enable: bool,
    #[serde(with = "serde_duration_ext")]
    pub service_expire_time: Duration,
    #[serde(with = "serde_duration_ext")]
    pub service_refresh_interval: Duration,
    #[serde(with = "serde_duration_ext")]
    pub service_list_refresh_interval: Duration,
    pub persist_enable: bool,
    pub persist_dir: String,
}

fn default_local_cache_name() -> String {
    "memory".to_string()
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginConfig {
    pub name: String,
    pub options: Option<HashMap<String, String>>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientConfig {
    pub id: String,
    pub labels: HashMap<String, String>,
    #[serde(default)]
    pub service_identity_discovery: Option<ServiceIdentityDiscoveryConfig>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceIdentityDiscoveryConfig {
    pub namespace: String,
    pub service: String,
    pub control_plane_token: String,
}

impl std::fmt::Debug for ServiceIdentityDiscoveryConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServiceIdentityDiscoveryConfig")
            .field("namespace", &self.namespace)
            .field("service", &self.service)
            .field("control_plane_token", &"[REDACTED]")
            .finish()
    }
}

impl ServiceIdentityDiscoveryConfig {
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if self.namespace.trim().is_empty() {
            return Err("service identity namespace must not be empty");
        }
        if self.service.trim().is_empty() {
            return Err("service identity service must not be empty");
        }
        if self.control_plane_token.is_empty()
            || !self
                .control_plane_token
                .as_bytes()
                .iter()
                .all(|value| (0x20..=0x7e).contains(value))
        {
            return Err("service identity token must be a non-empty ASCII metadata value");
        }
        Ok(())
    }
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservabilityConfig {
    #[serde(default = "default_true")]
    pub enable: bool,
    #[serde(default)]
    pub service_name: Option<String>,
    #[serde(default)]
    pub service_version: Option<String>,
    #[serde(default)]
    pub service_instance_id: Option<String>,
    #[serde(default)]
    pub events: ObservabilityEventsConfig,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            enable: true,
            service_name: None,
            service_version: None,
            service_instance_id: None,
            events: ObservabilityEventsConfig::default(),
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservabilityEventsConfig {
    #[serde(default = "default_true")]
    pub governance: bool,
    #[serde(default = "default_true")]
    pub telemetry_endpoint: bool,
    #[serde(default = "default_true")]
    pub config_reload: bool,
}

impl Default for ObservabilityEventsConfig {
    fn default() -> Self {
        Self {
            governance: true,
            telemetry_endpoint: true,
            config_reload: true,
        }
    }
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observability_config_defaults_when_missing() {
        let config: GlobalConfig = serde_yaml::from_str(MINIMAL_GLOBAL_CONFIG).unwrap();

        assert!(config.observability.enable);
        assert!(config.observability.events.governance);
    }

    #[test]
    fn observability_config_parses_local_options() {
        let config: GlobalConfig = serde_yaml::from_str(&format!(
            "{}{}",
            MINIMAL_GLOBAL_CONFIG,
            r#"
observability:
  enable: true
  serviceName: checkout
  serviceVersion: 1.2.0
  serviceInstanceId: instance-a
  events:
    governance: true
    telemetryEndpoint: true
    configReload: false
"#
        ))
        .unwrap();

        assert_eq!(
            config.observability.service_name.as_deref(),
            Some("checkout")
        );
        assert!(!config.observability.events.config_reload);
    }

    #[test]
    fn service_identity_config_is_optional() {
        let config: GlobalConfig = serde_yaml::from_str(MINIMAL_GLOBAL_CONFIG).unwrap();

        assert!(config.client.service_identity_discovery.is_none());
    }

    #[test]
    fn service_identity_config_parses_and_redacts_token_from_debug_output() {
        let config: GlobalConfig = serde_yaml::from_str(&MINIMAL_GLOBAL_CONFIG.replace(
            "  labels: {}",
            "  labels: {}\n  serviceIdentityDiscovery:\n    namespace: production\n    service: orders\n    controlPlaneToken: secret-token",
        ))
        .unwrap();

        let identity = config.client.service_identity_discovery.unwrap();
        assert_eq!(identity.namespace, "production");
        assert_eq!(identity.service, "orders");
        assert_eq!(identity.control_plane_token, "secret-token");
        let debug = format!("{identity:?}");
        assert!(!debug.contains("secret-token"));
        assert!(debug.contains("[REDACTED]"));
        assert!(identity.validate().is_ok());
    }

    #[test]
    fn service_identity_config_rejects_missing_or_invalid_authentication_fields() {
        let mut identity = ServiceIdentityDiscoveryConfig {
            namespace: String::new(),
            service: "orders".to_string(),
            control_plane_token: "service-secret".to_string(),
        };
        assert_eq!(
            identity.validate(),
            Err("service identity namespace must not be empty")
        );

        identity.namespace = "production".to_string();
        identity.control_plane_token = "secret\nheader".to_string();
        assert_eq!(
            identity.validate(),
            Err("service identity token must be a non-empty ASCII metadata value")
        );
    }

    #[test]
    fn service_identity_supports_plaintext_and_tls_discover_transport() {
        let grpc: GlobalConfig = serde_yaml::from_str(&MINIMAL_GLOBAL_CONFIG.replace(
            "  labels: {}",
            "  labels: {}\n  serviceIdentityDiscovery:\n    namespace: production\n    service: orders\n    controlPlaneToken: secret-token",
        ))
        .unwrap();
        assert_eq!(grpc.server_connectors.get_protocol(), "grpc");

        let grpcs: GlobalConfig = serde_yaml::from_str(
            &MINIMAL_GLOBAL_CONFIG
                .replace("protocol: grpc", "protocol: grpcs")
                .replace(
                    "  labels: {}",
                    "  labels: {}\n  serviceIdentityDiscovery:\n    namespace: production\n    service: orders\n    controlPlaneToken: secret-token",
                ),
        )
        .unwrap();
        assert_eq!(grpcs.server_connectors.get_protocol(), "grpc");
    }

    const MINIMAL_GLOBAL_CONFIG: &str = r#"
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
location:
  providers: []
client:
  id: rust-client
  labels: {}
"#;
}
