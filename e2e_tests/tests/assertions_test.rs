use e2e_tests::assertions::{
    assert_fault_detect_probe, assert_fault_detect_request, assert_instance_metadata,
    assert_lossless_behavior, assert_mirror_request, assert_mock_response, assert_ratelimit_quota,
    assert_security_denied, LosslessBehaviorResult,
};
use pole_rust::{
    core::model::{
        circuitbreaker::CheckResult,
        error::{ErrorCode, PoleError},
        naming::Instance,
    },
    discovery::req::InstanceResponse,
    faultdetect::FaultDetectResult,
    ratelimit::req::QuotaResponse,
};
use pole_specification::v1::MockResponse;

#[test]
fn mock_assertion_passes_when_sdk_returns_expected_mock_response() {
    let result = assert_mock_response(Ok(InstanceResponse {
        instance: Default::default(),
        mock_response: Some(MockResponse {
            status_code: 200,
            code: "E2E_MOCK".to_string(),
            body: "{\"mocked\":true,\"service\":\"svc\"}".to_string(),
            ..Default::default()
        }),
    }));

    assert!(result.unwrap().contains("mock response matched"));
}

#[test]
fn mock_assertion_fails_when_sdk_does_not_return_mock_response() {
    let result = assert_mock_response(Ok(InstanceResponse {
        instance: Default::default(),
        mock_response: None,
    }));

    assert_eq!(result.unwrap_err(), "mock response missing from SDK result");
}

#[test]
fn security_assertion_passes_when_sdk_returns_unauthorized_error() {
    let result = assert_security_denied(Err(PoleError::new(
        ErrorCode::UNAUTHORIZED,
        "traffic security denied".to_string(),
    )));

    assert!(result.unwrap().contains("security deny matched"));
}

#[test]
fn security_assertion_fails_when_sdk_request_is_allowed() {
    let result = assert_security_denied(Ok(InstanceResponse {
        instance: Default::default(),
        mock_response: None,
    }));

    assert_eq!(
        result.unwrap_err(),
        "security request unexpectedly succeeded"
    );
}

#[test]
fn ratelimit_assertion_passes_when_second_quota_is_rejected() {
    let result = assert_ratelimit_quota(
        Ok(QuotaResponse {
            allowed: true,
            message: "ok".to_string(),
        }),
        Ok(QuotaResponse {
            allowed: false,
            message: "limited".to_string(),
        }),
    );

    assert!(result.unwrap().contains("ratelimit matched"));
}

#[test]
fn ratelimit_assertion_fails_when_second_quota_is_allowed() {
    let result = assert_ratelimit_quota(
        Ok(QuotaResponse {
            allowed: true,
            message: "ok".to_string(),
        }),
        Ok(QuotaResponse {
            allowed: true,
            message: "unexpected".to_string(),
        }),
    );

    assert_eq!(
        result.unwrap_err(),
        "ratelimit second quota unexpectedly allowed"
    );
}

#[test]
fn instance_metadata_assertion_passes_when_selected_instance_has_expected_label() {
    let result = assert_instance_metadata(
        "routing",
        Ok(InstanceResponse {
            instance: Instance {
                metadata: [("e2e-route".to_string(), "primary".to_string())].into(),
                ..Default::default()
            },
            mock_response: None,
        }),
        "e2e-route",
        "primary",
    );

    assert!(result.unwrap().contains("routing metadata matched"));
}

#[test]
fn instance_metadata_assertion_fails_when_selected_instance_has_wrong_label() {
    let result = assert_instance_metadata(
        "lane-routing",
        Ok(InstanceResponse {
            instance: Instance {
                metadata: [("lane".to_string(), "green".to_string())].into(),
                ..Default::default()
            },
            mock_response: None,
        }),
        "lane",
        "blue",
    );

    assert_eq!(
        result.unwrap_err(),
        "lane-routing selected instance metadata mismatch: lane=Some(\"green\")"
    );
}

#[test]
fn circuitbreaker_assertion_passes_when_check_is_blocked_after_failures() {
    let result = e2e_tests::assertions::assert_circuitbreaker_open(Ok(CheckResult {
        pass: false,
        rule_name: "cb".to_string(),
        fallback_info: None,
    }));

    assert!(result.unwrap().contains("circuitbreaker opened"));
}

#[test]
fn circuitbreaker_assertion_fails_when_check_still_passes() {
    let result = e2e_tests::assertions::assert_circuitbreaker_open(Ok(CheckResult::pass()));

    assert_eq!(
        result.unwrap_err(),
        "circuitbreaker check unexpectedly passed"
    );
}

#[test]
fn mirror_assertion_passes_when_shadow_receiver_gets_expected_request() {
    let result = assert_mirror_request(
        Ok("POST /e2e/mirror HTTP/1.1\r\nx-e2e: mirror\r\n\r\n{\"mirror\":true}".to_string()),
        "/e2e/mirror",
        "{\"mirror\":true}",
    );

    assert!(result.unwrap().contains("mirror request matched"));
}

#[test]
fn mirror_assertion_fails_when_shadow_receiver_gets_wrong_body() {
    let result = assert_mirror_request(
        Ok("POST /e2e/mirror HTTP/1.1\r\n\r\n{}".to_string()),
        "/e2e/mirror",
        "{\"mirror\":true}",
    );

    assert_eq!(
        result.unwrap_err(),
        "mirror request body mismatch: expected {\"mirror\":true}"
    );
}

#[test]
fn fault_detect_assertion_passes_when_probe_succeeds() {
    let result = FaultDetectResult {
        rule_id: "e2e-rule".to_string(),
        success: true,
        message: String::new(),
    };

    let message = assert_fault_detect_probe(result).unwrap();

    assert!(message.contains("e2e-rule"));
}

#[test]
fn fault_detect_assertion_fails_when_probe_fails() {
    let result = FaultDetectResult {
        rule_id: "e2e-rule".to_string(),
        success: false,
        message: "http fault detect request failed".to_string(),
    };

    let err = assert_fault_detect_probe(result).unwrap_err();

    assert!(err.contains("http fault detect request failed"));
}

#[test]
fn fault_detect_request_assertion_passes_when_probe_hits_expected_path_and_header() {
    let result = assert_fault_detect_request(
        Ok("GET /health HTTP/1.1\r\nx-pole-e2e: run-4\r\n\r\n".to_string()),
        "/health",
        "x-pole-e2e",
        "run-4",
    );

    assert!(result.unwrap().contains("fault-detect request matched"));
}

#[test]
fn fault_detect_request_assertion_fails_when_header_is_missing() {
    let result = assert_fault_detect_request(
        Ok("GET /health HTTP/1.1\r\nhost: 127.0.0.1\r\n\r\n".to_string()),
        "/health",
        "x-pole-e2e",
        "run-4",
    );

    assert_eq!(
        result.unwrap_err(),
        "fault-detect request header mismatch: expected x-pole-e2e: run-4"
    );
}

#[test]
fn lossless_assertion_passes_when_delay_and_endpoints_match() {
    let result = assert_lossless_behavior(Ok(LosslessBehaviorResult {
        rule_fetched: true,
        delay_observed: true,
        register_count: 1,
        readiness_status: 200,
        offline_status: 200,
        readiness_after_offline_status: 503,
        deregister_count: 1,
    }));

    assert!(result.unwrap().contains("lossless behavior matched"));
}

#[test]
fn lossless_assertion_fails_when_delay_is_not_observed() {
    let result = assert_lossless_behavior(Ok(LosslessBehaviorResult {
        rule_fetched: true,
        delay_observed: false,
        register_count: 1,
        readiness_status: 200,
        offline_status: 200,
        readiness_after_offline_status: 503,
        deregister_count: 1,
    }));

    assert_eq!(
        result.unwrap_err(),
        "lossless delay register was not observed before register"
    );
}
