#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FaultDetectProtocol {
    Http,
    Tcp,
    Udp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaultDetectPlan {
    pub rule_id: String,
    pub namespace: String,
    pub service: String,
    pub interval_secs: u32,
    pub timeout_secs: u32,
    pub port: u32,
    pub protocol: FaultDetectProtocol,
    pub http_config: Option<HttpFaultDetectConfig>,
    pub tcp_config: Option<TcpFaultDetectConfig>,
    pub udp_config: Option<UdpFaultDetectConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpFaultDetectConfig {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpFaultDetectConfig {
    pub send: String,
    pub receive: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdpFaultDetectConfig {
    pub send: String,
    pub receive: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaultDetectResult {
    pub rule_id: String,
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaultDetectTarget {
    pub host: String,
    pub plans: Vec<FaultDetectPlan>,
}
