use crate::core::{
    model::{
        cache::{EventType, ResourceEventKey},
        error::{ErrorCode, PoleError},
        naming::{Instance, ServiceRule},
        ArgumentType,
    },
    plugin::{
        cache::{Filter, ResourceCache},
        router::RouteContext,
    },
};
use crate::plugins::router::rule::helper::{match_label_value, traffic_match_rule_match};
use crate::traffic::matcher::{ApiMatchIndex, ApiMatchInput};
pub use crate::traffic::{
    policy::api::MirrorSender,
    policy::req::{
        MirrorRequest, TrafficGovernanceResult, TrafficGovernanceRules, TrafficSecurityDecision,
    },
};
use bytes::Bytes;
use http::{header::HeaderName, HeaderValue, Request as HttpRequest};
use pole_specification::v1::{
    MirrorDestination, MockResponse, SourceService, TrafficMirror, TrafficMock,
    TrafficSecurityAction, TrafficSecurityRejectEffect, TrafficSecurityRule,
};
use std::{
    any::Any,
    collections::{hash_map::DefaultHasher, HashMap},
    hash::{Hash, Hasher},
    sync::Arc,
    time::Duration,
};
use tokio::task::JoinHandle;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

pub struct GrpcMirrorBody;

impl GrpcMirrorBody {
    pub fn uncompressed_unary(message: &[u8]) -> Vec<u8> {
        let mut body = Vec::with_capacity(5 + message.len());
        body.push(0);
        body.extend_from_slice(&(message.len() as u32).to_be_bytes());
        body.extend_from_slice(message);
        body
    }
}

#[derive(Clone)]
pub struct HttpMirrorSender {
    destination_base_urls: HashMap<String, String>,
    resource_cache: Option<Arc<Box<dyn ResourceCache>>>,
    timeout: Duration,
}

impl HttpMirrorSender {
    pub fn new(destination_base_urls: HashMap<String, String>, timeout: Duration) -> Self {
        Self {
            destination_base_urls,
            resource_cache: None,
            timeout,
        }
    }

    pub fn with_resource_cache(
        resource_cache: Arc<Box<dyn ResourceCache>>,
        timeout: Duration,
    ) -> Self {
        Self {
            destination_base_urls: HashMap::new(),
            resource_cache: Some(resource_cache),
            timeout,
        }
    }
}

#[derive(Clone)]
pub struct GrpcMirrorSender {
    destination_base_urls: HashMap<String, String>,
    resource_cache: Option<Arc<Box<dyn ResourceCache>>>,
    timeout: Duration,
}

impl GrpcMirrorSender {
    pub fn new(destination_base_urls: HashMap<String, String>, timeout: Duration) -> Self {
        Self {
            destination_base_urls,
            resource_cache: None,
            timeout,
        }
    }

    pub fn with_resource_cache(
        resource_cache: Arc<Box<dyn ResourceCache>>,
        timeout: Duration,
    ) -> Self {
        Self {
            destination_base_urls: HashMap::new(),
            resource_cache: Some(resource_cache),
            timeout,
        }
    }
}

#[async_trait::async_trait]
impl MirrorSender for HttpMirrorSender {
    async fn send(
        &self,
        destination: MirrorDestination,
        request: MirrorRequest,
    ) -> Result<(), PoleError> {
        let destination_key = format!("{}#{}", destination.namespace, destination.service);
        let base_url = match self.destination_base_urls.get(&destination_key) {
            Some(base_url) => base_url.clone(),
            None => self.resolve_destination_base_url(&destination).await?,
        };
        let target = parse_http_base_url(&base_url)?;
        timeout(self.timeout, send_http_mirror_request(target, request))
            .await
            .map_err(|_| {
                PoleError::new(
                    ErrorCode::ServerError,
                    format!("mirror destination {} timed out", destination_key),
                )
            })?
    }
}

#[async_trait::async_trait]
impl MirrorSender for GrpcMirrorSender {
    async fn send(
        &self,
        destination: MirrorDestination,
        request: MirrorRequest,
    ) -> Result<(), PoleError> {
        let destination_key = format!("{}#{}", destination.namespace, destination.service);
        let base_url = match self.destination_base_urls.get(&destination_key) {
            Some(base_url) => base_url.clone(),
            None => self.resolve_destination_base_url(&destination).await?,
        };
        let target = parse_grpc_base_url(&base_url)?;
        timeout(self.timeout, send_grpc_mirror_request(target, request))
            .await
            .map_err(|_| {
                PoleError::new(
                    ErrorCode::ServerError,
                    format!("grpc mirror destination {} timed out", destination_key),
                )
            })?
    }
}

impl HttpMirrorSender {
    async fn resolve_destination_base_url(
        &self,
        destination: &MirrorDestination,
    ) -> Result<String, PoleError> {
        let Some(resource_cache) = &self.resource_cache else {
            let destination_key = format!("{}#{}", destination.namespace, destination.service);
            return Err(PoleError::new(
                ErrorCode::ServiceNotFound,
                format!("mirror destination {} is not configured", destination_key),
            ));
        };

        let mut filter = HashMap::new();
        filter.insert("service".to_string(), destination.service.clone());
        let cache_item = resource_cache
            .load_service_instances(Filter {
                resource_key: ResourceEventKey {
                    namespace: destination.namespace.clone(),
                    event_type: EventType::Instance,
                    filter,
                },
                internal_request: false,
                include_cache: true,
                timeout: self.timeout,
            })
            .await?;
        let instances = cache_item.list_instances(true).await;
        instances
            .iter()
            .find(|instance| mirror_instance_matches(instance, destination))
            .map(mirror_instance_base_url)
            .transpose()?
            .ok_or_else(|| {
                PoleError::new(
                    ErrorCode::ServiceNotFound,
                    format!(
                        "no available mirror destination instance: {}#{}",
                        destination.namespace, destination.service
                    ),
                )
            })
    }
}

impl GrpcMirrorSender {
    async fn resolve_destination_base_url(
        &self,
        destination: &MirrorDestination,
    ) -> Result<String, PoleError> {
        let Some(resource_cache) = &self.resource_cache else {
            let destination_key = format!("{}#{}", destination.namespace, destination.service);
            return Err(PoleError::new(
                ErrorCode::ServiceNotFound,
                format!(
                    "grpc mirror destination {} is not configured",
                    destination_key
                ),
            ));
        };

        let mut filter = HashMap::new();
        filter.insert("service".to_string(), destination.service.clone());
        let cache_item = resource_cache
            .load_service_instances(Filter {
                resource_key: ResourceEventKey {
                    namespace: destination.namespace.clone(),
                    event_type: EventType::Instance,
                    filter,
                },
                internal_request: false,
                include_cache: true,
                timeout: self.timeout,
            })
            .await?;
        let instances = cache_item.list_instances(true).await;
        instances
            .iter()
            .find(|instance| mirror_instance_matches(instance, destination))
            .map(|instance| mirror_instance_base_url_with_protocol(instance, "grpc"))
            .transpose()?
            .ok_or_else(|| {
                PoleError::new(
                    ErrorCode::ServiceNotFound,
                    format!(
                        "no available grpc mirror destination instance: {}#{}",
                        destination.namespace, destination.service
                    ),
                )
            })
    }
}

fn mirror_instance_matches(instance: &Instance, destination: &MirrorDestination) -> bool {
    instance.is_available()
        && destination.labels.iter().all(|(key, value)| {
            instance
                .metadata
                .get(key)
                .map(|actual| match_label_value(value, actual.clone()))
                .unwrap_or(false)
        })
}

fn mirror_instance_base_url(instance: &Instance) -> Result<String, PoleError> {
    mirror_instance_base_url_with_protocol(instance, "http")
}

fn mirror_instance_base_url_with_protocol(
    instance: &Instance,
    expected_protocol: &str,
) -> Result<String, PoleError> {
    if instance.ip.is_empty() || instance.port == 0 {
        return Err(PoleError::new(
            ErrorCode::InvalidServerResponse,
            format!(
                "invalid mirror destination instance address: {}:{}",
                instance.ip, instance.port
            ),
        ));
    }
    let protocol = if instance.protocol.is_empty() {
        expected_protocol
    } else {
        instance.protocol.as_str()
    };
    if protocol != expected_protocol {
        return Err(PoleError::new(
            ErrorCode::ApiInvalidArgument,
            format!("unsupported mirror destination protocol: {}", protocol),
        ));
    }
    Ok(format!(
        "{}://{}:{}",
        expected_protocol, instance.ip, instance.port
    ))
}

pub fn dispatch_mirror_requests(
    sender: Arc<dyn MirrorSender>,
    destinations: Vec<MirrorDestination>,
    request: MirrorRequest,
) -> Vec<JoinHandle<()>> {
    destinations
        .into_iter()
        .map(|destination| {
            let sender = sender.clone();
            let request = request.clone();
            tokio::spawn(async move {
                if let Err(err) = sender.send(destination, request).await {
                    crate::warn!("[traffic][mirror] mirror dispatch failed: {:?}", err);
                }
            })
        })
        .collect()
}

struct HttpTarget {
    host: String,
    port: u16,
}

struct GrpcTarget {
    host: String,
    port: u16,
}

fn parse_http_base_url(base_url: &str) -> Result<HttpTarget, PoleError> {
    let Some(address) = base_url.strip_prefix("http://") else {
        return Err(PoleError::new(
            ErrorCode::ApiInvalidArgument,
            format!("only http mirror destination is supported: {}", base_url),
        ));
    };
    let address = address.trim_end_matches('/');
    let (host, port) = match address.rsplit_once(':') {
        Some((host, port)) => {
            let port = port.parse::<u16>().map_err(|_| {
                PoleError::new(
                    ErrorCode::ApiInvalidArgument,
                    format!("invalid mirror destination port: {}", base_url),
                )
            })?;
            (host.to_string(), port)
        }
        None => (address.to_string(), 80),
    };
    if host.is_empty() {
        return Err(PoleError::new(
            ErrorCode::ApiInvalidArgument,
            format!("invalid mirror destination host: {}", base_url),
        ));
    }

    Ok(HttpTarget { host, port })
}

fn parse_grpc_base_url(base_url: &str) -> Result<GrpcTarget, PoleError> {
    let address = base_url
        .strip_prefix("grpc://")
        .or_else(|| base_url.strip_prefix("http://"))
        .ok_or_else(|| {
            PoleError::new(
                ErrorCode::ApiInvalidArgument,
                format!("only grpc mirror destination is supported: {}", base_url),
            )
        })?
        .trim_end_matches('/');
    let (host, port) = match address.rsplit_once(':') {
        Some((host, port)) => {
            let port = port.parse::<u16>().map_err(|_| {
                PoleError::new(
                    ErrorCode::ApiInvalidArgument,
                    format!("invalid grpc mirror destination port: {}", base_url),
                )
            })?;
            (host.to_string(), port)
        }
        None => (address.to_string(), 80),
    };
    if host.is_empty() {
        return Err(PoleError::new(
            ErrorCode::ApiInvalidArgument,
            format!("invalid grpc mirror destination host: {}", base_url),
        ));
    }

    Ok(GrpcTarget { host, port })
}

async fn send_http_mirror_request(
    target: HttpTarget,
    request: MirrorRequest,
) -> Result<(), PoleError> {
    let mut stream = TcpStream::connect(format!("{}:{}", target.host, target.port))
        .await
        .map_err(|err| {
            PoleError::new(
                ErrorCode::ServerError,
                format!("connect mirror destination failed: {}", err),
            )
        })?;
    let path = if request.path.is_empty() {
        "/"
    } else {
        request.path.as_str()
    };
    let mut raw = format!(
        "{} {} HTTP/1.1\r\nhost: {}\r\ncontent-length: {}\r\nconnection: close\r\n",
        request.method,
        path,
        target.host,
        request.body.len()
    )
    .into_bytes();
    for (key, value) in request.headers.iter() {
        raw.extend_from_slice(format!("{}: {}\r\n", key, value).as_bytes());
    }
    raw.extend_from_slice(b"\r\n");
    raw.extend_from_slice(&request.body);

    stream.write_all(&raw).await.map_err(|err| {
        PoleError::new(
            ErrorCode::ServerError,
            format!("write mirror request failed: {}", err),
        )
    })?;
    let mut response = [0u8; 64];
    let n = stream.read(&mut response).await.map_err(|err| {
        PoleError::new(
            ErrorCode::ServerError,
            format!("read mirror response failed: {}", err),
        )
    })?;
    if n == 0 {
        return Err(PoleError::new(
            ErrorCode::InvalidServerResponse,
            "empty mirror response".to_string(),
        ));
    }
    let response = String::from_utf8_lossy(&response[..n]);
    if !response.starts_with("HTTP/1.1 2") && !response.starts_with("HTTP/1.1 3") {
        return Err(PoleError::new(
            ErrorCode::InvalidServerResponse,
            format!(
                "mirror destination returned {}",
                response.lines().next().unwrap_or("")
            ),
        ));
    }
    Ok(())
}

async fn send_grpc_mirror_request(
    target: GrpcTarget,
    request: MirrorRequest,
) -> Result<(), PoleError> {
    let stream = TcpStream::connect(format!("{}:{}", target.host, target.port))
        .await
        .map_err(|err| {
            PoleError::new(
                ErrorCode::ServerError,
                format!("connect grpc mirror destination failed: {}", err),
            )
        })?;
    let (mut sender, connection) = h2::client::handshake(stream).await.map_err(|err| {
        PoleError::new(
            ErrorCode::ServerError,
            format!("grpc mirror h2 handshake failed: {}", err),
        )
    })?;
    tokio::spawn(async move {
        if let Err(err) = connection.await {
            crate::warn!(
                "[traffic][mirror] grpc mirror h2 connection failed: {}",
                err
            );
        }
    });

    let path = if request.path.is_empty() {
        "/"
    } else {
        request.path.as_str()
    };
    let mut builder = HttpRequest::builder()
        .method("POST")
        .uri(format!("http://{}:{}{}", target.host, target.port, path))
        .header("te", "trailers")
        .header("content-type", "application/grpc");
    for (key, value) in request.headers.iter() {
        if key.eq_ignore_ascii_case("content-type")
            || key.eq_ignore_ascii_case("content-length")
            || key.eq_ignore_ascii_case("connection")
            || key.eq_ignore_ascii_case("te")
        {
            continue;
        }
        let header_name = HeaderName::from_bytes(key.as_bytes()).map_err(|err| {
            PoleError::new(
                ErrorCode::ApiInvalidArgument,
                format!("invalid grpc mirror header name {}: {}", key, err),
            )
        })?;
        let header_value = HeaderValue::from_str(value).map_err(|err| {
            PoleError::new(
                ErrorCode::ApiInvalidArgument,
                format!("invalid grpc mirror header value {}: {}", key, err),
            )
        })?;
        builder = builder.header(header_name, header_value);
    }
    let h2_request = builder.body(()).map_err(|err| {
        PoleError::new(
            ErrorCode::ApiInvalidArgument,
            format!("invalid grpc mirror request: {}", err),
        )
    })?;
    let (response, mut body_sender) = sender.send_request(h2_request, false).map_err(|err| {
        PoleError::new(
            ErrorCode::ServerError,
            format!("send grpc mirror request headers failed: {}", err),
        )
    })?;
    body_sender
        .send_data(Bytes::from(request.body), true)
        .map_err(|err| {
            PoleError::new(
                ErrorCode::ServerError,
                format!("send grpc mirror request body failed: {}", err),
            )
        })?;
    let response = response.await.map_err(|err| {
        PoleError::new(
            ErrorCode::ServerError,
            format!("read grpc mirror response failed: {}", err),
        )
    })?;
    if !response.status().is_success() && !response.status().is_redirection() {
        return Err(PoleError::new(
            ErrorCode::InvalidServerResponse,
            format!("grpc mirror destination returned {}", response.status()),
        ));
    }
    Ok(())
}

pub fn evaluate_traffic_governance(
    ctx: &RouteContext,
    rules: TrafficGovernanceRules,
) -> TrafficGovernanceResult {
    TrafficGovernanceResult {
        security: evaluate_security(ctx, &rules.security_rules),
        mirrors: evaluate_mirrors(ctx, &rules.mirror_rules),
        mock: evaluate_mock(ctx, &rules.mock_rules),
    }
}

pub async fn evaluate_traffic_governance_from_cache(
    ctx: &RouteContext,
    resource_cache: Arc<Box<dyn ResourceCache>>,
    timeout: Duration,
) -> Result<TrafficGovernanceResult, PoleError> {
    let security_rule = load_traffic_service_rule(
        resource_cache.clone(),
        ctx,
        EventType::TrafficSecurityRule,
        timeout,
    )
    .await?;
    let mirror_rule = load_traffic_service_rule(
        resource_cache.clone(),
        ctx,
        EventType::TrafficMirrorRule,
        timeout,
    )
    .await?;
    let mock_rule =
        load_traffic_service_rule(resource_cache, ctx, EventType::TrafficMockRule, timeout).await?;

    let rules = traffic_governance_rules_from_cached_rules(
        security_rule.rules,
        mirror_rule.rules,
        mock_rule.rules,
    )?;

    Ok(evaluate_traffic_governance(ctx, rules))
}

async fn load_traffic_service_rule(
    resource_cache: Arc<Box<dyn ResourceCache>>,
    ctx: &RouteContext,
    event_type: EventType,
    timeout: Duration,
) -> Result<ServiceRule, PoleError> {
    let mut filter = HashMap::new();
    filter.insert("service".to_string(), ctx.route_info.callee.name.clone());
    resource_cache
        .load_service_rule(Filter {
            resource_key: ResourceEventKey {
                namespace: ctx.route_info.callee.namespace.clone(),
                event_type,
                filter,
            },
            internal_request: false,
            include_cache: true,
            timeout,
        })
        .await
}

pub fn traffic_governance_rules_from_cached_rules(
    security_rules: Vec<Box<dyn Any + Send>>,
    mirror_rules: Vec<Box<dyn Any + Send>>,
    mock_rules: Vec<Box<dyn Any + Send>>,
) -> Result<TrafficGovernanceRules, PoleError> {
    Ok(TrafficGovernanceRules {
        security_rules: downcast_rule_list(security_rules, "TrafficSecurityRule")?,
        mirror_rules: downcast_rule_list(mirror_rules, "TrafficMirror")?,
        mock_rules: downcast_rule_list(mock_rules, "TrafficMock")?,
    })
}

fn downcast_rule_list<T: 'static>(
    rules: Vec<Box<dyn Any + Send>>,
    expected: &str,
) -> Result<Vec<T>, PoleError> {
    let mut typed_rules = Vec::with_capacity(rules.len());
    for rule in rules {
        let type_id = rule.type_id();
        match rule.downcast::<T>() {
            Ok(rule) => typed_rules.push(*rule),
            Err(_) => {
                return Err(PoleError::new(
                    ErrorCode::InvalidRule,
                    format!(
                        "rule type error, expect {}, but got {:?}",
                        expected, type_id
                    ),
                ));
            }
        }
    }
    Ok(typed_rules)
}

fn evaluate_security(
    ctx: &RouteContext,
    security_rules: &[TrafficSecurityRule],
) -> TrafficSecurityDecision {
    let mut rules = security_rules
        .iter()
        .filter(|rule| rule.enable)
        .collect::<Vec<_>>();
    rules.sort_by(|a, b| a.priority.cmp(&b.priority));

    let entries = rules.iter().flat_map(|rule| {
        rule.policies
            .iter()
            .map(|policy| (policy, policy.apis.as_slice()))
    });
    let input = api_match_input(ctx);
    let index = ApiMatchIndex::new(entries);

    for policy in index.candidates(&input) {
        if !traffic_rule_matches(ctx, policy.traffic_match_rule.as_ref()) {
            continue;
        }
        return security_decision(policy.action(), policy.reject_effect.clone());
    }

    TrafficSecurityDecision::default()
}

fn evaluate_mirrors(ctx: &RouteContext, mirror_rules: &[TrafficMirror]) -> Vec<MirrorDestination> {
    let mut rules = mirror_rules
        .iter()
        .filter(|rule| rule.enable)
        .collect::<Vec<_>>();
    rules.sort_by(|a, b| a.priority.cmp(&b.priority));

    let entries = rules.iter().flat_map(|rule| {
        if !source_service_matches(rule.caller.as_ref(), ctx) {
            return Vec::new();
        }
        rule.rules
            .iter()
            .filter(|mirror_rule| !mirror_rule.disable && mirror_rule.mirror_percent != 0)
            .map(|mirror_rule| (mirror_rule, mirror_rule.apis.as_slice()))
            .collect::<Vec<_>>()
    });
    let input = api_match_input(ctx);
    let index = ApiMatchIndex::new(entries);
    let mut destinations = Vec::new();

    for mirror_rule in index.candidates(&input) {
        if !traffic_rule_matches(ctx, mirror_rule.traffic_match_rule.as_ref()) {
            continue;
        }
        if let Some(destination) = &mirror_rule.destination {
            let sample_salt = format!(
                "mirror#{}#{}#{}#{}",
                ctx.route_info.caller.namespace,
                ctx.route_info.caller.name,
                destination.namespace,
                destination.service
            );
            if !traffic_sample_hit(ctx, mirror_rule.mirror_percent, &sample_salt) {
                continue;
            }
            destinations.push(destination.clone());
        }
    }

    destinations
}

fn evaluate_mock(ctx: &RouteContext, mock_rules: &[TrafficMock]) -> Option<MockResponse> {
    let mut rules = mock_rules
        .iter()
        .filter(|rule| rule.enable)
        .collect::<Vec<_>>();
    rules.sort_by(|a, b| a.priority.cmp(&b.priority));

    let entries = rules.iter().flat_map(|rule| {
        if !source_service_matches(rule.caller.as_ref(), ctx) {
            return Vec::new();
        }
        rule.rules
            .iter()
            .filter(|mock_rule| !mock_rule.disable && mock_rule.mock_percent != 0)
            .map(|mock_rule| (mock_rule, mock_rule.apis.as_slice()))
            .collect::<Vec<_>>()
    });
    let input = api_match_input(ctx);
    let index = ApiMatchIndex::new(entries);

    for mock_rule in index.candidates(&input) {
        if !traffic_rule_matches(ctx, mock_rule.traffic_match_rule.as_ref()) {
            continue;
        }
        if let Some(response) = &mock_rule.response {
            let sample_salt = format!(
                "mock#{}#{}#{}#{}",
                ctx.route_info.caller.namespace,
                ctx.route_info.caller.name,
                response.code,
                response.body
            );
            if !traffic_sample_hit(ctx, mock_rule.mock_percent, &sample_salt) {
                continue;
            }
            return Some(response.clone());
        }
    }

    None
}

fn traffic_sample_hit(ctx: &RouteContext, percent: u32, salt: &str) -> bool {
    if percent == 0 {
        return false;
    }
    if percent >= 100 {
        return true;
    }

    let mut hasher = DefaultHasher::new();
    salt.hash(&mut hasher);
    ctx.route_info.caller.namespace.hash(&mut hasher);
    ctx.route_info.caller.name.hash(&mut hasher);
    ctx.route_info.callee.namespace.hash(&mut hasher);
    ctx.route_info.callee.name.hash(&mut hasher);
    let mut metadata = ctx.route_info.metadata.iter().collect::<Vec<_>>();
    metadata.sort_by(|a, b| a.0.cmp(b.0));
    for (key, value) in metadata {
        key.hash(&mut hasher);
        value.hash(&mut hasher);
    }

    hasher.finish() % 100 < percent as u64
}

fn security_decision(
    action: TrafficSecurityAction,
    reject_effect: Option<TrafficSecurityRejectEffect>,
) -> TrafficSecurityDecision {
    match action {
        TrafficSecurityAction::TrafficSecurityAllow => TrafficSecurityDecision::default(),
        TrafficSecurityAction::TrafficSecurityDeny => TrafficSecurityDecision {
            allowed: false,
            reject_effect,
        },
    }
}

fn traffic_rule_matches(
    ctx: &RouteContext,
    rule: Option<&pole_specification::v1::TrafficMatchRule>,
) -> bool {
    rule.map(|rule| traffic_match_rule_match(ctx, rule))
        .unwrap_or(true)
}

fn api_match_input(ctx: &RouteContext) -> ApiMatchInput {
    ApiMatchInput::new(
        (ctx.route_info.traffic_label_provider)(ArgumentType::Method, ""),
        (ctx.route_info.traffic_label_provider)(ArgumentType::Path, ""),
    )
}

fn source_service_matches(source: Option<&SourceService>, ctx: &RouteContext) -> bool {
    source
        .map(|source| {
            string_matches_service(&source.namespace, &ctx.route_info.caller.namespace)
                && string_matches_service(&source.service, &ctx.route_info.caller.name)
        })
        .unwrap_or(true)
}

fn string_matches_service(rule_value: &str, actual: &str) -> bool {
    rule_value.is_empty() || rule_value == "*" || rule_value == actual
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        model::{
            cache::{EventType, ResourceEventKey, ServiceInstancesCacheItem},
            config::{ConfigFile, ConfigGroup},
            error::{ErrorCode, PoleError},
            naming::{Instance, ServiceRule, Services},
            router::RouteInfo,
            ArgumentType,
        },
        plugin::{
            cache::{Filter, ResourceCache, ResourceCacheFailover, ResourceListener},
            plugins::Plugin,
            router::RouteContext,
        },
    };
    use h2::server;
    use http::Response;
    use pole_specification::v1::{
        match_string::{MatchStringType, ValueType},
        source_match, traffic_match_rule, Api, MatchString, MirrorDestination, MirrorRule,
        MockResponse, MockRule, SourceMatch, SourceService, TrafficMatchRule, TrafficMirror,
        TrafficMock, TrafficSecurityAction, TrafficSecurityPolicy, TrafficSecurityRejectEffect,
        TrafficSecurityRule,
    };
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
        time::Duration,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::{mpsc, Notify};

    #[derive(Default)]
    struct FakeTrafficRuleCache {
        calls: Arc<Mutex<Vec<ResourceEventKey>>>,
        mirror_instances: Vec<Instance>,
    }

    impl Plugin for FakeTrafficRuleCache {
        fn init(&mut self) {}

        fn destroy(&self) {}

        fn name(&self) -> String {
            "fakeTrafficRuleCache".to_string()
        }
    }

    #[async_trait::async_trait]
    impl ResourceCache for FakeTrafficRuleCache {
        fn set_failover_provider(&mut self, _failover: Arc<dyn ResourceCacheFailover>) {}

        async fn load_service_rule(&self, filter: Filter) -> Result<ServiceRule, PoleError> {
            self.calls.lock().unwrap().push(filter.resource_key.clone());
            let rules: Vec<Box<dyn std::any::Any + Send>> = match filter.resource_key.event_type {
                EventType::TrafficSecurityRule => vec![Box::new(TrafficSecurityRule {
                    id: "security-1".to_string(),
                    enable: true,
                    policies: vec![TrafficSecurityPolicy {
                        apis: vec![api()],
                        traffic_match_rule: Some(header_match("x-user", "alice")),
                        action: TrafficSecurityAction::TrafficSecurityDeny.into(),
                        reject_effect: Some(TrafficSecurityRejectEffect {
                            code: "DENIED".to_string(),
                            message: "blocked".to_string(),
                        }),
                        ..TrafficSecurityPolicy::default()
                    }],
                    ..TrafficSecurityRule::default()
                })],
                EventType::TrafficMirrorRule => vec![Box::new(TrafficMirror {
                    id: "mirror-1".to_string(),
                    enable: true,
                    caller: Some(SourceService {
                        namespace: "default".to_string(),
                        service: "frontend".to_string(),
                    }),
                    rules: vec![MirrorRule {
                        traffic_match_rule: Some(header_match("x-debug", "true")),
                        apis: vec![api()],
                        destination: Some(MirrorDestination {
                            namespace: "shadow".to_string(),
                            service: "orders-shadow".to_string(),
                            labels: HashMap::new(),
                        }),
                        mirror_percent: 100,
                        ..MirrorRule::default()
                    }],
                    ..TrafficMirror::default()
                })],
                EventType::TrafficMockRule => vec![Box::new(TrafficMock {
                    id: "mock-1".to_string(),
                    enable: true,
                    caller: Some(SourceService {
                        namespace: "default".to_string(),
                        service: "frontend".to_string(),
                    }),
                    rules: vec![MockRule {
                        apis: vec![api()],
                        traffic_match_rule: Some(header_match("x-user", "alice")),
                        response: Some(MockResponse {
                            code: "OK".to_string(),
                            body: "{\"ok\":true}".to_string(),
                            ..MockResponse::default()
                        }),
                        mock_percent: 100,
                        ..MockRule::default()
                    }],
                    ..TrafficMock::default()
                })],
                _ => Vec::new(),
            };

            Ok(ServiceRule {
                rules,
                revision: "rev-1".to_string(),
                initialized: true,
            })
        }

        async fn load_services(&self, _filter: Filter) -> Result<Services, PoleError> {
            Err(test_error())
        }

        async fn load_service_instances(
            &self,
            filter: Filter,
        ) -> Result<ServiceInstancesCacheItem, PoleError> {
            self.calls.lock().unwrap().push(filter.resource_key);
            let item = ServiceInstancesCacheItem::new();
            {
                let mut value = item.value.write().await;
                value.clone_from(&self.mirror_instances);
            }
            {
                let mut available = item.available_instances.write().await;
                *available = self
                    .mirror_instances
                    .iter()
                    .filter(|instance| instance.is_available())
                    .cloned()
                    .collect();
            }
            item.finish_initialize();
            Ok(item)
        }

        async fn load_config_file(&self, _filter: Filter) -> Result<ConfigFile, PoleError> {
            Err(test_error())
        }

        async fn load_config_group_files(&self, _filter: Filter) -> Result<ConfigGroup, PoleError> {
            Err(test_error())
        }

        async fn register_resource_listener(&self, _listener: Arc<dyn ResourceListener>) {}
    }

    fn test_error() -> PoleError {
        PoleError::new(ErrorCode::InternalError, "not used".to_string())
    }

    fn traffic_label_provider(arg_type: ArgumentType, key: &str) -> Option<String> {
        match (arg_type, key) {
            (ArgumentType::Header, "x-user") => Some("alice".to_string()),
            (ArgumentType::Header, "x-debug") => Some("true".to_string()),
            (ArgumentType::Method, "") => Some("GET".to_string()),
            (ArgumentType::Path, "") => Some("/orders/42".to_string()),
            _ => None,
        }
    }

    fn route_ctx() -> RouteContext {
        let mut route_info = RouteInfo::default();
        route_info.caller.namespace = "default".to_string();
        route_info.caller.name = "frontend".to_string();
        route_info.callee.namespace = "default".to_string();
        route_info.callee.name = "orders".to_string();
        route_info.traffic_label_provider = traffic_label_provider;
        RouteContext {
            route_info,
            extensions: None,
        }
    }

    fn header_match(key: &str, value: &str) -> TrafficMatchRule {
        TrafficMatchRule {
            arguments: vec![SourceMatch {
                r#type: source_match::Type::Header.into(),
                key: key.to_string(),
                value: Some(MatchString {
                    r#type: MatchStringType::Exact.into(),
                    value: value.to_string(),
                    value_type: ValueType::Text.into(),
                }),
            }],
            random_percent: 100,
            match_mode: traffic_match_rule::TrafficMatchMode::And.into(),
        }
    }

    fn api() -> Api {
        Api {
            protocol: "*".to_string(),
            method: "GET".to_string(),
            path: Some(MatchString {
                r#type: MatchStringType::Regex.into(),
                value: r"^/orders/\d+$".to_string(),
                value_type: ValueType::Text.into(),
            }),
        }
    }

    #[test]
    fn traffic_sample_hit_respects_percent_boundaries_and_is_stable() {
        let ctx = route_ctx();

        assert!(!traffic_sample_hit(&ctx, 0, "mirror"));
        assert!(traffic_sample_hit(&ctx, 100, "mirror"));
        assert_eq!(
            traffic_sample_hit(&ctx, 37, "mirror"),
            traffic_sample_hit(&ctx, 37, "mirror")
        );
    }

    #[test]
    fn traffic_governance_denies_with_matching_security_policy() {
        let result = evaluate_traffic_governance(
            &route_ctx(),
            TrafficGovernanceRules {
                security_rules: vec![TrafficSecurityRule {
                    id: "security-1".to_string(),
                    enable: true,
                    policies: vec![TrafficSecurityPolicy {
                        apis: vec![api()],
                        traffic_match_rule: Some(header_match("x-user", "alice")),
                        action: TrafficSecurityAction::TrafficSecurityDeny.into(),
                        reject_effect: Some(TrafficSecurityRejectEffect {
                            code: "DENIED".to_string(),
                            message: "blocked".to_string(),
                        }),
                        ..TrafficSecurityPolicy::default()
                    }],
                    ..TrafficSecurityRule::default()
                }],
                ..TrafficGovernanceRules::default()
            },
        );

        assert!(!result.security.allowed);
        assert_eq!(result.security.reject_effect.unwrap().code, "DENIED");
    }

    #[test]
    fn traffic_governance_returns_matching_mirror_destination() {
        let result = evaluate_traffic_governance(
            &route_ctx(),
            TrafficGovernanceRules {
                mirror_rules: vec![TrafficMirror {
                    id: "mirror-1".to_string(),
                    enable: true,
                    caller: Some(SourceService {
                        namespace: "default".to_string(),
                        service: "frontend".to_string(),
                    }),
                    rules: vec![MirrorRule {
                        traffic_match_rule: Some(header_match("x-debug", "true")),
                        apis: vec![api()],
                        destination: Some(MirrorDestination {
                            namespace: "shadow".to_string(),
                            service: "orders-shadow".to_string(),
                            labels: HashMap::new(),
                        }),
                        mirror_percent: 100,
                        ..MirrorRule::default()
                    }],
                    ..TrafficMirror::default()
                }],
                ..TrafficGovernanceRules::default()
            },
        );

        assert_eq!(result.mirrors.len(), 1);
        assert_eq!(result.mirrors[0].service, "orders-shadow");
    }

    #[test]
    fn traffic_governance_returns_matching_mock_response() {
        let result = evaluate_traffic_governance(
            &route_ctx(),
            TrafficGovernanceRules {
                mock_rules: vec![TrafficMock {
                    id: "mock-1".to_string(),
                    enable: true,
                    caller: Some(SourceService {
                        namespace: "default".to_string(),
                        service: "frontend".to_string(),
                    }),
                    rules: vec![MockRule {
                        apis: vec![api()],
                        traffic_match_rule: Some(header_match("x-user", "alice")),
                        response: Some(MockResponse {
                            code: "OK".to_string(),
                            body: "{\"ok\":true}".to_string(),
                            ..MockResponse::default()
                        }),
                        mock_percent: 100,
                        ..MockRule::default()
                    }],
                    ..TrafficMock::default()
                }],
                ..TrafficGovernanceRules::default()
            },
        );

        assert_eq!(result.mock.unwrap().code, "OK");
    }

    #[test]
    fn traffic_governance_rules_downcast_cached_rule_types() {
        let rules = traffic_governance_rules_from_cached_rules(
            vec![Box::new(TrafficSecurityRule {
                id: "security-1".to_string(),
                ..TrafficSecurityRule::default()
            })],
            vec![Box::new(TrafficMirror {
                id: "mirror-1".to_string(),
                ..TrafficMirror::default()
            })],
            vec![Box::new(TrafficMock {
                id: "mock-1".to_string(),
                ..TrafficMock::default()
            })],
        )
        .unwrap();

        assert_eq!(rules.security_rules[0].id, "security-1");
        assert_eq!(rules.mirror_rules[0].id, "mirror-1");
        assert_eq!(rules.mock_rules[0].id, "mock-1");
    }

    #[tokio::test]
    async fn traffic_governance_loads_security_mirror_and_mock_rules_from_cache() {
        let cache = FakeTrafficRuleCache::default();
        let calls = cache.calls.clone();
        let result = evaluate_traffic_governance_from_cache(
            &route_ctx(),
            Arc::new(Box::new(cache)),
            Duration::from_secs(1),
        )
        .await
        .unwrap();

        assert!(!result.security.allowed);
        assert_eq!(result.mirrors[0].service, "orders-shadow");
        assert_eq!(result.mock.unwrap().code, "OK");
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].event_type, EventType::TrafficSecurityRule);
        assert_eq!(calls[0].namespace, "default");
        assert_eq!(calls[0].filter.get("service").unwrap(), "orders");
        assert_eq!(calls[1].event_type, EventType::TrafficMirrorRule);
        assert_eq!(calls[2].event_type, EventType::TrafficMockRule);
    }

    struct BlockingMirrorSender {
        seen: Arc<Mutex<Vec<(MirrorDestination, MirrorRequest)>>>,
        started: mpsc::UnboundedSender<()>,
        release: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl MirrorSender for BlockingMirrorSender {
        async fn send(
            &self,
            destination: MirrorDestination,
            request: MirrorRequest,
        ) -> Result<(), PoleError> {
            self.seen.lock().unwrap().push((destination, request));
            let _ = self.started.send(());
            self.release.notified().await;
            Ok(())
        }
    }

    #[tokio::test]
    async fn mirror_dispatch_spawns_request_copies_without_blocking_caller() {
        let (started_tx, mut started_rx) = mpsc::unbounded_channel();
        let release = Arc::new(Notify::new());
        let sender = Arc::new(BlockingMirrorSender {
            seen: Arc::new(Mutex::new(Vec::new())),
            started: started_tx,
            release: release.clone(),
        });
        let request = MirrorRequest {
            method: "POST".to_string(),
            path: "/orders/42".to_string(),
            headers: HashMap::from([("x-debug".to_string(), "true".to_string())]),
            body: b"{\"id\":42}".to_vec(),
        };
        let destinations = vec![
            MirrorDestination {
                namespace: "shadow".to_string(),
                service: "orders-shadow-a".to_string(),
                labels: HashMap::new(),
            },
            MirrorDestination {
                namespace: "shadow".to_string(),
                service: "orders-shadow-b".to_string(),
                labels: HashMap::new(),
            },
        ];

        let handles = dispatch_mirror_requests(sender.clone(), destinations, request.clone());

        assert_eq!(handles.len(), 2);
        started_rx.recv().await.unwrap();
        started_rx.recv().await.unwrap();
        let seen = sender.seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].1, request);
        drop(seen);

        release.notify_waiters();
        for handle in handles {
            handle.await.unwrap();
        }
    }

    #[tokio::test]
    async fn http_mirror_sender_resolves_destination_from_available_service_instance() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let received = Arc::new(Mutex::new(String::new()));
        let received_clone = received.clone();
        let _server_task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = stream.read(&mut buf).await.unwrap();
            *received_clone.lock().unwrap() = String::from_utf8_lossy(&buf[..n]).to_string();
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        });
        let cache = FakeTrafficRuleCache {
            mirror_instances: vec![
                Instance {
                    namespace: "shadow".to_string(),
                    service: "orders-shadow".to_string(),
                    ip: "127.0.0.1".to_string(),
                    port: address.port() as u32,
                    health: true,
                    isolated: false,
                    weight: 100,
                    ..Instance::default()
                },
                Instance {
                    namespace: "shadow".to_string(),
                    service: "orders-shadow".to_string(),
                    ip: "127.0.0.1".to_string(),
                    port: 1,
                    health: false,
                    isolated: false,
                    weight: 100,
                    ..Instance::default()
                },
            ],
            ..FakeTrafficRuleCache::default()
        };
        let sender = HttpMirrorSender::with_resource_cache(
            Arc::new(Box::new(cache)),
            Duration::from_secs(1),
        );
        let request = MirrorRequest {
            method: "PUT".to_string(),
            path: "/orders/42".to_string(),
            headers: HashMap::new(),
            body: b"{}".to_vec(),
        };

        sender
            .send(
                MirrorDestination {
                    namespace: "shadow".to_string(),
                    service: "orders-shadow".to_string(),
                    labels: HashMap::new(),
                },
                request,
            )
            .await
            .unwrap();

        let received = received.lock().unwrap().clone();
        assert!(received.starts_with("PUT /orders/42 HTTP/1.1"));
    }

    #[tokio::test]
    async fn http_mirror_sender_posts_request_to_configured_destination() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let received = Arc::new(Mutex::new(String::new()));
        let received_clone = received.clone();
        let _server_task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = stream.read(&mut buf).await.unwrap();
            *received_clone.lock().unwrap() = String::from_utf8_lossy(&buf[..n]).to_string();
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        });
        let sender = HttpMirrorSender::new(
            HashMap::from([(
                "shadow#orders-shadow".to_string(),
                format!("http://{}", address),
            )]),
            Duration::from_secs(1),
        );
        let request = MirrorRequest {
            method: "POST".to_string(),
            path: "/orders/42?debug=true".to_string(),
            headers: HashMap::from([("x-debug".to_string(), "true".to_string())]),
            body: b"{\"id\":42}".to_vec(),
        };

        sender
            .send(
                MirrorDestination {
                    namespace: "shadow".to_string(),
                    service: "orders-shadow".to_string(),
                    labels: HashMap::new(),
                },
                request,
            )
            .await
            .unwrap();

        let received = received.lock().unwrap().clone();
        assert!(received.starts_with("POST /orders/42?debug=true HTTP/1.1"));
        assert!(received.contains("\r\nx-debug: true\r\n"));
        assert!(received.contains("\r\ncontent-length: 9\r\n"));
        assert!(received.ends_with("{\"id\":42}"));
    }

    #[test]
    fn grpc_mirror_body_wraps_uncompressed_unary_message() {
        let body = GrpcMirrorBody::uncompressed_unary(b"abc");

        assert_eq!(body, vec![0, 0, 0, 0, 3, b'a', b'b', b'c']);
    }

    #[tokio::test]
    async fn grpc_mirror_sender_posts_framed_body_to_configured_destination() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let received = Arc::new(Mutex::new(None));
        let received_clone = received.clone();
        let _server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut connection = server::handshake(stream).await.unwrap();
            let Some(request) = connection.accept().await else {
                panic!("expected grpc mirror request");
            };
            let (request, mut respond) = request.unwrap();
            let path = request.uri().path().to_string();
            let content_type = request
                .headers()
                .get("content-type")
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            let x_debug = request
                .headers()
                .get("x-debug")
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            let mut body = request.into_body();
            let mut received_body = Vec::new();
            while let Some(chunk) = body.data().await {
                received_body.extend_from_slice(&chunk.unwrap());
            }
            *received_clone.lock().unwrap() = Some((path, content_type, x_debug, received_body));
            let response = Response::builder()
                .status(200)
                .header("content-type", "application/grpc")
                .body(())
                .unwrap();
            respond.send_response(response, true).unwrap();
            let _ = tokio::time::timeout(Duration::from_millis(100), connection.accept()).await;
        });
        let sender = GrpcMirrorSender::new(
            HashMap::from([(
                "shadow#orders-shadow".to_string(),
                format!("grpc://{}", address),
            )]),
            Duration::from_secs(1),
        );
        let request = MirrorRequest {
            method: "POST".to_string(),
            path: "/example.OrderService/Create".to_string(),
            headers: HashMap::from([("x-debug".to_string(), "true".to_string())]),
            body: GrpcMirrorBody::uncompressed_unary(b"abc"),
        };

        sender
            .send(
                MirrorDestination {
                    namespace: "shadow".to_string(),
                    service: "orders-shadow".to_string(),
                    labels: HashMap::new(),
                },
                request.clone(),
            )
            .await
            .unwrap();

        let (path, content_type, x_debug, body) = received.lock().unwrap().clone().unwrap();
        assert_eq!(path, "/example.OrderService/Create");
        assert_eq!(content_type, "application/grpc");
        assert_eq!(x_debug, "true");
        assert_eq!(body, request.body);
    }
}
