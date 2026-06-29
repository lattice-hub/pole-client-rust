use e2e_tests::{
    cases::{CaseKind, E2eCase},
    control_plan::control_plane_plan,
    flows::FlowResource,
};

#[test]
fn routing_plan_uses_console_create_publish_and_cleanup_endpoints() {
    let plan = control_plane_plan(
        E2eCase::new("routing", CaseKind::Routing),
        &FlowResource::new("run-1", "routing"),
    )
    .expect("routing should have a control-plane plan");

    assert_eq!(plan.create.path, "/naming/v1/routings");
    assert_eq!(plan.publish.path, "/naming/v1/routings/releases");
    assert_eq!(
        plan.cleanup_actions[0].path,
        "/naming/v1/routings/releases/delete"
    );
    assert_eq!(plan.cleanup_actions[1].path, "/naming/v1/routings/delete");
    assert_eq!(plan.create.body[0]["name"], "e2e-run-1-routing");
    assert_eq!(plan.publish.body[0]["rule_name"], "e2e-run-1-routing");
}

#[test]
fn all_governance_cases_have_control_plane_endpoint_mapping() {
    let cases = [
        (CaseKind::Routing, "routing", "/naming/v1/routings"),
        (CaseKind::RateLimit, "ratelimit", "/naming/v1/ratelimits"),
        (
            CaseKind::CircuitBreaker,
            "circuitbreaker",
            "/naming/v1/circuitbreakers",
        ),
        (
            CaseKind::LaneRouting,
            "lane-routing",
            "/naming/v1/lane/groups",
        ),
        (
            CaseKind::FaultDetect,
            "fault-detect",
            "/naming/v1/faultdetectors",
        ),
        (CaseKind::Lossless, "lossless", "/naming/v1/lossless"),
        (CaseKind::Mirror, "mirror", "/naming/v1/traffic/mirrors"),
        (
            CaseKind::AuthSecurity,
            "auth-security",
            "/naming/v1/traffic/security",
        ),
        (CaseKind::Mock, "mock", "/naming/v1/traffic/mocks"),
    ];

    for (kind, name, create_path) in cases {
        let plan = control_plane_plan(E2eCase::new(name, kind), &FlowResource::new("run-2", name))
            .unwrap_or_else(|| panic!("{name} should have a control-plane plan"));
        assert_eq!(plan.create.path, create_path);
        assert!(
            plan.create.body[0]["e2e"].as_bool().unwrap_or(false)
                || plan.create.body[0]["metadata"]["e2e"] == "true"
        );
        assert_eq!(plan.cleanup_actions.len(), 2);
    }
}

#[test]
fn mock_plan_uses_spec_like_rule_payload() {
    let plan = control_plane_plan(
        E2eCase::new("mock", CaseKind::Mock),
        &FlowResource::new("run-2", "mock"),
    )
    .expect("mock should have a control-plane plan");
    let rule = &plan.create.body[0];

    assert_eq!(rule["enable"], true);
    assert_eq!(rule["rules"][0]["mock_percent"], 100);
    assert_eq!(rule["callee"]["namespace"], "e2e-run-2");
    assert_eq!(rule["callee"]["service"], "e2e-run-2-mock");
    assert_eq!(
        rule["rules"][0]["traffic_match_rule"]["arguments"][0]["type"],
        "HEADER"
    );
    assert_eq!(
        rule["rules"][0]["traffic_match_rule"]["arguments"][0]["key"],
        "x-pole-e2e"
    );
    assert_eq!(
        rule["rules"][0]["traffic_match_rule"]["arguments"][0]["value"]["value"],
        "mock"
    );
    assert_eq!(rule["rules"][0]["response"]["code"], "E2E_MOCK");
    assert!(rule["rules"][0]["response"]["body"]
        .as_str()
        .unwrap()
        .contains("e2e-run-2-mock"));
}

#[test]
fn security_plan_uses_spec_like_deny_payload() {
    let plan = control_plane_plan(
        E2eCase::new("auth-security", CaseKind::AuthSecurity),
        &FlowResource::new("run-2", "auth-security"),
    )
    .expect("security should have a control-plane plan");
    let rule = &plan.create.body[0];

    assert_eq!(rule["target_service"]["namespace"], "e2e-run-2");
    assert_eq!(rule["target_service"]["service"], "e2e-run-2-auth-security");
    assert_eq!(rule["policies"][0]["action"], "TRAFFIC_SECURITY_DENY");
    assert_eq!(rule["policies"][0]["reject_effect"]["code"], "E2E_DENIED");
    assert_eq!(
        rule["policies"][0]["traffic_match_rule"]["arguments"][0]["type"],
        "HEADER"
    );
    assert_eq!(
        rule["policies"][0]["traffic_match_rule"]["arguments"][0]["key"],
        "x-pole-e2e-deny"
    );
    assert_eq!(
        rule["policies"][0]["traffic_match_rule"]["arguments"][0]["value"]["value"],
        "true"
    );
}

#[test]
fn routing_plan_uses_spec_like_custom_route_payload() {
    let plan = control_plane_plan(
        E2eCase::new("routing", CaseKind::Routing),
        &FlowResource::new("run-5", "routing"),
    )
    .expect("routing should have a control-plane plan");
    let rule = &plan.create.body[0];

    assert_eq!(rule["route_policy"], "RulePolicy");
    assert_eq!(
        rule["routing_config"]["rules"][0]["name"],
        "e2e-run-5-routing"
    );
    assert_eq!(
        rule["routing_config"]["rules"][0]["arguments"]["arguments"][0]["key"],
        "x-pole-e2e-route"
    );
    assert_eq!(
        rule["routing_config"]["rules"][0]["destinations"][0]["labels"]["e2e-route"]["value"],
        "primary"
    );
}

#[test]
fn ratelimit_plan_uses_spec_like_local_qps_payload() {
    let plan = control_plane_plan(
        E2eCase::new("ratelimit", CaseKind::RateLimit),
        &FlowResource::new("run-5", "ratelimit"),
    )
    .expect("ratelimit should have a control-plane plan");
    let rule = &plan.create.body[0];

    assert_eq!(rule["type"], "LOCAL");
    assert_eq!(rule["rules"][0]["resource"], "QPS");
    assert_eq!(rule["rules"][0]["amounts"][0]["max_amount"], 1);
    assert_eq!(rule["rules"][0]["failover"], "FAILOVER_LOCAL");
}

#[test]
fn circuitbreaker_plan_uses_spec_like_consecutive_error_payload() {
    let plan = control_plane_plan(
        E2eCase::new("circuitbreaker", CaseKind::CircuitBreaker),
        &FlowResource::new("run-5", "circuitbreaker"),
    )
    .expect("circuitbreaker should have a control-plane plan");
    let rule = &plan.create.body[0];

    assert_eq!(rule["level"], "SERVICE");
    assert_eq!(
        rule["block_configs"][0]["block_config"]["trigger_conditions"][0]["trigger_type"],
        "CONSECUTIVE_ERROR"
    );
    assert_eq!(
        rule["block_configs"][0]["block_config"]["trigger_conditions"][0]["error_count"],
        2
    );
    assert_eq!(
        rule["block_configs"][0]["recover_condition"]["sleep_window"],
        1
    );
}

#[test]
fn lane_plan_uses_spec_like_lane_rule_payload() {
    let plan = control_plane_plan(
        E2eCase::new("lane-routing", CaseKind::LaneRouting),
        &FlowResource::new("run-5", "lane-routing"),
    )
    .expect("lane should have a control-plane plan");
    let rule = &plan.create.body[0];

    assert_eq!(rule["rules"][0]["label_key"], "lane");
    assert_eq!(rule["rules"][0]["default_label_value"], "blue");
    assert_eq!(rule["rules"][0]["match_mode"], "STRICT");
    assert_eq!(
        rule["rules"][0]["traffic_match_rule"]["arguments"][0]["key"],
        "x-pole-e2e-lane"
    );
}

#[test]
fn fault_detect_plan_uses_spec_like_http_probe_payload() {
    let plan = control_plane_plan(
        E2eCase::new("fault-detect", CaseKind::FaultDetect),
        &FlowResource::new("run-5", "fault-detect"),
    )
    .expect("fault detect should have a control-plane plan");
    let rule = &plan.create.body[0];

    assert_eq!(rule["rules"][0]["protocol"], "HTTP");
    assert_eq!(rule["rules"][0]["interval"], 1);
    assert_eq!(rule["rules"][0]["timeout"], 1);
    assert_eq!(rule["rules"][0]["http_config"]["method"], "GET");
}

#[test]
fn lossless_plan_uses_spec_like_delay_register_payload() {
    let plan = control_plane_plan(
        E2eCase::new("lossless", CaseKind::Lossless),
        &FlowResource::new("run-5", "lossless"),
    )
    .expect("lossless should have a control-plane plan");
    let rule = &plan.create.body[0];

    assert_eq!(rule["lossless_online"]["delay_register"]["enable"], true);
    assert_eq!(
        rule["lossless_online"]["delay_register"]["strategy"],
        "DELAY_BY_TIME"
    );
    assert_eq!(rule["lossless_online"]["readiness"]["enable"], true);
    assert_eq!(rule["lossless_offline"]["enable"], true);
}

#[test]
fn mirror_plan_uses_spec_like_http_mirror_payload() {
    let plan = control_plane_plan(
        E2eCase::new("mirror", CaseKind::Mirror),
        &FlowResource::new("run-5", "mirror"),
    )
    .expect("mirror should have a control-plane plan");
    let rule = &plan.create.body[0];

    assert_eq!(rule["rules"][0]["mirror_percent"], 100);
    assert_eq!(rule["callee"]["service"], "e2e-run-5-mirror");
    assert_eq!(
        rule["rules"][0]["traffic_match_rule"]["arguments"][0]["key"],
        "x-pole-e2e-mirror"
    );
    assert_eq!(
        rule["rules"][0]["destination"]["service"],
        "e2e-run-5-mirror-shadow"
    );
}

#[test]
fn non_governance_cases_do_not_have_control_plane_rule_plan() {
    let plan = control_plane_plan(
        E2eCase::new("connectivity", CaseKind::Connectivity),
        &FlowResource::new("run-3", "connectivity"),
    );

    assert!(plan.is_none());
}
