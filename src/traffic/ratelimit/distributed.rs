use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use dashmap::DashMap;
use pole_specification::{
    polaris::metric::v2::{
        rate_limit_grpc_client::RateLimitGrpcClient, LimitTarget, Mode, QuotaAccounting,
        QuotaConsumption as RemoteQuotaConsumption, QuotaCounter, QuotaMode as RemoteQuotaMode,
        QuotaReservation, QuotaReserveRequest, QuotaSettleRequest, QuotaTotal, QuotaUpdateRequest,
        RateLimitCmd, RateLimitInitRequest, RateLimitRequest as RemoteRateLimitRequest,
        RateLimitResponse as RemoteRateLimitResponse, TimeAdjustRequest,
    },
    v1::{limit_trigger, limit_trigger::AmountMode, LimitTrigger, RateLimit},
};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::time::timeout;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Endpoint;

use crate::{
    core::model::{
        error::{ErrorCode, PoleError},
        naming::Instance,
    },
    traffic::ratelimit::{
        api::{QuotaConsumption, QuotaLeaseBackend},
        req::{QuotaRequest, QuotaResource},
    },
};

const RATE_LIMIT_SUCCESS_CODE: u32 = 200_000;
const TIME_ADJUST_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct WindowKey {
    endpoint: String,
    cluster_namespace: String,
    cluster_service: String,
    target_namespace: String,
    target_service: String,
    labels: String,
    rule_id: String,
    trigger_name: String,
    rule_revision: String,
}

impl WindowKey {
    fn from_rule(
        req: &QuotaRequest,
        rule: &RateLimit,
        trigger: &LimitTrigger,
        endpoint: &str,
    ) -> Result<Self, PoleError> {
        let cluster = rule.cluster.as_ref().ok_or_else(|| {
            PoleError::new(
                ErrorCode::InvalidRule,
                format!(
                    "global rate limit rule {} does not declare cluster",
                    rule.id
                ),
            )
        })?;
        if cluster.namespace.is_empty() || cluster.service.is_empty() {
            return Err(PoleError::new(
                ErrorCode::InvalidRule,
                format!("global rate limit rule {} has an invalid cluster", rule.id),
            ));
        }
        Ok(Self {
            endpoint: endpoint.to_string(),
            cluster_namespace: cluster.namespace.clone(),
            cluster_service: cluster.service.clone(),
            target_namespace: req.namespace.clone(),
            target_service: req.service.clone(),
            labels: super::default::remote_quota_labels(req, trigger),
            rule_id: rule.id.clone(),
            trigger_name: trigger.name.clone(),
            rule_revision: rule.revision.clone(),
        })
    }

    fn target(&self) -> LimitTarget {
        LimitTarget {
            namespace: self.target_namespace.clone(),
            service: self.target_service.clone(),
            labels: self.labels.clone(),
            labels_list: Vec::new(),
        }
    }
}

struct InitializedWindow {
    session_id: u64,
    client_key: u32,
    counters: Vec<QuotaCounter>,
}

#[derive(Clone)]
struct LeaseCounterGroup {
    resource: QuotaResource,
    amount: u32,
    accounting: QuotaAccounting,
    counter_keys: Vec<u32>,
}

struct QuotaWindow {
    key: WindowKey,
    initialized: Mutex<Option<InitializedWindow>>,
    initialize_lock: Mutex<()>,
}

impl QuotaWindow {
    fn new(key: WindowKey) -> Self {
        Self {
            key,
            initialized: Mutex::new(None),
            initialize_lock: Mutex::new(()),
        }
    }
}

type PendingResponse = oneshot::Sender<Result<RemoteRateLimitResponse, String>>;

struct QuotaSession {
    id: u64,
    sender: mpsc::Sender<RemoteRateLimitRequest>,
    pending: Mutex<Option<PendingResponse>>,
    request_lock: Mutex<()>,
    clock_offset_ms: AtomicI64,
    last_time_adjust_ms: AtomicI64,
    time_adjust_lock: Mutex<()>,
    endpoint: String,
    request_timeout: Duration,
    healthy: AtomicBool,
}

impl QuotaSession {
    async fn connect(
        id: u64,
        endpoint: String,
        request_timeout: Duration,
    ) -> Result<Arc<Self>, PoleError> {
        let channel = Endpoint::from_shared(format!("http://{endpoint}"))
            .map_err(|error| network_error("create rate limit endpoint", error))?
            .connect_lazy();
        let mut client = RateLimitGrpcClient::new(channel);
        let adjust_started_at = now_millis();
        let adjustment = timeout(request_timeout, client.time_adjust(TimeAdjustRequest {}))
            .await
            .map_err(|_| api_timeout("rate limit time adjustment timed out"))?
            .map_err(|error| network_error("adjust rate limit server time", error))?
            .into_inner();
        let adjust_finished_at = now_millis();
        let local_midpoint = adjust_started_at + (adjust_finished_at - adjust_started_at) / 2;
        let clock_offset_ms = adjustment.server_timestamp - local_midpoint;

        let (sender, receiver) = mpsc::channel(128);
        let response = timeout(
            request_timeout,
            client.service(ReceiverStream::new(receiver)),
        )
        .await
        .map_err(|_| api_timeout("open rate limit stream timed out"))?
        .map_err(|error| network_error("open rate limit stream", error))?;

        let session = Arc::new(Self {
            id,
            sender,
            pending: Mutex::new(None),
            request_lock: Mutex::new(()),
            clock_offset_ms: AtomicI64::new(clock_offset_ms),
            last_time_adjust_ms: AtomicI64::new(adjust_finished_at),
            time_adjust_lock: Mutex::new(()),
            endpoint,
            request_timeout,
            healthy: AtomicBool::new(true),
        });
        let response_session = session.clone();
        tokio::spawn(async move {
            let mut stream = response.into_inner();
            loop {
                match stream.message().await {
                    Ok(Some(message)) => {
                        if let Some(pending) = response_session.pending.lock().await.take() {
                            let _ = pending.send(Ok(message));
                        }
                    }
                    Ok(None) => {
                        response_session
                            .mark_failed("rate limit stream closed".to_string())
                            .await;
                        return;
                    }
                    Err(error) => {
                        response_session
                            .mark_failed(format!("rate limit stream receive failed: {error}"))
                            .await;
                        return;
                    }
                }
            }
        });
        Ok(session)
    }

    async fn mark_failed(&self, message: String) {
        self.healthy.store(false, Ordering::Release);
        if let Some(pending) = self.pending.lock().await.take() {
            let _ = pending.send(Err(message));
        }
    }

    async fn round_trip(
        &self,
        request: RemoteRateLimitRequest,
    ) -> Result<RemoteRateLimitResponse, PoleError> {
        let _request_guard = self.request_lock.lock().await;
        if !self.healthy.load(Ordering::Acquire) || self.sender.is_closed() {
            return Err(network_error_message("rate limit stream is unavailable"));
        }
        let expected_cmd = request.cmd;
        let (sender, receiver) = oneshot::channel();
        *self.pending.lock().await = Some(sender);
        if self.sender.send(request).await.is_err() {
            self.mark_failed("rate limit stream is unavailable".to_string())
                .await;
            return Err(network_error_message("rate limit stream is unavailable"));
        }
        let response = match timeout(self.request_timeout, receiver).await {
            Ok(Ok(Ok(response))) => response,
            Ok(Ok(Err(message))) => return Err(network_error_message(message)),
            Ok(Err(_)) => return Err(network_error_message("rate limit response was canceled")),
            Err(_) => {
                self.mark_failed("rate limit request timed out".to_string())
                    .await;
                return Err(api_timeout("rate limit request timed out"));
            }
        };
        if response.cmd != expected_cmd {
            self.mark_failed("rate limit response command is out of order".to_string())
                .await;
            return Err(PoleError::new(
                ErrorCode::InvalidResponse,
                "rate limit response command is out of order".to_string(),
            ));
        }
        Ok(response)
    }

    async fn ensure_window(
        &self,
        window: &QuotaWindow,
        client_id: &str,
        trigger: &LimitTrigger,
    ) -> Result<(u32, Vec<QuotaCounter>), PoleError> {
        if let Some(initialized) = window.initialized.lock().await.as_ref() {
            if initialized.session_id == self.id {
                return Ok((initialized.client_key, initialized.counters.clone()));
            }
        }
        let _initialize_guard = window.initialize_lock.lock().await;
        if let Some(initialized) = window.initialized.lock().await.as_ref() {
            if initialized.session_id == self.id {
                return Ok((initialized.client_key, initialized.counters.clone()));
            }
        }
        let response = self
            .round_trip(remote_request(
                RateLimitCmd::Init,
                Some(build_init_request(client_id, &window.key, trigger)?),
                None,
                None,
                None,
            ))
            .await?;
        let init = response.rate_limit_init_response.ok_or_else(|| {
            PoleError::new(
                ErrorCode::InvalidResponse,
                "rate limit init response is missing".to_string(),
            )
        })?;
        if init.code != RATE_LIMIT_SUCCESS_CODE {
            return Err(PoleError::new(
                ErrorCode::ServerError,
                format!("rate limit init returned code {}", init.code),
            ));
        }
        if init.counters.is_empty() {
            return Err(PoleError::new(
                ErrorCode::InvalidResponse,
                "rate limit init returned no counters".to_string(),
            ));
        }
        let initialized = InitializedWindow {
            session_id: self.id,
            client_key: init.client_key,
            counters: init.counters,
        };
        let result = (initialized.client_key, initialized.counters.clone());
        *window.initialized.lock().await = Some(initialized);
        Ok(result)
    }

    async fn reserve(
        self: &Arc<Self>,
        client_key: u32,
        groups: Vec<LeaseCounterGroup>,
        req: &QuotaRequest,
    ) -> Result<DistributedReserveResult, PoleError> {
        self.adjust_time_if_due().await;
        let response = self
            .round_trip(remote_request(
                RateLimitCmd::Reserve,
                None,
                Some(QuotaReserveRequest {
                    client_key,
                    idempotency_key: uuid::Uuid::new_v4().to_string(),
                    reservations: groups
                        .iter()
                        .flat_map(|group| {
                            group
                                .counter_keys
                                .iter()
                                .map(move |counter_key| QuotaReservation {
                                    counter_key: *counter_key,
                                    amount: group.amount,
                                })
                        })
                        .collect(),
                    ttl_seconds: ttl_seconds(req.lease_ttl),
                    timestamp: self.server_timestamp(),
                }),
                None,
                None,
            ))
            .await?;
        let reserve = response.quota_reserve_response.ok_or_else(|| {
            PoleError::new(
                ErrorCode::InvalidResponse,
                "quota reserve response is missing".to_string(),
            )
        })?;
        if reserve.code != RATE_LIMIT_SUCCESS_CODE {
            return Ok(DistributedReserveResult::Rejected(format!(
                "distributed rate limit quota exhausted, code {}",
                reserve.code
            )));
        }
        if reserve.lease_id.is_empty() {
            return Err(PoleError::new(
                ErrorCode::InvalidResponse,
                "quota reserve response has an empty lease id".to_string(),
            ));
        }
        Ok(DistributedReserveResult::Reserved(Arc::new(
            DistributedLeaseBackend {
                session: self.clone(),
                client_key,
                lease_id: reserve.lease_id,
                groups,
            },
        )))
    }

    async fn update_lease(
        &self,
        client_key: u32,
        lease_id: &str,
        consumptions: Vec<RemoteQuotaConsumption>,
        sequence: u64,
    ) -> Result<(), PoleError> {
        self.adjust_time_if_due().await;
        let response = self
            .round_trip(remote_request(
                RateLimitCmd::Update,
                None,
                None,
                Some(QuotaUpdateRequest {
                    client_key,
                    lease_id: lease_id.to_string(),
                    consumptions: consumptions.clone(),
                    sequence,
                    timestamp: self.server_timestamp(),
                }),
                None,
            ))
            .await?;
        let update = response.quota_update_response.ok_or_else(|| {
            PoleError::new(
                ErrorCode::InvalidResponse,
                "quota update response is missing".to_string(),
            )
        })?;
        if update.code != RATE_LIMIT_SUCCESS_CODE {
            return Err(PoleError::new(
                ErrorCode::ServerError,
                format!("quota update returned code {}", update.code),
            ));
        }
        if update.sequence != sequence
            || normalize_consumptions(update.consumptions) != normalize_consumptions(consumptions)
        {
            return Err(PoleError::new(
                ErrorCode::InvalidResponse,
                "quota update response does not match the request".to_string(),
            ));
        }
        Ok(())
    }

    async fn settle_lease(
        &self,
        client_key: u32,
        lease_id: &str,
        groups: &[LeaseCounterGroup],
        consumptions: Vec<RemoteQuotaConsumption>,
        sequence: u64,
    ) -> Result<(), PoleError> {
        self.adjust_time_if_due().await;
        let requested = normalize_consumptions(consumptions.clone());
        let response = self
            .round_trip(remote_request(
                RateLimitCmd::Settle,
                None,
                None,
                None,
                Some(QuotaSettleRequest {
                    client_key,
                    lease_id: lease_id.to_string(),
                    consumptions,
                    sequence,
                    timestamp: self.server_timestamp(),
                }),
            ))
            .await?;
        let settle = response.quota_settle_response.ok_or_else(|| {
            PoleError::new(
                ErrorCode::InvalidResponse,
                "quota settle response is missing".to_string(),
            )
        })?;
        if settle.code != RATE_LIMIT_SUCCESS_CODE {
            return Err(PoleError::new(
                ErrorCode::ServerError,
                format!("quota settle returned code {}", settle.code),
            ));
        }
        let settlements = settle
            .settlements
            .into_iter()
            .map(|settlement| {
                (
                    settlement.counter_key,
                    (settlement.consumed_total, settlement.returned_amount),
                )
            })
            .collect::<HashMap<_, _>>();
        let valid = groups.iter().all(|group| {
            group.counter_keys.iter().all(|counter_key| {
                let consumed_total = requested
                    .iter()
                    .find(|(key, _)| key == counter_key)
                    .map(|(_, total)| *total)
                    .unwrap_or(u32::MAX);
                let expected_returned = match group.accounting {
                    QuotaAccounting::Consumable => group.amount.saturating_sub(consumed_total),
                    QuotaAccounting::Occupancy => group.amount,
                };
                settlements.get(counter_key) == Some(&(consumed_total, expected_returned))
            })
        }) && settlements.len()
            == groups
                .iter()
                .map(|group| group.counter_keys.len())
                .sum::<usize>();
        if !valid {
            return Err(PoleError::new(
                ErrorCode::InvalidResponse,
                "quota settle response does not match the request".to_string(),
            ));
        }
        Ok(())
    }

    fn server_timestamp(&self) -> i64 {
        now_millis() + self.clock_offset_ms.load(Ordering::Relaxed)
    }

    async fn adjust_time_if_due(&self) {
        let now = now_millis();
        if now - self.last_time_adjust_ms.load(Ordering::Acquire)
            < TIME_ADJUST_INTERVAL.as_millis() as i64
        {
            return;
        }
        let _guard = self.time_adjust_lock.lock().await;
        let started_at = now_millis();
        if started_at - self.last_time_adjust_ms.load(Ordering::Acquire)
            < TIME_ADJUST_INTERVAL.as_millis() as i64
        {
            return;
        }
        let channel = match Endpoint::from_shared(format!("http://{}", self.endpoint)) {
            Ok(endpoint) => endpoint.connect_lazy(),
            Err(_) => return,
        };
        let mut client = RateLimitGrpcClient::new(channel);
        let adjustment = timeout(
            self.request_timeout,
            client.time_adjust(TimeAdjustRequest {}),
        )
        .await;
        let finished_at = now_millis();
        self.last_time_adjust_ms
            .store(finished_at, Ordering::Release);
        if let Ok(Ok(response)) = adjustment {
            let midpoint = started_at + (finished_at - started_at) / 2;
            self.clock_offset_ms.store(
                response.into_inner().server_timestamp - midpoint,
                Ordering::Release,
            );
        }
    }
}

struct DistributedLeaseBackend {
    session: Arc<QuotaSession>,
    client_key: u32,
    lease_id: String,
    groups: Vec<LeaseCounterGroup>,
}

#[async_trait::async_trait]
impl QuotaLeaseBackend for DistributedLeaseBackend {
    async fn update(
        &self,
        consumptions: Vec<QuotaConsumption>,
        sequence: u64,
    ) -> Result<(), PoleError> {
        let remote = expand_consumptions(&self.groups, &consumptions)?;
        self.session
            .update_lease(self.client_key, &self.lease_id, remote, sequence)
            .await
    }

    async fn finish(
        &self,
        consumptions: Vec<QuotaConsumption>,
        sequence: u64,
    ) -> Result<(), PoleError> {
        let remote = expand_consumptions(&self.groups, &consumptions)?;
        self.session
            .settle_lease(
                self.client_key,
                &self.lease_id,
                &self.groups,
                remote,
                sequence,
            )
            .await
    }

    fn abandon(&self, consumptions: Vec<QuotaConsumption>, sequence: u64) {
        let Ok(remote) = expand_consumptions(&self.groups, &consumptions) else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!("quota lease dropped outside a Tokio runtime; waiting for server TTL");
            return;
        };
        let session = self.session.clone();
        let client_key = self.client_key;
        let lease_id = self.lease_id.clone();
        let groups = self.groups.clone();
        runtime.spawn(async move {
            if let Err(error) = session
                .settle_lease(
                    client_key,
                    &lease_id,
                    &groups,
                    remote,
                    sequence,
                )
                .await
            {
                tracing::warn!(error = %error, "quota lease drop settlement failed; waiting for server TTL");
            }
        });
    }
}

pub(super) enum DistributedReserveResult {
    Reserved(Arc<dyn QuotaLeaseBackend>),
    Rejected(String),
}

#[derive(Clone, Copy)]
pub(super) struct DistributedQuotaSpec<'a> {
    pub rule: &'a RateLimit,
    pub trigger: &'a LimitTrigger,
    pub amount: u32,
}

pub(super) struct DistributedQuotaManager {
    sessions: DashMap<String, Arc<QuotaSession>>,
    windows: DashMap<WindowKey, Arc<QuotaWindow>>,
    endpoint_bindings: DashMap<String, String>,
    connect_lock: Mutex<()>,
    next_session_id: AtomicU64,
}

impl Default for DistributedQuotaManager {
    fn default() -> Self {
        Self {
            sessions: DashMap::new(),
            windows: DashMap::new(),
            endpoint_bindings: DashMap::new(),
            connect_lock: Mutex::new(()),
            next_session_id: AtomicU64::new(1),
        }
    }
}

impl DistributedQuotaManager {
    pub(super) async fn reserve_many(
        &self,
        req: &QuotaRequest,
        specs: &[DistributedQuotaSpec<'_>],
        instances: &[Instance],
        client_id: &str,
    ) -> Result<DistributedReserveResult, PoleError> {
        if specs.is_empty() {
            return Err(PoleError::new(
                ErrorCode::ApiInvalidArgument,
                "distributed quota reservation is empty".to_string(),
            ));
        }
        ensure_same_cluster(specs)?;
        let affinity = specs
            .iter()
            .map(|spec| window_affinity(req, spec.rule, spec.trigger))
            .collect::<Result<Vec<_>, _>>()?
            .join("|");
        let endpoint = self.endpoint_for(instances, &affinity)?;
        let session = self.session_for(&endpoint, req.timeout).await?;
        let mut client_key = None;
        let mut groups = Vec::with_capacity(specs.len());
        let mut counter_keys = HashSet::new();
        for spec in specs {
            let key = WindowKey::from_rule(req, spec.rule, spec.trigger, &endpoint)?;
            self.remove_stale_windows(&key);
            let window = self
                .windows
                .entry(key.clone())
                .or_insert_with(|| Arc::new(QuotaWindow::new(key)))
                .clone();
            let (current_client_key, counters) = session
                .ensure_window(&window, client_id, spec.trigger)
                .await?;
            if let Some(client_key) = client_key {
                if client_key != current_client_key {
                    return Err(PoleError::new(
                        ErrorCode::InvalidResponse,
                        "rate limit init responses disagree on client key".to_string(),
                    ));
                }
            } else {
                client_key = Some(current_client_key);
            }
            if counters
                .iter()
                .any(|counter| !counter_keys.insert(counter.counter_key))
            {
                return Err(PoleError::new(
                    ErrorCode::InvalidResponse,
                    "rate limit init returned duplicate counter keys".to_string(),
                ));
            }
            groups.push(LeaseCounterGroup {
                resource: quota_resource(spec.trigger.resource())?,
                amount: spec.amount,
                accounting: quota_accounting(spec.trigger.resource()),
                counter_keys: counters
                    .into_iter()
                    .map(|counter| counter.counter_key)
                    .collect(),
            });
        }
        session.reserve(client_key.unwrap(), groups, req).await
    }

    fn remove_stale_windows(&self, current: &WindowKey) {
        let stale = self
            .windows
            .iter()
            .filter_map(|entry| {
                let key = entry.key();
                (key.cluster_namespace == current.cluster_namespace
                    && key.cluster_service == current.cluster_service
                    && key.target_namespace == current.target_namespace
                    && key.target_service == current.target_service
                    && key.labels == current.labels
                    && key.rule_id == current.rule_id
                    && key.trigger_name == current.trigger_name
                    && key != current)
                    .then(|| key.clone())
            })
            .collect::<Vec<_>>();
        for key in stale {
            self.windows.remove(&key);
        }
    }

    async fn session_for(
        &self,
        endpoint: &str,
        request_timeout: Duration,
    ) -> Result<Arc<QuotaSession>, PoleError> {
        if let Some(session) = self.sessions.get(endpoint) {
            if session.healthy.load(Ordering::Acquire) && !session.sender.is_closed() {
                return Ok(session.clone());
            }
        }
        let _guard = self.connect_lock.lock().await;
        if let Some(session) = self.sessions.get(endpoint) {
            if session.healthy.load(Ordering::Acquire) && !session.sender.is_closed() {
                return Ok(session.clone());
            }
        }
        self.sessions.remove(endpoint);
        let id = self.next_session_id.fetch_add(1, Ordering::Relaxed);
        let session = QuotaSession::connect(id, endpoint.to_string(), request_timeout).await?;
        self.sessions.insert(endpoint.to_string(), session.clone());
        Ok(session)
    }

    fn endpoint_for(&self, instances: &[Instance], affinity: &str) -> Result<String, PoleError> {
        let available = available_grpc_endpoints(instances)?;
        if let Some(endpoint) = self.endpoint_bindings.get(affinity) {
            if available.binary_search(endpoint.value()).is_ok() {
                return Ok(endpoint.clone());
            }
        }
        let endpoint = select_endpoint_from_available(&available, affinity);
        self.endpoint_bindings
            .insert(affinity.to_string(), endpoint.clone());
        Ok(endpoint)
    }
}

fn build_init_request(
    client_id: &str,
    key: &WindowKey,
    trigger: &LimitTrigger,
) -> Result<RateLimitInitRequest, PoleError> {
    let accounting = quota_accounting(trigger.resource());
    let totals = match trigger.resource() {
        limit_trigger::Resource::Qps | limit_trigger::Resource::Token => trigger
            .amounts
            .iter()
            .filter(|amount| amount.max_amount > 0)
            .map(|amount| QuotaTotal {
                mode: quota_mode(trigger).into(),
                duration: amount_duration_seconds(amount),
                max_amount: amount.max_amount,
                accounting: accounting.into(),
            })
            .collect::<Vec<_>>(),
        limit_trigger::Resource::Concurrency => trigger
            .concurrency_amount
            .as_ref()
            .filter(|amount| amount.max_amount > 0)
            .map(|amount| {
                vec![QuotaTotal {
                    mode: quota_mode(trigger).into(),
                    duration: 0,
                    max_amount: amount.max_amount,
                    accounting: accounting.into(),
                }]
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    if totals.is_empty() {
        return Err(PoleError::new(
            ErrorCode::InvalidRule,
            format!("rate limit trigger {} has no quota total", trigger.name),
        ));
    }
    Ok(RateLimitInitRequest {
        target: Some(key.target()),
        client_id: client_id.to_string(),
        totals,
        slide_count: 0,
        mode: Mode::BatchOccupy.into(),
    })
}

fn remote_request(
    cmd: RateLimitCmd,
    init: Option<RateLimitInitRequest>,
    reserve: Option<QuotaReserveRequest>,
    update: Option<QuotaUpdateRequest>,
    settle: Option<QuotaSettleRequest>,
) -> RemoteRateLimitRequest {
    RemoteRateLimitRequest {
        cmd: cmd.into(),
        rate_limit_init_request: init,
        quota_reserve_request: reserve,
        rate_limit_batch_init_request: None,
        quota_update_request: update,
        quota_settle_request: settle,
    }
}

fn quota_accounting(resource: limit_trigger::Resource) -> QuotaAccounting {
    match resource {
        limit_trigger::Resource::Concurrency => QuotaAccounting::Occupancy,
        _ => QuotaAccounting::Consumable,
    }
}

fn quota_resource(resource: limit_trigger::Resource) -> Result<QuotaResource, PoleError> {
    match resource {
        limit_trigger::Resource::Qps => Ok(QuotaResource::Qps),
        limit_trigger::Resource::Token => Ok(QuotaResource::Token),
        limit_trigger::Resource::Concurrency => Ok(QuotaResource::Concurrency),
        _ => Err(PoleError::new(
            ErrorCode::InvalidRule,
            "unsupported distributed quota resource".to_string(),
        )),
    }
}

fn expand_consumptions(
    groups: &[LeaseCounterGroup],
    consumptions: &[QuotaConsumption],
) -> Result<Vec<RemoteQuotaConsumption>, PoleError> {
    let mut expanded = Vec::new();
    for group in groups {
        let consumed_total = consumptions
            .iter()
            .find(|consumption| consumption.resource == group.resource)
            .map(|consumption| consumption.consumed_total)
            .ok_or_else(|| {
                PoleError::new(
                    ErrorCode::ApiInvalidArgument,
                    format!("quota consumption for {:?} is missing", group.resource),
                )
            })?;
        expanded.extend(
            group
                .counter_keys
                .iter()
                .map(|counter_key| RemoteQuotaConsumption {
                    counter_key: *counter_key,
                    consumed_total,
                }),
        );
    }
    Ok(expanded)
}

fn quota_mode(trigger: &LimitTrigger) -> RemoteQuotaMode {
    match trigger.amount_mode() {
        AmountMode::ShareEqually => RemoteQuotaMode::Divide,
        AmountMode::GlobalTotal => RemoteQuotaMode::Whole,
    }
}

fn normalize_consumptions(consumptions: Vec<RemoteQuotaConsumption>) -> Vec<(u32, u32)> {
    let mut normalized = consumptions
        .into_iter()
        .map(|consumption| (consumption.counter_key, consumption.consumed_total))
        .collect::<Vec<_>>();
    normalized.sort_unstable();
    normalized
}

fn ttl_seconds(ttl: Duration) -> u32 {
    let rounded = ttl
        .as_secs()
        .saturating_add(u64::from(ttl.subsec_nanos() > 0))
        .max(1);
    rounded.min(u64::from(u32::MAX)) as u32
}

fn amount_duration_seconds(amount: &pole_specification::v1::Amount) -> u32 {
    amount
        .valid_duration
        .map(|duration| duration.seconds.max(1) as u32)
        .unwrap_or(1)
}

fn window_affinity(
    req: &QuotaRequest,
    rule: &RateLimit,
    trigger: &LimitTrigger,
) -> Result<String, PoleError> {
    let cluster = rule.cluster.as_ref().ok_or_else(|| {
        PoleError::new(
            ErrorCode::InvalidRule,
            format!(
                "global rate limit rule {} does not declare cluster",
                rule.id
            ),
        )
    })?;
    Ok(format!(
        "{}#{}#{}#{}#{}",
        cluster.namespace,
        cluster.service,
        req.namespace,
        req.service,
        super::default::remote_quota_labels(req, trigger)
    ))
}

fn ensure_same_cluster(specs: &[DistributedQuotaSpec<'_>]) -> Result<(), PoleError> {
    let first = specs
        .first()
        .and_then(|spec| spec.rule.cluster.as_ref())
        .ok_or_else(|| {
            PoleError::new(
                ErrorCode::InvalidRule,
                "global rate limit rule does not declare cluster".to_string(),
            )
        })?;
    if specs.iter().all(|spec| {
        spec.rule
            .cluster
            .as_ref()
            .map(|cluster| cluster.namespace == first.namespace && cluster.service == first.service)
            == Some(true)
    }) {
        return Ok(());
    }
    Err(PoleError::new(
        ErrorCode::InvalidRule,
        "one quota lease cannot span multiple limiter clusters".to_string(),
    ))
}

fn available_grpc_endpoints(instances: &[Instance]) -> Result<Vec<String>, PoleError> {
    let mut available = instances
        .iter()
        .filter(|instance| {
            instance.is_available() && instance.protocol.eq_ignore_ascii_case("grpc")
        })
        .map(|instance| format!("{}:{}", instance.ip, instance.port))
        .collect::<Vec<_>>();
    if available.is_empty() {
        return Err(PoleError::new(
            ErrorCode::InstanceNotFound,
            "rate limit cluster has no available grpc instance".to_string(),
        ));
    }
    available.sort_unstable();
    available.dedup();
    Ok(available)
}

fn select_endpoint_from_available(available: &[String], affinity: &str) -> String {
    let digest = Sha256::digest(affinity.as_bytes());
    let hash = u64::from_be_bytes(
        digest[..8]
            .try_into()
            .expect("sha256 prefix is eight bytes"),
    );
    available[hash as usize % available.len()].clone()
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn api_timeout(message: impl Into<String>) -> PoleError {
    PoleError::new(ErrorCode::ApiTimeout, message.into())
}

fn network_error(context: &str, error: impl std::fmt::Display) -> PoleError {
    network_error_message(format!("{context}: {error}"))
}

fn network_error_message(message: impl Into<String>) -> PoleError {
    PoleError::new(ErrorCode::NetworkError, message.into())
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        pin::Pin,
        sync::{
            atomic::{AtomicU32, Ordering},
            Arc, Mutex as StdMutex,
        },
    };

    use pole_specification::{
        polaris::metric::v2::{
            rate_limit_grpc_server::{RateLimitGrpc, RateLimitGrpcServer},
            QuotaLeft, QuotaReserveResponse, QuotaSettleResponse, QuotaSettlement,
            QuotaUpdateResponse, RateLimitInitResponse, RateLimitResponse, TimeAdjustResponse,
        },
        v1::{rate_limit, Amount, ConcurrencyAmount, RateLimitCluster},
    };
    use prost_types::Duration as ProstDuration;
    use tokio::net::TcpListener;
    use tokio_stream::{
        wrappers::{ReceiverStream, TcpListenerStream},
        Stream,
    };
    use tonic::{Request, Response, Status};

    use super::*;
    use crate::{core::model::ArgumentType, traffic::ratelimit::api::QuotaLease};

    const REJECTED_CODE: u32 = 429_001;
    const INVALID_LEASE_CODE: u32 = 400_001;

    #[derive(Clone, Copy)]
    struct CounterState {
        max_amount: u32,
        accounting: QuotaAccounting,
        committed: u32,
        reserved: u32,
    }

    struct ServerLease {
        reservations: HashMap<u32, u32>,
        consumptions: HashMap<u32, u32>,
        sequence: u64,
    }

    #[derive(Default)]
    struct ServerState {
        counters: HashMap<u32, CounterState>,
        leases: HashMap<String, ServerLease>,
        settled: HashMap<String, Vec<QuotaSettlement>>,
        next_lease: u64,
    }

    #[derive(Clone, Default)]
    struct Observation {
        init_count: Arc<AtomicU32>,
        reserve_count: Arc<AtomicU32>,
        update_count: Arc<AtomicU32>,
        settle_count: Arc<AtomicU32>,
        update_consumptions: Arc<StdMutex<Vec<Vec<RemoteQuotaConsumption>>>>,
    }

    struct TestRateLimitServer {
        state: Arc<StdMutex<ServerState>>,
        observation: Observation,
    }

    #[tonic::async_trait]
    impl RateLimitGrpc for TestRateLimitServer {
        type ServiceStream = Pin<Box<dyn Stream<Item = Result<RateLimitResponse, Status>> + Send>>;

        async fn service(
            &self,
            request: Request<tonic::Streaming<RemoteRateLimitRequest>>,
        ) -> Result<Response<Self::ServiceStream>, Status> {
            let mut inbound = request.into_inner();
            let (sender, receiver) = mpsc::channel(16);
            let state = self.state.clone();
            let observation = self.observation.clone();
            tokio::spawn(async move {
                while let Ok(Some(request)) = inbound.message().await {
                    let response = match RateLimitCmd::try_from(request.cmd).ok() {
                        Some(RateLimitCmd::Init) => {
                            observation.init_count.fetch_add(1, Ordering::Relaxed);
                            let Some(init) = request.rate_limit_init_request else {
                                return;
                            };
                            let mut state = state.lock().unwrap();
                            let first_counter_key = 100 + state.counters.len() as u32;
                            let counters = init
                                .totals
                                .iter()
                                .enumerate()
                                .map(|(index, total)| {
                                    let counter_key = first_counter_key + index as u32;
                                    state.counters.insert(
                                        counter_key,
                                        CounterState {
                                            max_amount: total.max_amount,
                                            accounting: QuotaAccounting::try_from(total.accounting)
                                                .unwrap(),
                                            committed: 0,
                                            reserved: 0,
                                        },
                                    );
                                    QuotaCounter {
                                        duration: total.duration,
                                        counter_key,
                                        left: i64::from(total.max_amount),
                                        mode: total.mode,
                                        client_count: 1,
                                    }
                                })
                                .collect();
                            RateLimitResponse {
                                cmd: RateLimitCmd::Init.into(),
                                rate_limit_init_response: Some(RateLimitInitResponse {
                                    code: RATE_LIMIT_SUCCESS_CODE,
                                    target: init.target,
                                    client_key: 7,
                                    counters,
                                    slide_count: 1,
                                    timestamp: now_millis(),
                                }),
                                ..Default::default()
                            }
                        }
                        Some(RateLimitCmd::Reserve) => {
                            observation.reserve_count.fetch_add(1, Ordering::Relaxed);
                            let Some(reserve) = request.quota_reserve_request else {
                                return;
                            };
                            let mut state = state.lock().unwrap();
                            let allowed = reserve.reservations.iter().all(|reservation| {
                                state
                                    .counters
                                    .get(&reservation.counter_key)
                                    .map(|counter| {
                                        counter
                                            .committed
                                            .saturating_add(counter.reserved)
                                            .saturating_add(reservation.amount)
                                            <= counter.max_amount
                                    })
                                    .unwrap_or(false)
                            });
                            if !allowed {
                                RateLimitResponse {
                                    cmd: RateLimitCmd::Reserve.into(),
                                    quota_reserve_response: Some(QuotaReserveResponse {
                                        code: REJECTED_CODE,
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                }
                            } else {
                                for reservation in &reserve.reservations {
                                    state
                                        .counters
                                        .get_mut(&reservation.counter_key)
                                        .unwrap()
                                        .reserved += reservation.amount;
                                }
                                state.next_lease += 1;
                                let lease_id = format!("lease-{}", state.next_lease);
                                state.leases.insert(
                                    lease_id.clone(),
                                    ServerLease {
                                        reservations: reserve
                                            .reservations
                                            .iter()
                                            .map(|reservation| {
                                                (reservation.counter_key, reservation.amount)
                                            })
                                            .collect(),
                                        consumptions: reserve
                                            .reservations
                                            .iter()
                                            .map(|reservation| (reservation.counter_key, 0))
                                            .collect(),
                                        sequence: 0,
                                    },
                                );
                                RateLimitResponse {
                                    cmd: RateLimitCmd::Reserve.into(),
                                    quota_reserve_response: Some(QuotaReserveResponse {
                                        code: RATE_LIMIT_SUCCESS_CODE,
                                        lease_id,
                                        quota_lefts: reserve
                                            .reservations
                                            .iter()
                                            .map(|reservation| {
                                                let counter = state
                                                    .counters
                                                    .get(&reservation.counter_key)
                                                    .unwrap();
                                                QuotaLeft {
                                                    counter_key: reservation.counter_key,
                                                    left: i64::from(
                                                        counter.max_amount
                                                            - counter.committed
                                                            - counter.reserved,
                                                    ),
                                                    mode: Mode::BatchOccupy.into(),
                                                    client_count: 1,
                                                }
                                            })
                                            .collect(),
                                        expires_at: now_millis()
                                            + i64::from(reserve.ttl_seconds) * 1_000,
                                        timestamp: now_millis(),
                                    }),
                                    ..Default::default()
                                }
                            }
                        }
                        Some(RateLimitCmd::Update) => {
                            observation.update_count.fetch_add(1, Ordering::Relaxed);
                            let Some(update) = request.quota_update_request else {
                                return;
                            };
                            observation
                                .update_consumptions
                                .lock()
                                .unwrap()
                                .push(update.consumptions.clone());
                            let mut state = state.lock().unwrap();
                            let code = match state.leases.get_mut(&update.lease_id) {
                                Some(lease)
                                    if update.sequence == lease.sequence + 1
                                        && update.consumptions.iter().all(|consumption| {
                                            let current = lease
                                                .consumptions
                                                .get(&consumption.counter_key)
                                                .copied()
                                                .unwrap_or(u32::MAX);
                                            let reserved = lease
                                                .reservations
                                                .get(&consumption.counter_key)
                                                .copied()
                                                .unwrap_or(0);
                                            consumption.consumed_total >= current
                                                && consumption.consumed_total <= reserved
                                        }) =>
                                {
                                    lease.sequence = update.sequence;
                                    for consumption in &update.consumptions {
                                        lease.consumptions.insert(
                                            consumption.counter_key,
                                            consumption.consumed_total,
                                        );
                                    }
                                    RATE_LIMIT_SUCCESS_CODE
                                }
                                _ => INVALID_LEASE_CODE,
                            };
                            RateLimitResponse {
                                cmd: RateLimitCmd::Update.into(),
                                quota_update_response: Some(QuotaUpdateResponse {
                                    code,
                                    consumptions: update.consumptions,
                                    sequence: update.sequence,
                                    timestamp: now_millis(),
                                }),
                                ..Default::default()
                            }
                        }
                        Some(RateLimitCmd::Settle) => {
                            observation.settle_count.fetch_add(1, Ordering::Relaxed);
                            let Some(settle) = request.quota_settle_request else {
                                return;
                            };
                            let mut state = state.lock().unwrap();
                            if let Some(previous) = state.settled.get(&settle.lease_id).cloned() {
                                RateLimitResponse {
                                    cmd: RateLimitCmd::Settle.into(),
                                    quota_settle_response: Some(QuotaSettleResponse {
                                        code: RATE_LIMIT_SUCCESS_CODE,
                                        settlements: previous,
                                        timestamp: now_millis(),
                                    }),
                                    ..Default::default()
                                }
                            } else {
                                let Some(lease) = state.leases.remove(&settle.lease_id) else {
                                    return;
                                };
                                if settle.sequence != lease.sequence + 1 {
                                    return;
                                }
                                let consumed = settle
                                    .consumptions
                                    .iter()
                                    .map(|consumption| {
                                        (consumption.counter_key, consumption.consumed_total)
                                    })
                                    .collect::<HashMap<_, _>>();
                                let settlements = lease
                                    .reservations
                                    .iter()
                                    .map(|(counter_key, amount)| {
                                        let consumed_total = *consumed.get(counter_key).unwrap();
                                        let counter = state.counters.get_mut(counter_key).unwrap();
                                        counter.reserved -= amount;
                                        let returned_amount = match counter.accounting {
                                            QuotaAccounting::Consumable => {
                                                counter.committed += consumed_total;
                                                amount - consumed_total
                                            }
                                            QuotaAccounting::Occupancy => *amount,
                                        };
                                        QuotaSettlement {
                                            counter_key: *counter_key,
                                            consumed_total,
                                            returned_amount,
                                        }
                                    })
                                    .collect::<Vec<_>>();
                                state.settled.insert(settle.lease_id, settlements.clone());
                                RateLimitResponse {
                                    cmd: RateLimitCmd::Settle.into(),
                                    quota_settle_response: Some(QuotaSettleResponse {
                                        code: RATE_LIMIT_SUCCESS_CODE,
                                        settlements,
                                        timestamp: now_millis(),
                                    }),
                                    ..Default::default()
                                }
                            }
                        }
                        _ => return,
                    };
                    if sender.send(Ok(response)).await.is_err() {
                        return;
                    }
                }
            });
            Ok(Response::new(Box::pin(ReceiverStream::new(receiver))))
        }

        async fn time_adjust(
            &self,
            _request: Request<TimeAdjustRequest>,
        ) -> Result<Response<TimeAdjustResponse>, Status> {
            Ok(Response::new(TimeAdjustResponse {
                server_timestamp: now_millis(),
            }))
        }
    }

    async fn start_server() -> (u32, Observation, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = u32::from(listener.local_addr().unwrap().port());
        let observation = Observation::default();
        let server = TestRateLimitServer {
            state: Arc::new(StdMutex::new(ServerState::default())),
            observation: observation.clone(),
        };
        let task = tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(RateLimitGrpcServer::new(server))
                .serve_with_incoming(TcpListenerStream::new(listener))
                .await
                .unwrap();
        });
        (port, observation, task)
    }

    fn no_traffic_label(_: ArgumentType, _: &str) -> Option<String> {
        None
    }

    fn quota_request(resource: QuotaResource, amount: u32) -> QuotaRequest {
        QuotaRequest {
            flow_id: "flow-1".to_string(),
            timeout: Duration::from_secs(1),
            service: "orders".to_string(),
            namespace: "default".to_string(),
            method: "POST /chat".to_string(),
            traffic_label_provider: no_traffic_label,
            quotas: vec![crate::traffic::ratelimit::req::QuotaAmount { resource, amount }],
            lease_ttl: Duration::from_secs(30),
        }
    }

    fn global_consumable_rule(
        resource: limit_trigger::Resource,
        max_amount: u32,
        windows: usize,
    ) -> RateLimit {
        RateLimit {
            id: "global-rule".to_string(),
            revision: "rev-1".to_string(),
            r#type: rate_limit::Type::Global.into(),
            cluster: Some(RateLimitCluster {
                namespace: "Pole".to_string(),
                service: "pole-limiter".to_string(),
            }),
            rules: vec![LimitTrigger {
                name: "quota".to_string(),
                resource: resource.into(),
                amounts: (0..windows)
                    .map(|index| Amount {
                        max_amount,
                        valid_duration: Some(ProstDuration {
                            seconds: 60 + index as i64,
                            nanos: 0,
                        }),
                        ..Default::default()
                    })
                    .collect(),
                amount_mode: AmountMode::GlobalTotal.into(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn global_concurrency_rule(max_amount: u32) -> RateLimit {
        let mut rule = global_consumable_rule(limit_trigger::Resource::Qps, 1, 1);
        rule.rules[0].resource = limit_trigger::Resource::Concurrency.into();
        rule.rules[0].amounts.clear();
        rule.rules[0].concurrency_amount = Some(ConcurrencyAmount { max_amount });
        rule
    }

    fn global_mixed_rule() -> RateLimit {
        let mut rule = global_consumable_rule(limit_trigger::Resource::Qps, 1, 1);
        let token = LimitTrigger {
            name: "token".to_string(),
            resource: limit_trigger::Resource::Token.into(),
            amounts: vec![Amount {
                max_amount: 100,
                valid_duration: Some(ProstDuration {
                    seconds: 60,
                    nanos: 0,
                }),
                ..Default::default()
            }],
            amount_mode: AmountMode::GlobalTotal.into(),
            ..Default::default()
        };
        let concurrency = LimitTrigger {
            name: "concurrency".to_string(),
            resource: limit_trigger::Resource::Concurrency.into(),
            concurrency_amount: Some(ConcurrencyAmount { max_amount: 1 }),
            amount_mode: AmountMode::GlobalTotal.into(),
            ..Default::default()
        };
        rule.rules[0].name = "qps".to_string();
        rule.rules.extend([token, concurrency]);
        rule
    }

    fn mixed_quota_request() -> QuotaRequest {
        let mut req = quota_request(QuotaResource::Token, 80);
        req.quotas = vec![
            crate::traffic::ratelimit::req::QuotaAmount {
                resource: QuotaResource::Qps,
                amount: 1,
            },
            crate::traffic::ratelimit::req::QuotaAmount {
                resource: QuotaResource::Token,
                amount: 80,
            },
            crate::traffic::ratelimit::req::QuotaAmount {
                resource: QuotaResource::Concurrency,
                amount: 1,
            },
        ];
        req
    }

    fn limiter_instance(port: u32) -> Instance {
        Instance {
            ip: "127.0.0.1".to_string(),
            port,
            protocol: "grpc".to_string(),
            health: true,
            weight: 100,
            ..Default::default()
        }
    }

    async fn reserve_lease(
        manager: &DistributedQuotaManager,
        req: &QuotaRequest,
        rule: &RateLimit,
        port: u32,
    ) -> Result<QuotaLease, String> {
        let specs = rule
            .rules
            .iter()
            .filter_map(|trigger| {
                let resource = quota_resource(trigger.resource()).ok()?;
                req.amount_for(resource).map(|amount| DistributedQuotaSpec {
                    rule,
                    trigger,
                    amount,
                })
            })
            .collect::<Vec<_>>();
        match manager
            .reserve_many(req, &specs, &[limiter_instance(port)], "test-client")
            .await
            .map_err(|error| error.to_string())?
        {
            DistributedReserveResult::Reserved(backend) => Ok(QuotaLease::new(
                req.quotas.clone(),
                req.quotas
                    .iter()
                    .map(|quota| QuotaConsumption {
                        resource: quota.resource,
                        consumed_total: if quota.resource == QuotaResource::Qps {
                            1
                        } else {
                            0
                        },
                    })
                    .collect(),
                backend,
            )),
            DistributedReserveResult::Rejected(message) => Err(message),
        }
    }

    #[tokio::test]
    async fn distributed_tpm_updates_every_counter_and_returns_unused_quota() {
        let (port, observation, server) = start_server().await;
        let manager = DistributedQuotaManager::default();
        let rule = global_consumable_rule(limit_trigger::Resource::Token, 100, 2);
        let lease = reserve_lease(
            &manager,
            &quota_request(QuotaResource::Token, 80),
            &rule,
            port,
        )
        .await
        .unwrap();

        lease.update(40).await.unwrap();
        lease.update(40).await.unwrap();
        lease.finish(60).await.unwrap();
        reserve_lease(
            &manager,
            &quota_request(QuotaResource::Token, 40),
            &rule,
            port,
        )
        .await
        .unwrap();
        assert!(reserve_lease(
            &manager,
            &quota_request(QuotaResource::Token, 41),
            &rule,
            port,
        )
        .await
        .is_err());

        assert_eq!(observation.update_count.load(Ordering::Relaxed), 1);
        let updates = observation.update_consumptions.lock().unwrap();
        assert_eq!(updates[0].len(), 2);
        assert!(updates[0]
            .iter()
            .all(|consumption| consumption.consumed_total == 40));
        server.abort();
    }

    #[tokio::test]
    async fn distributed_rpm_commits_at_finish() {
        let (port, _, server) = start_server().await;
        let manager = DistributedQuotaManager::default();
        let rule = global_consumable_rule(limit_trigger::Resource::Qps, 1, 1);
        let lease = reserve_lease(&manager, &quota_request(QuotaResource::Qps, 1), &rule, port)
            .await
            .unwrap();

        lease.finish(1).await.unwrap();
        assert!(
            reserve_lease(&manager, &quota_request(QuotaResource::Qps, 1), &rule, port,)
                .await
                .is_err()
        );
        server.abort();
    }

    #[tokio::test]
    async fn distributed_concurrency_releases_occupancy() {
        let (port, _, server) = start_server().await;
        let manager = DistributedQuotaManager::default();
        let rule = global_concurrency_rule(1);
        let req = quota_request(QuotaResource::Concurrency, 1);
        let lease = reserve_lease(&manager, &req, &rule, port).await.unwrap();

        assert!(reserve_lease(&manager, &req, &rule, port).await.is_err());
        lease.finish(0).await.unwrap();
        reserve_lease(&manager, &req, &rule, port).await.unwrap();
        server.abort();
    }

    #[tokio::test]
    async fn distributed_drop_settles_once_with_last_confirmed_total() {
        let (port, observation, server) = start_server().await;
        let manager = DistributedQuotaManager::default();
        let rule = global_consumable_rule(limit_trigger::Resource::Token, 100, 1);
        let lease = reserve_lease(
            &manager,
            &quota_request(QuotaResource::Token, 80),
            &rule,
            port,
        )
        .await
        .unwrap();
        lease.update(30).await.unwrap();

        drop(lease);
        timeout(Duration::from_secs(1), async {
            while observation.settle_count.load(Ordering::Relaxed) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        reserve_lease(
            &manager,
            &quota_request(QuotaResource::Token, 70),
            &rule,
            port,
        )
        .await
        .unwrap();
        assert_eq!(observation.settle_count.load(Ordering::Relaxed), 1);
        server.abort();
    }

    #[tokio::test]
    async fn distributed_mixed_metrics_share_one_protocol_lease() {
        let (port, observation, server) = start_server().await;
        let manager = DistributedQuotaManager::default();
        let rule = global_mixed_rule();
        let lease = reserve_lease(&manager, &mixed_quota_request(), &rule, port)
            .await
            .unwrap();

        assert_eq!(observation.reserve_count.load(Ordering::Relaxed), 1);
        assert!(lease.update(40).await.is_err());
        lease
            .update_consumptions(&[QuotaConsumption {
                resource: QuotaResource::Token,
                consumed_total: 40,
            }])
            .await
            .unwrap();
        lease
            .finish_consumptions(&[QuotaConsumption {
                resource: QuotaResource::Token,
                consumed_total: 60,
            }])
            .await
            .unwrap();

        {
            let updates = observation.update_consumptions.lock().unwrap();
            assert_eq!(updates[0].len(), 3);
        }
        reserve_lease(
            &manager,
            &quota_request(QuotaResource::Concurrency, 1),
            &rule,
            port,
        )
        .await
        .unwrap();
        reserve_lease(
            &manager,
            &quota_request(QuotaResource::Token, 40),
            &rule,
            port,
        )
        .await
        .unwrap();
        assert!(
            reserve_lease(&manager, &quota_request(QuotaResource::Qps, 1), &rule, port,)
                .await
                .is_err()
        );
        server.abort();
    }
}
