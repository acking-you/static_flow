use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashMap},
    env,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering as AtomicOrdering},
        Arc, Weak,
    },
    time::{Duration, Instant, SystemTime},
};

use anyhow::{anyhow, Context, Result};
use dashmap::DashMap;
use parking_lot::Mutex;
use reqwest::header::HeaderValue as ReqwestHeaderValue;
use serde::Deserialize;
use static_flow_shared::llm_gateway_store::{LlmGatewayKeyRecord, LlmGatewayStore};
use tokio::{
    sync::{mpsc, Mutex as AsyncMutex},
    time::MissedTickBehavior,
};

use super::{accounts::AccountPool, types::LlmGatewayRateLimitStatusResponse};
use crate::{state::LlmGatewayRuntimeConfig, upstream_proxy::UpstreamProxyRegistry};

const CLEANER_TICK_SECONDS: u64 = 1;

/// Long-lived runtime state shared by all gateway handlers.
#[derive(Clone)]
pub struct LlmGatewayRuntimeState {
    pub(crate) store: Arc<LlmGatewayStore>,
    pub(crate) runtime_config: Arc<tokio::sync::RwLock<LlmGatewayRuntimeConfig>>,
    pub(crate) auth_source: Arc<CodexAuthSource>,
    pub(crate) account_pool: Arc<AccountPool>,
    pub(crate) upstream_proxy_registry: Arc<UpstreamProxyRegistry>,
    pub(crate) key_cache: Arc<LlmGatewayKeyCache>,
    pub(crate) request_scheduler: Arc<LlmGatewayKeyRequestScheduler>,
    pub(crate) rate_limit_status: Arc<tokio::sync::RwLock<LlmGatewayRateLimitStatusResponse>>,
    pub(crate) usage_write_lock: Arc<AsyncMutex<()>>,
}

impl LlmGatewayRuntimeState {
    /// Construct the shared runtime state used by all LLM gateway requests.
    pub fn new(
        store: Arc<LlmGatewayStore>,
        runtime_config: Arc<tokio::sync::RwLock<LlmGatewayRuntimeConfig>>,
        account_pool: Arc<AccountPool>,
        upstream_proxy_registry: Arc<UpstreamProxyRegistry>,
    ) -> Result<Self> {
        Ok(Self {
            store,
            runtime_config,
            auth_source: Arc::new(CodexAuthSource::new()),
            account_pool,
            upstream_proxy_registry,
            key_cache: Arc::new(LlmGatewayKeyCache::new()),
            request_scheduler: Arc::new(LlmGatewayKeyRequestScheduler::new()),
            rate_limit_status: Arc::new(tokio::sync::RwLock::new(
                LlmGatewayRateLimitStatusResponse {
                    status: "loading".to_string(),
                    refresh_interval_seconds: 60,
                    last_checked_at: None,
                    last_success_at: None,
                    source_url: String::new(),
                    error_message: None,
                    buckets: Vec::new(),
                },
            )),
            usage_write_lock: Arc::new(AsyncMutex::new(())),
        })
    }

    pub(crate) async fn build_upstream_client(&self) -> Result<reqwest::Client> {
        let builder = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .pool_max_idle_per_host(32)
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_keepalive(Duration::from_secs(30));
        let builder = self
            .upstream_proxy_registry
            .apply_provider_proxy(
                static_flow_shared::llm_gateway_store::LLM_GATEWAY_PROVIDER_CODEX,
                builder,
            )
            .await
            .context("failed to resolve codex upstream proxy")?;
        builder
            .build()
            .context("failed to build llm gateway reqwest client")
    }
}

#[derive(Debug, Clone)]
struct KeyRequestState {
    in_flight: usize,
    next_start_at: Instant,
}

#[derive(Debug, Clone)]
pub(crate) struct CodexKeyRequestLimitRejection {
    pub reason: &'static str,
    pub in_flight: usize,
    pub max_concurrency: Option<u64>,
    pub min_start_interval_ms: Option<u64>,
    pub wait: Option<Duration>,
    pub elapsed_since_last_start_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct LlmGatewayKeyRequestScheduler {
    states: Arc<Mutex<HashMap<String, KeyRequestState>>>,
}

#[derive(Debug)]
pub(crate) struct CodexKeyRequestLease {
    scheduler: Option<Arc<LlmGatewayKeyRequestScheduler>>,
    key_id: String,
    released: bool,
}

impl LlmGatewayKeyRequestScheduler {
    fn new() -> Self {
        Self {
            states: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) fn try_acquire(
        self: &Arc<Self>,
        key: &LlmGatewayKeyRecord,
    ) -> Result<CodexKeyRequestLease, CodexKeyRequestLimitRejection> {
        let max_concurrency = key.request_max_concurrency.filter(|value| *value > 0);
        let min_start_interval_ms = key.request_min_start_interval_ms;
        if max_concurrency.is_none() && min_start_interval_ms.is_none() {
            return Ok(CodexKeyRequestLease {
                scheduler: None,
                key_id: key.id.clone(),
                released: false,
            });
        }

        let now = Instant::now();
        let mut states = self.states.lock();
        let state = states
            .entry(key.id.clone())
            .or_insert_with(|| KeyRequestState {
                in_flight: 0,
                next_start_at: now,
            });

        if let Some(limit) = max_concurrency {
            if state.in_flight >= limit as usize {
                return Err(CodexKeyRequestLimitRejection {
                    reason: "local_concurrency_limit",
                    in_flight: state.in_flight,
                    max_concurrency,
                    min_start_interval_ms,
                    wait: None,
                    elapsed_since_last_start_ms: None,
                });
            }
        }

        if let Some(interval_ms) = min_start_interval_ms {
            if now < state.next_start_at {
                let wait = state.next_start_at.saturating_duration_since(now);
                let elapsed_since_last_start_ms =
                    interval_ms.saturating_sub(wait.as_millis() as u64);
                return Err(CodexKeyRequestLimitRejection {
                    reason: "local_start_interval",
                    in_flight: state.in_flight,
                    max_concurrency,
                    min_start_interval_ms,
                    wait: Some(wait),
                    elapsed_since_last_start_ms: Some(elapsed_since_last_start_ms),
                });
            }
        }

        state.in_flight += 1;
        state.next_start_at = min_start_interval_ms
            .map(|value| now + Duration::from_millis(value))
            .unwrap_or(now);

        Ok(CodexKeyRequestLease {
            scheduler: Some(self.clone()),
            key_id: key.id.clone(),
            released: false,
        })
    }

    fn release(&self, key_id: &str) {
        let now = Instant::now();
        let mut states = self.states.lock();
        let remove_entry = if let Some(state) = states.get_mut(key_id) {
            if state.in_flight > 0 {
                state.in_flight -= 1;
            }
            state.in_flight == 0 && state.next_start_at <= now
        } else {
            false
        };
        if remove_entry {
            states.remove(key_id);
        }
    }
}

impl Drop for CodexKeyRequestLease {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        if let Some(scheduler) = self.scheduler.as_ref() {
            scheduler.release(&self.key_id);
        }
    }
}

/// In-memory snapshot of the upstream Codex login state.
#[derive(Debug, Clone)]
pub(crate) struct CodexAuthSnapshot {
    pub access_token: String,
    pub account_id: Option<String>,
    modified_at: Option<SystemTime>,
}

impl CodexAuthSnapshot {
    /// Build a snapshot without filesystem mtime (used by the account pool).
    pub(crate) fn from_tokens(access_token: String, account_id: Option<String>) -> Self {
        Self {
            access_token,
            account_id,
            modified_at: None,
        }
    }
}

/// Minimal shape read from `~/.codex/auth.json`.
#[derive(Debug, Clone, Deserialize)]
struct CodexAuthFile {
    #[serde(default)]
    tokens: CodexAuthTokens,
}

/// Token fields needed to authenticate against the upstream Codex backend.
#[derive(Debug, Clone, Default, Deserialize)]
struct CodexAuthTokens {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    account_id: Option<String>,
}

/// File-backed auth source with mtime-based hot reload.
pub(crate) struct CodexAuthSource {
    path: PathBuf,
    cached: tokio::sync::RwLock<Option<CodexAuthSnapshot>>,
}

impl CodexAuthSource {
    /// Create a new auth source bound to the resolved Codex auth.json path.
    pub(crate) fn new() -> Self {
        Self {
            path: codex_auth_path(),
            cached: tokio::sync::RwLock::new(None),
        }
    }

    /// Return the current cached-or-reloaded upstream auth snapshot.
    pub(crate) async fn current(&self) -> Result<CodexAuthSnapshot> {
        self.load_if_needed(false).await
    }

    /// Force a reload from disk after an upstream authentication failure.
    pub(crate) async fn force_reload(&self) -> Result<CodexAuthSnapshot> {
        self.load_if_needed(true).await
    }

    /// Reload auth.json only when the caller requests it or the file changed.
    async fn load_if_needed(&self, force: bool) -> Result<CodexAuthSnapshot> {
        let metadata = tokio::fs::metadata(&self.path)
            .await
            .with_context(|| format!("failed to stat `{}`", self.path.display()))?;
        let modified_at = metadata.modified().ok();

        if !force {
            if let Some(cached) = self.cached.read().await.clone() {
                if modified_matches(cached.modified_at, modified_at) {
                    return Ok(cached);
                }
            }
        }

        let raw = tokio::fs::read_to_string(&self.path)
            .await
            .with_context(|| format!("failed to read `{}`", self.path.display()))?;
        let parsed: CodexAuthFile = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse `{}`", self.path.display()))?;
        let access_token = parsed
            .tokens
            .access_token
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("`{}` missing tokens.access_token", self.path.display()))?;
        let snapshot = CodexAuthSnapshot {
            access_token,
            account_id: parsed
                .tokens
                .account_id
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            modified_at,
        };
        *self.cached.write().await = Some(snapshot.clone());
        Ok(snapshot)
    }
}

/// Compare two optional mtimes without treating missing metadata as an error.
fn modified_matches(left: Option<SystemTime>, right: Option<SystemTime>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left == right,
        (None, None) => true,
        _ => false,
    }
}

/// Resolve the upstream Codex auth.json path, honoring explicit overrides
/// first.
fn codex_auth_path() -> PathBuf {
    if let Ok(path) = env::var("CODEX_AUTH_JSON_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    let home = env::var("HOME").unwrap_or_else(|_| "/home/ts_user".to_string());
    PathBuf::from(home).join(".codex").join("auth.json")
}

/// Renewable in-memory cache for validated API keys.
pub(crate) struct LlmGatewayKeyCache {
    index: Arc<DashMap<String, Weak<CachedKeyLease>>>,
    queue: Arc<Mutex<BinaryHeap<ExpiringLease>>>,
    next_seq: AtomicU64,
    cleanup_tx: mpsc::UnboundedSender<String>,
}

impl LlmGatewayKeyCache {
    pub(crate) fn new() -> Self {
        let index = Arc::new(DashMap::new());
        let queue = Arc::new(Mutex::new(BinaryHeap::new()));
        let (cleanup_tx, mut cleanup_rx) = mpsc::unbounded_channel::<String>();
        let cache = Self {
            index: index.clone(),
            queue: queue.clone(),
            next_seq: AtomicU64::new(1),
            cleanup_tx: cleanup_tx.clone(),
        };
        let cleanup_index = index;
        tokio::spawn(async move {
            while let Some(key_hash) = cleanup_rx.recv().await {
                let should_remove = cleanup_index
                    .get(&key_hash)
                    .map(|entry| entry.value().upgrade().is_none())
                    .unwrap_or(false);
                if should_remove {
                    cleanup_index.remove(&key_hash);
                }
            }
        });

        let cleaner_queue = queue;
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(CLEANER_TICK_SECONDS));
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                let mut expired = Vec::<ExpiringLease>::new();
                {
                    let mut queue = cleaner_queue.lock();
                    let now = Instant::now();
                    while queue.peek().is_some_and(|entry| entry.expires_at <= now) {
                        if let Some(entry) = queue.pop() {
                            expired.push(entry);
                        }
                    }
                }
                drop(expired);
            }
        });
        cache
    }

    pub(crate) fn get(&self, key_hash: &str) -> Option<Arc<CachedKeyLease>> {
        let lease = self
            .index
            .get(key_hash)
            .and_then(|entry| entry.value().upgrade());
        if let Some(lease) = lease {
            if lease.expires_at > Instant::now() {
                return Some(lease);
            }
        }
        self.index.remove(key_hash);
        None
    }

    pub(crate) fn renew(&self, record: LlmGatewayKeyRecord, ttl: Duration) -> Arc<CachedKeyLease> {
        let key_hash = record.key_hash.clone();
        let expires_at = Instant::now() + ttl;
        let lease = Arc::new(CachedKeyLease {
            key_hash: key_hash.clone(),
            record,
            expires_at,
            cleanup_tx: self.cleanup_tx.clone(),
        });
        self.index.insert(key_hash, Arc::downgrade(&lease));
        let seq = self.next_seq.fetch_add(1, AtomicOrdering::Relaxed);
        self.queue.lock().push(ExpiringLease {
            expires_at,
            seq,
            _lease: lease.clone(),
        });
        lease
    }

    pub(crate) fn invalidate(&self, key_hash: &str) {
        self.index.remove(key_hash);
    }
}

/// One renewable cache lease tracked by weak references and expiry timers.
pub(crate) struct CachedKeyLease {
    key_hash: String,
    pub record: LlmGatewayKeyRecord,
    expires_at: Instant,
    cleanup_tx: mpsc::UnboundedSender<String>,
}

impl Drop for CachedKeyLease {
    fn drop(&mut self) {
        let _ = self.cleanup_tx.send(self.key_hash.clone());
    }
}

/// Heap entry used by the background cleaner to retire expired cache leases.
struct ExpiringLease {
    expires_at: Instant,
    seq: u64,
    _lease: Arc<CachedKeyLease>,
}

impl PartialEq for ExpiringLease {
    fn eq(&self, other: &Self) -> bool {
        self.expires_at == other.expires_at && self.seq == other.seq
    }
}

impl Eq for ExpiringLease {}

impl PartialOrd for ExpiringLease {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ExpiringLease {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .expires_at
            .cmp(&self.expires_at)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}

/// Read the live auth-cache TTL from the shared gateway runtime state.
pub(crate) async fn gateway_auth_cache_ttl(gateway: &LlmGatewayRuntimeState) -> u64 {
    gateway.runtime_config.read().await.auth_cache_ttl_seconds
}

/// Build a reqwest bearer Authorization header value.
pub(crate) fn bearer_header(token: &str) -> Result<ReqwestHeaderValue> {
    ReqwestHeaderValue::from_str(&format!("Bearer {token}"))
        .context("failed to build bearer header")
}

#[cfg(test)]
mod tests {
    use static_flow_shared::llm_gateway_store::{
        LLM_GATEWAY_KEY_STATUS_ACTIVE, LLM_GATEWAY_PROTOCOL_OPENAI, LLM_GATEWAY_PROVIDER_CODEX,
    };

    use super::*;

    fn sample_key() -> LlmGatewayKeyRecord {
        LlmGatewayKeyRecord {
            id: "key-1".to_string(),
            name: "test-key".to_string(),
            secret: "sfk_test".to_string(),
            key_hash: "hash".to_string(),
            status: LLM_GATEWAY_KEY_STATUS_ACTIVE.to_string(),
            provider_type: LLM_GATEWAY_PROVIDER_CODEX.to_string(),
            protocol_family: LLM_GATEWAY_PROTOCOL_OPENAI.to_string(),
            public_visible: false,
            quota_billable_limit: 1_000_000,
            usage_input_uncached_tokens: 0,
            usage_input_cached_tokens: 0,
            usage_output_tokens: 0,
            usage_billable_tokens: 0,
            usage_credit_total: 0.0,
            usage_credit_missing_events: 0,
            last_used_at: None,
            created_at: 0,
            updated_at: 0,
            route_strategy: None,
            fixed_account_name: None,
            auto_account_names: None,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
        }
    }

    #[test]
    fn codex_request_scheduler_allows_unlimited_keys_without_tracking() {
        let scheduler = Arc::new(LlmGatewayKeyRequestScheduler::new());
        let lease = scheduler
            .try_acquire(&sample_key())
            .expect("unlimited keys should acquire immediately");
        assert_eq!(lease.key_id, "key-1");
        assert!(lease.scheduler.is_none());
    }

    #[test]
    fn codex_request_scheduler_enforces_concurrency_and_start_interval() {
        let scheduler = Arc::new(LlmGatewayKeyRequestScheduler::new());
        let mut key = sample_key();
        key.request_max_concurrency = Some(1);
        key.request_min_start_interval_ms = Some(250);

        let first_lease = scheduler
            .try_acquire(&key)
            .expect("first request should acquire");

        let concurrency_rejection = scheduler
            .try_acquire(&key)
            .expect_err("second in-flight request should be rejected");
        assert_eq!(concurrency_rejection.reason, "local_concurrency_limit");
        assert_eq!(concurrency_rejection.in_flight, 1);
        assert_eq!(concurrency_rejection.max_concurrency, Some(1));
        assert_eq!(concurrency_rejection.min_start_interval_ms, Some(250));

        drop(first_lease);

        let pacing_rejection = scheduler
            .try_acquire(&key)
            .expect_err("request restart should honor min start interval");
        assert_eq!(pacing_rejection.reason, "local_start_interval");
        assert_eq!(pacing_rejection.in_flight, 0);
        assert_eq!(pacing_rejection.max_concurrency, Some(1));
        assert_eq!(pacing_rejection.min_start_interval_ms, Some(250));
        assert!(pacing_rejection.wait.is_some());
        assert!(pacing_rejection.elapsed_since_last_start_ms.is_some());
    }
}
