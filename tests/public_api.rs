use pole_rust::discovery::api::{new_lossless_api, new_lossless_api_by_context, LosslessAPI};

#[test]
fn lossless_api_constructors_are_public() {
    let _ctor = new_lossless_api;
    let _ctx_ctor = new_lossless_api_by_context;

    fn _accept_lossless_api<T: LosslessAPI>() {}
}

#[test]
fn governance_modules_expose_api_and_req_boundaries() {
    use pole_rust::traffic::{
        faultdetect::{
            api::{FaultDetectProbeExecutor, FaultDetectReporter},
            req::{FaultDetectPlan, FaultDetectResult, FaultDetectTarget},
        },
        policy::{
            api::MirrorSender,
            req::{MirrorRequest, TrafficGovernanceResult},
        },
    };

    fn _accept_fault_detect_executor<T: FaultDetectProbeExecutor>() {}
    fn _accept_fault_detect_reporter<T: FaultDetectReporter>() {}
    fn _accept_mirror_sender<T: MirrorSender>() {}

    let _plan = std::mem::size_of::<FaultDetectPlan>();
    let _result = std::mem::size_of::<FaultDetectResult>();
    let _target = std::mem::size_of::<FaultDetectTarget>();
    let _mirror_request = std::mem::size_of::<MirrorRequest>();
    let _governance = std::mem::size_of::<TrafficGovernanceResult>();
}

#[test]
fn traffic_domain_exposes_all_governance_capabilities() {
    use pole_rust::traffic::{
        circuitbreaker::api::CircuitBreakerAPI, faultdetect::api::FaultDetectProbeExecutor,
        policy::api::MirrorSender, ratelimit::api::RateLimitAPI, router::api::RouterAPI,
    };

    fn _accept_router<T: RouterAPI>() {}
    fn _accept_ratelimit<T: RateLimitAPI>() {}
    fn _accept_circuitbreaker<T: CircuitBreakerAPI>() {}
    fn _accept_faultdetect<T: FaultDetectProbeExecutor>() {}
    fn _accept_policy_sender<T: MirrorSender>() {}
}

#[test]
fn ratelimit_exposes_quota_lease_and_multi_resource_types() {
    use pole_rust::traffic::ratelimit::{
        api::{QuotaConsumption, QuotaLease},
        req::{QuotaAmount, QuotaRequest, QuotaResource},
    };

    let _lease = std::mem::size_of::<QuotaLease>();
    let _request = std::mem::size_of::<QuotaRequest>();
    let _amount = QuotaAmount {
        resource: QuotaResource::Token,
        amount: 1,
    };
    let _consumption = QuotaConsumption {
        resource: QuotaResource::Token,
        consumed_total: 1,
    };
}

#[test]
fn observability_exposes_public_semantic_boundaries() {
    use pole_rust::observability::{
        api::{NoopObservabilityRecorder, ObservabilityRecorder},
        default::DefaultObservability,
        req::{
            GovernanceDecisionContext, MetricRecord, ObservabilityConfig, ObservabilityEvent,
            ResourceAttributes, SpanAttributes, TelemetryEndpoint,
        },
    };

    fn _accept_recorder<T: ObservabilityRecorder>() {}

    _accept_recorder::<NoopObservabilityRecorder>();
    let _default = std::mem::size_of::<DefaultObservability>();
    let _config = std::mem::size_of::<ObservabilityConfig>();
    let _resource = std::mem::size_of::<ResourceAttributes>();
    let _endpoint = std::mem::size_of::<TelemetryEndpoint>();
    let _metric = std::mem::size_of::<MetricRecord>();
    let _event = std::mem::size_of::<ObservabilityEvent>();
    let _span = std::mem::size_of::<SpanAttributes>();
    let _decision = std::mem::size_of::<GovernanceDecisionContext>();
}

#[test]
fn workload_identity_exposes_explicit_transport_adapters() {
    use pole_rust::identity::{
        AuthenticatedCaller, WorkloadCredentialClientInterceptor,
        WorkloadCredentialServerInterceptor, WorkloadIdentity, WORKLOAD_CREDENTIAL_HEADER,
    };

    let _identity = std::mem::size_of::<WorkloadIdentity>();
    let _caller = std::mem::size_of::<AuthenticatedCaller>();
    let _client = std::mem::size_of::<WorkloadCredentialClientInterceptor>();
    let _server = std::mem::size_of::<WorkloadCredentialServerInterceptor>();
    assert_eq!(WORKLOAD_CREDENTIAL_HEADER, "x-pole-workload-credential");
}

#[test]
fn legacy_top_level_governance_paths_stay_compatible() {
    use pole_rust::{
        circuitbreaker::api::CircuitBreakerAPI, faultdetect::api::FaultDetectProbeExecutor,
        ratelimit::api::RateLimitAPI, router::api::RouterAPI,
    };

    fn _accept_router<T: RouterAPI>() {}
    fn _accept_ratelimit<T: RateLimitAPI>() {}
    fn _accept_circuitbreaker<T: CircuitBreakerAPI>() {}
    fn _accept_faultdetect<T: FaultDetectProbeExecutor>() {}
}
