use std::{
    error::Error,
    fmt,
    future::Future,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use pole_rust::core::context::SDKContext;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;

use crate::{
    assertions::{
        assert_circuitbreaker_open, assert_fault_detect_request, assert_instance_metadata,
        assert_lossless_behavior, assert_mirror_request, assert_mock_response,
        assert_ratelimit_quota, assert_security_denied, LosslessBehaviorResult,
    },
    config::RunConfig,
    connectivity::probe_console,
    console::ConsoleClient,
    control_plan::{
        control_plane_plan, execute_control_plane_cleanup, execute_control_plane_setup,
        ControlPlaneExecutionStatus,
    },
    flows::{
        config_flow_inputs, discovery_flow_inputs, fault_detect_flow_inputs, lane_flow_inputs,
        lossless_flow_inputs, mirror_flow_inputs, mock_flow_inputs, ratelimit_flow_inputs,
        routing_flow_inputs, security_flow_inputs, FlowResource, LANE_TRAFFIC_HEADER_VALUE,
        MIRROR_REQUEST_BODY, MIRROR_REQUEST_PATH, ROUTING_TRAFFIC_HEADER_VALUE,
    },
    report::CaseReport,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseKind {
    Connectivity,
    ServiceDiscovery,
    ConfigCenter,
    Routing,
    RateLimit,
    CircuitBreaker,
    LaneRouting,
    FaultDetect,
    Lossless,
    Mirror,
    AuthSecurity,
    Mock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct E2eCase {
    name: &'static str,
    kind: CaseKind,
}

impl E2eCase {
    pub const fn new(name: &'static str, kind: CaseKind) -> Self {
        Self { name, kind }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn kind(&self) -> CaseKind {
        self.kind
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseSelectionError {
    name: String,
}

impl fmt::Display for CaseSelectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown e2e case: {}", self.name)
    }
}

impl Error for CaseSelectionError {}

pub fn default_cases() -> Vec<E2eCase> {
    vec![
        E2eCase::new("connectivity", CaseKind::Connectivity),
        E2eCase::new("service-discovery", CaseKind::ServiceDiscovery),
        E2eCase::new("config-center", CaseKind::ConfigCenter),
        E2eCase::new("routing", CaseKind::Routing),
        E2eCase::new("ratelimit", CaseKind::RateLimit),
        E2eCase::new("circuitbreaker", CaseKind::CircuitBreaker),
        E2eCase::new("lane-routing", CaseKind::LaneRouting),
        E2eCase::new("fault-detect", CaseKind::FaultDetect),
        E2eCase::new("lossless", CaseKind::Lossless),
        E2eCase::new("mirror", CaseKind::Mirror),
        E2eCase::new("auth-security", CaseKind::AuthSecurity),
        E2eCase::new("mock", CaseKind::Mock),
    ]
}

pub fn select_cases(
    registry: &[E2eCase],
    filters: &[String],
) -> Result<Vec<E2eCase>, CaseSelectionError> {
    if filters.is_empty() {
        return Ok(registry.to_vec());
    }

    for filter in filters {
        if !registry.iter().any(|case| case.name == filter) {
            return Err(CaseSelectionError {
                name: filter.clone(),
            });
        }
    }

    Ok(registry
        .iter()
        .copied()
        .filter(|case| filters.iter().any(|filter| filter == case.name))
        .collect())
}

pub struct CaseContext<'a> {
    pub config: &'a RunConfig,
    pub run_id: &'a str,
}

impl E2eCase {
    pub async fn run(&self, ctx: &CaseContext<'_>) -> CaseReport {
        let start = std::time::Instant::now();
        match self.kind {
            CaseKind::Connectivity => run_connectivity(ctx, self.name, start.elapsed()).await,
            CaseKind::ServiceDiscovery => run_service_discovery(ctx, self.name).await,
            CaseKind::ConfigCenter => run_config_center(ctx, self.name).await,
            _ => run_governance_planned_case(ctx, *self).await,
        }
    }
}

async fn run_connectivity(ctx: &CaseContext<'_>, name: &str, _elapsed: Duration) -> CaseReport {
    let start = std::time::Instant::now();
    if let Err(err) = probe_console(&ctx.config.console_url, Duration::from_secs(3)).await {
        return CaseReport::failed(name, start.elapsed(), err.to_string());
    }

    match SDKContext::create_by_addresses(ctx.config.sdk_addresses()) {
        Ok(_sdk) => CaseReport::passed(
            name,
            start.elapsed(),
            format!("sdk context created for run {}", ctx.run_id),
        ),
        Err(err) => CaseReport::failed(
            name,
            start.elapsed(),
            format!("sdk context creation failed: {err}"),
        ),
    }
}

async fn run_service_discovery(ctx: &CaseContext<'_>, name: &str) -> CaseReport {
    use pole_rust::discovery::api::{
        new_consumer_api_by_context, new_provider_api_by_context, ConsumerAPI, ProviderAPI,
    };

    let start = std::time::Instant::now();
    let sdk = match SDKContext::create_by_addresses(ctx.config.sdk_addresses()) {
        Ok(sdk) => Arc::new(sdk),
        Err(err) => {
            return CaseReport::failed(name, start.elapsed(), format!("sdk init failed: {err}"))
        }
    };
    let provider = match new_provider_api_by_context(sdk.clone()) {
        Ok(provider) => provider,
        Err(err) => {
            return CaseReport::failed(
                name,
                start.elapsed(),
                format!("provider api init failed: {err}"),
            )
        }
    };
    let consumer = match new_consumer_api_by_context(sdk) {
        Ok(consumer) => consumer,
        Err(err) => {
            return CaseReport::failed(
                name,
                start.elapsed(),
                format!("consumer api init failed: {err}"),
            )
        }
    };

    let inputs = discovery_flow_inputs(&FlowResource::new(ctx.run_id, name));
    if let Err(err) = provider.register(inputs.register.clone()).await {
        return CaseReport::failed(name, start.elapsed(), format!("register failed: {err}"));
    }
    if let Err(err) = provider
        .heartbeat(inputs.register.to_heartbeat_request())
        .await
    {
        let _ = provider.deregister(inputs.deregister).await;
        return CaseReport::failed(name, start.elapsed(), format!("heartbeat failed: {err}"));
    }
    match consumer.get_all_instance(inputs.get_all).await {
        Ok(resp) if !resp.instances.instances.is_empty() => {}
        Ok(_) => {
            let _ = provider.deregister(inputs.deregister).await;
            return CaseReport::failed(name, start.elapsed(), "registered instance not discovered");
        }
        Err(err) => {
            let _ = provider.deregister(inputs.deregister).await;
            return CaseReport::failed(name, start.elapsed(), format!("discover failed: {err}"));
        }
    }
    if let Err(err) = consumer.get_one_instance(inputs.get_one).await {
        let _ = provider.deregister(inputs.deregister).await;
        return CaseReport::failed(name, start.elapsed(), format!("get one failed: {err}"));
    }
    if let Err(err) = provider.deregister(inputs.deregister).await {
        return CaseReport::failed(name, start.elapsed(), format!("deregister failed: {err}"));
    }

    CaseReport::passed(
        name,
        start.elapsed(),
        "service register/discover/deregister passed",
    )
}

async fn run_config_center(ctx: &CaseContext<'_>, name: &str) -> CaseReport {
    use pole_rust::config::api::{new_config_file_api_by_context, ConfigFileAPI};

    let start = std::time::Instant::now();
    let sdk = match SDKContext::create_by_addresses(ctx.config.sdk_addresses()) {
        Ok(sdk) => Arc::new(sdk),
        Err(err) => {
            return CaseReport::failed(name, start.elapsed(), format!("sdk init failed: {err}"))
        }
    };
    let config_api = match new_config_file_api_by_context(sdk) {
        Ok(api) => api,
        Err(err) => {
            return CaseReport::failed(
                name,
                start.elapsed(),
                format!("config api init failed: {err}"),
            )
        }
    };

    let inputs = config_flow_inputs(&FlowResource::new(ctx.run_id, name));
    if let Err(err) = config_api
        .upsert_publish_config_file(inputs.upsert.clone())
        .await
    {
        return CaseReport::failed(
            name,
            start.elapsed(),
            format!("upsert publish config failed: {err}"),
        );
    }
    match config_api.get_config_file(inputs.get).await {
        Ok(file) if file.content == inputs.file.content => {}
        Ok(file) => {
            return CaseReport::failed(
                name,
                start.elapsed(),
                format!("config content mismatch: {}", file.content),
            )
        }
        Err(err) => {
            return CaseReport::failed(name, start.elapsed(), format!("get config failed: {err}"))
        }
    }

    CaseReport::passed(name, start.elapsed(), "config upsert/publish/get passed")
}

async fn run_governance_planned_case(ctx: &CaseContext<'_>, case: E2eCase) -> CaseReport {
    let start = std::time::Instant::now();
    let resource = FlowResource::new(ctx.run_id, case.name());
    match control_plane_plan(case, &resource) {
        Some(plan) => {
            if !ctx.config.execute_governance_control_plane {
                return CaseReport::failed(
                    case.name(),
                    start.elapsed(),
                    format!(
                        "control-plane plan exists but behavioral assertions are not implemented: create={}, publish={}, cleanup={}",
                        plan.create.path,
                        plan.publish.path,
                        plan.cleanup_actions
                            .iter()
                            .map(|action| action.path.as_str())
                            .collect::<Vec<_>>()
                            .join(",")
                    ),
                );
            }

            let client = match ConsoleClient::new(
                ctx.config.console_url.clone(),
                ctx.config.token.clone(),
                Duration::from_secs(5),
            ) {
                Ok(client) => client,
                Err(err) => {
                    return CaseReport::failed(
                        case.name(),
                        start.elapsed(),
                        format!("control-plane client init failed: {err}"),
                    )
                }
            };
            let setup = execute_control_plane_setup(&client, &plan).await;
            if setup.status == ControlPlaneExecutionStatus::Failed {
                let cleanup = execute_control_plane_cleanup(&client, &plan).await;
                return CaseReport::failed(
                    case.name(),
                    start.elapsed(),
                    format!(
                        "control-plane setup failed: {}; cleanup: {}",
                        setup.message(),
                        cleanup.message()
                    ),
                );
            }

            let assertion = run_governance_behavior_assertion(ctx, case.kind(), &resource).await;
            let cleanup = execute_control_plane_cleanup(&client, &plan).await;
            if cleanup.status == ControlPlaneExecutionStatus::Failed {
                return CaseReport::failed(
                    case.name(),
                    start.elapsed(),
                    format!("control-plane cleanup failed: {}", cleanup.message()),
                );
            }

            match assertion {
                Ok(message) => CaseReport::passed(case.name(), start.elapsed(), message),
                Err(message) => CaseReport::failed(case.name(), start.elapsed(), message),
            }
        }
        None => CaseReport::failed(
            case.name(),
            start.elapsed(),
            "case is registered but no control-plane plan exists",
        ),
    }
}

async fn run_governance_behavior_assertion(
    ctx: &CaseContext<'_>,
    kind: CaseKind,
    resource: &FlowResource,
) -> Result<String, String> {
    match kind {
        CaseKind::Mock => {
            let result = run_mock_behavior(ctx, resource).await;
            assert_mock_response(result)
                .map_err(|err| format!("mock behavior assertion failed: {err}"))
        }
        CaseKind::AuthSecurity => {
            let result = run_security_behavior(ctx, resource).await;
            assert_security_denied(result)
                .map_err(|err| format!("security behavior assertion failed: {err}"))
        }
        CaseKind::RateLimit => {
            let (first, second) = run_ratelimit_behavior(ctx, resource).await;
            assert_ratelimit_quota(first, second)
                .map_err(|err| format!("ratelimit behavior assertion failed: {err}"))
        }
        CaseKind::Routing => {
            let result = run_instance_route_behavior(ctx, routing_flow_inputs(resource)).await;
            assert_instance_metadata("routing", result, "e2e-route", ROUTING_TRAFFIC_HEADER_VALUE)
                .map_err(|err| format!("routing behavior assertion failed: {err}"))
        }
        CaseKind::LaneRouting => {
            let result = run_instance_route_behavior(ctx, lane_flow_inputs(resource)).await;
            assert_instance_metadata("lane-routing", result, "lane", LANE_TRAFFIC_HEADER_VALUE)
                .map_err(|err| format!("lane-routing behavior assertion failed: {err}"))
        }
        CaseKind::CircuitBreaker => {
            let result = run_circuitbreaker_behavior(ctx, resource).await;
            assert_circuitbreaker_open(result)
                .map_err(|err| format!("circuitbreaker behavior assertion failed: {err}"))
        }
        CaseKind::Mirror => {
            let result = run_mirror_behavior(ctx, resource).await;
            assert_mirror_request(result, MIRROR_REQUEST_PATH, MIRROR_REQUEST_BODY)
                .map_err(|err| format!("mirror behavior assertion failed: {err}"))
        }
        CaseKind::FaultDetect => {
            let inputs = fault_detect_flow_inputs(resource);
            let expected_header = (inputs.probe_header.0, inputs.probe_header.1.clone());
            let result = run_fault_detect_behavior(ctx, inputs)
                .await
                .map(|(raw, _)| raw);
            assert_fault_detect_request(
                result,
                crate::flows::FAULT_DETECT_PROBE_PATH,
                expected_header.0,
                &expected_header.1,
            )
            .map_err(|err| format!("fault-detect behavior assertion failed: {err}"))
        }
        CaseKind::Lossless => {
            let result = run_lossless_behavior(ctx, lossless_flow_inputs(resource)).await;
            assert_lossless_behavior(result)
                .map_err(|err| format!("lossless behavior assertion failed: {err}"))
        }
        _ => {
            Err("control-plane steps passed; behavioral assertions are not implemented".to_string())
        }
    }
}

async fn run_mock_behavior(
    ctx: &CaseContext<'_>,
    resource: &FlowResource,
) -> Result<pole_rust::discovery::req::InstanceResponse, pole_rust::core::model::error::PoleError> {
    use pole_rust::discovery::api::{new_consumer_api_by_context, ConsumerAPI};

    let sdk = Arc::new(SDKContext::create_by_addresses(ctx.config.sdk_addresses())?);
    let consumer = new_consumer_api_by_context(sdk)?;
    consumer
        .get_one_instance(mock_flow_inputs(resource).get_one)
        .await
}

async fn run_security_behavior(
    ctx: &CaseContext<'_>,
    resource: &FlowResource,
) -> Result<pole_rust::discovery::req::InstanceResponse, pole_rust::core::model::error::PoleError> {
    use pole_rust::discovery::api::{new_consumer_api_by_context, ConsumerAPI};

    let sdk = Arc::new(SDKContext::create_by_addresses(ctx.config.sdk_addresses())?);
    let consumer = new_consumer_api_by_context(sdk)?;
    consumer
        .get_one_instance(security_flow_inputs(resource).get_one)
        .await
}

async fn run_ratelimit_behavior(
    ctx: &CaseContext<'_>,
    resource: &FlowResource,
) -> (
    Result<
        pole_rust::traffic::ratelimit::req::QuotaResponse,
        pole_rust::core::model::error::PoleError,
    >,
    Result<
        pole_rust::traffic::ratelimit::req::QuotaResponse,
        pole_rust::core::model::error::PoleError,
    >,
) {
    use pole_rust::traffic::ratelimit::api::{new_ratelimit_api_by_context, RateLimitAPI};

    let sdk = match SDKContext::create_by_addresses(ctx.config.sdk_addresses()) {
        Ok(sdk) => Arc::new(sdk),
        Err(err) => return (Err(err.clone()), Err(err)),
    };
    let api = match new_ratelimit_api_by_context(sdk) {
        Ok(api) => api,
        Err(err) => return (Err(err.clone()), Err(err)),
    };
    let inputs = ratelimit_flow_inputs(resource);
    let first = api.get_quota(inputs.quota.clone()).await;
    let second = api.get_quota(inputs.quota).await;
    (first, second)
}

async fn run_instance_route_behavior(
    ctx: &CaseContext<'_>,
    inputs: crate::flows::InstanceRouteFlowInputs,
) -> Result<pole_rust::discovery::req::InstanceResponse, pole_rust::core::model::error::PoleError> {
    use pole_rust::discovery::api::{
        new_consumer_api_by_context, new_provider_api_by_context, ConsumerAPI, ProviderAPI,
    };

    let sdk = Arc::new(SDKContext::create_by_addresses(ctx.config.sdk_addresses())?);
    let provider = new_provider_api_by_context(sdk.clone())?;
    let consumer = new_consumer_api_by_context(sdk)?;

    for register in &inputs.register_instances {
        provider.register(register.clone()).await?;
        provider
            .heartbeat(register.clone().to_heartbeat_request())
            .await?;
    }

    let result = consumer.get_one_instance(inputs.get_one).await;
    let mut cleanup_error = None;
    for deregister in inputs.deregister_instances {
        if let Err(err) = provider.deregister(deregister).await {
            cleanup_error.get_or_insert(err);
        }
    }

    match (result, cleanup_error) {
        (Ok(response), None) => Ok(response),
        (Ok(_), Some(err)) => Err(err),
        (Err(err), _) => Err(err),
    }
}

async fn run_circuitbreaker_behavior(
    ctx: &CaseContext<'_>,
    resource: &FlowResource,
) -> Result<
    pole_rust::core::model::circuitbreaker::CheckResult,
    pole_rust::core::model::error::PoleError,
> {
    use pole_rust::{
        circuitbreaker::{api::CircuitBreakerAPI, default::DefaultCircuitBreakerAPI},
        core::model::circuitbreaker::{ResourceStat, RetStatus},
    };

    let sdk = Arc::new(SDKContext::create_by_addresses(ctx.config.sdk_addresses())?);
    let api = DefaultCircuitBreakerAPI::new(sdk);
    for _ in 0..2 {
        api.report_stat(ResourceStat {
            resource: circuitbreaker_service_resource(resource),
            ret_code: "500".to_string(),
            delay: Duration::from_millis(1),
            status: RetStatus::RetFail,
        })
        .await?;
    }

    api.check_resource(circuitbreaker_service_resource(resource))
        .await
}

async fn run_fault_detect_behavior(
    ctx: &CaseContext<'_>,
    inputs: crate::flows::FaultDetectFlowInputs,
) -> Result<(String, (&'static str, String)), String> {
    use pole_rust::discovery::api::{
        new_consumer_api_by_context, new_provider_api_by_context, ConsumerAPI, ProviderAPI,
    };
    use pole_specification::v1::FaultDetector;
    use tokio::{net::TcpListener, time::sleep};

    let listener = TcpListener::bind(format!(
        "127.0.0.1:{}",
        crate::flows::FAULT_DETECT_PROBE_PORT
    ))
    .await
    .map_err(|err| format!("bind fault-detect receiver failed: {err}"))?;
    let receiver = tokio::spawn(async move {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|err| format!("accept fault-detect request failed: {err}"))?;
        let request = read_http_request(&mut stream).await?;
        write_http_response(&mut stream).await?;
        Ok::<String, String>(request)
    });

    let sdk = Arc::new(
        SDKContext::create_by_addresses(ctx.config.sdk_addresses())
            .map_err(|err| format!("sdk init failed: {err}"))?,
    );
    let provider = new_provider_api_by_context(sdk.clone())
        .map_err(|err| format!("provider api init failed: {err}"))?;
    let consumer = new_consumer_api_by_context(sdk)
        .map_err(|err| format!("consumer api init failed: {err}"))?;

    let mut registered = false;
    let mut receiver = Some(receiver);
    let run_result = match provider.register(inputs.register_target.clone()).await {
        Ok(_) => {
            registered = true;
            match provider
                .heartbeat(inputs.register_target.to_heartbeat_request())
                .await
            {
                Ok(_) => match consumer.get_service_rule(inputs.get_rule).await {
                    Ok(rule) => {
                        let has_fault_detector = rule.rules.iter().any(|rule| {
                            rule.downcast_ref::<FaultDetector>()
                                .map(|detector| !detector.rules.is_empty())
                                .unwrap_or(false)
                        });
                        if !has_fault_detector {
                            Err("fault-detect rule did not downcast to FaultDetector".to_string())
                        } else {
                            match consumer.get_all_instance(inputs.get_all).await {
                                Ok(_) => {
                                    sleep(Duration::from_millis(50)).await;
                                    let receiver = receiver.take().unwrap();
                                    timeout(Duration::from_secs(3), receiver)
                                        .await
                                        .map_err(|_| "fault-detect probe timed out".to_string())?
                                        .map_err(|err| {
                                            format!("fault-detect receiver join failed: {err}")
                                        })?
                                }
                                Err(err) => {
                                    Err(format!("get fault-detect target instances failed: {err}"))
                                }
                            }
                        }
                    }
                    Err(err) => Err(format!("get fault-detect rule failed: {err}")),
                },
                Err(err) => Err(format!("heartbeat fault-detect target failed: {err}")),
            }
        }
        Err(err) => Err(format!("register fault-detect target failed: {err}")),
    };

    if let Some(receiver) = receiver {
        receiver.abort();
    }
    if registered {
        if let Err(err) = provider.deregister(inputs.deregister_target).await {
            return Err(format!("deregister fault-detect target failed: {err}"));
        }
    }
    run_result.map(|raw| (raw, inputs.probe_header))
}

async fn run_lossless_behavior(
    ctx: &CaseContext<'_>,
    inputs: crate::flows::LosslessFlowInputs,
) -> Result<LosslessBehaviorResult, String> {
    use pole_rust::discovery::api::{
        new_consumer_api_by_context, new_lossless_api_by_context, ConsumerAPI,
    };
    use pole_specification::v1::LosslessRule;
    use tokio::net::TcpListener;

    let sdk = Arc::new(
        SDKContext::create_by_addresses(ctx.config.sdk_addresses())
            .map_err(|err| format!("sdk init failed: {err}"))?,
    );
    let consumer = new_consumer_api_by_context(sdk)
        .map_err(|err| format!("consumer api init failed: {err}"))?;
    let rule_response = consumer
        .get_service_rule(inputs.get_rule)
        .await
        .map_err(|err| format!("get lossless rule failed: {err}"))?;
    let rule = rule_response
        .rules
        .iter()
        .find_map(|rule| rule.downcast_ref::<LosslessRule>().cloned())
        .ok_or_else(|| "lossless rule did not downcast to LosslessRule".to_string())?;

    let lossless_context = SDKContext::create_by_addresses(ctx.config.sdk_addresses())
        .map_err(|err| format!("lossless sdk init failed: {err}"))?;
    let lossless = new_lossless_api_by_context(lossless_context)
        .map_err(|err| format!("lossless api init failed: {err}"))?;
    let instance = Arc::new(E2eLosslessInstance {
        namespace: inputs.namespace,
        service: inputs.service,
        ip: inputs.instance_ip.to_string(),
        port: inputs.instance_port,
    });
    let action = Arc::new(RecordingLosslessAction::default());
    lossless.set_action_provider(instance.clone(), action.clone());

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|err| format!("bind lossless endpoint failed: {err}"))?;
    let endpoint_addr = listener
        .local_addr()
        .map_err(|err| format!("read lossless endpoint addr failed: {err}"))?;
    let endpoint_handle = lossless.serve_endpoint(listener, instance.clone());
    let register_handle = lossless.schedule_register(instance, &rule);

    tokio::time::sleep(Duration::from_millis(200)).await;
    let delay_observed = action.register_count() == 0;
    timeout(Duration::from_secs(3), register_handle)
        .await
        .map_err(|_| "lossless register schedule timed out".to_string())?
        .map_err(|err| format!("lossless register task failed: {err}"))?;

    let readiness_status = http_status(endpoint_addr, "/readiness").await?;
    let offline_status = http_status(endpoint_addr, "/offline").await?;
    let readiness_after_offline_status = http_status(endpoint_addr, "/readiness").await?;
    endpoint_handle.abort();

    Ok(LosslessBehaviorResult {
        rule_fetched: true,
        delay_observed,
        register_count: action.register_count(),
        readiness_status,
        offline_status,
        readiness_after_offline_status,
        deregister_count: action.deregister_count(),
    })
}

#[derive(Default)]
struct RecordingLosslessAction {
    registers: AtomicUsize,
    deregisters: AtomicUsize,
}

impl RecordingLosslessAction {
    fn register_count(&self) -> usize {
        self.registers.load(Ordering::SeqCst)
    }

    fn deregister_count(&self) -> usize {
        self.deregisters.load(Ordering::SeqCst)
    }
}

impl pole_rust::discovery::req::LosslessActionProvider for RecordingLosslessAction {
    fn get_name(&self) -> String {
        "e2e-recording-lossless-action".to_string()
    }

    fn do_register(&self, _prop: pole_rust::discovery::req::InstanceProperties) {
        self.registers.fetch_add(1, Ordering::SeqCst);
    }

    fn do_deregister(&self) {
        self.deregisters.fetch_add(1, Ordering::SeqCst);
    }

    fn is_enable_healthcheck(&self) -> bool {
        false
    }

    fn do_healthcheck(&self) -> bool {
        true
    }
}

struct E2eLosslessInstance {
    namespace: String,
    service: String,
    ip: String,
    port: u32,
}

impl pole_rust::discovery::req::BaseInstance for E2eLosslessInstance {
    fn get_namespace(&self) -> String {
        self.namespace.clone()
    }

    fn get_service(&self) -> String {
        self.service.clone()
    }

    fn get_ip(&self) -> String {
        self.ip.clone()
    }

    fn get_port(&self) -> u32 {
        self.port
    }
}

async fn http_status(addr: std::net::SocketAddr, path: &str) -> Result<u16, String> {
    let mut stream = tokio::net::TcpStream::connect(addr)
        .await
        .map_err(|err| format!("connect lossless endpoint failed: {err}"))?;
    stream
        .write_all(format!("GET {path} HTTP/1.1\r\nhost: {addr}\r\n\r\n").as_bytes())
        .await
        .map_err(|err| format!("write lossless endpoint request failed: {err}"))?;
    let mut buf = [0_u8; 1024];
    let n = stream
        .read(&mut buf)
        .await
        .map_err(|err| format!("read lossless endpoint response failed: {err}"))?;
    let raw = String::from_utf8_lossy(&buf[..n]);
    raw.lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .ok_or_else(|| format!("parse lossless endpoint status failed: {raw}"))
}

fn circuitbreaker_service_resource(
    resource: &FlowResource,
) -> pole_rust::core::model::circuitbreaker::Resource {
    use pole_rust::core::model::{
        circuitbreaker::{Resource, ServiceResource},
        naming::ServiceKey,
    };

    Resource::ServiceResource(ServiceResource::new(ServiceKey {
        namespace: resource.namespace(),
        name: resource.name(),
    }))
}

async fn run_mirror_behavior(
    ctx: &CaseContext<'_>,
    resource: &FlowResource,
) -> Result<String, String> {
    use pole_rust::{
        discovery::api::{new_provider_api_by_context, ProviderAPI},
        router::api::{new_router_api_by_context, RouterAPI},
    };
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|err| format!("bind mirror receiver failed: {err}"))?;
    let shadow_port = listener
        .local_addr()
        .map_err(|err| format!("read mirror receiver addr failed: {err}"))?
        .port() as u32;
    let receiver = tokio::spawn(async move {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|err| format!("accept mirror request failed: {err}"))?;
        let request = read_http_request(&mut stream).await?;
        write_http_response(&mut stream).await?;
        Ok::<String, String>(request)
    });

    let sdk = Arc::new(
        SDKContext::create_by_addresses(ctx.config.sdk_addresses())
            .map_err(|err| format!("sdk init failed: {err}"))?,
    );
    let provider = new_provider_api_by_context(sdk.clone())
        .map_err(|err| format!("provider api init failed: {err}"))?;
    let router =
        new_router_api_by_context(sdk).map_err(|err| format!("router api init failed: {err}"))?;
    let inputs = mirror_flow_inputs(resource, shadow_port);
    provider
        .register(inputs.register_shadow.clone())
        .await
        .map_err(|err| format!("register mirror shadow failed: {err}"))?;
    provider
        .heartbeat(inputs.register_shadow.to_heartbeat_request())
        .await
        .map_err(|err| format!("heartbeat mirror shadow failed: {err}"))?;

    let route_result = router
        .router(inputs.route_request)
        .await
        .map(|_| ())
        .map_err(|err| format!("mirror route failed: {err}"));
    finish_mirror_behavior(
        route_result,
        receiver,
        async {
            provider
                .deregister(inputs.deregister_shadow)
                .await
                .map_err(|err| format!("deregister mirror shadow failed: {err}"))
        },
        Duration::from_secs(3),
    )
    .await
}

async fn finish_mirror_behavior<C>(
    route_result: Result<(), String>,
    receiver: tokio::task::JoinHandle<Result<String, String>>,
    cleanup: C,
    receive_timeout: Duration,
) -> Result<String, String>
where
    C: Future<Output = Result<(), String>>,
{
    if let Err(route_error) = route_result {
        receiver.abort();
        cleanup.await?;
        return Err(route_error);
    }

    let received = timeout(receive_timeout, receiver)
        .await
        .map_err(|_| "mirror request timed out".to_string())?
        .map_err(|err| format!("mirror receiver join failed: {err}"))?;
    cleanup.await?;
    received
}

async fn read_http_request(stream: &mut tokio::net::TcpStream) -> Result<String, String> {
    let mut raw = Vec::new();
    let mut buf = [0_u8; 1024];
    loop {
        let n = stream
            .read(&mut buf)
            .await
            .map_err(|err| format!("read mirror request failed: {err}"))?;
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
        if String::from_utf8_lossy(&raw).contains("\r\n\r\n") {
            break;
        }
    }
    let header_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|idx| idx + 4)
        .unwrap_or(raw.len());
    let headers = String::from_utf8_lossy(&raw[..header_end]).to_ascii_lowercase();
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while raw.len().saturating_sub(header_end) < content_length {
        let n = stream
            .read(&mut buf)
            .await
            .map_err(|err| format!("read mirror body failed: {err}"))?;
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
    }
    Ok(String::from_utf8_lossy(&raw).to_string())
}

async fn write_http_response(stream: &mut tokio::net::TcpStream) -> Result<(), String> {
    stream
        .write_all(b"HTTP/1.1 204 No Content\r\ncontent-length: 0\r\n\r\n")
        .await
        .map_err(|err| format!("write mirror response failed: {err}"))
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    use super::finish_mirror_behavior;

    #[tokio::test]
    async fn mirror_finish_waits_for_receiver_before_cleanup() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let receiver_events = events.clone();
        let receiver = tokio::spawn(async move {
            receiver_events.lock().unwrap().push("receiver");
            Ok::<_, String>("POST /e2e/mirror HTTP/1.1\r\n\r\n{}".to_string())
        });
        let cleanup_events = events.clone();

        let result = finish_mirror_behavior(
            Ok(()),
            receiver,
            async move {
                cleanup_events.lock().unwrap().push("cleanup");
                Ok(())
            },
            Duration::from_secs(1),
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(*events.lock().unwrap(), vec!["receiver", "cleanup"]);
    }

    #[tokio::test]
    async fn mirror_finish_cleans_up_when_route_fails() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let receiver_events = events.clone();
        let receiver = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(10)).await;
            receiver_events.lock().unwrap().push("receiver");
            Ok::<_, String>("unexpected".to_string())
        });
        let cleanup_events = events.clone();

        let result = finish_mirror_behavior(
            Err("route failed".to_string()),
            receiver,
            async move {
                cleanup_events.lock().unwrap().push("cleanup");
                Ok(())
            },
            Duration::from_millis(10),
        )
        .await;

        assert_eq!(result.unwrap_err(), "route failed");
        assert_eq!(*events.lock().unwrap(), vec!["cleanup"]);
    }
}
