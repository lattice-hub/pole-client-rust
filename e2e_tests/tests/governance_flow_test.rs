use e2e_tests::flows::{
    fault_detect_flow_inputs, lane_flow_inputs, lossless_flow_inputs, mirror_flow_inputs,
    mock_flow_inputs, ratelimit_flow_inputs, routing_flow_inputs, security_flow_inputs,
    FlowResource,
};
use pole_rust::core::model::ArgumentType;
use pole_rust::discovery::req::ServiceRuleType;

#[test]
fn mock_flow_input_carries_header_that_matches_mock_payload() {
    let resource = FlowResource::new("run-4", "mock");
    let inputs = mock_flow_inputs(&resource);

    assert_eq!(inputs.namespace, "e2e-run-4");
    assert_eq!(inputs.service, "e2e-run-4-mock");
    assert_eq!(inputs.get_one.namespace, "e2e-run-4");
    assert_eq!(inputs.get_one.service, "e2e-run-4-mock");
    assert_eq!(
        (inputs.get_one.route_info.traffic_label_provider)(ArgumentType::Header, "x-pole-e2e"),
        Some("mock".to_string())
    );
}

#[test]
fn security_flow_input_carries_header_that_matches_deny_payload() {
    let resource = FlowResource::new("run-4", "auth-security");
    let inputs = security_flow_inputs(&resource);

    assert_eq!(inputs.namespace, "e2e-run-4");
    assert_eq!(inputs.service, "e2e-run-4-auth-security");
    assert_eq!(inputs.get_one.namespace, "e2e-run-4");
    assert_eq!(inputs.get_one.service, "e2e-run-4-auth-security");
    assert_eq!(
        (inputs.get_one.route_info.traffic_label_provider)(ArgumentType::Header, "x-pole-e2e-deny"),
        Some("true".to_string())
    );
}

#[test]
fn ratelimit_flow_input_carries_header_that_matches_local_qps_payload() {
    let resource = FlowResource::new("run-4", "ratelimit");
    let inputs = ratelimit_flow_inputs(&resource);

    assert_eq!(inputs.namespace, "e2e-run-4");
    assert_eq!(inputs.service, "e2e-run-4-ratelimit");
    assert_eq!(inputs.quota.namespace, "e2e-run-4");
    assert_eq!(inputs.quota.service, "e2e-run-4-ratelimit");
    assert_eq!(
        (inputs.quota.traffic_label_provider)(ArgumentType::Header, "x-pole-e2e-ratelimit"),
        Some("limited".to_string())
    );
}

#[test]
fn routing_flow_registers_primary_and_secondary_instances_and_requests_primary_route() {
    let resource = FlowResource::new("run-4", "routing");
    let inputs = routing_flow_inputs(&resource);

    assert_eq!(inputs.namespace, "e2e-run-4");
    assert_eq!(inputs.service, "e2e-run-4-routing");
    assert_eq!(inputs.register_instances.len(), 2);
    assert_eq!(
        inputs.register_instances[0].metadata.get("e2e-route"),
        Some(&"primary".to_string())
    );
    assert_eq!(
        inputs.register_instances[1].metadata.get("e2e-route"),
        Some(&"secondary".to_string())
    );
    assert_eq!(
        (inputs.get_one.route_info.traffic_label_provider)(
            ArgumentType::Header,
            "x-pole-e2e-route"
        ),
        Some("primary".to_string())
    );
}

#[test]
fn lane_flow_registers_blue_and_green_instances_and_requests_blue_lane() {
    let resource = FlowResource::new("run-4", "lane-routing");
    let inputs = lane_flow_inputs(&resource);

    assert_eq!(inputs.namespace, "e2e-run-4");
    assert_eq!(inputs.service, "e2e-run-4-lane-routing");
    assert_eq!(inputs.register_instances.len(), 2);
    assert_eq!(
        inputs.register_instances[0].metadata.get("lane"),
        Some(&"blue".to_string())
    );
    assert_eq!(
        inputs.register_instances[1].metadata.get("lane"),
        Some(&"green".to_string())
    );
    assert_eq!(
        (inputs.get_one.route_info.traffic_label_provider)(ArgumentType::Header, "x-pole-e2e-lane"),
        Some("blue".to_string())
    );
}

#[test]
fn mirror_flow_registers_shadow_instance_and_carries_mirror_request() {
    let resource = FlowResource::new("run-4", "mirror");
    let inputs = mirror_flow_inputs(&resource, 19090);

    assert_eq!(inputs.namespace, "e2e-run-4");
    assert_eq!(inputs.service, "e2e-run-4-mirror");
    assert_eq!(inputs.shadow_service, "e2e-run-4-mirror-shadow");
    assert_eq!(inputs.register_shadow.service, "e2e-run-4-mirror-shadow");
    assert_eq!(inputs.register_shadow.port, 19090);
    assert_eq!(inputs.register_shadow.protocol, "http");
    assert_eq!(
        (inputs.route_request.route_info.traffic_label_provider)(
            ArgumentType::Header,
            "x-pole-e2e-mirror"
        ),
        Some("true".to_string())
    );
    assert_eq!(
        inputs.route_request.mirror_request.as_ref().unwrap().path,
        "/e2e/mirror"
    );
    assert_eq!(
        inputs.route_request.mirror_request.as_ref().unwrap().body,
        b"{\"mirror\":true}".to_vec()
    );
}

#[test]
fn fault_detect_flow_fetches_fault_detector_rule_for_target_service() {
    let resource = FlowResource::new("run-4", "fault-detect");
    let inputs = fault_detect_flow_inputs(&resource);

    assert_eq!(inputs.namespace, "e2e-run-4");
    assert_eq!(inputs.service, "e2e-run-4-fault-detect");
    assert_eq!(inputs.get_rule.namespace, "e2e-run-4");
    assert_eq!(inputs.get_rule.service, "e2e-run-4-fault-detect");
    assert_eq!(inputs.register_target.namespace, "e2e-run-4");
    assert_eq!(inputs.register_target.service, "e2e-run-4-fault-detect");
    assert_eq!(inputs.register_target.ip, "127.0.0.1");
    assert_eq!(inputs.register_target.port, 18080);
    assert_eq!(inputs.get_all.service, "e2e-run-4-fault-detect");
    assert_eq!(inputs.deregister_target.port, 18080);
    assert_eq!(inputs.probe_path, "/health");
    assert_eq!(inputs.probe_header, ("x-pole-e2e", "run-4".to_string()));
    assert!(matches!(
        inputs.get_rule.rule_type,
        ServiceRuleType::FaultDetector
    ));
}

#[test]
fn lossless_flow_fetches_lossless_rule_for_target_service() {
    let resource = FlowResource::new("run-4", "lossless");
    let inputs = lossless_flow_inputs(&resource);

    assert_eq!(inputs.namespace, "e2e-run-4");
    assert_eq!(inputs.service, "e2e-run-4-lossless");
    assert_eq!(inputs.get_rule.namespace, "e2e-run-4");
    assert_eq!(inputs.get_rule.service, "e2e-run-4-lossless");
    assert!(matches!(
        inputs.get_rule.rule_type,
        ServiceRuleType::Lossless
    ));
    assert_eq!(inputs.instance_ip, "127.0.0.1");
    assert_eq!(inputs.instance_port, 19081);
}
