//! Proxy configuration: proxy config view, endpoint health checks, provider
//! bindings, create/patch payloads, and the default per-provider bindings.

use serde::{Deserialize, Serialize};

use super::{PROVIDER_CODEX, PROVIDER_KIRO};

/// Admin-facing projection of one reusable upstream proxy config.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminProxyConfig {
    /// Proxy config id.
    pub id: String,
    /// Human-readable proxy name.
    pub name: String,
    /// Proxy URL.
    pub proxy_url: String,
    /// Optional proxy username.
    pub proxy_username: Option<String>,
    /// Optional proxy password.
    pub proxy_password: Option<String>,
    /// Config status.
    pub status: String,
    /// Creation timestamp.
    pub created_at: i64,
    /// Update timestamp.
    pub updated_at: i64,
    /// Node id used to resolve the effective proxy value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_node_id: Option<String>,
    /// Source used to produce the effective proxy fields.
    #[serde(default)]
    pub effective_source: String,
    /// Whether this row has a node-local override.
    #[serde(default)]
    pub has_node_override: bool,
    /// Whether this caller may edit slot metadata such as name/create/delete.
    #[serde(default)]
    pub can_edit_slot_metadata: bool,
    /// Latest Codex endpoint check observed from this node scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_codex_check: Option<AdminProxyEndpointCheck>,
    /// Latest Kiro endpoint check observed from this node scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_kiro_check: Option<AdminProxyEndpointCheck>,
}

/// Latest connectivity probe result for one proxy/provider endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminProxyEndpointCheck {
    /// Probed upstream URL.
    pub target_url: String,
    /// Whether the proxy reached the target at transport level.
    pub reachable: bool,
    /// HTTP status observed from the target, when available.
    pub status_code: Option<u16>,
    /// Measured elapsed time in milliseconds.
    pub latency_ms: i64,
    /// Short error or non-success response summary.
    pub error_message: Option<String>,
    /// Probe timestamp.
    pub checked_at: i64,
}

/// Probe result to persist for one proxy/provider endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminProxyEndpointCheckUpdate {
    /// Proxy config id.
    pub proxy_config_id: String,
    /// Provider type.
    pub provider_type: String,
    /// Probed upstream URL.
    pub target_url: String,
    /// Whether the proxy reached the target at transport level.
    pub reachable: bool,
    /// HTTP status observed from the target, when available.
    pub status_code: Option<u16>,
    /// Measured elapsed time in milliseconds.
    pub latency_ms: i64,
    /// Short error or non-success response summary.
    pub error_message: Option<String>,
    /// Probe timestamp.
    pub checked_at_ms: i64,
}

/// New reusable proxy config row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAdminProxyConfig {
    /// Proxy config id.
    pub id: String,
    /// Human-readable proxy name.
    pub name: String,
    /// Proxy URL.
    pub proxy_url: String,
    /// Optional proxy username.
    pub proxy_username: Option<String>,
    /// Optional proxy password.
    pub proxy_password: Option<String>,
    /// Creation timestamp.
    pub created_at_ms: i64,
}

/// Patch for one reusable proxy config.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdminProxyConfigPatch {
    /// New proxy name.
    pub name: Option<String>,
    /// New proxy URL.
    pub proxy_url: Option<String>,
    /// New optional proxy username.
    pub proxy_username: Option<Option<String>>,
    /// New optional proxy password.
    pub proxy_password: Option<Option<String>>,
    /// New status.
    pub status: Option<String>,
    /// Update timestamp.
    pub updated_at_ms: i64,
}

/// Effective provider-level proxy binding shown in admin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminProxyBinding {
    /// Provider type.
    pub provider_type: String,
    /// Source used to resolve the effective proxy.
    pub effective_source: String,
    /// Explicitly bound proxy config id.
    pub bound_proxy_config_id: Option<String>,
    /// Effective proxy config name.
    pub effective_proxy_config_name: Option<String>,
    /// Effective proxy URL.
    pub effective_proxy_url: Option<String>,
    /// Effective proxy username.
    pub effective_proxy_username: Option<String>,
    /// Effective proxy password.
    pub effective_proxy_password: Option<String>,
    /// Binding update timestamp.
    pub binding_updated_at: Option<i64>,
    /// Error message for invalid bindings.
    pub error_message: Option<String>,
}

/// Return the default unbound proxy binding views for supported providers.
pub fn default_proxy_bindings() -> Vec<AdminProxyBinding> {
    [PROVIDER_CODEX, PROVIDER_KIRO]
        .into_iter()
        .map(default_proxy_binding)
        .collect()
}

pub fn default_proxy_binding(provider_type: &str) -> AdminProxyBinding {
    AdminProxyBinding {
        provider_type: provider_type.to_string(),
        effective_source: "none".to_string(),
        bound_proxy_config_id: None,
        effective_proxy_config_name: None,
        effective_proxy_url: None,
        effective_proxy_username: None,
        effective_proxy_password: None,
        binding_updated_at: None,
        error_message: None,
    }
}
