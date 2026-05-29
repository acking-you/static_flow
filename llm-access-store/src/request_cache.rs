use std::time::Duration;

use anyhow::Context;
use llm_access_core::store::{
    AdminKiroBalanceView, AdminKiroCacheView, AdminProxyBinding, AdminProxyConfig,
    CodexRateLimitStatus, ProviderProxyConfig, DEFAULT_KIRO_CONTEXT_USAGE_MIN_REQUEST_TOKENS,
};
use redis::{AsyncCommands, Commands};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::records::RuntimeConfigRecord;

const AUTH_CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const RUNTIME_CONFIG_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const REQUEST_SNAPSHOT_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const ACCOUNT_VIEW_TTL: Duration = Duration::from_secs(4 * 60 * 60);
const ACCOUNT_AUTH_TTL: Duration = Duration::from_secs(4 * 60 * 60);
const CODEX_STATUS_TTL: Duration = Duration::from_secs(4 * 60 * 60);
const PROXY_METADATA_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const USAGE_PROXY_ATTRIBUTION_TTL: Duration = Duration::from_secs(30 * 60);
const USAGE_CATALOG_LOOKUP_TTL: Duration = Duration::from_secs(15 * 60);
const USAGE_CATALOG_EVENT_LOCATOR_TTL: Duration = Duration::from_secs(30 * 60);
const NEGATIVE_AUTH_TTL: Duration = Duration::from_secs(5 * 60);

const fn default_true() -> bool {
    true
}

const fn default_kiro_context_usage_min_request_tokens() -> u64 {
    DEFAULT_KIRO_CONTEXT_USAGE_MIN_REQUEST_TOKENS
}

/// Shared Valkey configuration for the request-path cache layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestCacheConfig {
    /// Redis or Valkey connection URL.
    pub url: String,
    /// Stable namespace prefix used for all cache entries of one deployment.
    pub key_prefix: String,
}

#[derive(Debug, Clone)]
pub(crate) struct RequestCache {
    client: redis::Client,
    key_prefix: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CachedAuthenticatedKey {
    pub key_id: String,
    pub key_name: String,
    pub provider_type: String,
    pub protocol_family: String,
    pub status: String,
    pub quota_billable_limit: i64,
    pub billable_tokens_used: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CachedAuthLookup {
    pub key: Option<CachedAuthenticatedKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CachedRuntimeConfigLookup {
    pub record: Option<RuntimeConfigRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct CachedCodexStatusLookup {
    pub snapshot: Option<CodexRateLimitStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CachedProxyConfigsLookup {
    #[serde(default)]
    pub generation: i64,
    pub configs: Vec<AdminProxyConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CachedProxyBindingLookup {
    #[serde(default)]
    pub generation: i64,
    pub binding: AdminProxyBinding,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CachedUsageProxyAttributionView {
    pub provider_type: String,
    pub account_name: String,
    pub proxy_source: String,
    pub proxy_config_id: Option<String>,
    pub proxy_config_name: Option<String>,
    pub proxy_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CachedUsageProxyAttributionLookup {
    #[serde(default)]
    pub generation: i64,
    pub attribution: Option<CachedUsageProxyAttributionView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CachedCodexRequestSnapshot {
    pub key: CachedAuthenticatedKey,
    pub generation: i64,
    pub route_strategy: String,
    pub account_group_id_at_event: Option<String>,
    pub selected_account_names: Vec<String>,
    pub use_all_active_accounts: bool,
    pub request_max_concurrency: Option<u64>,
    pub request_min_start_interval_ms: Option<u64>,
    #[serde(default = "default_true")]
    pub codex_fast_enabled: bool,
    pub codex_weight_free: i64,
    pub codex_weight_plus: i64,
    pub codex_weight_pro5x: i64,
    pub codex_weight_pro20x: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CachedKiroRequestSnapshot {
    pub key: CachedAuthenticatedKey,
    pub generation: i64,
    pub route_strategy: String,
    pub account_group_id_at_event: Option<String>,
    pub selected_account_names: Vec<String>,
    pub use_all_active_accounts: bool,
    pub request_max_concurrency: Option<u64>,
    pub request_min_start_interval_ms: Option<u64>,
    pub request_validation_enabled: bool,
    pub cache_estimation_enabled: bool,
    pub zero_cache_debug_enabled: bool,
    pub full_request_logging_enabled: bool,
    #[serde(default)]
    pub remote_media_resolution_enabled: bool,
    #[serde(default = "default_true")]
    pub latency_routing_enabled: bool,
    pub model_name_map_json: String,
    pub cache_kmodels_json: String,
    pub cache_policy_json: String,
    #[serde(default = "default_kiro_context_usage_min_request_tokens")]
    pub context_usage_min_request_tokens: u64,
    pub prefix_cache_mode: String,
    pub prefix_cache_max_tokens: u64,
    pub prefix_cache_entry_ttl_seconds: u64,
    pub conversation_anchor_max_entries: u64,
    pub conversation_anchor_ttl_seconds: u64,
    pub billable_model_multipliers_json: String,
    pub status_refresh_interval_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CachedProxyConfig {
    pub proxy_url: String,
    pub proxy_username: Option<String>,
    pub proxy_password: Option<String>,
}

impl From<ProviderProxyConfig> for CachedProxyConfig {
    fn from(value: ProviderProxyConfig) -> Self {
        Self {
            proxy_url: value.proxy_url,
            proxy_username: value.proxy_username,
            proxy_password: value.proxy_password,
        }
    }
}

impl From<CachedProxyConfig> for ProviderProxyConfig {
    fn from(value: CachedProxyConfig) -> Self {
        Self {
            proxy_url: value.proxy_url,
            proxy_username: value.proxy_username,
            proxy_password: value.proxy_password,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CachedCodexAccountView {
    #[serde(default)]
    pub generation: i64,
    pub account_name: String,
    pub status: String,
    pub map_gpt53_codex_to_spark: bool,
    pub auth_refresh_enabled: bool,
    pub route_weight_tier: Option<String>,
    pub request_max_concurrency: Option<u64>,
    pub request_min_start_interval_ms: Option<u64>,
    pub last_refresh_at_ms: Option<i64>,
    pub last_error: Option<String>,
    pub access_token: Option<String>,
    pub proxy: Option<CachedProxyConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct CachedKiroAccountView {
    #[serde(default)]
    pub generation: i64,
    pub account_name: String,
    pub profile_arn: Option<String>,
    pub user_id: Option<String>,
    pub status: String,
    pub request_max_concurrency: Option<u64>,
    pub request_min_start_interval_ms: Option<u64>,
    pub disabled: bool,
    pub minimum_remaining_credits_before_block: f64,
    pub api_region: String,
    pub proxy: Option<CachedProxyConfig>,
    pub routing_identity: String,
    pub cached_balance: Option<AdminKiroBalanceView>,
    pub cached_cache: Option<AdminKiroCacheView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CachedAccountAuth {
    pub auth_json: String,
}

impl RequestCache {
    const USAGE_CATALOG_CACHE_NAMESPACE: &str = "usage:catalog:v2";

    pub(crate) fn new(config: RequestCacheConfig) -> anyhow::Result<Self> {
        let client = redis::Client::open(config.url.clone())
            .with_context(|| format!("open request cache redis client `{}`", config.url))?;
        Ok(Self {
            client,
            key_prefix: config.key_prefix,
        })
    }

    pub(crate) fn auth_key(&self, secret_hash: &str) -> String {
        format!("{}:auth:{secret_hash}", self.key_prefix)
    }

    pub(crate) fn request_snapshot_key(&self, provider: &str, key_id: &str) -> String {
        format!("{}:req:{provider}:{key_id}", self.key_prefix)
    }

    pub(crate) fn runtime_config_key(&self) -> String {
        format!("{}:runtime:config", self.key_prefix)
    }

    pub(crate) fn codex_status_key(&self) -> String {
        format!("{}:status:codex", self.key_prefix)
    }

    pub(crate) fn proxy_configs_key(&self, scope: &str) -> String {
        format!("{}:proxy:scope:{scope}:configs", self.key_prefix)
    }

    pub(crate) fn proxy_binding_key(&self, provider: &str, scope: &str) -> String {
        format!("{}:proxy:scope:{scope}:binding:{provider}", self.key_prefix)
    }

    pub(crate) fn account_view_key(
        &self,
        provider: &str,
        account_name: &str,
        scope: &str,
    ) -> String {
        format!("{}:acct:view:scope:{scope}:{provider}:{account_name}", self.key_prefix)
    }

    pub(crate) fn account_auth_key(&self, provider: &str, account_name: &str) -> String {
        format!("{}:acct:auth:{provider}:{account_name}", self.key_prefix)
    }

    pub(crate) fn dispatch_generation_key(&self, provider: &str) -> String {
        format!("{}:gen:dispatch:{provider}", self.key_prefix)
    }

    pub(crate) fn usage_proxy_attribution_key(
        &self,
        provider: &str,
        account_name: &str,
        scope: &str,
    ) -> String {
        format!("{}:usage:proxy:scope:{scope}:{provider}:{account_name}", self.key_prefix)
    }

    pub(crate) fn usage_catalog_generation_key(&self) -> String {
        format!("{}:{}:gen", self.key_prefix, Self::USAGE_CATALOG_CACHE_NAMESPACE)
    }

    pub(crate) fn usage_catalog_rollups_key(&self) -> String {
        format!("{}:{}:rollups", self.key_prefix, Self::USAGE_CATALOG_CACHE_NAMESPACE)
    }

    pub(crate) fn usage_catalog_filtered_segments_key(&self, query_fingerprint: &str) -> String {
        format!(
            "{}:{}:segments:filtered:{query_fingerprint}",
            self.key_prefix,
            Self::USAGE_CATALOG_CACHE_NAMESPACE
        )
    }

    pub(crate) fn usage_catalog_event_locator_key(&self, event_id: &str) -> String {
        format!("{}:{}:event:{event_id}", self.key_prefix, Self::USAGE_CATALOG_CACHE_NAMESPACE)
    }

    pub(crate) fn usage_catalog_filter_options_key(&self, query_fingerprint: &str) -> String {
        format!(
            "{}:{}:filter-options:{query_fingerprint}",
            self.key_prefix,
            Self::USAGE_CATALOG_CACHE_NAMESPACE
        )
    }

    pub(crate) fn auth_ttl(&self, secret_hash: &str) -> Duration {
        deterministic_jitter_ttl(&self.auth_key(secret_hash), AUTH_CACHE_TTL, 0.8, 1.2)
    }

    pub(crate) fn negative_auth_ttl(&self, secret_hash: &str) -> Duration {
        deterministic_jitter_ttl(&self.auth_key(secret_hash), NEGATIVE_AUTH_TTL, 0.6, 1.4)
    }

    pub(crate) fn runtime_config_ttl(&self) -> Duration {
        deterministic_jitter_ttl(&self.runtime_config_key(), RUNTIME_CONFIG_TTL, 0.8, 1.2)
    }

    pub(crate) fn request_snapshot_ttl(&self, provider: &str, key_id: &str) -> Duration {
        deterministic_jitter_ttl(
            &self.request_snapshot_key(provider, key_id),
            REQUEST_SNAPSHOT_TTL,
            0.8,
            1.2,
        )
    }

    pub(crate) fn codex_status_ttl(&self) -> Duration {
        deterministic_jitter_ttl(&self.codex_status_key(), CODEX_STATUS_TTL, 0.75, 1.25)
    }

    pub(crate) fn proxy_configs_ttl(&self, scope: &str) -> Duration {
        deterministic_jitter_ttl(&self.proxy_configs_key(scope), PROXY_METADATA_TTL, 0.8, 1.2)
    }

    pub(crate) fn proxy_binding_ttl(&self, provider: &str, scope: &str) -> Duration {
        deterministic_jitter_ttl(
            &self.proxy_binding_key(provider, scope),
            PROXY_METADATA_TTL,
            0.8,
            1.2,
        )
    }

    pub(crate) fn account_view_ttl(
        &self,
        provider: &str,
        account_name: &str,
        scope: &str,
    ) -> Duration {
        deterministic_jitter_ttl(
            &self.account_view_key(provider, account_name, scope),
            ACCOUNT_VIEW_TTL,
            0.75,
            1.25,
        )
    }

    pub(crate) fn account_auth_ttl(&self, provider: &str, account_name: &str) -> Duration {
        deterministic_jitter_ttl(
            &self.account_auth_key(provider, account_name),
            ACCOUNT_AUTH_TTL,
            0.75,
            1.25,
        )
    }

    pub(crate) fn usage_catalog_rollups_ttl(&self) -> Duration {
        deterministic_jitter_ttl(
            &self.usage_catalog_rollups_key(),
            USAGE_CATALOG_LOOKUP_TTL,
            0.8,
            1.2,
        )
    }

    pub(crate) fn usage_proxy_attribution_ttl(
        &self,
        provider: &str,
        account_name: &str,
        scope: &str,
    ) -> Duration {
        deterministic_jitter_ttl(
            &self.usage_proxy_attribution_key(provider, account_name, scope),
            USAGE_PROXY_ATTRIBUTION_TTL,
            0.8,
            1.2,
        )
    }

    pub(crate) fn usage_catalog_filtered_segments_ttl(&self, query_fingerprint: &str) -> Duration {
        deterministic_jitter_ttl(
            &self.usage_catalog_filtered_segments_key(query_fingerprint),
            USAGE_CATALOG_LOOKUP_TTL,
            0.8,
            1.2,
        )
    }

    pub(crate) fn usage_catalog_event_locator_ttl(&self, event_id: &str) -> Duration {
        deterministic_jitter_ttl(
            &self.usage_catalog_event_locator_key(event_id),
            USAGE_CATALOG_EVENT_LOCATOR_TTL,
            0.8,
            1.2,
        )
    }

    pub(crate) fn usage_catalog_filter_options_ttl(&self, query_fingerprint: &str) -> Duration {
        deterministic_jitter_ttl(
            &self.usage_catalog_filter_options_key(query_fingerprint),
            USAGE_CATALOG_LOOKUP_TTL,
            0.8,
            1.2,
        )
    }

    pub(crate) async fn get_json<T>(&self, key: &str) -> anyhow::Result<Option<T>>
    where
        T: DeserializeOwned,
    {
        let mut conn = self.connection().await?;
        let value: Option<String> = conn
            .get(key)
            .await
            .with_context(|| format!("redis GET `{key}`"))?;
        value
            .map(|json| serde_json::from_str(&json).context("decode request cache json"))
            .transpose()
    }

    pub(crate) async fn mget_json<T>(&self, keys: &[String]) -> anyhow::Result<Vec<Option<T>>>
    where
        T: DeserializeOwned,
    {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let mut conn = self.connection().await?;
        let raw: Vec<Option<String>> = redis::cmd("MGET")
            .arg(keys)
            .query_async(&mut conn)
            .await
            .context("redis MGET request cache json")?;
        raw.into_iter()
            .map(|value| {
                value
                    .map(|json| serde_json::from_str(&json).context("decode request cache json"))
                    .transpose()
            })
            .collect()
    }

    pub(crate) async fn set_json<T>(
        &self,
        key: &str,
        value: &T,
        ttl: Duration,
    ) -> anyhow::Result<()>
    where
        T: Serialize,
    {
        let payload = serde_json::to_string(value).context("encode request cache json")?;
        let ttl_seconds = duration_to_redis_secs(ttl);
        let mut conn = self.connection().await?;
        redis::cmd("SET")
            .arg(key)
            .arg(payload)
            .arg("EX")
            .arg(ttl_seconds)
            .query_async::<()>(&mut conn)
            .await
            .with_context(|| format!("redis SET `{key}`"))?;
        Ok(())
    }

    pub(crate) async fn delete(&self, key: &str) -> anyhow::Result<()> {
        let mut conn = self.connection().await?;
        let _: usize = conn
            .del(key)
            .await
            .with_context(|| format!("redis DEL `{key}`"))?;
        Ok(())
    }

    pub(crate) async fn delete_many<'a, I>(&self, keys: I) -> anyhow::Result<()>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let keys = keys
            .into_iter()
            .filter(|key| !key.is_empty())
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        if keys.is_empty() {
            return Ok(());
        }
        let mut conn = self.connection().await?;
        let _: usize = conn
            .del(keys)
            .await
            .context("redis DEL many request cache keys")?;
        Ok(())
    }

    pub(crate) async fn get_i64(&self, key: &str) -> anyhow::Result<Option<i64>> {
        let mut conn = self.connection().await?;
        let value: Option<i64> = conn
            .get(key)
            .await
            .with_context(|| format!("redis GET integer `{key}`"))?;
        Ok(value)
    }

    pub(crate) async fn incr(&self, key: &str) -> anyhow::Result<i64> {
        let mut conn = self.connection().await?;
        conn.incr(key, 1)
            .await
            .with_context(|| format!("redis INCR `{key}`"))
    }

    pub(crate) fn get_json_blocking<T>(&self, key: &str) -> anyhow::Result<Option<T>>
    where
        T: DeserializeOwned,
    {
        let mut conn = self.connection_blocking()?;
        let value: Option<String> = conn
            .get(key)
            .with_context(|| format!("redis GET `{key}`"))?;
        value
            .map(|json| serde_json::from_str(&json).context("decode request cache json"))
            .transpose()
    }

    pub(crate) fn set_json_blocking<T>(
        &self,
        key: &str,
        value: &T,
        ttl: Duration,
    ) -> anyhow::Result<()>
    where
        T: Serialize,
    {
        let payload = serde_json::to_string(value).context("encode request cache json")?;
        let ttl_seconds = duration_to_redis_secs(ttl);
        let mut conn = self.connection_blocking()?;
        redis::cmd("SET")
            .arg(key)
            .arg(payload)
            .arg("EX")
            .arg(ttl_seconds)
            .query::<()>(&mut conn)
            .with_context(|| format!("redis SET `{key}`"))?;
        Ok(())
    }

    pub(crate) fn get_i64_blocking(&self, key: &str) -> anyhow::Result<Option<i64>> {
        let mut conn = self.connection_blocking()?;
        let value: Option<i64> = conn
            .get(key)
            .with_context(|| format!("redis GET integer `{key}`"))?;
        Ok(value)
    }

    pub(crate) fn incr_blocking(&self, key: &str) -> anyhow::Result<i64> {
        let mut conn = self.connection_blocking()?;
        conn.incr(key, 1)
            .with_context(|| format!("redis INCR `{key}`"))
    }

    async fn connection(&self) -> anyhow::Result<redis::aio::MultiplexedConnection> {
        self.client
            .get_multiplexed_async_connection()
            .await
            .context("connect request cache redis")
    }

    fn connection_blocking(&self) -> anyhow::Result<redis::Connection> {
        self.client
            .get_connection()
            .context("connect blocking request cache redis")
    }
}

pub(crate) fn deterministic_jitter_ttl(
    key: &str,
    base: Duration,
    floor_ratio: f64,
    ceil_ratio: f64,
) -> Duration {
    let digest = Sha256::digest(key.as_bytes());
    let raw = u64::from_be_bytes(digest[..8].try_into().expect("8 digest bytes"));
    let normalized = (raw as f64) / (u64::MAX as f64);
    let ratio = floor_ratio + ((ceil_ratio - floor_ratio) * normalized);
    Duration::from_secs_f64(base.as_secs_f64() * ratio)
}

fn duration_to_redis_secs(ttl: Duration) -> u64 {
    ttl.as_secs().max(1)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    #[test]
    fn deterministic_jitter_is_stable_for_same_key() {
        let a = super::deterministic_jitter_ttl(
            "llma:req:codex:key-1",
            Duration::from_secs(6 * 3600),
            0.8,
            1.2,
        );
        let b = super::deterministic_jitter_ttl(
            "llma:req:codex:key-1",
            Duration::from_secs(6 * 3600),
            0.8,
            1.2,
        );
        assert_eq!(a, b);
    }

    #[test]
    fn deterministic_jitter_differs_for_different_keys() {
        let a = super::deterministic_jitter_ttl(
            "llma:req:codex:key-1",
            Duration::from_secs(6 * 3600),
            0.8,
            1.2,
        );
        let b = super::deterministic_jitter_ttl(
            "llma:req:codex:key-2",
            Duration::from_secs(6 * 3600),
            0.8,
            1.2,
        );
        assert_ne!(a, b);
    }

    #[test]
    fn request_cache_key_namespace_is_stable() {
        let cache = super::RequestCache::new(super::RequestCacheConfig {
            url: "redis://127.0.0.1:6379/0".to_string(),
            key_prefix: "llma:test".to_string(),
        })
        .expect("build request cache");

        assert_eq!(cache.auth_key("secret-hash"), "llma:test:auth:secret-hash".to_string());
        assert_eq!(
            cache.request_snapshot_key("codex", "key-1"),
            "llma:test:req:codex:key-1".to_string()
        );
        assert_eq!(
            cache.account_view_key("kiro", "acct-1", "core"),
            "llma:test:acct:view:scope:core:kiro:acct-1".to_string()
        );
        assert_eq!(
            cache.account_auth_key("codex", "acct-1"),
            "llma:test:acct:auth:codex:acct-1".to_string()
        );
        assert_eq!(
            cache.proxy_configs_key("edge-a"),
            "llma:test:proxy:scope:edge-a:configs".to_string()
        );
        assert_eq!(
            cache.proxy_binding_key("kiro", "edge-a"),
            "llma:test:proxy:scope:edge-a:binding:kiro".to_string()
        );
        assert_eq!(
            cache.account_view_key("kiro", "acct-1", "edge-a"),
            "llma:test:acct:view:scope:edge-a:kiro:acct-1".to_string()
        );
        assert_eq!(
            cache.dispatch_generation_key("kiro"),
            "llma:test:gen:dispatch:kiro".to_string()
        );
        assert_eq!(
            cache.usage_proxy_attribution_key("codex", "acct-1", "edge-a"),
            "llma:test:usage:proxy:scope:edge-a:codex:acct-1".to_string()
        );
        assert_eq!(
            cache.usage_catalog_generation_key(),
            "llma:test:usage:catalog:v2:gen".to_string()
        );
        assert_eq!(
            cache.usage_catalog_rollups_key(),
            "llma:test:usage:catalog:v2:rollups".to_string()
        );
        assert_eq!(
            cache.usage_catalog_filtered_segments_key("start:-:end:-:key:key-1:provider:codex"),
            "llma:test:usage:catalog:v2:segments:filtered:start:-:end:-:key:key-1:provider:codex"
                .to_string()
        );
        assert_eq!(
            cache.usage_catalog_event_locator_key("evt-1"),
            "llma:test:usage:catalog:v2:event:evt-1".to_string()
        );
        assert_eq!(
            cache.usage_catalog_filter_options_key("start:-:end:-:key:-:provider:-:filters:-"),
            "llma:test:usage:catalog:v2:filter-options:start:-:end:-:key:-:provider:-:filters:-"
                .to_string()
        );
        assert_eq!(cache.runtime_config_key(), "llma:test:runtime:config".to_string());
        assert_eq!(cache.codex_status_key(), "llma:test:status:codex".to_string());
    }

    #[test]
    fn cached_authenticated_key_round_trips_as_json() {
        let payload = super::CachedAuthenticatedKey {
            key_id: "key-1".to_string(),
            key_name: "demo".to_string(),
            provider_type: "codex".to_string(),
            protocol_family: "openai".to_string(),
            status: "active".to_string(),
            quota_billable_limit: 1234,
            billable_tokens_used: 321,
        };

        let json = serde_json::to_string(&payload).expect("serialize auth payload");
        let decoded: super::CachedAuthenticatedKey =
            serde_json::from_str(&json).expect("deserialize auth payload");

        assert_eq!(decoded, payload);
    }

    #[test]
    fn cached_runtime_config_lookup_round_trips_as_json() {
        let payload = super::CachedRuntimeConfigLookup {
            record: Some(crate::records::RuntimeConfigRecord::default()),
        };

        let json = serde_json::to_string(&payload).expect("serialize runtime config payload");
        let decoded: super::CachedRuntimeConfigLookup =
            serde_json::from_str(&json).expect("deserialize runtime config payload");

        assert_eq!(decoded, payload);
    }

    #[test]
    fn cached_codex_status_lookup_round_trips_as_json() {
        let payload = super::CachedCodexStatusLookup {
            snapshot: Some(llm_access_core::store::CodexRateLimitStatus::loading(300)),
        };

        let json = serde_json::to_string(&payload).expect("serialize codex status payload");
        let decoded: super::CachedCodexStatusLookup =
            serde_json::from_str(&json).expect("deserialize codex status payload");

        assert_eq!(decoded, payload);
    }
}
