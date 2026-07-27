use std::collections::HashMap;

use e2e_tests::config::{RunConfig, RunConfigError};

fn env(entries: &[(&str, &str)]) -> HashMap<String, String> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

#[test]
fn parses_required_console_and_shared_client_addr_from_args() {
    let cfg = RunConfig::from_sources(
        [
            "e2e_tests",
            "--console-url",
            "http://127.0.0.1:8080/",
            "--client-addr",
            "127.0.0.1:8091",
            "--case",
            "connectivity",
            "--report-dir",
            "target/custom-e2e",
            "--skip-cleanup",
        ],
        env(&[]),
    )
    .expect("args should parse");

    assert_eq!(cfg.console_url.as_str(), "http://127.0.0.1:8080");
    assert_eq!(
        cfg.sdk_addresses(),
        vec![
            "discover://127.0.0.1:8091".to_string(),
            "config://127.0.0.1:8091".to_string(),
        ]
    );
    assert_eq!(cfg.case_filter, vec!["connectivity".to_string()]);
    assert_eq!(cfg.report_dir.to_string_lossy(), "target/custom-e2e");
    assert!(cfg.skip_cleanup);
}

#[test]
fn explicit_discover_and_config_addrs_override_shared_client_addr() {
    let cfg = RunConfig::from_sources(
        [
            "e2e_tests",
            "--console-url",
            "http://127.0.0.1:8080",
            "--client-addr",
            "127.0.0.1:8091",
            "--discover-addr",
            "10.0.0.1:8091",
            "--config-addr",
            "10.0.0.2:8093",
        ],
        env(&[]),
    )
    .expect("args should parse");

    assert_eq!(
        cfg.sdk_addresses(),
        vec![
            "discover://10.0.0.1:8091".to_string(),
            "config://10.0.0.2:8093".to_string(),
        ]
    );
}

#[test]
fn builds_sdk_bootstrap_configuration_from_e2e_addresses() {
    let cfg = RunConfig::from_sources(
        [
            "e2e_tests",
            "--console-url",
            "http://127.0.0.1:8080",
            "--discover-addr",
            "10.0.0.1:8091",
            "--config-addr",
            "10.0.0.2:8093",
        ],
        env(&[]),
    )
    .expect("args should parse");

    let configuration = cfg
        .sdk_configuration("e2e-client")
        .expect("e2e bootstrap configuration should be valid");

    assert_eq!(configuration.global.client.id, "e2e-client");
    assert_eq!(
        configuration.global.server_connectors.discover.addresses,
        vec!["10.0.0.1:8091".to_string()]
    );
    assert_eq!(
        configuration.global.server_connectors.config.addresses,
        vec!["10.0.0.2:8093".to_string()]
    );
}

#[test]
fn builds_independent_labeled_clients_for_gray_release_verification() {
    let cfg = RunConfig::from_sources(
        [
            "e2e_tests",
            "--console-url",
            "http://127.0.0.1:8080",
            "--client-addr",
            "127.0.0.1:8091",
        ],
        env(&[]),
    )
    .unwrap();

    let blue = cfg
        .sdk_configuration_with_labels(
            "client-blue",
            HashMap::from([("tenant".to_string(), "blue".to_string())]),
        )
        .unwrap();
    let green = cfg
        .sdk_configuration_with_labels(
            "client-green",
            HashMap::from([("tenant".to_string(), "green".to_string())]),
        )
        .unwrap();

    assert_eq!(blue.global.client.id, "client-blue");
    assert_eq!(blue.global.client.labels["tenant"], "blue");
    assert_eq!(green.global.client.id, "client-green");
    assert_eq!(green.global.client.labels["tenant"], "green");
}

#[test]
fn reads_required_inputs_from_environment() {
    let cfg = RunConfig::from_sources(
        ["e2e_tests"],
        env(&[
            ("POLE_E2E_CONSOLE_URL", "http://console:8080"),
            ("POLE_E2E_CLIENT_ADDR", "control-plane:8091"),
            ("POLE_E2E_TOKEN", "token-1"),
        ]),
    )
    .expect("env should parse");

    assert_eq!(cfg.console_url.as_str(), "http://console:8080");
    assert_eq!(cfg.token.as_deref(), Some("token-1"));
    assert_eq!(
        cfg.sdk_addresses(),
        vec![
            "discover://control-plane:8091".to_string(),
            "config://control-plane:8091".to_string(),
        ]
    );
    assert!(!cfg.execute_governance_control_plane);
}

#[test]
fn reports_missing_required_inputs() {
    let err = RunConfig::from_sources(["e2e_tests"], env(&[])).unwrap_err();

    assert!(matches!(
        err,
        RunConfigError::MissingRequiredInput { field }
            if field == "console-url/client-addr"
    ));
}

#[test]
fn parses_governance_control_plane_execution_switch() {
    let cfg = RunConfig::from_sources(
        [
            "e2e_tests",
            "--console-url",
            "http://127.0.0.1:8080",
            "--client-addr",
            "127.0.0.1:8091",
            "--execute-governance-control-plane",
        ],
        env(&[]),
    )
    .expect("args should parse");

    assert!(cfg.execute_governance_control_plane);
}
