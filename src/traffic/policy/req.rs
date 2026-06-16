use pole_specification::v1::{
    MirrorDestination, MockResponse, TrafficMirror, TrafficMock, TrafficSecurityRejectEffect,
    TrafficSecurityRule,
};
use std::collections::HashMap;

#[derive(Default)]
pub struct TrafficGovernanceRules {
    pub security_rules: Vec<TrafficSecurityRule>,
    pub mirror_rules: Vec<TrafficMirror>,
    pub mock_rules: Vec<TrafficMock>,
}

#[derive(Default, Clone)]
pub struct TrafficGovernanceResult {
    pub security: TrafficSecurityDecision,
    pub mirrors: Vec<MirrorDestination>,
    pub mock: Option<MockResponse>,
}

#[derive(Clone)]
pub struct TrafficSecurityDecision {
    pub allowed: bool,
    pub reject_effect: Option<TrafficSecurityRejectEffect>,
}

impl Default for TrafficSecurityDecision {
    fn default() -> Self {
        Self {
            allowed: true,
            reject_effect: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MirrorRequest {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}
