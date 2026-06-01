//! Cluster node identity, shared metadata, and runtime role helpers.

use std::{sync::Arc, time::Duration};

use anyhow::{anyhow, Context};
use redis::AsyncCommands;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::{sync::RwLock, task::JoinHandle};

const NODE_ID_ENV: &str = "LLM_ACCESS_NODE_ID";
const NODE_CLASS_ENV: &str = "LLM_ACCESS_NODE_CLASS";
const NODE_DISPLAY_NAME_ENV: &str = "LLM_ACCESS_NODE_DISPLAY_NAME";
const NODE_REGION_ENV: &str = "LLM_ACCESS_NODE_REGION";
const NODE_API_BASE_URL_ENV: &str = "LLM_ACCESS_NODE_API_BASE_URL";
const NODE_WORKER_BASE_URL_ENV: &str = "LLM_ACCESS_NODE_WORKER_BASE_URL";

const CLUSTER_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const CLUSTER_PRIMARY_TTL: Duration = Duration::from_secs(60);
const CLUSTER_NODE_TTL: Duration = Duration::from_secs(120);

/// Static node class configured at deploy time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeClass {
    /// Primary node with JuiceFS mounted.
    Core,
    /// API-only edge node without JuiceFS.
    Edge,
}

/// Dynamic runtime role resolved after startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeRuntimeRole {
    /// Current cluster primary.
    Primary,
    /// Secondary edge node that proxies usage traffic to the primary.
    EdgeSecondary,
    /// Node is alive but the primary is unavailable or not yet published.
    Degraded,
}

/// How this node serves usage queries right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageQueryMode {
    /// This node is the primary and serves local usage data.
    LocalPrimary,
    /// This node proxies usage data to the primary.
    ProxiedPrimary,
    /// This node currently has no reachable primary view.
    PrimaryUnavailable,
}

/// Stable node identity configured at deploy time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeIdentity {
    /// Stable node id chosen by deployment config.
    pub node_id: String,
    /// Static node class.
    pub node_class: NodeClass,
    /// Optional human-readable label for admin surfaces.
    pub display_name: Option<String>,
    /// Optional region string for admin surfaces.
    pub region: Option<String>,
    /// Optional public API base URL.
    pub api_base_url: Option<String>,
    /// Optional worker base URL used for proxy/relay targets.
    pub worker_base_url: Option<String>,
}

/// Shared primary metadata published in Valkey.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterPrimarySnapshot {
    /// Node id of the current primary.
    pub node_id: String,
    /// Optional public API base URL.
    pub api_base_url: Option<String>,
    /// Optional worker base URL used by edge workers.
    pub worker_base_url: Option<String>,
    /// Publish timestamp.
    pub published_at_ms: i64,
}

/// Shared node heartbeat metadata published in Valkey.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterNodeSnapshot {
    /// Stable node id.
    pub node_id: String,
    /// Static node class.
    pub node_class: NodeClass,
    /// Current runtime role.
    pub runtime_role: NodeRuntimeRole,
    /// Optional display name.
    pub display_name: Option<String>,
    /// Optional region string.
    pub region: Option<String>,
    /// Optional public API base URL.
    pub api_base_url: Option<String>,
    /// Optional worker base URL.
    pub worker_base_url: Option<String>,
    /// Current primary node id when known.
    pub primary_node_id: Option<String>,
    /// Current usage query mode for this node.
    pub usage_query_mode: UsageQueryMode,
    /// Heartbeat timestamp.
    pub last_heartbeat_at_ms: i64,
}

/// In-process snapshot used by API handlers and worker logic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterRuntimeSnapshot {
    /// Configured node identity.
    pub node: NodeIdentity,
    /// Current runtime role.
    pub runtime_role: NodeRuntimeRole,
    /// Current primary when known.
    pub primary: Option<ClusterPrimarySnapshot>,
    /// Current usage query mode.
    pub usage_query_mode: UsageQueryMode,
    /// Last cluster heartbeat timestamp.
    pub last_heartbeat_at_ms: i64,
}

/// Shared cluster runtime state for one process.
pub struct ClusterRuntimeState {
    cache: ClusterCache,
    snapshot: Arc<RwLock<ClusterRuntimeSnapshot>>,
    _heartbeat: JoinHandle<()>,
}

impl ClusterRuntimeState {
    /// Build the optional cluster runtime from storage config.
    pub async fn from_storage_config(
        config: &crate::config::StorageConfig,
    ) -> anyhow::Result<Option<Arc<Self>>> {
        let Some(node_identity) = config.node_identity.clone() else {
            return Ok(None);
        };
        let Some(cache_config) = crate::config::resolve_request_cache_config(config)? else {
            anyhow::bail!(
                "cluster node identity requires request cache configuration for shared discovery"
            );
        };
        Self::start(node_identity, cache_config).await.map(Some)
    }

    /// Start cluster discovery and heartbeat publishing.
    pub async fn start(
        node_identity: NodeIdentity,
        cache_config: llm_access_store::request_cache::RequestCacheConfig,
    ) -> anyhow::Result<Arc<Self>> {
        let cache = ClusterCache::new(cache_config)?;
        let initial = resolve_runtime_snapshot(&node_identity, cache.load_primary().await?);
        let snapshot = Arc::new(RwLock::new(initial));
        let snapshot_for_task = Arc::clone(&snapshot);
        let cache_for_task = cache.clone();
        let identity_for_task = node_identity.clone();
        let heartbeat = tokio::spawn(async move {
            let mut interval = tokio::time::interval(CLUSTER_HEARTBEAT_INTERVAL);
            loop {
                interval.tick().await;
                let primary = match identity_for_task.node_class {
                    NodeClass::Core => {
                        Some(primary_snapshot_from_identity(&identity_for_task, now_ms()))
                    },
                    NodeClass::Edge => match cache_for_task.load_primary().await {
                        Ok(primary) => primary,
                        Err(err) => {
                            tracing::warn!("failed to refresh primary cluster snapshot: {err:#}");
                            None
                        },
                    },
                };
                let next = resolve_runtime_snapshot(&identity_for_task, primary);
                if let Err(err) = cache_for_task.publish_node(&next).await {
                    tracing::warn!("failed to publish cluster node heartbeat: {err:#}");
                }
                if next.runtime_role == NodeRuntimeRole::Primary {
                    if let Err(err) = cache_for_task.publish_primary(&next).await {
                        tracing::warn!("failed to publish primary cluster snapshot: {err:#}");
                    }
                }
                *snapshot_for_task.write().await = next;
            }
        });
        let state = Arc::new(Self {
            cache,
            snapshot,
            _heartbeat: heartbeat,
        });
        let current = state.snapshot().await;
        state.cache.publish_node(&current).await?;
        if current.runtime_role == NodeRuntimeRole::Primary {
            state.cache.publish_primary(&current).await?;
        }
        Ok(state)
    }

    /// Return the latest in-memory cluster snapshot.
    pub async fn snapshot(&self) -> ClusterRuntimeSnapshot {
        self.snapshot.read().await.clone()
    }

    /// Return the current runtime role.
    pub async fn runtime_role(&self) -> NodeRuntimeRole {
        self.snapshot.read().await.runtime_role
    }

    /// Return the current primary worker base URL when known.
    pub async fn primary_worker_base_url(&self) -> Option<String> {
        self.snapshot
            .read()
            .await
            .primary
            .as_ref()
            .and_then(|primary| primary.worker_base_url.clone())
    }

    /// Return the current primary node id when known.
    pub async fn primary_node_id(&self) -> Option<String> {
        self.snapshot
            .read()
            .await
            .primary
            .as_ref()
            .map(|primary| primary.node_id.clone())
    }
}

#[derive(Clone)]
struct ClusterCache {
    client: redis::Client,
    key_prefix: String,
}

impl ClusterCache {
    fn new(config: llm_access_store::request_cache::RequestCacheConfig) -> anyhow::Result<Self> {
        let client = redis::Client::open(config.url.clone())
            .with_context(|| format!("open cluster redis client `{}`", config.url))?;
        Ok(Self {
            client,
            key_prefix: config.key_prefix,
        })
    }

    async fn load_primary(&self) -> anyhow::Result<Option<ClusterPrimarySnapshot>> {
        self.get_json(&self.primary_key()).await
    }

    async fn publish_primary(&self, snapshot: &ClusterRuntimeSnapshot) -> anyhow::Result<()> {
        let primary = snapshot
            .primary
            .clone()
            .context("cluster runtime snapshot is missing primary view")?;
        self.set_json(&self.primary_key(), &primary, CLUSTER_PRIMARY_TTL)
            .await
    }

    async fn publish_node(&self, snapshot: &ClusterRuntimeSnapshot) -> anyhow::Result<()> {
        let node = ClusterNodeSnapshot {
            node_id: snapshot.node.node_id.clone(),
            node_class: snapshot.node.node_class,
            runtime_role: snapshot.runtime_role,
            display_name: snapshot.node.display_name.clone(),
            region: snapshot.node.region.clone(),
            api_base_url: snapshot.node.api_base_url.clone(),
            worker_base_url: snapshot.node.worker_base_url.clone(),
            primary_node_id: snapshot
                .primary
                .as_ref()
                .map(|primary| primary.node_id.clone()),
            usage_query_mode: snapshot.usage_query_mode,
            last_heartbeat_at_ms: snapshot.last_heartbeat_at_ms,
        };
        self.set_json(&self.node_key(&snapshot.node.node_id), &node, CLUSTER_NODE_TTL)
            .await
    }

    fn primary_key(&self) -> String {
        format!("{}:cluster:primary", self.key_prefix)
    }

    fn node_key(&self, node_id: &str) -> String {
        format!("{}:cluster:node:{node_id}", self.key_prefix)
    }

    async fn get_json<T>(&self, key: &str) -> anyhow::Result<Option<T>>
    where
        T: DeserializeOwned,
    {
        let mut conn = self.connection().await?;
        let value: Option<String> = conn
            .get(key)
            .await
            .with_context(|| format!("redis GET `{key}`"))?;
        value
            .map(|json| serde_json::from_str(&json).context("decode cluster cache json"))
            .transpose()
    }

    async fn set_json<T>(&self, key: &str, value: &T, ttl: Duration) -> anyhow::Result<()>
    where
        T: Serialize,
    {
        let payload = serde_json::to_string(value).context("encode cluster cache json")?;
        let mut conn = self.connection().await?;
        redis::cmd("SET")
            .arg(key)
            .arg(payload)
            .arg("EX")
            .arg(ttl.as_secs().max(1))
            .query_async::<()>(&mut conn)
            .await
            .with_context(|| format!("redis SET `{key}`"))?;
        Ok(())
    }

    async fn connection(&self) -> anyhow::Result<redis::aio::MultiplexedConnection> {
        self.client
            .get_multiplexed_async_connection()
            .await
            .context("connect cluster redis")
    }
}

/// Parse a deploy-time node class.
pub fn parse_node_class(raw: &str) -> anyhow::Result<NodeClass> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "core" => Ok(NodeClass::Core),
        "edge" => Ok(NodeClass::Edge),
        _ => Err(anyhow!("unsupported node class `{raw}`")),
    }
}

/// Load the optional node identity from environment variables.
pub fn load_node_identity_from_env() -> anyhow::Result<Option<NodeIdentity>> {
    let node_id = std::env::var(NODE_ID_ENV).ok().map(trimmed_non_empty);
    let node_class = std::env::var(NODE_CLASS_ENV).ok().map(trimmed_non_empty);
    match (node_id, node_class) {
        (None, None) => Ok(None),
        (Some(None) | None, Some(_)) | (Some(_), Some(None) | None) => Err(anyhow!(
            "`{NODE_ID_ENV}` and `{NODE_CLASS_ENV}` must both be set when cluster identity is \
             configured"
        )),
        (Some(Some(node_id)), Some(Some(node_class_raw))) => Ok(Some(NodeIdentity {
            node_id,
            node_class: parse_node_class(&node_class_raw)
                .with_context(|| format!("failed to parse `{NODE_CLASS_ENV}`"))?,
            display_name: optional_env(NODE_DISPLAY_NAME_ENV),
            region: optional_env(NODE_REGION_ENV),
            api_base_url: optional_env(NODE_API_BASE_URL_ENV),
            worker_base_url: optional_env(NODE_WORKER_BASE_URL_ENV),
        })),
    }
}

fn primary_snapshot_from_identity(
    node_identity: &NodeIdentity,
    published_at_ms: i64,
) -> ClusterPrimarySnapshot {
    ClusterPrimarySnapshot {
        node_id: node_identity.node_id.clone(),
        api_base_url: node_identity.api_base_url.clone(),
        worker_base_url: node_identity.worker_base_url.clone(),
        published_at_ms,
    }
}

fn resolve_runtime_snapshot(
    node_identity: &NodeIdentity,
    primary: Option<ClusterPrimarySnapshot>,
) -> ClusterRuntimeSnapshot {
    let last_heartbeat_at_ms = now_ms();
    match node_identity.node_class {
        NodeClass::Core => ClusterRuntimeSnapshot {
            node: node_identity.clone(),
            runtime_role: NodeRuntimeRole::Primary,
            primary: Some(primary_snapshot_from_identity(node_identity, last_heartbeat_at_ms)),
            usage_query_mode: UsageQueryMode::LocalPrimary,
            last_heartbeat_at_ms,
        },
        NodeClass::Edge => {
            let usage_query_mode = if primary.is_some() {
                UsageQueryMode::ProxiedPrimary
            } else {
                UsageQueryMode::PrimaryUnavailable
            };
            ClusterRuntimeSnapshot {
                node: node_identity.clone(),
                runtime_role: if primary.is_some() {
                    NodeRuntimeRole::EdgeSecondary
                } else {
                    NodeRuntimeRole::Degraded
                },
                primary,
                usage_query_mode,
                last_heartbeat_at_ms,
            }
        },
    }
}

fn optional_env(name: &str) -> Option<String> {
    std::env::var(name).ok().and_then(trimmed_non_empty)
}

fn trimmed_non_empty(raw: String) -> Option<String> {
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn node_class_parsing_rejects_unknown_values() {
        assert!(super::parse_node_class("core").is_ok());
        assert!(super::parse_node_class("edge").is_ok());
        assert!(super::parse_node_class("weird").is_err());
    }

    #[test]
    fn load_node_identity_returns_none_when_cluster_env_is_absent() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        clear_cluster_env();
        assert_eq!(super::load_node_identity_from_env().expect("load"), None);
    }

    #[test]
    fn load_node_identity_requires_complete_cluster_env() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        clear_cluster_env();
        std::env::set_var(super::NODE_ID_ENV, "node-a");
        assert!(super::load_node_identity_from_env().is_err());
        clear_cluster_env();
        std::env::set_var(super::NODE_CLASS_ENV, "edge");
        assert!(super::load_node_identity_from_env().is_err());
        clear_cluster_env();
    }

    #[test]
    fn load_node_identity_parses_optional_metadata() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        clear_cluster_env();
        std::env::set_var(super::NODE_ID_ENV, " node-a ");
        std::env::set_var(super::NODE_CLASS_ENV, " core ");
        std::env::set_var(super::NODE_DISPLAY_NAME_ENV, " api-a ");
        std::env::set_var(super::NODE_REGION_ENV, " hk ");
        std::env::set_var(super::NODE_API_BASE_URL_ENV, " https://a.example.com ");
        std::env::set_var(super::NODE_WORKER_BASE_URL_ENV, " http://127.0.0.1:19081 ");
        let identity = super::load_node_identity_from_env()
            .expect("load")
            .expect("identity");
        assert_eq!(identity.node_id, "node-a");
        assert_eq!(identity.node_class, super::NodeClass::Core);
        assert_eq!(identity.display_name.as_deref(), Some("api-a"));
        assert_eq!(identity.region.as_deref(), Some("hk"));
        assert_eq!(identity.api_base_url.as_deref(), Some("https://a.example.com"));
        assert_eq!(identity.worker_base_url.as_deref(), Some("http://127.0.0.1:19081"));
        clear_cluster_env();
    }

    #[test]
    fn edge_snapshot_becomes_degraded_without_primary() {
        let snapshot = super::resolve_runtime_snapshot(
            &super::NodeIdentity {
                node_id: "edge-a".to_string(),
                node_class: super::NodeClass::Edge,
                display_name: None,
                region: None,
                api_base_url: None,
                worker_base_url: None,
            },
            None,
        );
        assert_eq!(snapshot.runtime_role, super::NodeRuntimeRole::Degraded);
        assert_eq!(snapshot.usage_query_mode, super::UsageQueryMode::PrimaryUnavailable);
    }

    #[test]
    fn edge_snapshot_uses_proxied_mode_when_primary_exists() {
        let snapshot = super::resolve_runtime_snapshot(
            &super::NodeIdentity {
                node_id: "edge-a".to_string(),
                node_class: super::NodeClass::Edge,
                display_name: None,
                region: None,
                api_base_url: None,
                worker_base_url: None,
            },
            Some(super::ClusterPrimarySnapshot {
                node_id: "core-a".to_string(),
                api_base_url: Some("https://api.example.com".to_string()),
                worker_base_url: Some("http://10.0.0.1:19081".to_string()),
                published_at_ms: 1,
            }),
        );
        assert_eq!(snapshot.runtime_role, super::NodeRuntimeRole::EdgeSecondary);
        assert_eq!(snapshot.usage_query_mode, super::UsageQueryMode::ProxiedPrimary);
    }

    fn clear_cluster_env() {
        for key in [
            super::NODE_ID_ENV,
            super::NODE_CLASS_ENV,
            super::NODE_DISPLAY_NAME_ENV,
            super::NODE_REGION_ENV,
            super::NODE_API_BASE_URL_ENV,
            super::NODE_WORKER_BASE_URL_ENV,
        ] {
            std::env::remove_var(key);
        }
    }
}
