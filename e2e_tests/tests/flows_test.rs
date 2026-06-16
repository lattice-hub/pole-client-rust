use e2e_tests::flows::{config_flow_inputs, discovery_flow_inputs, FlowResource};

#[test]
fn discovery_flow_inputs_use_isolated_resource_names() {
    let resource = FlowResource::new("run-123", "service-discovery");
    let inputs = discovery_flow_inputs(&resource);

    assert_eq!(inputs.namespace, "e2e-run-123");
    assert_eq!(inputs.service, "e2e-run-123-service-discovery");
    assert_eq!(inputs.register.namespace, inputs.namespace);
    assert_eq!(inputs.register.service, inputs.service);
    assert_eq!(inputs.register.ip, "127.0.0.1");
    assert_eq!(inputs.register.port, 18080);
    assert_eq!(inputs.register.weight, 100);
    assert!(!inputs.register.auto_heartbeat);
    assert_eq!(inputs.get_all.namespace, inputs.namespace);
    assert_eq!(inputs.get_one.service, inputs.service);
    assert_eq!(inputs.deregister.port, inputs.register.port);
}

#[test]
fn config_flow_inputs_use_upsert_publish_contract() {
    let resource = FlowResource::new("run-123", "config-center");
    let inputs = config_flow_inputs(&resource);

    assert_eq!(inputs.namespace, "e2e-run-123");
    assert_eq!(inputs.group, "e2e-run-123-config-center");
    assert_eq!(inputs.file.name, "app.yaml");
    assert!(inputs.file.content.contains("run_id: run-123"));
    assert_eq!(
        inputs.upsert.release_name,
        "e2e-run-123-config-center-release"
    );
    assert_eq!(inputs.upsert.config_file.name, inputs.file.name);
    assert_eq!(inputs.get.namespace, inputs.namespace);
    assert_eq!(inputs.get.group, inputs.group);
    assert_eq!(inputs.get.file, inputs.file.name);
}
