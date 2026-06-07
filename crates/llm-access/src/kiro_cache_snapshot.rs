//! Cross-node Kiro cache snapshot persistence over Valkey.
//!
//! The Kiro cache simulator lives entirely in process memory, so a restart
//! cold-starts every prefix-cache prediction and proactive-compaction estimate.
//! This module periodically serializes that state into a gzip-framed blob (see
//! `llm_access_kiro::cache_sim::snapshot`) and stores it under a per-node
//! Valkey key, then restores it before serving traffic so a restart re-warms in
//! seconds.
//!
//! Every Valkey interaction is best-effort: any error is logged and skipped,
//! never blocking startup or the request path.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context;
use llm_access_core::store::{AdminConfigStore, AdminRuntimeConfig};
use llm_access_kiro::cache_sim::{
    KiroCacheSimulator, KiroSnapshotImportOutcome, SnapshotCaps, MAX_COMPRESSED_SNAPSHOT_BYTES,
};
use llm_access_store::request_cache::RequestCacheConfig;
use redis::AsyncCommands;
use tokio::{sync::watch, task::JoinHandle, time};

use crate::admin::kiro_cache_simulation_config_from_admin_config;

/// Node-id placeholder used on single-machine deployments without a cluster
/// identity, so the per-node key namespace stays stable.
const SINGLE_NODE_ID: &str = "_single";
/// Maximum number of peer snapshot keys considered during restore. Bounds both
/// the SCAN result set and the worst-case aggregate fetch when the shared
/// namespace accumulates many keys or is polluted with junk.
const MAX_PEER_SNAPSHOTS: usize = 32;
/// Maximum aggregate bytes pulled across all peer snapshots in one restore.
/// A second guard (on top of the per-key size cap) bounding transient memory.
const MAX_TOTAL_PEER_SNAPSHOT_BYTES: usize = 256 * 1024 * 1024;

/// Valkey-backed store for cross-node Kiro cache snapshots.
#[derive(Clone)]
pub(crate) struct KiroCacheSnapshotStore {
    client: redis::Client,
    key_prefix: String,
    node_id: String,
}

impl KiroCacheSnapshotStore {
    /// Open a snapshot store from the shared request-cache config, reusing its
    /// Valkey URL and key prefix.
    pub(crate) fn new(
        config: &RequestCacheConfig,
        node_id: Option<String>,
    ) -> anyhow::Result<Self> {
        let client = redis::Client::open(config.url.clone())
            .with_context(|| format!("open kiro cache snapshot redis client `{}`", config.url))?;
        Ok(Self {
            client,
            key_prefix: config.key_prefix.clone(),
            node_id: node_id.unwrap_or_else(|| SINGLE_NODE_ID.to_string()),
        })
    }

    fn node_key(&self, node_id: &str) -> String {
        format!("{}:kiro:cachesnap:node:{node_id}", self.key_prefix)
    }

    fn own_key(&self) -> String {
        self.node_key(&self.node_id)
    }

    fn scan_pattern(&self) -> String {
        format!("{}:kiro:cachesnap:node:*", self.key_prefix)
    }

    async fn connection(&self) -> anyhow::Result<redis::aio::MultiplexedConnection> {
        self.client
            .get_multiplexed_async_connection()
            .await
            .context("connect kiro cache snapshot redis")
    }

    /// Load this node's own snapshot blob, if present. The key is external
    /// Valkey state, so size-check with `STRLEN` first and skip an empty or
    /// oversized value before pulling the bulk string into memory — otherwise a
    /// single stale/corrupt own key bypasses the startup memory bound.
    async fn load_own(&self) -> anyhow::Result<Option<Vec<u8>>> {
        let mut conn = self.connection().await?;
        let key = self.own_key();
        let len: usize = conn
            .strlen(&key)
            .await
            .with_context(|| format!("redis STRLEN `{key}`"))?;
        if len == 0 || len > MAX_COMPRESSED_SNAPSHOT_BYTES {
            return Ok(None);
        }
        let value: Option<Vec<u8>> = conn
            .get(&key)
            .await
            .with_context(|| format!("redis GET `{key}`"))?;
        Ok(value)
    }

    /// Enumerate peer snapshot blobs via SCAN, excluding this node's own key.
    /// Uses a cursor loop (never `KEYS`) and bounds memory: at most
    /// `MAX_PEER_SNAPSHOTS` keys are considered, each is size-checked with
    /// `STRLEN` and skipped if it exceeds `MAX_COMPRESSED_SNAPSHOT_BYTES`, and
    /// the aggregate fetched bytes are capped at
    /// `MAX_TOTAL_PEER_SNAPSHOT_BYTES`. This keeps a corrupt/oversized key
    /// or a polluted namespace from blowing up startup memory before the
    /// decoder's own size guard runs.
    async fn load_peers(&self) -> anyhow::Result<Vec<Vec<u8>>> {
        let mut conn = self.connection().await?;
        let own_key = self.own_key();
        let pattern = self.scan_pattern();
        let mut cursor: u64 = 0;
        let mut keys: Vec<String> = Vec::new();
        'scan: loop {
            let (next, batch): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(256)
                .query_async(&mut conn)
                .await
                .with_context(|| format!("redis SCAN `{pattern}`"))?;
            for key in batch {
                if key == own_key {
                    continue;
                }
                keys.push(key);
                if keys.len() >= MAX_PEER_SNAPSHOTS {
                    break 'scan;
                }
            }
            cursor = next;
            if cursor == 0 {
                break;
            }
        }

        // Fetch per key with a STRLEN pre-check and a running byte budget, so an
        // oversized or junk value is skipped before it is pulled into memory.
        let mut blobs = Vec::with_capacity(keys.len());
        let mut total_bytes: usize = 0;
        for key in keys {
            let len: usize = conn
                .strlen(&key)
                .await
                .with_context(|| format!("redis STRLEN `{key}`"))?;
            if len == 0 || len > MAX_COMPRESSED_SNAPSHOT_BYTES {
                continue;
            }
            if total_bytes.saturating_add(len) > MAX_TOTAL_PEER_SNAPSHOT_BYTES {
                break;
            }
            let value: Option<Vec<u8>> = conn
                .get(&key)
                .await
                .with_context(|| format!("redis GET `{key}`"))?;
            if let Some(blob) = value {
                total_bytes = total_bytes.saturating_add(blob.len());
                blobs.push(blob);
            }
        }
        Ok(blobs)
    }

    /// Store this node's snapshot blob with a TTL. Redis strings are binary
    /// safe, so the gzip blob is written without base64.
    async fn store(&self, blob: &[u8], ttl: Duration) -> anyhow::Result<()> {
        let mut conn = self.connection().await?;
        let key = self.own_key();
        redis::cmd("SET")
            .arg(&key)
            .arg(blob)
            .arg("EX")
            .arg(ttl.as_secs().max(1))
            .query_async::<()>(&mut conn)
            .await
            .with_context(|| format!("redis SET `{key}`"))?;
        Ok(())
    }
}

/// Map an admin runtime config into snapshot size caps. `0` means "follow the
/// live budget", which the codec represents as `None`.
fn snapshot_caps_from_admin_config(config: &AdminRuntimeConfig) -> SnapshotCaps {
    SnapshotCaps {
        max_tokens: (config.kiro_cache_snapshot_max_tokens > 0)
            .then_some(config.kiro_cache_snapshot_max_tokens),
        max_anchor_entries: (config.kiro_cache_snapshot_max_anchor_entries > 0)
            .then_some(config.kiro_cache_snapshot_max_anchor_entries as usize),
    }
}

/// Restore the simulator from this node's snapshot plus peer snapshots.
/// Best-effort: any Valkey error leaves the simulator empty and is logged.
pub(crate) async fn restore_simulator(
    store: &KiroCacheSnapshotStore,
    simulator: &KiroCacheSimulator,
    config: &AdminRuntimeConfig,
) -> KiroSnapshotImportOutcome {
    let own = match store.load_own().await {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%error, "failed to load own kiro cache snapshot");
            None
        },
    };
    let peers = match store.load_peers().await {
        Ok(values) => values,
        Err(error) => {
            tracing::warn!(%error, "failed to load peer kiro cache snapshots");
            Vec::new()
        },
    };
    let sim_config = kiro_cache_simulation_config_from_admin_config(config);
    let caps = snapshot_caps_from_admin_config(config);
    simulator.import_snapshot(own.as_deref(), &peers, sim_config, caps, Instant::now())
}

/// Handle to the periodic snapshot task, draining a final flush on shutdown.
pub(crate) struct KiroCacheSnapshotHandle {
    shutdown_tx: watch::Sender<bool>,
    join: JoinHandle<()>,
}

impl KiroCacheSnapshotHandle {
    /// Signal shutdown and await the final flush.
    pub(crate) async fn shutdown(self) {
        let _ = self.shutdown_tx.send(true);
        let _ = self.join.await;
    }
}

/// Spawn the periodic flush task. It re-reads the runtime config every tick so
/// an admin toggling the feature on/off takes effect without a restart, and
/// flushes one final snapshot on shutdown.
pub(crate) fn spawn(
    store: KiroCacheSnapshotStore,
    simulator: Arc<KiroCacheSimulator>,
    admin_config_store: Arc<dyn AdminConfigStore>,
) -> KiroCacheSnapshotHandle {
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let join = tokio::spawn(async move {
        // Retain the last successfully fetched config so a transient store
        // error does not silently disable snapshotting by reverting to the
        // (disabled-by-default) `AdminRuntimeConfig::default()`.
        let mut last_config = AdminRuntimeConfig::default();
        loop {
            let config = match admin_config_store.get_admin_runtime_config().await {
                Ok(config) => {
                    last_config = config.clone();
                    config
                },
                Err(error) => {
                    tracing::warn!(
                        %error,
                        "failed to read runtime config for kiro cache snapshot; using last known config"
                    );
                    last_config.clone()
                },
            };
            let interval = Duration::from_secs(config.kiro_cache_snapshot_interval_seconds.max(1));
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        flush_snapshot(&store, &simulator, &config).await;
                        tracing::info!("kiro cache snapshot task shutting down");
                        return;
                    }
                }
                _ = time::sleep(interval) => {
                    flush_snapshot(&store, &simulator, &config).await;
                }
            }
        }
    });
    KiroCacheSnapshotHandle {
        shutdown_tx,
        join,
    }
}

/// Export and store one snapshot if the feature is enabled. All errors are
/// logged and swallowed.
async fn flush_snapshot(
    store: &KiroCacheSnapshotStore,
    simulator: &KiroCacheSimulator,
    config: &AdminRuntimeConfig,
) {
    if !config.kiro_cache_snapshot_enabled {
        return;
    }
    let sim_config = kiro_cache_simulation_config_from_admin_config(config);
    let caps = snapshot_caps_from_admin_config(config);
    let Some(blob) = simulator.export_snapshot(sim_config, caps, Instant::now()) else {
        return;
    };
    let ttl = Duration::from_secs(config.kiro_cache_snapshot_ttl_seconds.max(1));
    if let Err(error) = store.store(&blob, ttl).await {
        tracing::warn!(%error, "failed to store kiro cache snapshot");
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use llm_access_kiro::{
        cache_sim::{
            KiroCacheSimulationConfig, KiroCacheSimulationMode, KiroCacheSimulator,
            PromptProjection, SnapshotCaps,
        },
        wire::{
            AssistantMessage, ConversationState, CurrentMessage, HistoryAssistantMessage,
            HistoryUserMessage, Message, UserInputMessage,
        },
    };
    use llm_access_store::request_cache::RequestCacheConfig;

    use super::KiroCacheSnapshotStore;

    const VALKEY_ENV: &str = "LLM_ACCESS_TEST_VALKEY_URL";

    fn test_config() -> KiroCacheSimulationConfig {
        KiroCacheSimulationConfig {
            mode: KiroCacheSimulationMode::PrefixTree,
            prefix_cache_max_tokens: 100_000,
            prefix_cache_entry_ttl: Duration::from_secs(300),
            conversation_anchor_max_entries: 32,
            conversation_anchor_ttl: Duration::from_secs(300),
        }
    }

    fn warm_projection() -> PromptProjection {
        let state = ConversationState::new("conv-1")
            .with_history(vec![
                Message::User(HistoryUserMessage::new("existing history", "ignored-model")),
                Message::Assistant(HistoryAssistantMessage::new("done")),
            ])
            .with_current_message(CurrentMessage::new(UserInputMessage::new(
                "continue",
                "ignored-model",
            )));
        PromptProjection::from_conversation_state(&state)
    }

    #[tokio::test]
    #[ignore = "requires a local Valkey/Redis reachable via LLM_ACCESS_TEST_VALKEY_URL"]
    async fn snapshot_round_trips_through_valkey_with_peer_union() {
        let Ok(url) = std::env::var(VALKEY_ENV) else {
            eprintln!("skipping: {VALKEY_ENV} is not set");
            return;
        };
        let key_prefix = format!("sftest:{}", uuid::Uuid::new_v4());
        let cache_config = RequestCacheConfig {
            url,
            key_prefix: key_prefix.clone(),
        };
        let config = test_config();
        let caps = SnapshotCaps::default();
        let now = Instant::now();
        let projection = warm_projection();
        let assistant = AssistantMessage::new("assistant reply");

        // Node A warms a simulator, exports, and stores its snapshot.
        let store_a =
            KiroCacheSnapshotStore::new(&cache_config, Some("node-a".to_string())).expect("store");
        let warm = KiroCacheSimulator::default();
        warm.record_success(&projection, &assistant, "conv-a", true, config, now);
        let blob_a = warm.export_snapshot(config, caps, now).expect("export");
        store_a
            .store(&blob_a, Duration::from_secs(300))
            .await
            .expect("store a");

        // Node B stores a peer snapshot under a different key.
        let store_b =
            KiroCacheSnapshotStore::new(&cache_config, Some("node-b".to_string())).expect("store");
        let warm_b = KiroCacheSimulator::default();
        warm_b.record_success(&projection, &assistant, "conv-b", true, config, now);
        let blob_b = warm_b.export_snapshot(config, caps, now).expect("export b");
        store_b
            .store(&blob_b, Duration::from_secs(300))
            .await
            .expect("store b");

        // Node A restarts: own snapshot seeds the prefix tree, peer anchors join.
        let restored = KiroCacheSimulator::default();
        let own = store_a.load_own().await.expect("load own");
        let peers = store_a.load_peers().await.expect("load peers");
        assert!(own.is_some());
        assert_eq!(peers.len(), 1, "node-a should see exactly node-b as a peer");
        let outcome =
            restored.import_snapshot(own.as_deref(), &peers, config, caps, Instant::now());
        assert!(outcome.prefix_from_own);
        assert!(outcome.prefix_resident_tokens > 0);
        assert!(outcome.anchor_entries >= 1);

        let matched = restored.match_prefix(&projection, config, Instant::now());
        assert_eq!(matched.matched_pages, projection.stable_prefix_pages.len());

        // Clean up both test keys.
        let client = redis::Client::open(cache_config.url.clone()).expect("client");
        let mut conn = client
            .get_multiplexed_async_connection()
            .await
            .expect("conn");
        let _: () = redis::cmd("DEL")
            .arg(format!("{key_prefix}:kiro:cachesnap:node:node-a"))
            .arg(format!("{key_prefix}:kiro:cachesnap:node:node-b"))
            .query_async(&mut conn)
            .await
            .expect("del");
    }
}
