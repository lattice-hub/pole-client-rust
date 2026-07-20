use std::{collections::HashMap, time::Duration};

use pole_rust::{
    config::req::{GetConfigFileRequest, UpsertAndPublishConfigFileRequest},
    core::model::{
        config::ConfigFile,
        loadbalance::Criteria,
        naming::{Location, ServiceInfo, ServiceInstances},
        router::{RouteInfo, RouterChain, DEFAULT_ROUTER_LANE, DEFAULT_ROUTER_RULE},
        ArgumentType,
    },
    discovery::req::{
        GetAllInstanceRequest, GetOneInstanceRequest, GetServiceRuleRequest,
        InstanceDeregisterRequest, InstanceRegisterRequest, ServiceRuleType,
    },
    ratelimit::req::QuotaRequest,
    router::req::ProcessRouteRequest,
    traffic::policy::req::MirrorRequest,
};

pub const MOCK_TRAFFIC_HEADER: &str = "x-pole-e2e";
pub const MOCK_TRAFFIC_HEADER_VALUE: &str = "mock";
pub const SECURITY_TRAFFIC_HEADER: &str = "x-pole-e2e-deny";
pub const SECURITY_TRAFFIC_HEADER_VALUE: &str = "true";
pub const RATELIMIT_TRAFFIC_HEADER: &str = "x-pole-e2e-ratelimit";
pub const RATELIMIT_TRAFFIC_HEADER_VALUE: &str = "limited";
pub const ROUTING_TRAFFIC_HEADER: &str = "x-pole-e2e-route";
pub const ROUTING_TRAFFIC_HEADER_VALUE: &str = "primary";
pub const LANE_TRAFFIC_HEADER: &str = "x-pole-e2e-lane";
pub const LANE_TRAFFIC_HEADER_VALUE: &str = "blue";
pub const MIRROR_TRAFFIC_HEADER: &str = "x-pole-e2e-mirror";
pub const MIRROR_TRAFFIC_HEADER_VALUE: &str = "true";
pub const MIRROR_REQUEST_PATH: &str = "/e2e/mirror";
pub const MIRROR_REQUEST_BODY: &str = "{\"mirror\":true}";
pub const FAULT_DETECT_PROBE_PATH: &str = "/health";
pub const FAULT_DETECT_PROBE_HEADER: &str = "x-pole-e2e";
pub const FAULT_DETECT_PROBE_PORT: u32 = 18080;
pub const LOSSLESS_INSTANCE_PORT: u32 = 19081;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowResource {
    pub run_id: String,
    pub case_name: String,
}

impl FlowResource {
    pub fn new(run_id: impl Into<String>, case_name: impl Into<String>) -> Self {
        Self {
            run_id: run_id.into(),
            case_name: case_name.into(),
        }
    }

    pub fn namespace(&self) -> String {
        format!("e2e-{}", normalize(&self.run_id))
    }

    pub fn name(&self) -> String {
        format!("{}-{}", self.namespace(), normalize(&self.case_name))
    }
}

pub struct DiscoveryFlowInputs {
    pub namespace: String,
    pub service: String,
    pub register: InstanceRegisterRequest,
    pub get_all: GetAllInstanceRequest,
    pub get_one: GetOneInstanceRequest,
    pub deregister: InstanceDeregisterRequest,
}

pub fn discovery_flow_inputs(resource: &FlowResource) -> DiscoveryFlowInputs {
    let namespace = resource.namespace();
    let service = resource.name();
    let timeout = Duration::from_secs(3);
    let metadata = HashMap::from([
        ("e2e".to_string(), "true".to_string()),
        ("run_id".to_string(), resource.run_id.clone()),
    ]);
    let register = InstanceRegisterRequest {
        flow_id: format!("{}-register", resource.run_id),
        timeout,
        id: None,
        namespace: namespace.clone(),
        service: service.clone(),
        ip: "127.0.0.1".to_string(),
        port: 18080,
        vpc_id: "default".to_string(),
        version: "v1".to_string(),
        protocol: "http".to_string(),
        health: true,
        isolated: false,
        weight: 100,
        priority: 0,
        metadata,
        location: Location {
            region: "e2e-region".to_string(),
            zone: "e2e-zone".to_string(),
            campus: "e2e-campus".to_string(),
        },
        ttl: 5,
        auto_heartbeat: false,
    };

    DiscoveryFlowInputs {
        namespace: namespace.clone(),
        service: service.clone(),
        get_all: GetAllInstanceRequest {
            flow_id: format!("{}-get-all", resource.run_id),
            timeout,
            service: service.clone(),
            namespace: namespace.clone(),
        },
        get_one: GetOneInstanceRequest {
            flow_id: format!("{}-get-one", resource.run_id),
            timeout,
            service: service.clone(),
            namespace: namespace.clone(),
            criteria: Criteria {
                policy: "weightedRandom".to_string(),
                hash_key: resource.run_id.clone(),
            },
            route_info: RouteInfo::default(),
        },
        deregister: InstanceDeregisterRequest {
            flow_id: format!("{}-deregister", resource.run_id),
            timeout,
            namespace,
            service,
            ip: register.ip.clone(),
            port: register.port,
            vpc_id: register.vpc_id.clone(),
        },
        register,
    }
}

pub struct ConfigFlowInputs {
    pub namespace: String,
    pub group: String,
    pub file: ConfigFile,
    pub upsert: UpsertAndPublishConfigFileRequest,
    pub get: GetConfigFileRequest,
}

pub struct GovernanceFlowInputs {
    pub namespace: String,
    pub service: String,
    pub get_one: GetOneInstanceRequest,
}

pub struct RateLimitFlowInputs {
    pub namespace: String,
    pub service: String,
    pub quota: QuotaRequest,
}

pub struct InstanceRouteFlowInputs {
    pub namespace: String,
    pub service: String,
    pub register_instances: Vec<InstanceRegisterRequest>,
    pub get_one: GetOneInstanceRequest,
    pub deregister_instances: Vec<InstanceDeregisterRequest>,
}

pub struct MirrorFlowInputs {
    pub namespace: String,
    pub service: String,
    pub shadow_service: String,
    pub register_shadow: InstanceRegisterRequest,
    pub deregister_shadow: InstanceDeregisterRequest,
    pub route_request: ProcessRouteRequest,
}

pub struct FaultDetectFlowInputs {
    pub namespace: String,
    pub service: String,
    pub get_rule: GetServiceRuleRequest,
    pub register_target: InstanceRegisterRequest,
    pub get_all: GetAllInstanceRequest,
    pub deregister_target: InstanceDeregisterRequest,
    pub probe_path: &'static str,
    pub probe_header: (&'static str, String),
}

pub struct LosslessFlowInputs {
    pub namespace: String,
    pub service: String,
    pub get_rule: GetServiceRuleRequest,
    pub instance_ip: &'static str,
    pub instance_port: u32,
}

pub fn mock_flow_inputs(resource: &FlowResource) -> GovernanceFlowInputs {
    governance_flow_inputs(resource, mock_traffic_label_provider)
}

pub fn security_flow_inputs(resource: &FlowResource) -> GovernanceFlowInputs {
    governance_flow_inputs(resource, security_traffic_label_provider)
}

pub fn ratelimit_flow_inputs(resource: &FlowResource) -> RateLimitFlowInputs {
    let namespace = resource.namespace();
    let service = resource.name();

    RateLimitFlowInputs {
        namespace: namespace.clone(),
        service: service.clone(),
        quota: QuotaRequest {
            flow_id: format!("{}-quota", resource.run_id),
            timeout: Duration::from_secs(3),
            service,
            namespace,
            method: "GET /e2e".to_string(),
            traffic_label_provider: ratelimit_traffic_label_provider,
        },
    }
}

pub fn routing_flow_inputs(resource: &FlowResource) -> InstanceRouteFlowInputs {
    instance_route_flow_inputs(
        resource,
        "e2e-route",
        ROUTING_TRAFFIC_HEADER_VALUE,
        "secondary",
        routing_traffic_label_provider,
        DEFAULT_ROUTER_RULE,
    )
}

pub fn lane_flow_inputs(resource: &FlowResource) -> InstanceRouteFlowInputs {
    instance_route_flow_inputs(
        resource,
        "lane",
        LANE_TRAFFIC_HEADER_VALUE,
        "green",
        lane_traffic_label_provider,
        DEFAULT_ROUTER_LANE,
    )
}

pub fn mirror_flow_inputs(resource: &FlowResource, shadow_port: u32) -> MirrorFlowInputs {
    let namespace = resource.namespace();
    let service = resource.name();
    let shadow_service = format!("{service}-shadow");
    let register_shadow = InstanceRegisterRequest {
        flow_id: format!("{}-register-shadow", resource.run_id),
        timeout: Duration::from_secs(3),
        id: None,
        namespace: namespace.clone(),
        service: shadow_service.clone(),
        ip: "127.0.0.1".to_string(),
        port: shadow_port,
        vpc_id: "default".to_string(),
        version: "v1".to_string(),
        protocol: "http".to_string(),
        health: true,
        isolated: false,
        weight: 100,
        priority: 0,
        metadata: HashMap::from([
            ("e2e".to_string(), "true".to_string()),
            ("run_id".to_string(), resource.run_id.clone()),
        ]),
        location: Location {
            region: "e2e-region".to_string(),
            zone: "e2e-zone".to_string(),
            campus: "e2e-campus".to_string(),
        },
        ttl: 5,
        auto_heartbeat: false,
    };
    let deregister_shadow = InstanceDeregisterRequest {
        flow_id: format!("{}-deregister-shadow", resource.run_id),
        timeout: register_shadow.timeout,
        namespace: namespace.clone(),
        service: shadow_service.clone(),
        ip: register_shadow.ip.clone(),
        port: register_shadow.port,
        vpc_id: register_shadow.vpc_id.clone(),
    };
    let route_info = RouteInfo {
        callee: pole_rust::core::model::naming::ServiceKey {
            namespace: namespace.clone(),
            name: service.clone(),
        },
        traffic_label_provider: mirror_traffic_label_provider,
        ..Default::default()
    };
    let route_request = ProcessRouteRequest {
        service_instances: ServiceInstances::new(
            ServiceInfo {
                namespace: namespace.clone(),
                name: service.clone(),
                ..Default::default()
            },
            Vec::new(),
        ),
        route_info,
        mirror_request: Some(MirrorRequest {
            method: "POST".to_string(),
            path: MIRROR_REQUEST_PATH.to_string(),
            headers: HashMap::from([("x-e2e".to_string(), "mirror".to_string())]),
            body: MIRROR_REQUEST_BODY.as_bytes().to_vec(),
        }),
    };

    MirrorFlowInputs {
        namespace,
        service,
        shadow_service,
        register_shadow,
        deregister_shadow,
        route_request,
    }
}

pub fn fault_detect_flow_inputs(resource: &FlowResource) -> FaultDetectFlowInputs {
    let namespace = resource.namespace();
    let service = resource.name();
    let register_target = InstanceRegisterRequest {
        flow_id: format!("{}-faultdetect-register", resource.run_id),
        timeout: Duration::from_secs(3),
        id: None,
        namespace: namespace.clone(),
        service: service.clone(),
        ip: "127.0.0.1".to_string(),
        port: FAULT_DETECT_PROBE_PORT,
        vpc_id: "default".to_string(),
        version: "v1".to_string(),
        protocol: "http".to_string(),
        health: true,
        isolated: false,
        weight: 100,
        priority: 0,
        metadata: HashMap::from([
            ("e2e".to_string(), "true".to_string()),
            ("run_id".to_string(), resource.run_id.clone()),
        ]),
        location: Location {
            region: "e2e-region".to_string(),
            zone: "e2e-zone".to_string(),
            campus: "e2e-campus".to_string(),
        },
        ttl: 5,
        auto_heartbeat: false,
    };

    FaultDetectFlowInputs {
        namespace: namespace.clone(),
        service: service.clone(),
        get_rule: GetServiceRuleRequest {
            namespace: namespace.clone(),
            service: service.clone(),
            rule_type: ServiceRuleType::FaultDetector,
            timeout: Duration::from_secs(3),
        },
        get_all: GetAllInstanceRequest {
            flow_id: format!("{}-faultdetect-get-all", resource.run_id),
            timeout: Duration::from_secs(3),
            service: service.clone(),
            namespace: namespace.clone(),
        },
        deregister_target: InstanceDeregisterRequest {
            flow_id: format!("{}-faultdetect-deregister", resource.run_id),
            timeout: register_target.timeout,
            namespace,
            service,
            ip: register_target.ip.clone(),
            port: register_target.port,
            vpc_id: register_target.vpc_id.clone(),
        },
        register_target,
        probe_path: FAULT_DETECT_PROBE_PATH,
        probe_header: (FAULT_DETECT_PROBE_HEADER, resource.run_id.clone()),
    }
}

pub fn lossless_flow_inputs(resource: &FlowResource) -> LosslessFlowInputs {
    let namespace = resource.namespace();
    let service = resource.name();

    LosslessFlowInputs {
        namespace: namespace.clone(),
        service: service.clone(),
        get_rule: GetServiceRuleRequest {
            namespace,
            service,
            rule_type: ServiceRuleType::Lossless,
            timeout: Duration::from_secs(3),
        },
        instance_ip: "127.0.0.1",
        instance_port: LOSSLESS_INSTANCE_PORT,
    }
}

fn instance_route_flow_inputs(
    resource: &FlowResource,
    label_key: &str,
    selected_label: &str,
    other_label: &str,
    traffic_label_provider: fn(ArgumentType, &str) -> Option<String>,
    router_name: &str,
) -> InstanceRouteFlowInputs {
    let namespace = resource.namespace();
    let service = resource.name();
    let register_instances = vec![
        register_instance(
            resource,
            &namespace,
            &service,
            18081,
            label_key,
            selected_label,
        ),
        register_instance(
            resource,
            &namespace,
            &service,
            18082,
            label_key,
            other_label,
        ),
    ];
    let deregister_instances = register_instances
        .iter()
        .map(|register| InstanceDeregisterRequest {
            flow_id: format!("{}-deregister-{}", resource.run_id, register.port),
            timeout: register.timeout,
            namespace: register.namespace.clone(),
            service: register.service.clone(),
            ip: register.ip.clone(),
            port: register.port,
            vpc_id: register.vpc_id.clone(),
        })
        .collect();
    let route_info = RouteInfo {
        chain: RouterChain {
            core: vec![router_name.to_string()],
            ..Default::default()
        },
        traffic_label_provider,
        ..Default::default()
    };

    InstanceRouteFlowInputs {
        namespace: namespace.clone(),
        service: service.clone(),
        register_instances,
        get_one: GetOneInstanceRequest {
            flow_id: format!("{}-get-one", resource.run_id),
            timeout: Duration::from_secs(3),
            service,
            namespace,
            criteria: Criteria {
                policy: "weightedRandom".to_string(),
                hash_key: resource.run_id.clone(),
            },
            route_info,
        },
        deregister_instances,
    }
}

fn register_instance(
    resource: &FlowResource,
    namespace: &str,
    service: &str,
    port: u32,
    label_key: &str,
    label_value: &str,
) -> InstanceRegisterRequest {
    InstanceRegisterRequest {
        flow_id: format!("{}-register-{port}", resource.run_id),
        timeout: Duration::from_secs(3),
        id: None,
        namespace: namespace.to_string(),
        service: service.to_string(),
        ip: "127.0.0.1".to_string(),
        port,
        vpc_id: "default".to_string(),
        version: "v1".to_string(),
        protocol: "http".to_string(),
        health: true,
        isolated: false,
        weight: 100,
        priority: 0,
        metadata: HashMap::from([
            ("e2e".to_string(), "true".to_string()),
            ("run_id".to_string(), resource.run_id.clone()),
            (label_key.to_string(), label_value.to_string()),
        ]),
        location: Location {
            region: "e2e-region".to_string(),
            zone: "e2e-zone".to_string(),
            campus: "e2e-campus".to_string(),
        },
        ttl: 5,
        auto_heartbeat: false,
    }
}

fn governance_flow_inputs(
    resource: &FlowResource,
    traffic_label_provider: fn(ArgumentType, &str) -> Option<String>,
) -> GovernanceFlowInputs {
    let namespace = resource.namespace();
    let service = resource.name();
    let timeout = Duration::from_secs(3);
    let route_info = RouteInfo {
        traffic_label_provider,
        ..Default::default()
    };

    GovernanceFlowInputs {
        namespace: namespace.clone(),
        service: service.clone(),
        get_one: GetOneInstanceRequest {
            flow_id: format!("{}-get-one", resource.run_id),
            timeout,
            service,
            namespace,
            criteria: Criteria {
                policy: "weightedRandom".to_string(),
                hash_key: resource.run_id.clone(),
            },
            route_info,
        },
    }
}

fn mock_traffic_label_provider(arg_type: ArgumentType, key: &str) -> Option<String> {
    if arg_type == ArgumentType::Header && key.eq_ignore_ascii_case(MOCK_TRAFFIC_HEADER) {
        return Some(MOCK_TRAFFIC_HEADER_VALUE.to_string());
    }
    None
}

fn security_traffic_label_provider(arg_type: ArgumentType, key: &str) -> Option<String> {
    if arg_type == ArgumentType::Header && key.eq_ignore_ascii_case(SECURITY_TRAFFIC_HEADER) {
        return Some(SECURITY_TRAFFIC_HEADER_VALUE.to_string());
    }
    None
}

fn ratelimit_traffic_label_provider(arg_type: ArgumentType, key: &str) -> Option<String> {
    if arg_type == ArgumentType::Header && key.eq_ignore_ascii_case(RATELIMIT_TRAFFIC_HEADER) {
        return Some(RATELIMIT_TRAFFIC_HEADER_VALUE.to_string());
    }
    None
}

fn routing_traffic_label_provider(arg_type: ArgumentType, key: &str) -> Option<String> {
    if arg_type == ArgumentType::Header && key.eq_ignore_ascii_case(ROUTING_TRAFFIC_HEADER) {
        return Some(ROUTING_TRAFFIC_HEADER_VALUE.to_string());
    }
    None
}

fn lane_traffic_label_provider(arg_type: ArgumentType, key: &str) -> Option<String> {
    if arg_type == ArgumentType::Header && key.eq_ignore_ascii_case(LANE_TRAFFIC_HEADER) {
        return Some(LANE_TRAFFIC_HEADER_VALUE.to_string());
    }
    None
}

fn mirror_traffic_label_provider(arg_type: ArgumentType, key: &str) -> Option<String> {
    if arg_type == ArgumentType::Header && key.eq_ignore_ascii_case(MIRROR_TRAFFIC_HEADER) {
        return Some(MIRROR_TRAFFIC_HEADER_VALUE.to_string());
    }
    None
}

pub fn config_flow_inputs(resource: &FlowResource) -> ConfigFlowInputs {
    let namespace = resource.namespace();
    let group = resource.name();
    let file = ConfigFile {
        namespace: namespace.clone(),
        group: group.clone(),
        name: "app.yaml".to_string(),
        content: format!(
            "run_id: {}\ncase: {}\n",
            resource.run_id, resource.case_name
        ),
        labels: HashMap::from([
            ("e2e".to_string(), "true".to_string()),
            ("run_id".to_string(), resource.run_id.clone()),
        ]),
        ..Default::default()
    };
    let timeout = Duration::from_secs(3);

    ConfigFlowInputs {
        namespace: namespace.clone(),
        group: group.clone(),
        upsert: UpsertAndPublishConfigFileRequest {
            flow_id: format!("{}-upsert-config", resource.run_id),
            timeout,
            release_name: format!("{group}-release"),
            md5: String::new(),
            config_file: file.clone(),
        },
        get: GetConfigFileRequest {
            namespace,
            group,
            file: file.name.clone(),
            timeout,
        },
        file,
    }
}

fn normalize(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}
