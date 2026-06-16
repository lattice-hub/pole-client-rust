use serde_json::{json, Value};

use crate::{
    cases::{CaseKind, E2eCase},
    console::{ConsoleClient, ControlPlaneAction},
    flows::{
        FlowResource, MOCK_TRAFFIC_HEADER, MOCK_TRAFFIC_HEADER_VALUE, SECURITY_TRAFFIC_HEADER,
        SECURITY_TRAFFIC_HEADER_VALUE,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlPlanePlan {
    pub create: ControlPlaneAction,
    pub publish: ControlPlaneAction,
    pub cleanup_actions: Vec<ControlPlaneAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlPlaneExecutionStatus {
    Passed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlPlaneStepReport {
    pub name: String,
    pub path: String,
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlPlaneExecution {
    pub status: ControlPlaneExecutionStatus,
    pub steps: Vec<ControlPlaneStepReport>,
}

impl ControlPlaneExecution {
    pub fn message(&self) -> String {
        self.steps
            .iter()
            .filter(|step| !step.success)
            .map(|step| format!("{} {}: {}", step.name, step.path, step.message))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

pub async fn execute_control_plane_plan(
    client: &ConsoleClient,
    plan: &ControlPlanePlan,
) -> ControlPlaneExecution {
    let mut steps = execute_control_plane_setup(client, plan).await.steps;
    steps.extend(execute_control_plane_cleanup(client, plan).await.steps);

    ControlPlaneExecution {
        status: execution_status(&steps),
        steps,
    }
}

pub async fn execute_control_plane_setup(
    client: &ConsoleClient,
    plan: &ControlPlanePlan,
) -> ControlPlaneExecution {
    let mut steps = Vec::new();
    let mut failed = false;

    let create = execute_step(client, "create", &plan.create).await;
    failed |= !create.success;
    steps.push(create);

    if !failed {
        let publish = execute_step(client, "publish", &plan.publish).await;
        failed |= !publish.success;
        steps.push(publish);
    }

    ControlPlaneExecution {
        status: if failed {
            ControlPlaneExecutionStatus::Failed
        } else {
            ControlPlaneExecutionStatus::Passed
        },
        steps,
    }
}

pub async fn execute_control_plane_cleanup(
    client: &ConsoleClient,
    plan: &ControlPlanePlan,
) -> ControlPlaneExecution {
    let mut steps = Vec::new();
    for action in &plan.cleanup_actions {
        steps.push(execute_step(client, "cleanup", action).await);
    }

    ControlPlaneExecution {
        status: execution_status(&steps),
        steps,
    }
}

fn execution_status(steps: &[ControlPlaneStepReport]) -> ControlPlaneExecutionStatus {
    if steps.iter().any(|step| !step.success) {
        ControlPlaneExecutionStatus::Failed
    } else {
        ControlPlaneExecutionStatus::Passed
    }
}

async fn execute_step(
    client: &ConsoleClient,
    name: &str,
    action: &ControlPlaneAction,
) -> ControlPlaneStepReport {
    match client.execute_action(action).await {
        Ok(resp) => ControlPlaneStepReport {
            name: name.to_string(),
            path: action.path.clone(),
            success: true,
            message: format!("HTTP {}", resp.status),
        },
        Err(err) => ControlPlaneStepReport {
            name: name.to_string(),
            path: action.path.clone(),
            success: false,
            message: err.to_string(),
        },
    }
}

pub fn control_plane_plan(case: E2eCase, resource: &FlowResource) -> Option<ControlPlanePlan> {
    let endpoints = endpoints(case.kind())?;
    let rule = rule_body(case.kind(), resource);
    let release = json!([{
        "rule_name": resource.name(),
        "rule_id": resource.name(),
        "name": format!("{}-release", resource.name()),
        "namespace": resource.namespace(),
        "e2e": true,
    }]);

    Some(ControlPlanePlan {
        create: ControlPlaneAction::post(endpoints.create, json!([rule])),
        publish: ControlPlaneAction::post(endpoints.publish, release.clone()),
        cleanup_actions: vec![
            ControlPlaneAction::delete(endpoints.delete_release, release),
            ControlPlaneAction::delete(
                endpoints.delete_rule,
                json!([{
                    "id": resource.name(),
                    "name": resource.name(),
                    "namespace": resource.namespace(),
                    "e2e": true,
                }]),
            ),
        ],
    })
}

#[derive(Debug, Clone, Copy)]
struct Endpoints {
    create: &'static str,
    publish: &'static str,
    delete_release: &'static str,
    delete_rule: &'static str,
}

fn endpoints(kind: CaseKind) -> Option<Endpoints> {
    Some(match kind {
        CaseKind::Routing => Endpoints {
            create: "/naming/v1/routings",
            publish: "/naming/v1/routings/releases",
            delete_release: "/naming/v1/routings/releases/delete",
            delete_rule: "/naming/v1/routings/delete",
        },
        CaseKind::RateLimit => Endpoints {
            create: "/naming/v1/ratelimits",
            publish: "/naming/v1/ratelimits/releases",
            delete_release: "/naming/v1/ratelimits/releases/delete",
            delete_rule: "/naming/v1/ratelimits/delete",
        },
        CaseKind::CircuitBreaker => Endpoints {
            create: "/naming/v1/circuitbreakers",
            publish: "/naming/v1/circuitbreakers/releases",
            delete_release: "/naming/v1/circuitbreakers/releases/delete",
            delete_rule: "/naming/v1/circuitbreakers/delete",
        },
        CaseKind::LaneRouting => Endpoints {
            create: "/naming/v1/lane/groups",
            publish: "/naming/v1/lane/groups/releases",
            delete_release: "/naming/v1/lane/groups/releases/delete",
            delete_rule: "/naming/v1/lane/groups/delete",
        },
        CaseKind::FaultDetect => Endpoints {
            create: "/naming/v1/faultdetectors",
            publish: "/naming/v1/faultdetectors/releases",
            delete_release: "/naming/v1/faultdetectors/releases/delete",
            delete_rule: "/naming/v1/faultdetectors/delete",
        },
        CaseKind::Lossless => Endpoints {
            create: "/naming/v1/lossless",
            publish: "/naming/v1/lossless/releases",
            delete_release: "/naming/v1/lossless/releases/delete",
            delete_rule: "/naming/v1/lossless/delete",
        },
        CaseKind::Mirror => Endpoints {
            create: "/naming/v1/traffic/mirrors",
            publish: "/naming/v1/traffic/mirrors/releases",
            delete_release: "/naming/v1/traffic/mirrors/releases/delete",
            delete_rule: "/naming/v1/traffic/mirrors/delete",
        },
        CaseKind::AuthSecurity => Endpoints {
            create: "/naming/v1/traffic/security",
            publish: "/naming/v1/traffic/security/releases",
            delete_release: "/naming/v1/traffic/security/releases/delete",
            delete_rule: "/naming/v1/traffic/security/delete",
        },
        CaseKind::Mock => Endpoints {
            create: "/naming/v1/traffic/mocks",
            publish: "/naming/v1/traffic/mocks/releases",
            delete_release: "/naming/v1/traffic/mocks/releases/delete",
            delete_rule: "/naming/v1/traffic/mocks/delete",
        },
        CaseKind::Connectivity | CaseKind::ServiceDiscovery | CaseKind::ConfigCenter => {
            return None
        }
    })
}

fn rule_body(kind: CaseKind, resource: &FlowResource) -> Value {
    match kind {
        CaseKind::Routing => return routing_rule_body(resource),
        CaseKind::RateLimit => return ratelimit_rule_body(resource),
        CaseKind::CircuitBreaker => return circuitbreaker_rule_body(resource),
        CaseKind::LaneRouting => return lane_rule_body(resource),
        CaseKind::FaultDetect => return fault_detect_rule_body(resource),
        CaseKind::Lossless => return lossless_rule_body(resource),
        CaseKind::Mirror => return mirror_rule_body(resource),
        CaseKind::Mock => return mock_rule_body(resource),
        CaseKind::AuthSecurity => return security_rule_body(resource),
        _ => {}
    }

    json!({
        "id": resource.name(),
        "name": resource.name(),
        "namespace": resource.namespace(),
        "service": resource.name(),
        "enable": true,
        "e2e": true,
        "case": format!("{kind:?}"),
        "labels": {
            "e2e": "true",
            "run_id": resource.run_id,
        }
    })
}

fn routing_rule_body(resource: &FlowResource) -> Value {
    json!({
        "id": resource.name(),
        "name": resource.name(),
        "namespace": resource.namespace(),
        "enable": true,
        "route_policy": "RulePolicy",
        "priority": 1,
        "description": "pole-client-rust e2e routing rule",
        "metadata": metadata(resource),
        "routing_config": {
            "rules": [{
                "name": resource.name(),
                "arguments": header_match(crate::flows::ROUTING_TRAFFIC_HEADER, crate::flows::ROUTING_TRAFFIC_HEADER_VALUE),
                "destinations": [{
                    "namespace": resource.namespace(),
                    "service": resource.name(),
                    "labels": {
                        "e2e-route": match_string("primary")
                    },
                    "priority": 0,
                    "weight": 100,
                    "name": "primary"
                }]
            }]
        }
    })
}

fn ratelimit_rule_body(resource: &FlowResource) -> Value {
    json!({
        "id": resource.name(),
        "name": resource.name(),
        "service": resource.name(),
        "namespace": resource.namespace(),
        "priority": 1,
        "type": "LOCAL",
        "disable": false,
        "metadata": metadata(resource),
        "rules": [{
            "name": resource.name(),
            "resource": "QPS",
            "arguments": [{
                "type": "HEADER",
                "key": crate::flows::RATELIMIT_TRAFFIC_HEADER,
                "value": match_string(crate::flows::RATELIMIT_TRAFFIC_HEADER_VALUE)
            }],
            "amounts": [{
                "max_amount": 1,
                "valid_duration": {
                    "seconds": 1
                },
                "precision": 1
            }],
            "max_queue_delay": 0,
            "failover": "FAILOVER_LOCAL",
            "action": "reject",
            "amount_mode": "GLOBAL_TOTAL",
            "disable": false
        }]
    })
}

fn circuitbreaker_rule_body(resource: &FlowResource) -> Value {
    json!({
        "id": resource.name(),
        "name": resource.name(),
        "namespace": resource.namespace(),
        "enable": true,
        "description": "pole-client-rust e2e circuit breaker rule",
        "level": "SERVICE",
        "priority": 1,
        "metadata": metadata(resource),
        "block_configs": [{
            "name": resource.name(),
            "trigger_conditions": [{
                "trigger_type": "CONSECUTIVE_ERROR",
                "error_count": 2,
                "interval": 1,
                "minimum_request": 2
            }]
        }],
        "recover_condition": {
            "sleep_window": 1,
            "consecutive_success": 1
        },
        "max_ejection_percent": 100
    })
}

fn lane_rule_body(resource: &FlowResource) -> Value {
    json!({
        "id": resource.name(),
        "name": resource.name(),
        "description": "pole-client-rust e2e lane rule",
        "metadata": metadata(resource),
        "destinations": [{
            "namespace": resource.namespace(),
            "service": resource.name(),
            "labels": {
                "lane": match_string("blue")
            },
            "priority": 0,
            "weight": 100,
            "name": "blue"
        }],
        "rules": [{
            "id": resource.name(),
            "name": resource.name(),
            "group_name": resource.name(),
            "traffic_match_rule": header_match(crate::flows::LANE_TRAFFIC_HEADER, crate::flows::LANE_TRAFFIC_HEADER_VALUE),
            "default_label_value": crate::flows::LANE_TRAFFIC_HEADER_VALUE,
            "enable": true,
            "match_mode": "STRICT",
            "priority": 1,
            "label_key": "lane"
        }]
    })
}

fn fault_detect_rule_body(resource: &FlowResource) -> Value {
    json!({
        "revision": resource.run_id,
        "metadata": metadata(resource),
        "rules": [{
            "id": resource.name(),
            "name": resource.name(),
            "namespace": resource.namespace(),
            "description": "pole-client-rust e2e fault detect rule",
            "target_service": {
                "namespace": resource.namespace(),
                "service": resource.name()
            },
            "interval": 1,
            "timeout": 1,
            "port": 18080,
            "protocol": "HTTP",
            "http_config": {
                "method": "GET",
                "url": "/health",
                "headers": [{
                    "key": "x-pole-e2e",
                    "value": resource.run_id
                }]
            },
            "priority": 1,
            "metadata": metadata(resource)
        }]
    })
}

fn lossless_rule_body(resource: &FlowResource) -> Value {
    json!({
        "id": resource.name(),
        "service": resource.name(),
        "namespace": resource.namespace(),
        "metadata": metadata(resource),
        "lossless_online": {
            "delay_register": {
                "enable": true,
                "strategy": "DELAY_BY_TIME",
                "interval_second": 1
            },
            "readiness": {
                "enable": true
            }
        },
        "lossless_offline": {
            "enable": true
        }
    })
}

fn mirror_rule_body(resource: &FlowResource) -> Value {
    json!({
        "id": resource.name(),
        "name": resource.name(),
        "description": "pole-client-rust e2e mirror rule",
        "namespace": resource.namespace(),
        "service": resource.name(),
        "enable": true,
        "priority": 1,
        "metadata": metadata(resource),
        "rules": [{
            "source": {
                "namespace": resource.namespace(),
                "service": resource.name(),
                "traffic_match_rule": header_match(crate::flows::MIRROR_TRAFFIC_HEADER, crate::flows::MIRROR_TRAFFIC_HEADER_VALUE)
            },
            "destination": {
                "namespace": resource.namespace(),
                "service": format!("{}-shadow", resource.name())
            },
            "mirror_percent": 100,
            "disable": false
        }]
    })
}

fn mock_rule_body(resource: &FlowResource) -> Value {
    json!({
        "id": resource.name(),
        "name": resource.name(),
        "description": "pole-client-rust e2e mock rule",
        "namespace": resource.namespace(),
        "service": resource.name(),
        "enable": true,
        "priority": 1,
        "metadata": metadata(resource),
        "rules": [{
            "source": {
                "namespace": resource.namespace(),
                "service": resource.name(),
                "traffic_match_rule": header_match(MOCK_TRAFFIC_HEADER, MOCK_TRAFFIC_HEADER_VALUE),
            },
            "response": {
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": format!("{{\"mocked\":true,\"service\":\"{}\"}}", resource.name()),
                "code": "E2E_MOCK",
                "message": "mocked by pole-client-rust e2e"
            },
            "mock_percent": 100,
            "disable": false
        }]
    })
}

fn security_rule_body(resource: &FlowResource) -> Value {
    json!({
        "id": resource.name(),
        "name": resource.name(),
        "namespace": resource.namespace(),
        "service": resource.name(),
        "description": "pole-client-rust e2e traffic security rule",
        "priority": 1,
        "enable": true,
        "default_action": "TRAFFIC_SECURITY_ALLOW",
        "metadata": metadata(resource),
        "policies": [{
            "traffic_match_rule": header_match(SECURITY_TRAFFIC_HEADER, SECURITY_TRAFFIC_HEADER_VALUE),
            "action": "TRAFFIC_SECURITY_DENY",
            "reject_effect": {
                "status_code": 403,
                "code": "E2E_DENIED",
                "message": "blocked by pole-client-rust e2e"
            }
        }]
    })
}

fn header_match(key: &str, value: &str) -> Value {
    json!({
        "arguments": [{
            "type": "HEADER",
            "key": key,
            "value": {
                "type": "EXACT",
                "value": value,
                "value_type": "TEXT"
            }
        }],
        "random_percent": 100,
        "matchMode": "AND"
    })
}

fn match_string(value: &str) -> Value {
    json!({
        "type": "EXACT",
        "value": value,
        "value_type": "TEXT"
    })
}

fn metadata(resource: &FlowResource) -> Value {
    json!({
        "e2e": "true",
        "run_id": resource.run_id,
    })
}
