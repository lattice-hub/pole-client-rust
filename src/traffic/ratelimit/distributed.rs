use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
        Arc, Mutex as StdMutex,
    },
    time::Duration,
};

use dashmap::DashMap;
use pole_specification::{
    polaris::metric::v2::{
        rate_limit_grpcv2_client::RateLimitGrpcv2Client, LimitTarget, QuotaMode as RemoteQuotaMode,
        QuotaSum, QuotaTotal, RateLimitCmd, RateLimitInitRequest, RateLimitReportRequest,
        RateLimitRequest as RemoteRateLimitRequest, RateLimitResponse as RemoteRateLimitResponse,
        TimeAdjustRequest,
    },
    v1::{limit_trigger::AmountMode, LimitTrigger, RateLimit},
};
use sha2::{Digest, Sha256};
use tokio::{
    sync::{mpsc, Mutex, Notify},
    time::timeout,
};
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Endpoint;

use crate::{
    core::model::{
        error::{ErrorCode, PoleError},
        naming::Instance,
    },
    traffic::ratelimit::req::{QuotaRequest, QuotaResponse},
};

const RATE_LIMIT_SUCCESS_CODE: u32 = 200_000;
const RATE_LIMIT_NOT_FOUND_CODE: u32 = 404_001;
const RATE_LIMIT_INVALID_COUNTER_CODE: u32 = 400_214;
const TIME_ADJUST_INTERVAL: Duration = Duration::from_secs(30);
const WINDOW_CACHE_LIMIT: usize = 10_000;
const WINDOW_CLEANUP_INTERVAL: u64 = 256;

/// 分布式限流窗口的稳定身份。规则 revision 是 key 的一部分，规则更新后会重新 INIT，
/// 不会误用旧阈值或旧集群下发的剩余配额。
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
            // client_key 与 counter_key 由单条远端 gRPC 流分配，不能跨限流实例复用。
            endpoint: endpoint.to_string(),
            cluster_namespace: cluster.namespace.clone(),
            cluster_service: cluster.service.clone(),
            target_namespace: req.namespace.clone(),
            target_service: req.service.clone(),
            // limiter 只把 labels 当作跨 SDK counter 身份；规则 revision 保留在本地
            // WindowKey 触发重新 INIT，不能进入远端 labels 造成新旧 SDK 分桶。
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

struct RemoteCounter {
    counter_key: u32,
    left: i64,
    unreported_used: u32,
    inflight_used: VecDeque<PendingReports>,
}

struct PendingReports {
    epoch: i64,
    used: u32,
    count: u32,
}

#[derive(Default)]
struct WindowState {
    owner_session_id: u64,
    initializing: bool,
    initialized: bool,
    failure: Option<String>,
    client_key: u32,
    counters: HashMap<u32, RemoteCounter>,
}

struct QuotaWindow {
    key: WindowKey,
    state: Mutex<WindowState>,
    changed: Notify,
    last_used_ms: AtomicI64,
    idle_ttl_ms: i64,
    active_checks: AtomicU64,
}

impl QuotaWindow {
    fn new(key: WindowKey, trigger: &LimitTrigger) -> Self {
        let longest_window_ms = trigger
            .amounts
            .iter()
            .map(|amount| i64::from(amount_duration_seconds(amount)) * 1_000)
            .max()
            .unwrap_or(1_000);
        Self {
            key,
            state: Mutex::new(WindowState::default()),
            changed: Notify::new(),
            last_used_ms: AtomicI64::new(now_millis()),
            // 至少跨过两个最长配额周期再回收；短周期高基数维度也保留一分钟，
            // 避免正常间歇流量频繁 INIT。
            idle_ttl_ms: (longest_window_ms * 2).max(60_000),
            active_checks: AtomicU64::new(0),
        }
    }

    fn touch(&self) {
        self.last_used_ms.store(now_millis(), Ordering::Release);
    }

    fn is_expired(&self, now: i64) -> bool {
        now - self.last_used_ms.load(Ordering::Acquire) > self.idle_ttl_ms
    }

    async fn mark_initialized(
        &self,
        session_id: u64,
        client_key: u32,
        counters: Vec<pole_specification::polaris::metric::v2::QuotaCounter>,
    ) {
        let mut state = self.state.lock().await;
        if state.owner_session_id != session_id {
            return;
        }
        state.initializing = false;
        state.initialized = true;
        state.failure = None;
        state.client_key = client_key;
        state.counters = counters
            .into_iter()
            .map(|counter| {
                (
                    counter.duration,
                    RemoteCounter {
                        counter_key: counter.counter_key,
                        left: counter.left,
                        unreported_used: 0,
                        inflight_used: VecDeque::new(),
                    },
                )
            })
            .collect();
        drop(state);
        self.changed.notify_waiters();
    }

    async fn mark_failed(&self, session_id: u64, message: String) {
        let mut state = self.state.lock().await;
        if state.owner_session_id != session_id {
            return;
        }
        state.initializing = false;
        state.initialized = false;
        state.failure = Some(message);
        drop(state);
        self.changed.notify_waiters();
    }

    async fn invalidate(&self, session_id: u64) {
        let mut state = self.state.lock().await;
        if state.owner_session_id != session_id {
            return;
        }
        state.initializing = false;
        state.initialized = false;
        state.failure = None;
        state.client_key = 0;
        state.counters.clear();
        drop(state);
        self.changed.notify_waiters();
    }

    async fn update_counters(
        &self,
        session_id: u64,
        response_timestamp: i64,
        counters: Vec<pole_specification::polaris::metric::v2::QuotaLeft>,
    ) {
        let mut state = self.state.lock().await;
        if state.owner_session_id != session_id {
            return;
        }
        for counter in counters {
            if let Some((duration, local)) = state
                .counters
                .iter_mut()
                .find(|(_, local)| local.counter_key == counter.counter_key)
            {
                // 服务端回包已经计入当前 report，但不包含仍在流中排队的后续 report
                // 和尚未上报的本地扣减。按流顺序确认一批，再扣除剩余 pending，
                // 避免较早的绝对余量覆盖较新的本地消费。
                let period_ms = i64::from(*duration) * 1_000;
                let response_epoch = response_timestamp / period_ms;
                while local
                    .inflight_used
                    .front()
                    .map(|pending| pending.epoch < response_epoch)
                    .unwrap_or(false)
                {
                    local.inflight_used.pop_front();
                }
                // limiter 的主动 push 只在 left <= 0 时产生；正余量回包必然
                // 对应本客户端 report，可以安全确认同周期 FIFO 的一项。
                if counter.left > 0
                    && local
                        .inflight_used
                        .front()
                        .map(|pending| pending.epoch == response_epoch)
                        .unwrap_or(false)
                {
                    let pending = local
                        .inflight_used
                        .front_mut()
                        .expect("pending report exists after epoch check");
                    pending.count -= 1;
                    if pending.count == 0 {
                        local.inflight_used.pop_front();
                    }
                }
                let pending = local.unreported_used
                    + local
                        .inflight_used
                        .iter()
                        .map(|pending| pending.used.saturating_mul(pending.count))
                        .sum::<u32>();
                local.left = counter.left - i64::from(pending);
            }
        }
    }

    fn mark_reported(state: &mut WindowState, timestamp: i64, sums: &[QuotaSum]) {
        for sum in sums {
            if let Some((duration, counter)) = state
                .counters
                .iter_mut()
                .find(|(_, counter)| counter.counter_key == sum.counter_key)
            {
                counter.unreported_used = counter.unreported_used.saturating_sub(sum.used);
                // 每个 report 都对应一个服务端回包；拒绝报告 used=0 也必须占据
                // FIFO 槽位，否则它的回包会错误确认后面的消费报告。
                let epoch = timestamp / (i64::from(*duration) * 1_000);
                if let Some(pending) = counter.inflight_used.back_mut() {
                    if pending.epoch == epoch && pending.used == sum.used {
                        pending.count = pending.count.saturating_add(1);
                        continue;
                    }
                }
                counter.inflight_used.push_back(PendingReports {
                    epoch,
                    used: sum.used,
                    count: 1,
                });
            }
        }
    }

    async fn acquire(
        &self,
        trigger: &LimitTrigger,
    ) -> Result<(bool, Vec<QuotaSum>, u32), PoleError> {
        let mut state = self.state.lock().await;
        if !state.initialized {
            return Err(PoleError::new(
                ErrorCode::InvalidState,
                "distributed rate limit window is not initialized".to_string(),
            ));
        }

        let durations = trigger
            .amounts
            .iter()
            .map(amount_duration_seconds)
            .collect::<Vec<_>>();
        if durations.is_empty() {
            return Err(PoleError::new(
                ErrorCode::InvalidRule,
                format!("rate limit trigger {} has no amounts", trigger.name),
            ));
        }

        let allowed = durations.iter().all(|duration| {
            state
                .counters
                .get(duration)
                .map(|counter| counter.left > 0)
                .unwrap_or(false)
        });
        let mut sums = Vec::with_capacity(durations.len());
        for duration in durations {
            if let Some(counter) = state.counters.get_mut(&duration) {
                if allowed {
                    counter.left -= 1;
                    counter.unreported_used += 1;
                    sums.push(QuotaSum {
                        counter_key: counter.counter_key,
                        used: 1,
                        limited: 0,
                    });
                } else {
                    sums.push(QuotaSum {
                        counter_key: counter.counter_key,
                        used: 0,
                        limited: 1,
                    });
                }
            }
        }
        Ok((allowed, sums, state.client_key))
    }
}

struct WindowUseGuard {
    window: Arc<QuotaWindow>,
}

impl WindowUseGuard {
    fn new(window: Arc<QuotaWindow>) -> Self {
        window.active_checks.fetch_add(1, Ordering::AcqRel);
        Self { window }
    }
}

impl Drop for WindowUseGuard {
    fn drop(&mut self) {
        self.window.active_checks.fetch_sub(1, Ordering::AcqRel);
    }
}

/// 每个 endpoint 保持一条双向 gRPC 流。窗口初始化和用量上报都复用该流，
/// 服务端返回的剩余额度按 counter key 回写到对应窗口。
struct QuotaSession {
    id: u64,
    sender: mpsc::Sender<RemoteRateLimitRequest>,
    windows: DashMap<WindowKey, Arc<QuotaWindow>>,
    counters: DashMap<u32, Arc<QuotaWindow>>,
    clock_offset_ms: AtomicI64,
    last_time_adjust_ms: AtomicI64,
    time_adjust_lock: Mutex<()>,
    target_init_locks: DashMap<String, Arc<Mutex<()>>>,
    endpoint: String,
    request_timeout: Duration,
    healthy: AtomicBool,
    response_task: StdMutex<Option<tokio::task::AbortHandle>>,
}

impl QuotaSession {
    async fn connect(
        id: u64,
        endpoint: String,
        request_timeout: Duration,
    ) -> Result<Arc<Self>, PoleError> {
        let channel = Endpoint::from_shared(format!("http://{endpoint}"))
            .map_err(|err| network_error("create rate limit endpoint", err))?
            .connect_lazy();
        let (sender, receiver) = mpsc::channel(128);
        let mut client = RateLimitGrpcv2Client::new(channel);
        let adjust_started_at = now_millis();
        let adjustment = timeout(request_timeout, client.time_adjust(TimeAdjustRequest {}))
            .await
            .map_err(|_| {
                PoleError::new(
                    ErrorCode::ApiTimeout,
                    "rate limit time adjustment timed out".to_string(),
                )
            })?
            .map_err(|err| network_error("adjust rate limit server time", err))?
            .into_inner();
        let adjust_finished_at = now_millis();
        let local_midpoint = adjust_started_at + (adjust_finished_at - adjust_started_at) / 2;
        let clock_offset_ms = adjustment.server_timestamp - local_midpoint;
        let response = timeout(
            request_timeout,
            client.service(ReceiverStream::new(receiver)),
        )
        .await
        .map_err(|_| {
            PoleError::new(
                ErrorCode::ApiTimeout,
                "open rate limit stream timed out".to_string(),
            )
        })?
        .map_err(|err| network_error("open rate limit stream", err))?;

        let session = Arc::new(Self {
            id,
            sender,
            windows: DashMap::new(),
            counters: DashMap::new(),
            clock_offset_ms: AtomicI64::new(clock_offset_ms),
            last_time_adjust_ms: AtomicI64::new(adjust_finished_at),
            time_adjust_lock: Mutex::new(()),
            target_init_locks: DashMap::new(),
            endpoint,
            request_timeout,
            healthy: AtomicBool::new(true),
            response_task: StdMutex::new(None),
        });
        let response_session = session.clone();
        let response_task = tokio::spawn(async move {
            let mut stream = response.into_inner();
            loop {
                match stream.message().await {
                    Ok(Some(message)) => response_session.handle_response(message).await,
                    Ok(None) => {
                        response_session
                            .mark_stream_failed("rate limit stream closed".to_string())
                            .await;
                        return;
                    }
                    Err(err) => {
                        response_session
                            .mark_stream_failed(format!("rate limit stream receive failed: {err}"))
                            .await;
                        return;
                    }
                }
            }
        });
        *session.response_task.lock().unwrap() = Some(response_task.abort_handle());
        Ok(session)
    }

    async fn handle_response(&self, response: RemoteRateLimitResponse) {
        match RateLimitCmd::try_from(response.cmd).ok() {
            Some(RateLimitCmd::Init) => {
                let Some(init) = response.rate_limit_init_response else {
                    return;
                };
                let Some(target) = init.target.as_ref() else {
                    return;
                };
                let window = self.windows.iter().find_map(|entry| {
                    let key = entry.key();
                    (key.target_namespace == target.namespace
                        && key.target_service == target.service
                        && key.labels == target.labels)
                        .then(|| entry.value().clone())
                });
                let Some(window) = window else {
                    return;
                };
                if init.code != RATE_LIMIT_SUCCESS_CODE {
                    window
                        .mark_failed(
                            self.id,
                            format!("rate limit init returned code {}", init.code),
                        )
                        .await;
                    return;
                }
                for counter in &init.counters {
                    self.counters.insert(counter.counter_key, window.clone());
                }
                window
                    .mark_initialized(self.id, init.client_key, init.counters)
                    .await;
            }
            Some(RateLimitCmd::Acquire) => {
                let Some(report) = response.rate_limit_report_response else {
                    return;
                };
                if report.code != RATE_LIMIT_SUCCESS_CODE {
                    if report.code == RATE_LIMIT_NOT_FOUND_CODE {
                        self.invalidate_windows().await;
                    } else if report.code == RATE_LIMIT_INVALID_COUNTER_CODE {
                        self.mark_stream_failed(format!(
                            "rate limit report returned code {}",
                            report.code
                        ))
                        .await;
                    } else {
                        self.mark_windows_failed(format!(
                            "rate limit report returned code {}",
                            report.code
                        ))
                        .await;
                    }
                    return;
                }
                let mut grouped = HashMap::<WindowKey, Vec<_>>::new();
                let response_timestamp = report.timestamp;
                for counter in report.quota_lefts {
                    if let Some(window) = self.counters.get(&counter.counter_key) {
                        grouped.entry(window.key.clone()).or_default().push(counter);
                    }
                }
                for (key, counters) in grouped {
                    let window = self.windows.get(&key).map(|entry| entry.value().clone());
                    if let Some(window) = window {
                        window
                            .update_counters(self.id, response_timestamp, counters)
                            .await;
                    }
                }
            }
            _ => {}
        }
    }

    async fn mark_stream_failed(&self, message: String) {
        self.healthy.store(false, Ordering::Release);
        self.mark_windows_failed(message).await;
        self.counters.clear();
    }

    async fn mark_windows_failed(&self, message: String) {
        let windows = self
            .windows
            .iter()
            .map(|entry| entry.value().clone())
            .collect::<Vec<_>>();
        for window in windows {
            window.mark_failed(self.id, message.clone()).await;
        }
    }

    async fn invalidate_windows(&self) {
        let windows = self
            .windows
            .iter()
            .map(|entry| entry.value().clone())
            .collect::<Vec<_>>();
        self.counters.clear();
        for window in windows {
            window.invalidate(self.id).await;
        }
    }

    fn remove_window(&self, key: &WindowKey) {
        self.windows.remove(key);
        self.counters.retain(|_, window| &window.key != key);
        let target_still_active = self.windows.iter().any(|entry| {
            let current = entry.key();
            current.target_namespace == key.target_namespace
                && current.target_service == key.target_service
                && current.labels == key.labels
        });
        if !target_still_active {
            self.target_init_locks.remove(&format!(
                "{}#{}#{}",
                key.target_namespace, key.target_service, key.labels
            ));
        }
    }

    fn shutdown(&self) {
        self.healthy.store(false, Ordering::Release);
        if let Some(task) = self.response_task.lock().unwrap().take() {
            task.abort();
        }
    }

    async fn ensure_window(
        &self,
        window: Arc<QuotaWindow>,
        client_id: &str,
        trigger: &LimitTrigger,
        request_timeout: Duration,
    ) -> Result<(), PoleError> {
        let target_key = format!(
            "{}#{}#{}",
            window.key.target_namespace, window.key.target_service, window.key.labels
        );
        let init_lock = self
            .target_init_locks
            .entry(target_key)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _init_guard = init_lock.lock().await;
        let session_has_window = self.windows.contains_key(&window.key);
        let should_initialize = {
            let mut state = window.state.lock().await;
            if session_has_window && state.owner_session_id == self.id && state.initialized {
                return Ok(());
            }
            if state.owner_session_id == self.id && state.initializing {
                false
            } else {
                state.owner_session_id = self.id;
                state.initializing = true;
                // 当前 session 第一次使用该窗口时，即便窗口曾由断开的旧 session
                // 初始化，也必须清空旧 client/counter 标识并重新申请配额。
                state.initialized = false;
                state.failure = None;
                state.client_key = 0;
                state.counters.clear();
                true
            }
        };

        if should_initialize {
            // 同一个远端 target 更新 revision 时，响应只回显 target，不携带 revision。
            // session 内只保留该 target 的最新窗口，避免 INIT 响应落到旧 revision。
            let stale_keys = self
                .windows
                .iter()
                .filter_map(|entry| {
                    let key = entry.key();
                    (key.target_namespace == window.key.target_namespace
                        && key.target_service == window.key.target_service
                        && key.labels == window.key.labels
                        && *key != window.key)
                        .then(|| key.clone())
                })
                .collect::<Vec<_>>();
            self.windows.insert(window.key.clone(), window.clone());
            for key in stale_keys {
                self.remove_window(&key);
            }
            let init = build_init_request(client_id, &window.key, trigger);
            if self
                .sender
                .send(RemoteRateLimitRequest {
                    cmd: RateLimitCmd::Init.into(),
                    rate_limit_init_request: Some(init),
                    rate_limit_report_request: None,
                    rate_limit_batch_init_request: None,
                })
                .await
                .is_err()
            {
                let message = "rate limit stream is unavailable".to_string();
                self.mark_stream_failed(message.clone()).await;
                return Err(PoleError::new(ErrorCode::NetworkError, message));
            }
        }

        let initialized = timeout(request_timeout, async {
            loop {
                let notified = window.changed.notified();
                let state = window.state.lock().await;
                if state.initialized {
                    return Ok(());
                }
                if let Some(message) = &state.failure {
                    return Err(PoleError::new(ErrorCode::NetworkError, message.clone()));
                }
                drop(state);
                notified.await;
            }
        })
        .await;
        match initialized {
            Ok(result) => result,
            Err(_) => {
                let message = "rate limit init timed out".to_string();
                window.mark_failed(self.id, message.clone()).await;
                Err(PoleError::new(ErrorCode::ApiTimeout, message))
            }
        }
    }

    async fn report(
        &self,
        window: &QuotaWindow,
        client_key: u32,
        sums: Vec<QuotaSum>,
    ) -> Result<(), PoleError> {
        self.adjust_time_if_due().await;
        let report_timestamp = now_millis() + self.clock_offset_ms.load(Ordering::Relaxed);
        let request = RemoteRateLimitRequest {
            cmd: RateLimitCmd::Acquire.into(),
            rate_limit_init_request: None,
            rate_limit_report_request: Some(RateLimitReportRequest {
                client_key,
                quota_uses: sums.clone(),
                timestamp: report_timestamp,
            }),
            rate_limit_batch_init_request: None,
        };
        let send_result = {
            // 与回包更新共用同一把窗口锁，使 FIFO 登记与入流不可被回包插入。
            let mut state = window.state.lock().await;
            QuotaWindow::mark_reported(&mut state, report_timestamp, &sums);
            self.sender.try_send(request)
        };
        if send_result.is_err() {
            let message = "rate limit stream is unavailable or backpressured".to_string();
            self.mark_stream_failed(message.clone()).await;
            return Err(PoleError::new(ErrorCode::NetworkError, message));
        }
        Ok(())
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

        // TimeAdjust 是同 endpoint 上的独立 unary RPC；失败时保留上次偏移，
        // 不让周期性校时影响已经健康的配额流。
        let channel = match Endpoint::from_shared(format!("http://{}", self.endpoint)) {
            Ok(endpoint) => endpoint.connect_lazy(),
            Err(_) => {
                self.last_time_adjust_ms
                    .store(now_millis(), Ordering::Release);
                return;
            }
        };
        let mut client = RateLimitGrpcv2Client::new(channel);
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

/// 规则驱动的分布式限流连接管理器。连接仅在 GLOBAL 规则命中后创建，
/// cluster 变化或 rule revision 变化会自然落到新的 window，不污染本地限流路径。
pub(super) struct DistributedQuotaManager {
    sessions: DashMap<String, Arc<QuotaSession>>,
    windows: DashMap<WindowKey, Arc<QuotaWindow>>,
    current_windows: DashMap<String, WindowKey>,
    endpoint_bindings: DashMap<String, String>,
    connect_lock: Mutex<()>,
    window_cache_lock: StdMutex<()>,
    check_count: AtomicU64,
    next_session_id: AtomicU64,
}

impl Default for DistributedQuotaManager {
    fn default() -> Self {
        Self {
            sessions: DashMap::new(),
            windows: DashMap::new(),
            current_windows: DashMap::new(),
            endpoint_bindings: DashMap::new(),
            connect_lock: Mutex::new(()),
            window_cache_lock: StdMutex::new(()),
            check_count: AtomicU64::new(0),
            next_session_id: AtomicU64::new(1),
        }
    }
}

impl DistributedQuotaManager {
    pub(super) async fn check(
        &self,
        req: &QuotaRequest,
        rule: &RateLimit,
        trigger: &LimitTrigger,
        instances: &[Instance],
        client_id: &str,
    ) -> Result<QuotaResponse, PoleError> {
        let affinity = window_affinity(req, rule, trigger)?;
        let (window, _window_use, logical_key) = {
            let _cache_guard = self.window_cache_lock.lock().unwrap();
            self.cleanup_windows_if_needed();
            let endpoint = self.endpoint_for(instances, &affinity)?;
            let key = WindowKey::from_rule(req, rule, trigger, &endpoint)?;
            if !self.windows.contains_key(&key) && self.windows.len() >= WINDOW_CACHE_LIMIT {
                return Err(PoleError::new(
                    ErrorCode::InvalidState,
                    "distributed rate limit window cache is at capacity".to_string(),
                ));
            }
            let window = self
                .windows
                .entry(key.clone())
                .or_insert_with(|| Arc::new(QuotaWindow::new(key.clone(), trigger)))
                .clone();
            let logical_key = logical_window_key(&key);
            self.current_windows.insert(logical_key.clone(), key);
            let window_use = WindowUseGuard::new(window.clone());
            (window, window_use, logical_key)
        };
        window.touch();
        let session = self.session_for(&window.key.endpoint, req.timeout).await?;
        session
            .ensure_window(window.clone(), client_id, trigger, req.timeout)
            .await?;
        {
            // 等新 revision 完成 INIT 后再淘汰旧窗口，避免仍在途的旧 INIT
            // 失去接收者；target init lock 已保证此时旧回包处理完毕。
            let _cache_guard = self.window_cache_lock.lock().unwrap();
            let is_current = self
                .current_windows
                .get(&logical_key)
                .map(|current| current.value() == &window.key)
                .unwrap_or(false);
            if is_current {
                self.remove_stale_windows(&window.key);
            }
        }
        let (allowed, sums, client_key) = window.acquire(trigger).await?;
        session.report(&window, client_key, sums).await?;
        Ok(QuotaResponse {
            allowed,
            message: if allowed {
                String::new()
            } else {
                "distributed rate limit quota exhausted".to_string()
            },
        })
    }

    fn remove_stale_windows(&self, current: &WindowKey) {
        let stale = self
            .windows
            .iter()
            .filter_map(|entry| {
                let key = entry.key();
                ((key.cluster_namespace == current.cluster_namespace
                    && key.cluster_service == current.cluster_service
                    && key.target_namespace == current.target_namespace
                    && key.target_service == current.target_service
                    && key.labels == current.labels
                    && key.rule_id == current.rule_id
                    && key.trigger_name == current.trigger_name
                    && key != current)
                    && entry.value().active_checks.load(Ordering::Acquire) == 0)
                    .then(|| key.clone())
            })
            .collect::<Vec<_>>();
        for key in stale {
            self.remove_window(&key);
        }
    }

    fn cleanup_windows_if_needed(&self) {
        let check = self.check_count.fetch_add(1, Ordering::Relaxed);
        if self.windows.len() < WINDOW_CACHE_LIMIT && check % WINDOW_CLEANUP_INTERVAL != 0 {
            return;
        }
        let now = now_millis();
        let mut candidates = self
            .windows
            .iter()
            .map(|entry| {
                (
                    entry.key().clone(),
                    entry.value().last_used_ms.load(Ordering::Acquire),
                    entry.value().is_expired(now),
                    entry.value().active_checks.load(Ordering::Acquire) == 0,
                )
            })
            .collect::<Vec<_>>();
        let mut remove = candidates
            .iter()
            .filter_map(|(key, _, expired, inactive)| (*expired && *inactive).then(|| key.clone()))
            .collect::<Vec<_>>();
        let remaining = self.windows.len().saturating_sub(remove.len());
        if remaining >= WINDOW_CACHE_LIMIT {
            candidates.sort_unstable_by_key(|(_, last_used, _, _)| *last_used);
            let overflow = remaining - WINDOW_CACHE_LIMIT + 1;
            remove.extend(
                candidates
                    .into_iter()
                    .filter(|(_, _, expired, inactive)| !expired && *inactive)
                    .take(overflow)
                    .map(|(key, _, _, _)| key),
            );
        }
        for key in remove {
            self.remove_window(&key);
            let affinity = format!(
                "{}#{}#{}#{}#{}",
                key.cluster_namespace,
                key.cluster_service,
                key.target_namespace,
                key.target_service,
                key.labels
            );
            let still_active = self.windows.iter().any(|entry| {
                let current = entry.key();
                current.cluster_namespace == key.cluster_namespace
                    && current.cluster_service == key.cluster_service
                    && current.target_namespace == key.target_namespace
                    && current.target_service == key.target_service
                    && current.labels == key.labels
            });
            if !still_active {
                self.endpoint_bindings.remove(&affinity);
            }
        }
    }

    fn remove_window(&self, key: &WindowKey) {
        self.windows.remove(key);
        let logical_key = logical_window_key(key);
        let is_current = self
            .current_windows
            .get(&logical_key)
            .map(|current| current.value() == key)
            .unwrap_or(false);
        if is_current {
            self.current_windows.remove(&logical_key);
        }
        if let Some(session) = self.sessions.get(&key.endpoint) {
            session.remove_window(key);
        }
        let endpoint_in_use = self
            .windows
            .iter()
            .any(|entry| entry.key().endpoint == key.endpoint);
        if !endpoint_in_use {
            if let Some((_, session)) = self.sessions.remove(&key.endpoint) {
                session.shutdown();
            }
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
        let _connect_guard = self.connect_lock.lock().await;
        if let Some(session) = self.sessions.get(endpoint) {
            if session.healthy.load(Ordering::Acquire) && !session.sender.is_closed() {
                return Ok(session.clone());
            }
            drop(session);
            self.sessions.remove(endpoint);
        }
        let session_id = self.next_session_id.fetch_add(1, Ordering::Relaxed);
        let session =
            QuotaSession::connect(session_id, endpoint.to_string(), request_timeout).await?;
        self.sessions.insert(endpoint.to_string(), session.clone());
        Ok(session)
    }

    fn endpoint_for(&self, instances: &[Instance], affinity: &str) -> Result<String, PoleError> {
        let available = available_grpc_endpoints(instances)?;
        if let Some(endpoint) = self.endpoint_bindings.get(affinity) {
            if available.binary_search(endpoint.value()).is_ok() {
                return Ok(endpoint.clone());
            }
            drop(endpoint);
            self.endpoint_bindings.remove(affinity);
        }
        let endpoint = select_endpoint_from_available(&available, affinity);
        self.endpoint_bindings
            .insert(affinity.to_string(), endpoint.clone());
        Ok(endpoint)
    }

    #[cfg(test)]
    pub(super) async fn check_with_instances(
        &self,
        req: &QuotaRequest,
        rule: &RateLimit,
        trigger: &LimitTrigger,
        instances: &[Instance],
    ) -> Result<QuotaResponse, PoleError> {
        self.check(req, rule, trigger, instances, "test-client")
            .await
    }
}

fn build_init_request(
    client_id: &str,
    key: &WindowKey,
    trigger: &LimitTrigger,
) -> RateLimitInitRequest {
    RateLimitInitRequest {
        target: Some(key.target()),
        client_id: client_id.to_string(),
        totals: trigger
            .amounts
            .iter()
            .map(|amount| QuotaTotal {
                mode: quota_mode(trigger).into(),
                duration: amount_duration_seconds(amount),
                max_amount: amount.max_amount,
            })
            .collect(),
        slide_count: 0,
        // SDK 侧维护已授权配额并向服务端报告实际消耗，使用 batch occupy 与
        // Polaris Go 客户端保持一致；服务端返回的 counter.left 才是最终配额来源。
        mode: pole_specification::polaris::metric::v2::Mode::BatchOccupy.into(),
    }
}

fn quota_mode(trigger: &LimitTrigger) -> RemoteQuotaMode {
    match trigger.amount_mode() {
        AmountMode::ShareEqually => RemoteQuotaMode::Divide,
        AmountMode::GlobalTotal => RemoteQuotaMode::Whole,
    }
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

fn logical_window_key(key: &WindowKey) -> String {
    format!(
        "{}#{}#{}#{}#{}#{}#{}",
        key.cluster_namespace,
        key.cluster_service,
        key.target_namespace,
        key.target_service,
        key.labels,
        key.rule_id,
        key.trigger_name
    )
}

#[cfg(test)]
fn select_endpoint(instances: &[Instance], affinity: &str) -> Result<String, PoleError> {
    let available = available_grpc_endpoints(instances)?;
    Ok(select_endpoint_from_available(&available, affinity))
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

fn network_error(context: &str, error: impl std::fmt::Display) -> PoleError {
    PoleError::new(ErrorCode::NetworkError, format!("{context}: {error}"))
}

#[cfg(test)]
mod tests {
    use std::{
        pin::Pin,
        sync::{
            atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering},
            Arc, Mutex as StdMutex,
        },
        time::Duration,
    };

    use pole_specification::{
        polaris::metric::v2::{
            rate_limit_grpcv2_server::{RateLimitGrpcv2, RateLimitGrpcv2Server},
            QuotaCounter, QuotaLeft, RateLimitCmd, RateLimitInitResponse, RateLimitReportResponse,
            RateLimitRequest, RateLimitResponse, TimeAdjustRequest, TimeAdjustResponse,
        },
        v1::{
            limit_trigger, match_argument, match_string, rate_limit, Amount, LimitTrigger,
            MatchArgument, MatchString, RateLimit, RateLimitCluster,
        },
    };
    use prost_types::Duration as ProstDuration;
    use tokio::{net::TcpListener, sync::mpsc};
    use tokio_stream::{
        wrappers::{ReceiverStream, TcpListenerStream},
        Stream,
    };
    use tonic::{Request, Response, Status};

    use super::*;
    use crate::{
        core::{config::config::Configuration, context::SDKContext, model::ArgumentType},
        traffic::ratelimit::{api::RateLimitAPI, default::DefaultRateLimitAPI},
    };

    struct TestRateLimitServer {
        remaining: Arc<AtomicI64>,
        observation: TestServerObservation,
        clock_offset_ms: i64,
    }

    #[derive(Clone, Default)]
    struct TestServerObservation {
        init_count: Arc<AtomicU32>,
        client_ids: Arc<StdMutex<Vec<String>>>,
        labels: Arc<StdMutex<Vec<String>>>,
        report_timestamps: Arc<StdMutex<Vec<i64>>>,
        time_adjust_count: Arc<AtomicU32>,
        stream_count: Arc<AtomicU32>,
        report_code_once: Arc<AtomicU32>,
        close_after_report_error: Arc<AtomicBool>,
        init_delay_ms: Arc<AtomicU64>,
    }

    #[tonic::async_trait]
    impl RateLimitGrpcv2 for TestRateLimitServer {
        type ServiceStream = Pin<Box<dyn Stream<Item = Result<RateLimitResponse, Status>> + Send>>;

        async fn service(
            &self,
            request: Request<tonic::Streaming<RateLimitRequest>>,
        ) -> Result<Response<Self::ServiceStream>, Status> {
            self.observation
                .stream_count
                .fetch_add(1, Ordering::Relaxed);
            let mut inbound = request.into_inner();
            let (sender, receiver) = mpsc::channel(16);
            let remaining = self.remaining.clone();
            let observation = self.observation.clone();
            let clock_offset_ms = self.clock_offset_ms;
            tokio::spawn(async move {
                while let Ok(Some(request)) = inbound.message().await {
                    match RateLimitCmd::try_from(request.cmd).ok() {
                        Some(RateLimitCmd::Init) => {
                            let Some(init) = request.rate_limit_init_request else {
                                return;
                            };
                            observation.init_count.fetch_add(1, Ordering::Relaxed);
                            let init_delay_ms = observation.init_delay_ms.load(Ordering::Relaxed);
                            if init_delay_ms > 0 {
                                tokio::time::sleep(Duration::from_millis(init_delay_ms)).await;
                            }
                            observation
                                .client_ids
                                .lock()
                                .unwrap()
                                .push(init.client_id.clone());
                            observation
                                .labels
                                .lock()
                                .unwrap()
                                .push(init.target.as_ref().unwrap().labels.clone());
                            let duration =
                                init.totals.first().map(|total| total.duration).unwrap_or(1);
                            let response = RateLimitResponse {
                                cmd: RateLimitCmd::Init.into(),
                                rate_limit_init_response: Some(RateLimitInitResponse {
                                    code: RATE_LIMIT_SUCCESS_CODE,
                                    target: init.target,
                                    client_key: 7,
                                    counters: vec![QuotaCounter {
                                        duration,
                                        counter_key: 101,
                                        left: remaining.load(Ordering::Relaxed),
                                        mode: 1,
                                        client_count: 1,
                                    }],
                                    slide_count: 1,
                                    timestamp: now_millis() + clock_offset_ms,
                                }),
                                rate_limit_report_response: None,
                                rate_limit_batch_init_response: None,
                            };
                            if sender.send(Ok(response)).await.is_err() {
                                return;
                            }
                        }
                        Some(RateLimitCmd::Acquire) => {
                            let Some(report) = request.rate_limit_report_request else {
                                return;
                            };
                            observation
                                .report_timestamps
                                .lock()
                                .unwrap()
                                .push(report.timestamp);
                            let report_code =
                                observation.report_code_once.swap(0, Ordering::Relaxed);
                            if report_code != 0 {
                                let response = RateLimitResponse {
                                    cmd: RateLimitCmd::Acquire.into(),
                                    rate_limit_init_response: None,
                                    rate_limit_report_response: Some(RateLimitReportResponse {
                                        code: report_code,
                                        quota_lefts: Vec::new(),
                                        timestamp: now_millis() + clock_offset_ms,
                                    }),
                                    rate_limit_batch_init_response: None,
                                };
                                if sender.send(Ok(response)).await.is_err()
                                    || observation.close_after_report_error.load(Ordering::Relaxed)
                                {
                                    return;
                                }
                                continue;
                            }
                            let used = report.quota_uses.iter().map(|sum| sum.used).sum::<u32>();
                            let left = remaining.fetch_sub(i64::from(used), Ordering::Relaxed)
                                - i64::from(used);
                            let response = RateLimitResponse {
                                cmd: RateLimitCmd::Acquire.into(),
                                rate_limit_init_response: None,
                                rate_limit_report_response: Some(RateLimitReportResponse {
                                    code: RATE_LIMIT_SUCCESS_CODE,
                                    quota_lefts: report
                                        .quota_uses
                                        .iter()
                                        .map(|sum| QuotaLeft {
                                            counter_key: sum.counter_key,
                                            left,
                                            mode: 1,
                                            client_count: 1,
                                        })
                                        .collect(),
                                    timestamp: now_millis() + clock_offset_ms,
                                }),
                                rate_limit_batch_init_response: None,
                            };
                            if sender.send(Ok(response)).await.is_err() {
                                return;
                            }
                        }
                        _ => return,
                    }
                }
            });
            Ok(Response::new(Box::pin(ReceiverStream::new(receiver))))
        }

        async fn time_adjust(
            &self,
            _request: Request<TimeAdjustRequest>,
        ) -> Result<Response<TimeAdjustResponse>, Status> {
            self.observation
                .time_adjust_count
                .fetch_add(1, Ordering::Relaxed);
            Ok(Response::new(TimeAdjustResponse {
                server_timestamp: now_millis() + self.clock_offset_ms,
            }))
        }
    }

    async fn start_test_server() -> (u32, TestServerObservation, tokio::task::JoinHandle<()>) {
        start_test_server_with_clock_offset(0).await
    }

    async fn start_test_server_with_clock_offset(
        clock_offset_ms: i64,
    ) -> (u32, TestServerObservation, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let observation = TestServerObservation::default();
        let server = TestRateLimitServer {
            remaining: Arc::new(AtomicI64::new(2)),
            observation: observation.clone(),
            clock_offset_ms,
        };
        let task = tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(RateLimitGrpcv2Server::new(server))
                .serve_with_incoming(TcpListenerStream::new(listener))
                .await
                .unwrap();
        });
        (u32::from(port), observation, task)
    }

    fn entry_test_configuration() -> Configuration {
        let connector_map = serde_yaml::from_str(
            r#"
global:
  api:
    timeout: 1s
    maxRetryTimes: 1
    retryInterval: 1ms
    reportInterval: 1s
  serverConnectors:
    discover:
      addresses: [127.0.0.1:1]
      protocol: grpc
      connectTimeout: 1ms
      messageTimeout: 1ms
    config:
      addresses: [127.0.0.1:1]
      protocol: grpc
      connectTimeout: 1ms
      messageTimeout: 1ms
  location:
    providers:
      - name: local
        options: {}
  client:
    id: sdk-entry-client
    labels: {}
consumer:
  serviceRouter:
    beforeChain: []
    coreChain: []
    afterChain: []
  loadBalancer:
    defaultPolicy: weightedRandom
    plugins: []
  localCache:
    name: memory
    serviceExpireEnable: false
    serviceExpireTime: 1s
    serviceRefreshInterval: 1s
    serviceListRefreshInterval: 1s
    persistEnable: false
    persistDir: ./target/test-cache
provider:
  lossless:
    host: 127.0.0.1
    port: 0
    delayRegisterInterval: 1ms
    healthCheckInterval: 1ms
config:
  propertiesValueCacheSize: 1
  propertiesValueExpireTime: 1
  configFilter:
    enable: false
    chain: []
    plugin: {}
"#,
        );
        connector_map
            .or_else(|_| {
                serde_yaml::from_str(
                    r#"
global:
  api:
    timeout: 1s
    maxRetryTimes: 1
    retryInterval: 1ms
    reportInterval: 1s
  serverConnectors:
    addresses: [127.0.0.1:1]
    protocol: grpc
    connectTimeout: 1ms
    serverSwitchInterval: 1s
    messageTimeout: 1ms
    connectionIdleTimeout: 1s
    reconnectInterval: 1ms
  statReporter:
    enable: false
    chain: []
  location:
    providers:
      - name: local
        options: {}
  client:
    id: sdk-entry-client
    labels: {}
consumer:
  serviceRouter:
    beforeChain: []
    coreChain: []
    afterChain: []
  circuitBreaker:
    enable: false
    enableRemotePull: false
  loadBalancer:
    defaultPolicy: weightedRandom
    plugins: []
  localCache:
    name: memory
    serviceExpireEnable: false
    serviceExpireTime: 1s
    serviceRefreshInterval: 1s
    serviceListRefreshInterval: 1s
    persistEnable: false
    persistDir: ./target/test-cache
provider:
  rateLimit:
    enable: true
    service: pole.limiter
    namespace: Pole
    maxWindowCount: 100
    fallbackOnExceedWindowCount: pass
    remoteSyncTimeout: 1s
    maxQueuingTime: 1s
    reportMetrics: false
  lossless:
    enable: false
    host: 127.0.0.1
    port: 0
    delayRegisterInterval: 1ms
    healthCheckInterval: 1ms
config:
  propertiesValueCacheSize: 1
  propertiesValueExpireTime: 1
  configFilter:
    enable: false
    chain: []
    plugin: {}
"#,
                )
            })
            .unwrap()
    }

    fn no_traffic_label(_: ArgumentType, _: &str) -> Option<String> {
        None
    }

    fn alice_header(arg_type: ArgumentType, key: &str) -> Option<String> {
        (arg_type == ArgumentType::Header && key == "x-user").then(|| "alice".to_string())
    }

    fn bob_header(arg_type: ArgumentType, key: &str) -> Option<String> {
        (arg_type == ArgumentType::Header && key == "x-user").then(|| "bob".to_string())
    }

    fn quota_request() -> QuotaRequest {
        QuotaRequest {
            flow_id: "flow-1".to_string(),
            timeout: Duration::from_secs(1),
            service: "orders".to_string(),
            namespace: "default".to_string(),
            method: "GET".to_string(),
            traffic_label_provider: no_traffic_label,
        }
    }

    fn global_rule(revision: &str) -> RateLimit {
        RateLimit {
            id: "global-rule".to_string(),
            revision: revision.to_string(),
            r#type: rate_limit::Type::Global.into(),
            cluster: Some(RateLimitCluster {
                namespace: "Pole".to_string(),
                service: "pole-limiter".to_string(),
            }),
            rules: vec![LimitTrigger {
                name: "qps".to_string(),
                resource: limit_trigger::Resource::Qps.into(),
                amounts: vec![Amount {
                    max_amount: 2,
                    valid_duration: Some(ProstDuration {
                        seconds: 1,
                        nanos: 0,
                    }),
                    ..Default::default()
                }],
                amount_mode: limit_trigger::AmountMode::GlobalTotal.into(),
                ..Default::default()
            }],
            ..Default::default()
        }
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

    #[tokio::test]
    async fn public_get_quota_filters_instances_and_uses_engine_client_id() {
        let (limiter_port, observation, limiter_server) = start_test_server().await;
        let rule = global_rule("rev-1");
        let context =
            Arc::new(SDKContext::create_by_configuration(entry_test_configuration()).unwrap());
        let mut http = limiter_instance(1);
        http.protocol = "http".to_string();
        let api = DefaultRateLimitAPI::new_with_test_inputs(
            context,
            vec![rule],
            vec![http, limiter_instance(limiter_port)],
        );

        let response = api.get_quota(quota_request()).await.unwrap();

        assert!(response.allowed);
        assert_eq!(
            observation.client_ids.lock().unwrap().as_slice(),
            ["sdk-entry-client"]
        );
        assert_eq!(observation.init_count.load(Ordering::Relaxed), 1);

        limiter_server.abort();
    }

    #[tokio::test]
    async fn global_rule_uses_rule_cluster_and_reuses_its_initialized_window() {
        let (port, observation, server) = start_test_server().await;
        let manager = DistributedQuotaManager::default();
        let req = quota_request();
        let rule = global_rule("rev-1");
        let trigger = &rule.rules[0];
        let instances = vec![limiter_instance(port)];

        assert!(
            manager
                .check_with_instances(&req, &rule, trigger, &instances)
                .await
                .unwrap()
                .allowed
        );
        assert!(
            manager
                .check_with_instances(&req, &rule, trigger, &instances)
                .await
                .unwrap()
                .allowed
        );
        assert!(
            !manager
                .check_with_instances(&req, &rule, trigger, &instances)
                .await
                .unwrap()
                .allowed
        );
        assert_eq!(observation.init_count.load(Ordering::Relaxed), 1);

        server.abort();
    }

    #[tokio::test]
    async fn rule_revision_change_creates_a_new_remote_window() {
        let (port, observation, server) = start_test_server().await;
        let manager = DistributedQuotaManager::default();
        let req = quota_request();
        let instances = vec![limiter_instance(port)];
        let first = global_rule("rev-1");
        let second = global_rule("rev-2");

        manager
            .check_with_instances(&req, &first, &first.rules[0], &instances)
            .await
            .unwrap();
        manager
            .check_with_instances(&req, &second, &second.rules[0], &instances)
            .await
            .unwrap();
        assert_eq!(observation.init_count.load(Ordering::Relaxed), 2);
        assert_eq!(manager.windows.len(), 1);

        server.abort();
    }

    #[tokio::test]
    async fn report_response_does_not_restore_quota_consumed_while_in_flight() {
        let req = quota_request();
        let rule = global_rule("rev-1");
        let trigger = &rule.rules[0];
        let key = WindowKey::from_rule(&req, &rule, trigger, "127.0.0.1:8081").unwrap();
        let window = QuotaWindow::new(key, trigger);
        window.state.lock().await.owner_session_id = 1;
        window
            .mark_initialized(
                1,
                7,
                vec![QuotaCounter {
                    duration: 1,
                    counter_key: 101,
                    left: 2,
                    mode: 1,
                    client_count: 1,
                }],
            )
            .await;

        assert!(window.acquire(trigger).await.unwrap().0);
        assert!(window.acquire(trigger).await.unwrap().0);
        window
            .update_counters(
                1,
                now_millis(),
                vec![QuotaLeft {
                    counter_key: 101,
                    left: 1,
                    mode: 1,
                    client_count: 1,
                }],
            )
            .await;

        assert!(!window.acquire(trigger).await.unwrap().0);
    }

    #[tokio::test]
    async fn report_response_does_not_double_count_the_acknowledged_batch() {
        let req = quota_request();
        let rule = global_rule("rev-1");
        let trigger = &rule.rules[0];
        let key = WindowKey::from_rule(&req, &rule, trigger, "127.0.0.1:8081").unwrap();
        let window = QuotaWindow::new(key, trigger);
        window.state.lock().await.owner_session_id = 1;
        window
            .mark_initialized(
                1,
                7,
                vec![QuotaCounter {
                    duration: 1,
                    counter_key: 101,
                    left: 2,
                    mode: 1,
                    client_count: 1,
                }],
            )
            .await;

        let (allowed, sums, _) = window.acquire(trigger).await.unwrap();
        assert!(allowed);
        let timestamp = now_millis();
        {
            let mut state = window.state.lock().await;
            QuotaWindow::mark_reported(&mut state, timestamp, &sums);
        }
        window
            .update_counters(
                1,
                timestamp,
                vec![QuotaLeft {
                    counter_key: 101,
                    left: 1,
                    mode: 1,
                    client_count: 1,
                }],
            )
            .await;

        assert!(window.acquire(trigger).await.unwrap().0);
    }

    #[tokio::test]
    async fn non_positive_push_does_not_ack_local_report_fifo() {
        let req = quota_request();
        let rule = global_rule("rev-1");
        let trigger = &rule.rules[0];
        let key = WindowKey::from_rule(&req, &rule, trigger, "127.0.0.1:8081").unwrap();
        let window = QuotaWindow::new(key, trigger);
        window.state.lock().await.owner_session_id = 1;
        window
            .mark_initialized(
                1,
                7,
                vec![QuotaCounter {
                    duration: 1,
                    counter_key: 101,
                    left: 1,
                    mode: 1,
                    client_count: 1,
                }],
            )
            .await;

        let (_, used, _) = window.acquire(trigger).await.unwrap();
        let (_, limited, _) = window.acquire(trigger).await.unwrap();
        let timestamp = now_millis();
        {
            let mut state = window.state.lock().await;
            QuotaWindow::mark_reported(&mut state, timestamp, &used);
            QuotaWindow::mark_reported(&mut state, timestamp, &limited);
        }
        for _ in 0..2 {
            window
                .update_counters(
                    1,
                    timestamp,
                    vec![QuotaLeft {
                        counter_key: 101,
                        left: 0,
                        mode: 1,
                        client_count: 1,
                    }],
                )
                .await;
        }

        let state = window.state.lock().await;
        let counter = state.counters.get(&1).unwrap();
        assert_eq!(counter.inflight_used.len(), 2);
        assert!(counter.left <= 0);
    }

    #[tokio::test]
    async fn revision_init_is_serialized_for_the_same_remote_target() {
        let (port, observation, server) = start_test_server().await;
        observation.init_delay_ms.store(50, Ordering::Relaxed);
        let manager = Arc::new(DistributedQuotaManager::default());
        let req = quota_request();
        let instances = vec![limiter_instance(port)];
        let first = global_rule("rev-1");
        let second = global_rule("rev-2");

        let first_check = {
            let manager = manager.clone();
            let req = req.clone();
            let instances = instances.clone();
            tokio::spawn(async move {
                manager
                    .check_with_instances(&req, &first, &first.rules[0], &instances)
                    .await
            })
        };
        timeout(Duration::from_secs(1), async {
            while observation.init_count.load(Ordering::Relaxed) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let second_check = {
            let manager = manager.clone();
            let req = req.clone();
            let instances = instances.clone();
            tokio::spawn(async move {
                manager
                    .check_with_instances(&req, &second, &second.rules[0], &instances)
                    .await
            })
        };

        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(observation.init_count.load(Ordering::Relaxed), 1);
        first_check.await.unwrap().unwrap();
        second_check.await.unwrap().unwrap();
        assert_eq!(observation.init_count.load(Ordering::Relaxed), 2);

        server.abort();
    }

    #[tokio::test]
    async fn global_rule_sticks_to_one_grpc_endpoint() {
        let (first_port, first_observation, first_server) = start_test_server().await;
        let (second_port, second_observation, second_server) = start_test_server().await;
        let manager = DistributedQuotaManager::default();
        let req = quota_request();
        let rule = global_rule("rev-1");
        let instances = vec![limiter_instance(first_port), limiter_instance(second_port)];

        assert!(
            manager
                .check_with_instances(&req, &rule, &rule.rules[0], &instances)
                .await
                .unwrap()
                .allowed
        );
        assert!(
            manager
                .check_with_instances(&req, &rule, &rule.rules[0], &instances)
                .await
                .unwrap()
                .allowed
        );

        assert_eq!(
            first_observation.init_count.load(Ordering::Relaxed)
                + second_observation.init_count.load(Ordering::Relaxed),
            1
        );
        assert_eq!(manager.windows.len(), 1);

        first_server.abort();
        second_server.abort();
    }

    #[tokio::test]
    async fn endpoint_affinity_is_isolated_by_limiter_cluster() {
        let (first_port, _, first_server) = start_test_server().await;
        let (second_port, _, second_server) = start_test_server().await;
        let manager = DistributedQuotaManager::default();
        let req = quota_request();
        let first = global_rule("rev-1");
        let mut second = global_rule("rev-1");
        second.cluster.as_mut().unwrap().service = "pole-limiter-canary".to_string();

        manager
            .check_with_instances(
                &req,
                &first,
                &first.rules[0],
                &[limiter_instance(first_port)],
            )
            .await
            .unwrap();
        manager
            .check_with_instances(
                &req,
                &second,
                &second.rules[0],
                &[limiter_instance(second_port)],
            )
            .await
            .unwrap();

        assert_eq!(manager.endpoint_bindings.len(), 2);
        assert_eq!(manager.windows.len(), 2);

        first_server.abort();
        second_server.abort();
    }

    #[tokio::test]
    async fn expired_window_cleanup_closes_unused_endpoint_session() {
        let (port, _, server) = start_test_server().await;
        let manager = DistributedQuotaManager::default();
        let req = quota_request();
        let rule = global_rule("rev-1");
        let endpoint = format!("127.0.0.1:{port}");
        manager
            .check_with_instances(&req, &rule, &rule.rules[0], &[limiter_instance(port)])
            .await
            .unwrap();
        let window = manager.windows.iter().next().unwrap().value().clone();
        window
            .last_used_ms
            .store(now_millis() - window.idle_ttl_ms - 1, Ordering::Release);
        manager
            .check_count
            .store(WINDOW_CLEANUP_INTERVAL, Ordering::Relaxed);

        manager.cleanup_windows_if_needed();

        assert!(manager.windows.is_empty());
        assert!(!manager.sessions.contains_key(&endpoint));
        server.abort();
    }

    #[tokio::test]
    async fn active_window_does_not_move_when_new_grpc_instances_appear() {
        let (port, observation, server) = start_test_server().await;
        let manager = DistributedQuotaManager::default();
        let req = quota_request();
        let rule = global_rule("rev-1");
        let affinity = window_affinity(&req, &rule, &rule.rules[0]).unwrap();
        let active = limiter_instance(port);
        let mut expanded = vec![active.clone()];
        for candidate_port in 1..=128 {
            if candidate_port == port {
                continue;
            }
            expanded.push(limiter_instance(candidate_port));
            if select_endpoint(&expanded, &affinity).unwrap() != format!("127.0.0.1:{port}") {
                break;
            }
        }
        assert_ne!(
            select_endpoint(&expanded, &affinity).unwrap(),
            format!("127.0.0.1:{port}")
        );

        manager
            .check_with_instances(&req, &rule, &rule.rules[0], std::slice::from_ref(&active))
            .await
            .unwrap();
        manager
            .check_with_instances(&req, &rule, &rule.rules[0], &expanded)
            .await
            .unwrap();

        assert_eq!(observation.init_count.load(Ordering::Relaxed), 1);
        assert_eq!(manager.windows.len(), 1);

        server.abort();
    }

    #[tokio::test]
    async fn global_rule_ignores_http_instances_registered_with_limiter_service() {
        let (grpc_port, observation, server) = start_test_server().await;
        let manager = DistributedQuotaManager::default();
        let req = quota_request();
        let rule = global_rule("rev-1");
        let mut http = limiter_instance(1);
        http.protocol = "http".to_string();
        let instances = vec![http, limiter_instance(grpc_port)];

        assert!(
            manager
                .check_with_instances(&req, &rule, &rule.rules[0], &instances)
                .await
                .unwrap()
                .allowed
        );
        assert_eq!(observation.init_count.load(Ordering::Relaxed), 1);

        server.abort();
    }

    #[tokio::test]
    async fn remote_counter_labels_follow_the_canonical_method_dimension() {
        let (port, observation, server) = start_test_server().await;
        let manager = DistributedQuotaManager::default();
        let rule = global_rule("rev-1");
        let mut get = quota_request();
        get.method = "GET".to_string();
        let mut post = quota_request();
        post.method = "POST".to_string();
        let instances = vec![limiter_instance(port)];

        manager
            .check_with_instances(&get, &rule, &rule.rules[0], &instances)
            .await
            .unwrap();
        manager
            .check_with_instances(&post, &rule, &rule.rules[0], &instances)
            .await
            .unwrap();

        assert_eq!(
            observation.labels.lock().unwrap().as_slice(),
            ["GET|", "POST|"]
        );

        server.abort();
    }

    #[tokio::test]
    async fn remote_counter_labels_follow_canonical_dynamic_argument_dimensions() {
        let (port, observation, server) = start_test_server().await;
        let manager = DistributedQuotaManager::default();
        let mut rule = global_rule("rev-1");
        rule.rules[0].arguments = vec![MatchArgument {
            r#type: match_argument::Type::Header.into(),
            key: "x-user".to_string(),
            value: Some(MatchString {
                r#type: match_string::MatchStringType::Exact.into(),
                value: String::new(),
                value_type: match_string::ValueType::Parameter.into(),
            }),
        }];
        let mut alice = quota_request();
        alice.traffic_label_provider = alice_header;
        let mut bob = quota_request();
        bob.traffic_label_provider = bob_header;
        let instances = vec![limiter_instance(port)];

        manager
            .check_with_instances(&alice, &rule, &rule.rules[0], &instances)
            .await
            .unwrap();
        manager
            .check_with_instances(&bob, &rule, &rule.rules[0], &instances)
            .await
            .unwrap();

        assert_eq!(
            observation.labels.lock().unwrap().as_slice(),
            ["GET|HEADER:x-user:alice", "GET|HEADER:x-user:bob"]
        );

        server.abort();
    }

    #[tokio::test]
    async fn acquire_report_uses_time_adjusted_server_timestamp() {
        const CLOCK_OFFSET_MS: i64 = 60_000;
        let (port, observation, server) =
            start_test_server_with_clock_offset(CLOCK_OFFSET_MS).await;
        let manager = DistributedQuotaManager::default();
        let req = quota_request();
        let rule = global_rule("rev-1");
        let instances = vec![limiter_instance(port)];
        let earliest = now_millis() + CLOCK_OFFSET_MS;

        manager
            .check_with_instances(&req, &rule, &rule.rules[0], &instances)
            .await
            .unwrap();

        timeout(Duration::from_secs(1), async {
            loop {
                if !observation.report_timestamps.lock().unwrap().is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let latest = now_millis() + CLOCK_OFFSET_MS;
        let timestamp = observation.report_timestamps.lock().unwrap()[0];
        assert!(((earliest - 20)..=(latest + 20)).contains(&timestamp));
        assert_eq!(observation.time_adjust_count.load(Ordering::Relaxed), 1);

        server.abort();
    }

    #[tokio::test]
    async fn healthy_session_periodically_refreshes_its_clock_offset() {
        let (port, observation, server) = start_test_server().await;
        let manager = DistributedQuotaManager::default();
        let req = quota_request();
        let rule = global_rule("rev-1");
        let instances = vec![limiter_instance(port)];

        manager
            .check(&req, &rule, &rule.rules[0], &instances, "sdk-instance-id")
            .await
            .unwrap();
        let endpoint = format!("127.0.0.1:{port}");
        manager
            .sessions
            .get(&endpoint)
            .unwrap()
            .last_time_adjust_ms
            .store(0, Ordering::Release);
        manager
            .check(&req, &rule, &rule.rules[0], &instances, "sdk-instance-id")
            .await
            .unwrap();

        assert_eq!(observation.time_adjust_count.load(Ordering::Relaxed), 2);
        assert_eq!(
            observation.client_ids.lock().unwrap().as_slice(),
            ["sdk-instance-id"]
        );

        server.abort();
    }

    #[tokio::test]
    async fn acquire_not_found_reinitializes_windows_on_the_same_stream() {
        let (port, observation, server) = start_test_server().await;
        observation
            .report_code_once
            .store(RATE_LIMIT_NOT_FOUND_CODE, Ordering::Relaxed);
        let manager = DistributedQuotaManager::default();
        let req = quota_request();
        let rule = global_rule("rev-1");
        let instances = vec![limiter_instance(port)];

        manager
            .check_with_instances(&req, &rule, &rule.rules[0], &instances)
            .await
            .unwrap();
        timeout(Duration::from_secs(1), async {
            loop {
                manager
                    .check_with_instances(&req, &rule, &rule.rules[0], &instances)
                    .await
                    .unwrap();
                if observation.init_count.load(Ordering::Relaxed) >= 2 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        assert_eq!(observation.init_count.load(Ordering::Relaxed), 2);
        assert_eq!(observation.stream_count.load(Ordering::Relaxed), 1);
        assert_eq!(observation.time_adjust_count.load(Ordering::Relaxed), 1);

        server.abort();
    }

    #[tokio::test]
    async fn invalid_counter_reconnects_and_reinitializes_before_reuse() {
        let (port, observation, server) = start_test_server().await;
        observation
            .report_code_once
            .store(RATE_LIMIT_INVALID_COUNTER_CODE, Ordering::Relaxed);
        observation
            .close_after_report_error
            .store(true, Ordering::Relaxed);
        let manager = DistributedQuotaManager::default();
        let req = quota_request();
        let rule = global_rule("rev-1");
        let instances = vec![limiter_instance(port)];

        manager
            .check_with_instances(&req, &rule, &rule.rules[0], &instances)
            .await
            .unwrap();
        timeout(Duration::from_secs(1), async {
            loop {
                manager
                    .check_with_instances(&req, &rule, &rule.rules[0], &instances)
                    .await
                    .unwrap();
                if observation.init_count.load(Ordering::Relaxed) >= 2 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        assert_eq!(observation.init_count.load(Ordering::Relaxed), 2);
        assert_eq!(observation.stream_count.load(Ordering::Relaxed), 2);
        assert_eq!(observation.time_adjust_count.load(Ordering::Relaxed), 2);

        server.abort();
    }
}
