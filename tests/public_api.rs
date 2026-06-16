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
