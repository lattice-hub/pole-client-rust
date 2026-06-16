use crate::traffic::faultdetect::req::{FaultDetectPlan, FaultDetectResult};

#[async_trait::async_trait]
pub trait FaultDetectProbeExecutor: Send + Sync {
    async fn execute(&self, host: &str, plan: &FaultDetectPlan) -> FaultDetectResult;
}

#[async_trait::async_trait]
pub trait FaultDetectReporter: Send + Sync {
    async fn report(&self, plan: &FaultDetectPlan, result: &FaultDetectResult);
}
