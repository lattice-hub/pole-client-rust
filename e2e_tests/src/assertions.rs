use pole_rust::{
    core::model::{circuitbreaker::CheckResult, error::PoleError},
    discovery::req::InstanceResponse,
    faultdetect::FaultDetectResult,
    ratelimit::req::QuotaResponse,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LosslessBehaviorResult {
    pub rule_fetched: bool,
    pub delay_observed: bool,
    pub register_count: usize,
    pub readiness_status: u16,
    pub offline_status: u16,
    pub readiness_after_offline_status: u16,
    pub deregister_count: usize,
}

pub fn assert_mock_response(result: Result<InstanceResponse, PoleError>) -> Result<String, String> {
    let response = result.map_err(|err| format!("mock request failed: {err}"))?;
    let Some(mock) = response.mock_response else {
        return Err("mock response missing from SDK result".to_string());
    };

    if mock.status_code != 200 {
        return Err(format!(
            "mock response status mismatch: {}",
            mock.status_code
        ));
    }
    if mock.code != "E2E_MOCK" {
        return Err(format!("mock response code mismatch: {}", mock.code));
    }
    if !mock.body.contains("\"mocked\":true") {
        return Err(format!("mock response body mismatch: {}", mock.body));
    }

    Ok("mock response matched SDK result".to_string())
}

pub fn assert_security_denied(
    result: Result<InstanceResponse, PoleError>,
) -> Result<String, String> {
    match result {
        Ok(_) => Err("security request unexpectedly succeeded".to_string()),
        Err(err) => {
            let message = err.to_string();
            if message.contains("UNAUTHORIZED") || message.contains("traffic security denied") {
                Ok("security deny matched SDK result".to_string())
            } else {
                Err(format!(
                    "security request failed with unexpected error: {message}"
                ))
            }
        }
    }
}

pub fn assert_ratelimit_quota(
    first: Result<QuotaResponse, PoleError>,
    second: Result<QuotaResponse, PoleError>,
) -> Result<String, String> {
    let first = first.map_err(|err| format!("ratelimit first quota failed: {err}"))?;
    if !first.allowed {
        return Err(format!(
            "ratelimit first quota unexpectedly rejected: {}",
            first.message
        ));
    }

    let second = second.map_err(|err| format!("ratelimit second quota failed: {err}"))?;
    if second.allowed {
        return Err("ratelimit second quota unexpectedly allowed".to_string());
    }

    Ok("ratelimit matched local QPS quota".to_string())
}

pub fn assert_instance_metadata(
    case_name: &str,
    result: Result<InstanceResponse, PoleError>,
    key: &str,
    expected: &str,
) -> Result<String, String> {
    let response = result.map_err(|err| format!("{case_name} request failed: {err}"))?;
    let actual = response.instance.metadata.get(key);
    if actual.map(|value| value.as_str()) != Some(expected) {
        return Err(format!(
            "{case_name} selected instance metadata mismatch: {key}={actual:?}"
        ));
    }

    Ok(format!("{case_name} metadata matched selected instance"))
}

pub fn assert_circuitbreaker_open(
    result: Result<CheckResult, PoleError>,
) -> Result<String, String> {
    let check = result.map_err(|err| format!("circuitbreaker check failed: {err}"))?;
    if check.pass {
        return Err("circuitbreaker check unexpectedly passed".to_string());
    }

    Ok(format!(
        "circuitbreaker opened for rule {}",
        check.rule_name
    ))
}

pub fn assert_mirror_request(
    result: Result<String, String>,
    expected_path: &str,
    expected_body: &str,
) -> Result<String, String> {
    let raw = result.map_err(|err| format!("mirror receiver failed: {err}"))?;
    let request_line = raw.lines().next().unwrap_or_default();
    if !request_line.contains(expected_path) {
        return Err(format!(
            "mirror request path mismatch: expected {expected_path}"
        ));
    }
    if !raw.ends_with(expected_body) {
        return Err(format!(
            "mirror request body mismatch: expected {expected_body}"
        ));
    }

    Ok("mirror request matched shadow receiver".to_string())
}

pub fn assert_fault_detect_probe(result: FaultDetectResult) -> Result<String, String> {
    if !result.success {
        return Err(format!(
            "fault-detect probe failed for rule {}: {}",
            result.rule_id, result.message
        ));
    }

    Ok(format!(
        "fault-detect probe succeeded for rule {}",
        result.rule_id
    ))
}

pub fn assert_fault_detect_request(
    result: Result<String, String>,
    expected_path: &str,
    expected_header: &str,
    expected_header_value: &str,
) -> Result<String, String> {
    let raw = result.map_err(|err| format!("fault-detect receiver failed: {err}"))?;
    let request_line = raw.lines().next().unwrap_or_default();
    if !request_line.contains(expected_path) {
        return Err(format!(
            "fault-detect request path mismatch: expected {expected_path}"
        ));
    }

    let expected = format!(
        "{}: {}",
        expected_header.to_ascii_lowercase(),
        expected_header_value
    );
    let has_header = raw
        .lines()
        .any(|line| line.to_ascii_lowercase() == expected);
    if !has_header {
        return Err(format!(
            "fault-detect request header mismatch: expected {expected_header}: {expected_header_value}"
        ));
    }

    Ok("fault-detect request matched probe receiver".to_string())
}

pub fn assert_lossless_behavior(
    result: Result<LosslessBehaviorResult, String>,
) -> Result<String, String> {
    let result = result.map_err(|err| format!("lossless behavior failed: {err}"))?;
    if !result.rule_fetched {
        return Err("lossless rule was not fetched from SDK".to_string());
    }
    if !result.delay_observed {
        return Err("lossless delay register was not observed before register".to_string());
    }
    if result.register_count != 1 {
        return Err(format!(
            "lossless register count mismatch: {}",
            result.register_count
        ));
    }
    if result.readiness_status != 200 {
        return Err(format!(
            "lossless readiness status mismatch: {}",
            result.readiness_status
        ));
    }
    if result.offline_status != 200 {
        return Err(format!(
            "lossless offline status mismatch: {}",
            result.offline_status
        ));
    }
    if result.readiness_after_offline_status != 503 {
        return Err(format!(
            "lossless readiness-after-offline status mismatch: {}",
            result.readiness_after_offline_status
        ));
    }
    if result.deregister_count != 1 {
        return Err(format!(
            "lossless deregister count mismatch: {}",
            result.deregister_count
        ));
    }

    Ok("lossless behavior matched SDK rule and endpoints".to_string())
}
