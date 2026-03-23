use std::{
    cmp::Ordering,
    collections::BinaryHeap,
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
use reqwest::{header::HeaderValue as ReqwestHeaderValue, Proxy};
use serde::Deserialize;
use static_flow_shared::llm_gateway_store::{LlmGatewayKeyRecord, LlmGatewayStore};
use tokio::{
    sync::{mpsc, Mutex as AsyncMutex},
    time::MissedTickBehavior,
};

use super::types::LlmGatewayRateLimitStatusResponse;
use crate::state::LlmGatewayRuntimeConfig;

const CLEANER_TICK_SECONDS: u64 = 1;
const DEFAULT_UPSTREAM_PROXY_URL: &str = "http://127.0.0.1:11111";

/// Long-lived runtime state shared by all gateway handlers.
#[derive(Clone)]
pub struct LlmGatewayRuntimeState {
    pub(crate) store: Arc<LlmGatewayStore>,
    pub(crate) runtime_config: Arc<tokio::sync::RwLock<LlmGatewayRuntimeConfig>>,
    pub(crate) auth_source: Arc<CodexAuthSource>,
    pub(crate) key_cache: Arc<LlmGatewayKeyCache>,
    pub(crate) rate_limit_status: Arc<tokio::sync::RwLock<LlmGatewayRateLimitStatusResponse>>,
    pub(crate) client: reqwest::Client,
    pub(crate) usage_write_lock: Arc<AsyncMutex<()>>,
}

impl LlmGatewayRuntimeState {
    /// Construct the shared runtime state used by all LLM gateway requests.
    pub fn new(
        store: Arc<LlmGatewayStore>,
        runtime_config: Arc<tokio::sync::RwLock<LlmGatewayRuntimeConfig>>,
    ) -> Result<Self> {
        let client = build_llm_gateway_upstream_client()?;
        Ok(Self {
            store,
            runtime_config,
            auth_source: Arc::new(CodexAuthSource::new()),
            key_cache: Arc::new(LlmGatewayKeyCache::new()),
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
            client,
            usage_write_lock: Arc::new(AsyncMutex::new(())),
        })
    }
}

/// In-memory snapshot of the upstream Codex login state.
#[derive(Debug, Clone)]
pub(crate) struct CodexAuthSnapshot {
    pub access_token: String,
    pub account_id: Option<String>,
    modified_at: Option<SystemTime>,
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

/// Build the shared reqwest client used for all upstream Codex traffic.
pub(crate) fn build_llm_gateway_upstream_client() -> Result<reqwest::Client> {
    let proxy_url = env::var("STATICFLOW_LLM_GATEWAY_UPSTREAM_PROXY_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_UPSTREAM_PROXY_URL.to_string());
    let proxy =
        Proxy::all(proxy_url.as_str()).context("failed to build llm gateway upstream proxy")?;
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .pool_max_idle_per_host(32)
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(30))
        .proxy(proxy)
        .build()
        .context("failed to build llm gateway reqwest client")
}

/// Build a reqwest bearer Authorization header value.
pub(crate) fn bearer_header(token: &str) -> Result<ReqwestHeaderValue> {
    ReqwestHeaderValue::from_str(&format!("Bearer {token}"))
        .context("failed to build bearer header")
}
