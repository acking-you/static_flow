//! Provider routing contracts: authenticated key, provider proxy config,
//! Codex/Kiro route + auth-update views, and JWT/auth-error helpers.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::Value;

use super::kiro_account::{AdminKiroBalanceView, AdminKiroCacheView};
use crate::provider::RouteStrategy;

/// Key state used on the hot request path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedKey {
    /// Key id.
    pub key_id: String,
    /// Key display name.
    pub key_name: String,
    /// Provider type as snake_case string.
    pub provider_type: String,
    /// Protocol family as snake_case string.
    pub protocol_family: String,
    /// Key status.
    pub status: String,
    /// Billable quota limit.
    pub quota_billable_limit: i64,
    /// Billable usage already consumed.
    pub billable_tokens_used: i64,
}

/// Resolved proxy settings for one upstream provider request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderProxyConfig {
    /// Proxy URL accepted by `reqwest::Proxy`.
    pub proxy_url: String,
    /// Optional proxy username.
    pub proxy_username: Option<String>,
    /// Optional proxy password.
    pub proxy_password: Option<String>,
}

/// Resolved Codex account selected for one provider request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCodexRoute {
    /// Selected account name.
    pub account_name: String,
    /// Account group id from the key route config at resolution time.
    pub account_group_id_at_event: Option<String>,
    /// Effective route strategy from the key route config at resolution time.
    pub route_strategy_at_event: RouteStrategy,
    /// Persisted auth JSON for the selected account.
    pub auth_json: String,
    /// Whether this account maps public gpt-5.3-codex to Spark upstream.
    pub map_gpt53_codex_to_spark: bool,
    /// Whether this account may participate in automatic auth refresh.
    pub auth_refresh_enabled: bool,
    /// Whether Codex fast/priority requests are allowed for this key.
    pub codex_fast_enabled: bool,
    /// Request concurrency cap configured on this key route.
    pub request_max_concurrency: Option<u64>,
    /// Minimum interval between request starts configured on this key route.
    pub request_min_start_interval_ms: Option<u64>,
    /// Request concurrency cap configured on the selected account.
    pub account_request_max_concurrency: Option<u64>,
    /// Minimum interval between request starts configured on the selected
    /// account.
    pub account_request_min_start_interval_ms: Option<u64>,
    /// Cached account error message captured from the latest status view.
    pub cached_error_message: Option<String>,
    /// Resolved proxy settings for this upstream request.
    pub proxy: Option<ProviderProxyConfig>,
}

/// Refreshed Codex account credential fields persisted by the provider runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCodexAuthUpdate {
    /// Account name.
    pub account_name: String,
    /// Refreshed auth payload JSON.
    pub auth_json: String,
    /// Upstream account id when known.
    pub account_id: Option<String>,
    /// Runtime status after refresh.
    pub status: String,
    /// Last refresh or runtime error.
    pub last_error: Option<String>,
    /// Refresh timestamp.
    pub refreshed_at_ms: i64,
}

/// Whether a Codex auth error is terminal and should immediately block
/// request routing for that account until credentials are refreshed.
pub fn is_terminal_codex_auth_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("refresh_token_invalidated")
        || message.contains("refresh token has been invalidated")
        || message.contains("refresh_token_reused")
        || message.contains("refresh token has already been used")
        || ((message.contains("codex refresh token returned 401")
            || message.contains("codex refresh token returned 403"))
            && message.contains("invalid_request_error"))
        || ((message.contains("codex request returned 401")
            || message.contains("codex request returned 403"))
            && message.contains("after forced refresh"))
        || message.contains("codex auth refresh disabled and current access token expired")
}

/// Decode the JWT `exp` claim from one access token into Unix milliseconds.
pub fn jwt_expiry_unix_ms(token: &str) -> Option<i64> {
    let payload = token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    let value: Value = serde_json::from_slice(&decoded).ok()?;
    let exp_seconds = value.get("exp")?.as_i64()?;
    exp_seconds.checked_mul(1000)
}

/// Extract the current Codex access token expiry timestamp from persisted auth
/// JSON when it is available.
pub fn codex_auth_access_token_expires_at_ms(auth_json: &str) -> Option<i64> {
    let value: Value = serde_json::from_str(auth_json).ok()?;
    let access_token = json_string_any(&value, &["access_token", "accessToken"]).or_else(|| {
        value
            .get("tokens")
            .and_then(|tokens| json_string_any(tokens, &["access_token", "accessToken"]))
    })?;
    codex_access_token_expires_at_ms(Some(access_token.as_str()))
}

/// Decode the JWT expiry from a Codex access token string.
pub fn codex_access_token_expires_at_ms(access_token: Option<&str>) -> Option<i64> {
    let access_token = access_token
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    jwt_expiry_unix_ms(access_token)
}

fn json_string_any(value: &Value, fields: &[&str]) -> Option<String> {
    fields
        .iter()
        .find_map(|field| value.get(*field).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

/// Resolved Kiro account selected for one provider request.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderKiroRoute {
    /// Selected account name.
    pub account_name: String,
    /// Account group id from the key route config at resolution time.
    pub account_group_id_at_event: Option<String>,
    /// Effective route strategy from the key route config at resolution time.
    pub route_strategy_at_event: RouteStrategy,
    /// Persisted auth JSON for the selected account.
    pub auth_json: String,
    /// Profile ARN used by Kiro upstream requests.
    pub profile_arn: Option<String>,
    /// Effective API region used by Kiro upstream requests.
    pub api_region: String,
    /// Whether public request validation is enabled for this key.
    pub request_validation_enabled: bool,
    /// Whether cache estimation is enabled for this key.
    pub cache_estimation_enabled: bool,
    /// Whether zero-cache successes should retain diagnostic request bodies.
    pub zero_cache_debug_enabled: bool,
    /// Whether all successful Kiro requests should retain full request bodies.
    pub full_request_logging_enabled: bool,
    /// Whether public URL image/document sources may be fetched server-side.
    pub remote_media_resolution_enabled: bool,
    /// Whether recent Kiro latency metrics may influence route ordering.
    pub latency_routing_enabled: bool,
    /// JSON object mapping public model names to upstream Kiro model names.
    pub model_name_map_json: String,
    /// Effective Kiro cache k-model JSON for this key.
    pub cache_kmodels_json: String,
    /// Effective Kiro cache policy JSON for this key.
    pub cache_policy_json: String,
    /// Minimum request-side input tokens before trusting Kiro contextUsage.
    pub context_usage_min_request_tokens: u64,
    /// Proactive auto-compaction trigger in counted input tokens; `0` disables.
    pub compact_trigger_tokens: u64,
    /// Prefix-cache simulation mode.
    pub prefix_cache_mode: String,
    /// Prefix-cache maximum token budget.
    pub prefix_cache_max_tokens: u64,
    /// Prefix-cache entry TTL in seconds.
    pub prefix_cache_entry_ttl_seconds: u64,
    /// Conversation-anchor maximum entries.
    pub conversation_anchor_max_entries: u64,
    /// Conversation-anchor TTL in seconds.
    pub conversation_anchor_ttl_seconds: u64,
    /// Effective Kiro billable multiplier JSON for this key.
    pub billable_model_multipliers_json: String,
    /// Request concurrency cap configured on this key route.
    pub request_max_concurrency: Option<u64>,
    /// Minimum interval between request starts configured on this key route.
    pub request_min_start_interval_ms: Option<u64>,
    /// Request concurrency cap configured on the selected account.
    pub account_request_max_concurrency: Option<u64>,
    /// Minimum interval between request starts configured on the selected
    /// account.
    pub account_request_min_start_interval_ms: Option<u64>,
    /// Resolved proxy settings for this upstream request.
    pub proxy: Option<ProviderProxyConfig>,
    /// Runtime routing identity used for local throttles and cooldowns. This is
    /// the upstream user id when known, otherwise the account name.
    pub routing_identity: String,
    /// Last cached status label for diagnostics and route ordering.
    pub cached_status: Option<String>,
    /// Last cached remaining credits for fairness ordering.
    pub cached_remaining_credits: Option<f64>,
    /// Last cached balance payload for request-time status refreshes.
    pub cached_balance: Option<AdminKiroBalanceView>,
    /// Last cached status metadata for request-time status refreshes.
    pub cached_cache: Option<AdminKiroCacheView>,
    /// Status-refresh interval used when this route updates the cache.
    pub status_refresh_interval_seconds: u64,
    /// Cached-credit floor used before locally blocking this account.
    pub minimum_remaining_credits_before_block: f64,
}

/// Refreshed Kiro account credential fields persisted by the provider runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderKiroAuthUpdate {
    /// Account name.
    pub account_name: String,
    /// Refreshed auth payload JSON.
    pub auth_json: String,
    /// Kiro auth method.
    pub auth_method: String,
    /// Upstream account id when known.
    pub account_id: Option<String>,
    /// Kiro profile ARN when known.
    pub profile_arn: Option<String>,
    /// Upstream user id when known.
    pub user_id: Option<String>,
    /// Runtime status after refresh.
    pub status: String,
    /// Last refresh or runtime error.
    pub last_error: Option<String>,
    /// Refresh timestamp.
    pub refreshed_at_ms: i64,
}

impl AuthenticatedKey {
    /// Remaining billable token budget available to this key.
    pub fn remaining_billable(&self) -> i64 {
        self.quota_billable_limit
            .saturating_sub(self.billable_tokens_used)
    }
}
