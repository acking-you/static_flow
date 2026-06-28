//! Admin/data-plane types for the Kiro-owned direct Anthropic upstream pool.

use serde::{Deserialize, Serialize};

use super::ProviderProxyConfig;
use crate::provider::RouteStrategy;

/// Pool is disabled for the key.
pub const ANTHROPIC_UPSTREAM_POOL_MODE_DISABLED: &str = "disabled";
/// Try direct Anthropic pool first, then Kiro when direct upstream has no route
/// or returns a retryable upstream failure.
pub const ANTHROPIC_UPSTREAM_POOL_MODE_PREFERRED_BEFORE_KIRO: &str = "preferred_before_kiro";
/// Only use the direct Anthropic pool for this key.
pub const ANTHROPIC_UPSTREAM_POOL_MODE_ONLY: &str = "only";

/// Default direct Anthropic API base URL.
pub const DEFAULT_ANTHROPIC_UPSTREAM_BASE_URL: &str = "https://api.anthropic.com/v1";
/// Default manual scheduler weight for one direct Anthropic channel.
pub const DEFAULT_ANTHROPIC_UPSTREAM_WEIGHT: u64 = 100;
/// Default direct Anthropic channel concurrency.
pub const DEFAULT_ANTHROPIC_UPSTREAM_MAX_CONCURRENCY: u64 = 3;
/// Default direct Anthropic channel pacing interval.
pub const DEFAULT_ANTHROPIC_UPSTREAM_MIN_START_INTERVAL_MS: u64 = 0;

/// Return the normalized direct Anthropic pool mode, if supported.
pub fn normalize_anthropic_upstream_pool_mode(raw: &str) -> Option<&'static str> {
    match raw.trim() {
        ANTHROPIC_UPSTREAM_POOL_MODE_DISABLED => Some(ANTHROPIC_UPSTREAM_POOL_MODE_DISABLED),
        ANTHROPIC_UPSTREAM_POOL_MODE_PREFERRED_BEFORE_KIRO => {
            Some(ANTHROPIC_UPSTREAM_POOL_MODE_PREFERRED_BEFORE_KIRO)
        },
        ANTHROPIC_UPSTREAM_POOL_MODE_ONLY => Some(ANTHROPIC_UPSTREAM_POOL_MODE_ONLY),
        _ => None,
    }
}

/// Default direct Anthropic pool mode for existing keys.
pub fn default_anthropic_upstream_pool_mode() -> String {
    ANTHROPIC_UPSTREAM_POOL_MODE_DISABLED.to_string()
}

/// Canonicalize a possibly missing direct Anthropic pool mode.
pub fn canonical_anthropic_upstream_pool_mode(raw: Option<&str>) -> String {
    raw.and_then(normalize_anthropic_upstream_pool_mode)
        .unwrap_or(ANTHROPIC_UPSTREAM_POOL_MODE_DISABLED)
        .to_string()
}

/// Admin-facing token rollup for one direct Anthropic upstream channel.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminAnthropicUpstreamUsageRollup {
    /// Accumulated uncached input tokens.
    pub input_uncached_tokens: u64,
    /// Accumulated cached input tokens.
    pub input_cached_tokens: u64,
    /// Accumulated output tokens.
    pub output_tokens: u64,
    /// Accumulated billable tokens.
    pub billable_tokens: u64,
    /// Successful/error events where upstream usage was absent.
    pub usage_missing_events: u64,
    /// Last recorded usage timestamp.
    pub last_used_at: Option<i64>,
}

/// Admin-facing direct Anthropic channel card.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminAnthropicUpstreamChannel {
    /// Stable channel name.
    pub name: String,
    /// Human-facing status.
    pub status: String,
    /// Anthropic-compatible base URL, usually `https://api.anthropic.com/v1`.
    pub base_url: String,
    /// Whether an API key is configured without exposing the secret.
    pub has_api_key: bool,
    /// Manual scheduler weight.
    pub weight: u64,
    /// Channel-level concurrency cap.
    pub max_concurrency: u64,
    /// Channel-level pacing interval.
    pub min_start_interval_ms: u64,
    /// `inherit`, `direct`, or `fixed`.
    pub proxy_mode: String,
    /// Fixed proxy config id when `proxy_mode = fixed`.
    pub proxy_config_id: Option<String>,
    /// Last hot-path error, if any.
    pub last_error: Option<String>,
    /// Token rollup for this channel.
    pub usage: AdminAnthropicUpstreamUsageRollup,
    /// Creation timestamp.
    pub created_at: i64,
    /// Update timestamp.
    pub updated_at: i64,
}

/// One page of direct Anthropic channels.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminAnthropicUpstreamChannelsPage {
    /// Page rows.
    pub channels: Vec<AdminAnthropicUpstreamChannel>,
    /// Total rows before pagination.
    pub total: usize,
    /// Page limit.
    pub limit: usize,
    /// Page offset.
    pub offset: usize,
    /// Whether another page is available.
    pub has_more: bool,
}

/// New direct Anthropic channel after admin request normalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAdminAnthropicUpstreamChannel {
    /// Stable channel name.
    pub name: String,
    /// Channel status.
    pub status: String,
    /// Anthropic-compatible base URL.
    pub base_url: String,
    /// API key for `x-api-key`.
    pub api_key: String,
    /// Manual scheduler weight.
    pub weight: u64,
    /// Channel-level concurrency cap.
    pub max_concurrency: u64,
    /// Channel-level pacing interval.
    pub min_start_interval_ms: u64,
    /// `inherit`, `direct`, or `fixed`.
    pub proxy_mode: String,
    /// Fixed proxy config id when `proxy_mode = fixed`.
    pub proxy_config_id: Option<String>,
    /// Creation timestamp.
    pub created_at_ms: i64,
}

/// Direct Anthropic channel patch after admin request normalization.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdminAnthropicUpstreamChannelPatch {
    /// New channel status.
    pub status: Option<String>,
    /// New base URL.
    pub base_url: Option<String>,
    /// Replace API key. `None` means leave as-is.
    pub api_key: Option<String>,
    /// New manual scheduler weight.
    pub weight: Option<u64>,
    /// New channel-level concurrency cap.
    pub max_concurrency: Option<u64>,
    /// New channel-level pacing interval.
    pub min_start_interval_ms: Option<u64>,
    /// New proxy mode.
    pub proxy_mode: Option<String>,
    /// New fixed proxy config id.
    pub proxy_config_id: Option<Option<String>>,
    /// Explicitly clear last_error.
    pub clear_last_error: bool,
    /// Update timestamp.
    pub updated_at_ms: i64,
}

/// Hot-path usage increment for one direct Anthropic channel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AnthropicUpstreamChannelUsageDelta {
    /// Uncached input tokens.
    pub input_uncached_tokens: u64,
    /// Cached input tokens.
    pub input_cached_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Billable tokens.
    pub billable_tokens: u64,
    /// Whether upstream usage was missing.
    pub usage_missing: bool,
    /// Usage timestamp.
    pub used_at_ms: i64,
}

/// Resolved direct Anthropic channel selected for one provider request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderAnthropicUpstreamRoute {
    /// Selected channel name.
    pub channel_name: String,
    /// Pool mode captured from the key route config.
    pub pool_mode_at_event: String,
    /// Account group id from the key route config at resolution time.
    pub account_group_id_at_event: Option<String>,
    /// Effective route strategy from the Kiro key route config at resolution
    /// time.
    pub route_strategy_at_event: RouteStrategy,
    /// Anthropic-compatible base URL.
    pub base_url: String,
    /// API key sent as `x-api-key`.
    pub api_key: String,
    /// Manual scheduler weight.
    pub weight: u64,
    /// Key-level concurrency cap.
    pub request_max_concurrency: Option<u64>,
    /// Key-level pacing interval.
    pub request_min_start_interval_ms: Option<u64>,
    /// Channel-level concurrency cap.
    pub channel_max_concurrency: u64,
    /// Channel-level pacing interval.
    pub channel_min_start_interval_ms: u64,
    /// JSON object mapping public model names to upstream Anthropic model
    /// names.
    pub model_name_map_json: String,
    /// Effective Kiro billable multiplier JSON reused for key quota math.
    pub billable_model_multipliers_json: String,
    /// Resolved proxy settings for this upstream request.
    pub proxy: Option<ProviderProxyConfig>,
}

/// Per-request direct Anthropic routing plan for a Kiro key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderAnthropicUpstreamResolution {
    /// Canonical key-level pool mode.
    pub pool_mode: String,
    /// Eligible direct upstream candidates for this request.
    pub routes: Vec<ProviderAnthropicUpstreamRoute>,
}

impl ProviderAnthropicUpstreamResolution {
    /// Build the default disabled routing plan.
    pub fn disabled() -> Self {
        Self {
            pool_mode: default_anthropic_upstream_pool_mode(),
            routes: Vec::new(),
        }
    }
}
