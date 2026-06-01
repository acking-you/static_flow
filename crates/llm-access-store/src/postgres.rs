//! Postgres control-plane repository for `llm-access`.

use std::{
    collections::{BTreeMap, HashMap},
    env,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context;
use async_trait::async_trait;
use llm_access_core::{
    store::{
        self as core_store, AdminKiroBalanceView, AdminKiroCacheView, AdminProxyBinding,
        AdminProxyConfig, AdminProxyEndpointCheck, AuthenticatedKey, CodexRateLimitStatus,
        ControlStore,
    },
    usage::UsageEvent,
};
use sha2::{Digest, Sha256};
use sqlx_core::{
    arguments::Arguments, column::ColumnIndex, decode::Decode, encode::Encode, query::query_with,
    row::Row as SqlxRowTrait, types::Type,
};
use sqlx_postgres::{PgArguments, PgPool, PgPoolOptions, PgRow as SqlxPgRow, Postgres};
use tokio::sync::{Mutex, RwLock};

use crate::request_cache::{RequestCache, RequestCacheConfig};

mod cache;
mod cache_convert;
mod codex_account;
mod codex_routing;
mod config;
mod decode;
mod groups;
mod json;
mod keys;
mod kiro_account;
mod proxy;
mod proxy_support;
mod public;
mod review;
mod routes;
mod status;
mod usage;

#[cfg(test)]
use proxy_support::resolve_provider_proxy_config_from_context;


trait SqlxBindParam {
    fn add_to(&self, args: &mut PgArguments) -> anyhow::Result<()>;
}

impl<T> SqlxBindParam for T
where
    T: Clone + Send + Sync + for<'q> Encode<'q, Postgres> + Type<Postgres>,
{
    fn add_to(&self, args: &mut PgArguments) -> anyhow::Result<()> {
        args.add(self.clone())
            .map_err(|err| anyhow::anyhow!("encode sqlx postgres bind parameter: {err}"))?;
        Ok(())
    }
}

fn build_pg_arguments(params: &[&(dyn SqlxBindParam + Sync)]) -> anyhow::Result<PgArguments> {
    let mut args = PgArguments::default();
    for param in params {
        param.add_to(&mut args)?;
    }
    Ok(args)
}

struct PgRow(SqlxPgRow);

impl PgRow {
    fn get<'r, I, T>(&'r self, index: I) -> T
    where
        I: ColumnIndex<SqlxPgRow>,
        T: Decode<'r, Postgres> + Type<Postgres>,
    {
        self.0
            .try_get(index)
            .expect("decode sqlx postgres row column")
    }

    fn get_optional_bool(&self, name: &str) -> Option<bool> {
        self.0.try_get::<Option<bool>, _>(name).ok().flatten()
    }
}

const POSTGRES_MAX_BIND_PARAMS: usize = 65_535;
const USAGE_ROLLUP_PARAMS_PER_ROW: usize = 8;
const USAGE_ROLLUP_BATCH_ROW_LIMIT: usize = POSTGRES_MAX_BIND_PARAMS / USAGE_ROLLUP_PARAMS_PER_ROW;
const CODEX_STATUS_CACHE_TTL: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
struct CodexRouteCandidateRow {
    account_name: String,
    status: String,
    settings_json: String,
    last_refresh_at_ms: Option<i64>,
    last_error: Option<String>,
    access_token: Option<String>,
}

#[derive(Debug, Clone)]
struct KiroRouteCandidateRow {
    account_name: String,
    profile_arn: Option<String>,
    user_id: Option<String>,
    status: String,
    max_concurrency: Option<i64>,
    min_start_interval_ms: Option<i64>,
    proxy_config_id: Option<String>,
    disabled: bool,
    minimum_remaining_credits_before_block: f64,
    auth_profile_arn: Option<String>,
    api_region: Option<String>,
    proxy_mode: Option<String>,
    auth_proxy_config_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct UsageRollupDelta {
    input_uncached_tokens: i64,
    input_cached_tokens: i64,
    output_tokens: i64,
    billable_tokens: i64,
    credit_total: f64,
    credit_missing_events: i64,
    last_used_at_ms: i64,
}

fn aggregate_usage_rollup_deltas<'a>(
    events: &'a [UsageEvent],
) -> anyhow::Result<Vec<(&'a str, UsageRollupDelta)>> {
    let mut deltas = HashMap::<&'a str, UsageRollupDelta>::with_capacity(events.len());
    for event in events {
        let credit_delta = event
            .credit_usage
            .as_deref()
            .unwrap_or("0")
            .parse::<f64>()
            .context("parse usage event credit usage")?;
        let delta = deltas.entry(event.key_id.as_str()).or_default();
        delta.input_uncached_tokens = delta
            .input_uncached_tokens
            .saturating_add(event.input_uncached_tokens.max(0));
        delta.input_cached_tokens = delta
            .input_cached_tokens
            .saturating_add(event.input_cached_tokens.max(0));
        delta.output_tokens = delta
            .output_tokens
            .saturating_add(event.output_tokens.max(0));
        delta.billable_tokens = delta
            .billable_tokens
            .saturating_add(event.billable_tokens.max(0));
        delta.credit_total += credit_delta;
        delta.credit_missing_events = delta
            .credit_missing_events
            .saturating_add(event.credit_usage_missing as i64);
        delta.last_used_at_ms = delta.last_used_at_ms.max(event.created_at_ms);
    }
    Ok(deltas.into_iter().collect())
}

#[derive(Clone)]
struct SqlxClient {
    pool: PgPool,
}

impl SqlxClient {
    async fn connect(database_url: &str) -> anyhow::Result<Self> {
        let max_connections = env::var("LLM_ACCESS_CONTROL_PG_MAX_CONNECTIONS")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .map(|value| value.clamp(1, 32))
            .unwrap_or(4);
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .min_connections(0)
            .acquire_timeout(Duration::from_secs(10))
            .idle_timeout(Duration::from_secs(60))
            .max_lifetime(Duration::from_secs(30 * 60))
            .connect(database_url)
            .await
            .context("connect sqlx postgres control repository")?;
        Ok(Self {
            pool,
        })
    }

    fn is_closed(&self) -> bool {
        self.pool.is_closed()
    }

    async fn query_opt(
        &self,
        sql: &str,
        params: &[&(dyn SqlxBindParam + Sync)],
    ) -> anyhow::Result<Option<PgRow>> {
        let args = build_pg_arguments(params)?;
        Ok(query_with(sql, args)
            .fetch_optional(&self.pool)
            .await
            .context("query optional sqlx postgres row")?
            .map(PgRow))
    }

    async fn query_one(
        &self,
        sql: &str,
        params: &[&(dyn SqlxBindParam + Sync)],
    ) -> anyhow::Result<PgRow> {
        let args = build_pg_arguments(params)?;
        let row = query_with(sql, args)
            .fetch_one(&self.pool)
            .await
            .context("query one sqlx postgres row")?;
        Ok(PgRow(row))
    }

    async fn query(
        &self,
        sql: &str,
        params: &[&(dyn SqlxBindParam + Sync)],
    ) -> anyhow::Result<Vec<PgRow>> {
        let args = build_pg_arguments(params)?;
        let rows = query_with(sql, args)
            .fetch_all(&self.pool)
            .await
            .context("query many sqlx postgres rows")?;
        Ok(rows.into_iter().map(PgRow).collect())
    }

    async fn execute(
        &self,
        sql: &str,
        params: &[&(dyn SqlxBindParam + Sync)],
    ) -> anyhow::Result<u64> {
        let args = build_pg_arguments(params)?;
        let result = query_with(sql, args)
            .execute(&self.pool)
            .await
            .context("execute sqlx postgres statement")?;
        Ok(result.rows_affected())
    }

    #[cfg(test)]
    async fn batch_execute(&self, sql: &str) -> anyhow::Result<()> {
        sqlx_core::raw_sql::raw_sql(sql)
            .execute(&self.pool)
            .await
            .context("execute raw sqlx postgres statement")?;
        Ok(())
    }

    async fn transaction(&self) -> anyhow::Result<SqlxTransaction<'_>> {
        let tx = self
            .pool
            .begin()
            .await
            .context("begin sqlx postgres transaction")?;
        Ok(SqlxTransaction {
            inner: Mutex::new(Some(tx)),
        })
    }

    #[cfg(test)]
    async fn close(&self) {
        self.pool.close().await;
    }
}

struct SqlxTransaction<'a> {
    inner: Mutex<Option<sqlx_postgres::PgTransaction<'a>>>,
}

impl<'a> SqlxTransaction<'a> {
    async fn execute(
        &self,
        sql: &str,
        params: &[&(dyn SqlxBindParam + Sync)],
    ) -> anyhow::Result<u64> {
        let args = build_pg_arguments(params)?;
        let mut guard = self.inner.lock().await;
        let tx = guard
            .as_mut()
            .context("sqlx postgres transaction is already finished")?;
        let result = query_with(sql, args)
            .execute(&mut **tx)
            .await
            .context("execute sqlx postgres transaction statement")?;
        Ok(result.rows_affected())
    }

    async fn commit(self) -> anyhow::Result<()> {
        let mut guard = self.inner.lock().await;
        let tx = guard
            .take()
            .context("sqlx postgres transaction is already finished")?;
        drop(guard);
        tx.commit()
            .await
            .context("commit sqlx postgres transaction")?;
        Ok(())
    }
}

/// Async Postgres-backed control-plane repository.
pub struct PostgresControlRepository {
    client: SqlxClient,
    codex_status_cache: Arc<RwLock<Option<CachedCodexRateLimitStatus>>>,
    request_cache: Option<RequestCache>,
    proxy_scope: ProxyConfigScope,
}

/// Proxy attribution resolved for one consumed usage event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageProxyAttribution {
    /// Provider type that owns the account.
    pub provider_type: String,
    /// Account name used by the upstream request.
    pub account_name: String,
    /// Effective proxy source (`fixed`, `binding`, `none`, ...).
    pub proxy_source: String,
    /// Effective proxy config id when known.
    pub proxy_config_id: Option<String>,
    /// Effective proxy config name when known.
    pub proxy_config_name: Option<String>,
    /// Effective proxy URL when known.
    pub proxy_url: Option<String>,
}

#[derive(Debug, Clone)]
struct CachedCodexRateLimitStatus {
    snapshot: CodexRateLimitStatus,
    loaded_at: Instant,
}

type KiroCachedStatusParts = (Option<AdminKiroBalanceView>, AdminKiroCacheView);

struct KiroAdminAccountViewContext {
    default_cache: AdminKiroCacheView,
    status_by_account: BTreeMap<String, KiroCachedStatusParts>,
    proxy_configs_by_id: BTreeMap<String, AdminProxyConfig>,
    kiro_proxy_binding: AdminProxyBinding,
}

struct CodexAdminAccountViewContext {
    proxy_configs_by_id: BTreeMap<String, AdminProxyConfig>,
    codex_proxy_binding: AdminProxyBinding,
}

struct ProviderProxyResolutionContext {
    proxy_configs_by_id: BTreeMap<String, AdminProxyConfig>,
    binding: AdminProxyBinding,
}

/// Node-local scope used to resolve effective proxy slot contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyConfigScope {
    node_id: String,
    is_core: bool,
}

impl ProxyConfigScope {
    /// Default core scope used when cluster identity is not configured.
    pub fn core() -> Self {
        Self {
            node_id: "core".to_string(),
            is_core: true,
        }
    }

    /// Non-core node scope keyed by the configured node id.
    pub fn node(node_id: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            is_core: false,
        }
    }

    fn cache_key_segment(&self) -> &str {
        &self.node_id
    }

    fn scope_node_id(&self) -> Option<String> {
        Some(self.node_id.clone())
    }

    fn can_edit_slot_metadata(&self) -> bool {
        self.is_core
    }
}

#[derive(Debug, Clone)]
struct ProxyConfigNodeOverride {
    proxy_url: String,
    proxy_username: Option<String>,
    proxy_password: Option<String>,
    status: String,
    created_at_ms: i64,
    updated_at_ms: i64,
}

#[derive(Debug, Clone)]
struct ProxyEndpointCheckRow {
    proxy_config_id: String,
    provider_type: String,
    check: AdminProxyEndpointCheck,
}

#[derive(Debug, Clone)]
struct CodexAdminAccountListRow {
    account_name: String,
    account_id: Option<String>,
    status: String,
    map_gpt53_codex_to_spark: bool,
    auth_refresh_enabled: bool,
    route_weight_tier: Option<String>,
    proxy_mode: String,
    proxy_config_id: Option<String>,
    request_max_concurrency: Option<i64>,
    request_min_start_interval_ms: Option<i64>,
    last_refresh_at_ms: Option<i64>,
    last_error: Option<String>,
    access_token: Option<String>,
    plan_type: Option<String>,
    primary_remaining_percent: Option<f64>,
    secondary_remaining_percent: Option<f64>,
    last_usage_checked_at_ms: Option<i64>,
    last_usage_success_at_ms: Option<i64>,
    usage_error_message: Option<String>,
}

#[derive(Debug, Clone)]
struct KiroAdminAccountListRow {
    account_name: String,
    auth_method: String,
    profile_arn: Option<String>,
    user_id: Option<String>,
    status: String,
    provider: Option<String>,
    email: Option<String>,
    expires_at: Option<String>,
    auth_profile_arn: Option<String>,
    has_refresh_token: bool,
    disabled_json: bool,
    disabled_reason: Option<String>,
    source: Option<String>,
    source_db_path: Option<String>,
    last_imported_at: Option<i64>,
    subscription_title: Option<String>,
    region: Option<String>,
    auth_region: Option<String>,
    api_region: Option<String>,
    machine_id: Option<String>,
    max_concurrency: Option<i64>,
    auth_max_concurrency: Option<i64>,
    min_start_interval_ms: Option<i64>,
    auth_min_start_interval_ms: Option<i64>,
    minimum_remaining_credits_before_block: Option<f64>,
    proxy_mode: Option<String>,
    proxy_config_id: Option<String>,
    auth_proxy_config_id: Option<String>,
    proxy_url: Option<String>,
    last_error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(default)]
struct CodexAccountSettings {
    map_gpt53_codex_to_spark: bool,
    auth_refresh_enabled: bool,
    route_weight_tier: Option<String>,
    proxy_mode: String,
    proxy_config_id: Option<String>,
    request_max_concurrency: Option<u64>,
    request_min_start_interval_ms: Option<u64>,
}

impl Default for CodexAccountSettings {
    fn default() -> Self {
        Self {
            map_gpt53_codex_to_spark: false,
            auth_refresh_enabled: true,
            route_weight_tier: None,
            proxy_mode: "inherit".to_string(),
            proxy_config_id: None,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
        }
    }
}

impl PostgresControlRepository {
    /// Connect to the Postgres control plane and run pending migrations.
    pub async fn connect(
        database_url: &str,
        request_cache_config: Option<RequestCacheConfig>,
    ) -> anyhow::Result<Self> {
        Self::connect_with_proxy_scope(database_url, request_cache_config, ProxyConfigScope::core())
            .await
    }

    /// Connect to the Postgres control plane with an explicit proxy resolution
    /// scope.
    pub async fn connect_with_proxy_scope(
        database_url: &str,
        request_cache_config: Option<RequestCacheConfig>,
        proxy_scope: ProxyConfigScope,
    ) -> anyhow::Result<Self> {
        let client = SqlxClient::connect(database_url).await?;
        llm_access_migrations::run_postgres_migrations(&client.pool).await?;
        let request_cache = request_cache_config.map(RequestCache::new).transpose()?;
        Ok(Self {
            client,
            codex_status_cache: Arc::new(RwLock::new(None)),
            request_cache,
            proxy_scope,
        })
    }

    async fn connect_fresh_client(&self) -> anyhow::Result<SqlxClient> {
        Ok(self.client.clone())
    }

    async fn invalidate_proxy_metadata_cache(&self) {
        let Some(cache) = self.request_cache.as_ref() else {
            return;
        };
        let scope = self.proxy_scope.cache_key_segment();
        let configs_key = cache.proxy_configs_key(scope);
        let codex_binding_key = cache.proxy_binding_key(core_store::PROVIDER_CODEX, scope);
        let kiro_binding_key = cache.proxy_binding_key(core_store::PROVIDER_KIRO, scope);
        if let Err(err) = cache
            .delete_many([
                configs_key.as_str(),
                codex_binding_key.as_str(),
                kiro_binding_key.as_str(),
            ])
            .await
        {
            tracing::warn!(error = %err, "failed to invalidate proxy metadata cache");
        }
    }

    async fn current_proxy_metadata_generation(&self) -> i64 {
        let codex = self
            .current_dispatch_generation(core_store::PROVIDER_CODEX)
            .await;
        let kiro = self
            .current_dispatch_generation(core_store::PROVIDER_KIRO)
            .await;
        codex.max(kiro)
    }

    fn ensure_connection_alive(&self) -> anyhow::Result<()> {
        if self.client.is_closed() {
            anyhow::bail!("sqlx postgres control pool is closed");
        }
        Ok(())
    }

    async fn current_dispatch_generation(&self, provider: &str) -> i64 {
        let Some(cache) = self.request_cache.as_ref() else {
            return 0;
        };
        let key = cache.dispatch_generation_key(provider);
        match cache.get_i64(&key).await {
            Ok(Some(value)) => value,
            Ok(None) => 0,
            Err(err) => {
                tracing::warn!(
                    provider,
                    key = %key,
                    error = %err,
                    "request cache generation read failed; falling back to generation=0"
                );
                0
            },
        }
    }

    async fn bump_dispatch_generation(&self, provider: &str) {
        let Some(cache) = self.request_cache.as_ref() else {
            return;
        };
        let key = cache.dispatch_generation_key(provider);
        if let Err(err) = cache.incr(&key).await {
            tracing::warn!(
                provider,
                key = %key,
                error = %err,
                "request cache generation bump failed"
            );
        }
    }

    async fn count_rows(
        &self,
        count_all_sql: &str,
        count_status_sql: &str,
        status: Option<&str>,
    ) -> anyhow::Result<usize> {
        self.ensure_connection_alive()?;
        let count: i64 = if let Some(status) = status {
            self.client
                .query_one(count_status_sql, &[&status])
                .await
                .context("count filtered rows")?
                .get(0)
        } else {
            self.client
                .query_one(count_all_sql, &[])
                .await
                .context("count rows")?
                .get(0)
        };
        Ok(count.max(0) as usize)
    }
}

fn hash_bearer_secret(secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[async_trait]
impl ControlStore for PostgresControlRepository {
    async fn authenticate_bearer_secret(
        &self,
        secret: &str,
    ) -> anyhow::Result<Option<AuthenticatedKey>> {
        self.load_authenticated_key_cached(&hash_bearer_secret(secret))
            .await
    }

    async fn apply_usage_rollup(&self, event: &UsageEvent) -> anyhow::Result<()> {
        self.apply_usage_rollups_batch(std::slice::from_ref(event))
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use anyhow::Context;
    use llm_access_core::{
        provider::{ProtocolFamily, ProviderType, RouteStrategy},
        store::{
            AdminCodexAccountPageQuery, AdminCodexAccountSortMode, AdminCodexAccountStore,
            AdminConfigStore, AdminKeyStore, AdminProxyConfigPatch, AdminProxyStore,
            AdminReviewQueueStore, ControlStore, NewAdminProxyConfig,
            NewPublicAccountContributionRequest, PublicSubmissionStore, PublicUsageStore,
            UsageEventSink,
        },
    };
    use sha2::{Digest, Sha256};

    use super::SqlxClient;

    static TEST_DB_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

    async fn test_db_guard() -> tokio::sync::MutexGuard<'static, ()> {
        TEST_DB_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await
    }

    async fn reset_test_db(database_url: &str) -> anyhow::Result<()> {
        crate::initialize_postgres_target(database_url)
            .await
            .context("initialize postgres test database")?;
        let client = SqlxClient::connect(database_url)
            .await
            .context("connect postgres test database")?;
        client
            .batch_execute(
                "TRUNCATE TABLE
                    llm_account_import_job_items,
                    llm_account_import_jobs,
                    llm_codex_status_cache,
                    llm_sponsor_requests,
                    gpt2api_account_contribution_requests,
                    llm_account_contribution_requests,
                    llm_token_requests,
                    llm_kiro_status_cache,
                    llm_kiro_accounts,
                    llm_codex_accounts,
                    llm_proxy_config_endpoint_checks,
                    llm_proxy_config_node_overrides,
                    llm_proxy_bindings,
                    llm_proxy_configs,
                    llm_account_groups,
                    llm_runtime_config,
                    llm_usage_segment_events,
                    llm_usage_segment_key_rollups,
                    llm_usage_segments,
                    llm_key_usage_rollups,
                    llm_key_route_config,
                    llm_keys CASCADE",
            )
            .await
            .context("truncate postgres test fixtures")?;
        client.close().await;
        Ok(())
    }

    async fn seed_test_key_bundle(database_url: &str) -> anyhow::Result<()> {
        let client = SqlxClient::connect(database_url)
            .await
            .context("connect postgres test database")?;
        let key_hash = format!("{:x}", Sha256::digest(b"secret"));
        client
            .execute(
                "INSERT INTO llm_keys (
                    key_id, name, secret, key_hash, status, provider_type, protocol_family,
                    public_visible, quota_billable_limit, created_at_ms, updated_at_ms
                 ) VALUES (
                    'key-1', 'external', 'secret', $1, 'active', 'codex', 'openai',
                    TRUE, 1000, 1700000000000, 1700000000000
                 )",
                &[&key_hash],
            )
            .await
            .context("insert postgres test key row")?;
        client
            .batch_execute(
                "INSERT INTO llm_key_route_config (
                    key_id, route_strategy, fixed_account_name, auto_account_names_json,
                    account_group_id, model_name_map_json, request_max_concurrency,
                    request_min_start_interval_ms, codex_fast_enabled,
                    kiro_request_validation_enabled, kiro_cache_estimation_enabled,
                    kiro_zero_cache_debug_enabled, kiro_full_request_logging_enabled,
                    kiro_cache_policy_override_json,
                    kiro_billable_model_multipliers_override_json
                 ) VALUES (
                    'key-1', NULL, NULL, NULL, NULL, NULL, NULL, NULL,
                    TRUE, FALSE, FALSE, FALSE, FALSE, NULL, NULL
                 );
                 INSERT INTO llm_key_usage_rollups (
                    key_id, input_uncached_tokens, input_cached_tokens, output_tokens,
                    billable_tokens, credit_total, credit_missing_events, last_used_at_ms,
                    updated_at_ms
                 ) VALUES (
                    'key-1', 0, 0, 0, 0, '0', 0, NULL, 1700000000000
                 );",
            )
            .await
            .context("insert postgres test key config rows")?;
        client.close().await;
        Ok(())
    }

    async fn seed_test_kiro_key_page_fixture(database_url: &str) -> anyhow::Result<()> {
        let client = SqlxClient::connect(database_url)
            .await
            .context("connect postgres test database")?;
        let key_hash_new = format!("{:x}", Sha256::digest(b"kiro-secret-new"));
        let key_hash_mid = format!("{:x}", Sha256::digest(b"kiro-secret-mid"));
        let key_hash_old = format!("{:x}", Sha256::digest(b"kiro-secret-old"));
        client
            .batch_execute(&format!(
                "INSERT INTO llm_keys (
                        key_id, name, secret, key_hash, status, provider_type, protocol_family,
                        public_visible, quota_billable_limit, created_at_ms, updated_at_ms
                     ) VALUES
                        ('kiro-key-new', 'kiro-new', 'kiro-secret-new', '{key_hash_new}', \
                 'active', 'kiro', 'anthropic', TRUE, 1000, 300, 300),
                        ('kiro-key-mid', 'kiro-mid', 'kiro-secret-mid', '{key_hash_mid}', \
                 'active', 'kiro', 'anthropic', TRUE, 1000, 200, 200),
                        ('kiro-key-old', 'kiro-old', 'kiro-secret-old', '{key_hash_old}', \
                 'active', 'kiro', 'anthropic', TRUE, 1000, 100, 100);
                     INSERT INTO llm_key_route_config (
                        key_id, route_strategy, fixed_account_name, auto_account_names_json,
                        account_group_id, model_name_map_json, request_max_concurrency,
                        request_min_start_interval_ms, codex_fast_enabled,
                        kiro_request_validation_enabled, kiro_cache_estimation_enabled,
                        kiro_zero_cache_debug_enabled, kiro_full_request_logging_enabled,
                        kiro_cache_policy_override_json,
                        kiro_billable_model_multipliers_override_json
                     ) VALUES
                        ('kiro-key-new', 'auto', NULL, NULL, NULL, NULL, NULL, NULL, TRUE, TRUE, \
                 TRUE, FALSE, FALSE, NULL, NULL),
                        ('kiro-key-mid', 'fixed', 'kiro-a', NULL, 'group-beta', NULL, NULL, NULL, \
                 TRUE, TRUE, TRUE, FALSE, FALSE, NULL, NULL),
                        ('kiro-key-old', 'auto', NULL, '[\"kiro-a\", \"kiro-d\", \
                 \"kiro-a\"]'::jsonb, NULL, NULL, NULL, NULL, TRUE, TRUE, TRUE, FALSE, FALSE, \
                 NULL, NULL);
                     INSERT INTO llm_key_usage_rollups (
                        key_id, input_uncached_tokens, input_cached_tokens, output_tokens,
                        billable_tokens, credit_total, credit_missing_events, last_used_at_ms,
                        updated_at_ms
                     ) VALUES
                        ('kiro-key-new', 0, 0, 0, 0, '0', 0, NULL, 300),
                        ('kiro-key-mid', 0, 0, 0, 0, '0', 0, NULL, 200),
                        ('kiro-key-old', 0, 0, 0, 0, '0', 0, NULL, 100);
                     INSERT INTO llm_account_groups (
                        group_id, provider_type, name, account_names_json, created_at_ms, \
                 updated_at_ms
                     ) VALUES
                        ('group-beta', 'kiro', 'group-beta', '[\"kiro-b\", \"kiro-c\", \
                 \"kiro-b\"]'::jsonb, 10, 10);
                     INSERT INTO llm_kiro_accounts (
                        account_name, auth_method, account_id, profile_arn, user_id,
                        status, auth_json, max_concurrency, min_start_interval_ms,
                        proxy_config_id, last_refresh_at_ms, last_error, created_at_ms, \
                 updated_at_ms
                     ) VALUES
                        ('kiro-a', 'social', NULL, NULL, NULL, 'active', '{{}}'::jsonb, 1, 0, \
                 NULL, NULL, NULL, 10, 10),
                        ('kiro-b', 'social', NULL, NULL, NULL, 'active', '{{}}'::jsonb, 1, 0, \
                 NULL, NULL, NULL, 20, 20),
                        ('kiro-c', 'social', NULL, NULL, NULL, 'active', '{{}}'::jsonb, 1, 0, \
                 NULL, NULL, NULL, 30, 30),
                        ('kiro-d', 'social', NULL, NULL, NULL, 'active', '{{}}'::jsonb, 1, 0, \
                 NULL, NULL, NULL, 40, 40);
                     INSERT INTO llm_kiro_status_cache (
                        account_name, status, balance_json, cache_json, refreshed_at_ms,
                        expires_at_ms, last_error
                     ) VALUES
                        ('kiro-a', 'active', \
                 '{{\"current_usage\":60.0,\"usage_limit\":100.0,\"remaining\":40.0,\"\
                 next_reset_at\":null,\"subscription_title\":\"Pro\"}}'::jsonb, '{{}}'::jsonb, 1, \
                 2, NULL),
                        ('kiro-b', 'active', \
                 '{{\"current_usage\":50.0,\"usage_limit\":200.0,\"remaining\":150.0,\"\
                 next_reset_at\":null,\"subscription_title\":\"Pro\"}}'::jsonb, '{{}}'::jsonb, 1, \
                 2, NULL),
                        ('kiro-c', 'active', 'null'::jsonb, '{{}}'::jsonb, 1, 2, NULL),
                        ('kiro-d', 'active', \
                 '{{\"current_usage\":210.0,\"usage_limit\":300.0,\"remaining\":90.0,\"\
                 next_reset_at\":null,\"subscription_title\":\"Pro\"}}'::jsonb, '{{}}'::jsonb, 1, \
                 2, NULL);"
            ))
            .await
            .context("seed postgres kiro key page fixture")?;
        client.close().await;
        Ok(())
    }

    async fn seed_test_codex_account_page_fixture(database_url: &str) -> anyhow::Result<()> {
        let client = SqlxClient::connect(database_url)
            .await
            .context("connect postgres test database")?;
        client
            .batch_execute(
                r#"
                INSERT INTO llm_codex_accounts (
                    account_name, account_id, email, status, auth_json, settings_json,
                    last_refresh_at_ms, last_error, created_at_ms, updated_at_ms
                ) VALUES
                    (
                        'codex-new', 'acct-new', NULL, 'active',
                        '{"access_token":"token-new"}'::jsonb,
                        '{"auth_refresh_enabled":true,"map_gpt53_codex_to_spark":false,
                          "route_weight_tier":"auto","proxy_mode":"inherit"}'::jsonb,
                        290, NULL, 300, 300
                    ),
                    (
                        'codex-mid', 'acct-mid', NULL, 'disabled',
                        '{"access_token":"token-mid"}'::jsonb,
                        '{"auth_refresh_enabled":true,"map_gpt53_codex_to_spark":false,
                          "route_weight_tier":"free","proxy_mode":"inherit"}'::jsonb,
                        190, NULL, 200, 200
                    ),
                    (
                        'codex-old', 'acct-old', NULL, 'active',
                        '{"access_token":"token-old"}'::jsonb,
                        '{"auth_refresh_enabled":true,"map_gpt53_codex_to_spark":false,
                          "route_weight_tier":"plus","proxy_mode":"inherit"}'::jsonb,
                        90, 'refresh failed', 100, 100
                    );
                INSERT INTO llm_codex_status_cache (id, snapshot_json, updated_at_ms)
                VALUES (
                    'default',
                    '{
                        "status":"ready",
                        "refresh_interval_seconds":300,
                        "last_checked_at":400,
                        "last_success_at":400,
                        "source_url":"https://chatgpt.com/backend-api/codex/models",
                        "error_message":null,
                        "accounts":[
                            {
                                "name":"codex-new",
                                "status":"active",
                                "plan_type":"Pro",
                                "primary_remaining_percent":70.0,
                                "secondary_remaining_percent":80.0,
                                "last_usage_checked_at":400,
                                "last_usage_success_at":400,
                                "usage_error_message":null
                            },
                            {
                                "name":"codex-mid",
                                "status":"active",
                                "plan_type":"Pro",
                                "primary_remaining_percent":55.0,
                                "secondary_remaining_percent":60.0,
                                "last_usage_checked_at":400,
                                "last_usage_success_at":400,
                                "usage_error_message":null
                            },
                            {
                                "name":"codex-old",
                                "status":"active",
                                "plan_type":"Plus",
                                "primary_remaining_percent":20.0,
                                "secondary_remaining_percent":10.0,
                                "last_usage_checked_at":400,
                                "last_usage_success_at":400,
                                "usage_error_message":null
                            }
                        ],
                        "buckets":[]
                    }'::jsonb,
                    400
                );
                "#,
            )
            .await
            .context("seed postgres codex account page fixture")?;
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn postgres_repository_reads_runtime_config_and_authenticates_key() {
        let Ok(database_url) = std::env::var("TEST_POSTGRES_URL") else {
            eprintln!("skipping postgres integration test: TEST_POSTGRES_URL is not set");
            return;
        };
        let _guard = test_db_guard().await;
        reset_test_db(&database_url)
            .await
            .expect("reset postgres test database");
        seed_test_key_bundle(&database_url)
            .await
            .expect("seed postgres test key bundle");
        let repo = super::PostgresControlRepository::connect(&database_url, None)
            .await
            .expect("connect postgres repository");

        let config = repo
            .get_admin_runtime_config()
            .await
            .expect("runtime config");
        assert_eq!(config.codex_client_version.as_str(), "0.124.0");

        let key = repo
            .authenticate_bearer_secret("secret")
            .await
            .expect("lookup result")
            .expect("key must exist");
        assert_eq!(key.key_name, "external");
    }

    #[tokio::test]
    async fn postgres_repository_accepts_optional_request_cache_config() {
        let Ok(database_url) = std::env::var("TEST_POSTGRES_URL") else {
            eprintln!("skipping postgres integration test: TEST_POSTGRES_URL is not set");
            return;
        };
        let _guard = test_db_guard().await;
        reset_test_db(&database_url)
            .await
            .expect("reset postgres test database");

        let repo = super::PostgresControlRepository::connect(
            &database_url,
            Some(crate::request_cache::RequestCacheConfig {
                url: "redis://127.0.0.1:6379/0".to_string(),
                key_prefix: "llma:test".to_string(),
            }),
        )
        .await
        .expect("connect postgres repository with request cache");

        assert!(repo.request_cache.is_some());
    }

    #[tokio::test]
    async fn postgres_repository_resolves_proxy_configs_per_node_scope() {
        let Ok(database_url) = std::env::var("TEST_POSTGRES_URL") else {
            eprintln!("skipping postgres integration test: TEST_POSTGRES_URL is not set");
            return;
        };
        let _guard = test_db_guard().await;
        reset_test_db(&database_url)
            .await
            .expect("reset postgres test database");
        let core_repo = super::PostgresControlRepository::connect_with_proxy_scope(
            &database_url,
            None,
            super::ProxyConfigScope::core(),
        )
        .await
        .expect("connect core postgres repository");
        let edge_repo = super::PostgresControlRepository::connect_with_proxy_scope(
            &database_url,
            None,
            super::ProxyConfigScope::node("edge-a"),
        )
        .await
        .expect("connect edge postgres repository");

        core_repo
            .create_admin_proxy_config(NewAdminProxyConfig {
                id: "proxy-slot-1".to_string(),
                name: "slot 1".to_string(),
                proxy_url: "http://core.proxy:1111".to_string(),
                proxy_username: Some("core-user".to_string()),
                proxy_password: Some("core-pass".to_string()),
                created_at_ms: 100,
            })
            .await
            .expect("create core proxy slot");

        let inherited = edge_repo
            .get_admin_proxy_config("proxy-slot-1")
            .await
            .expect("load inherited edge proxy")
            .expect("edge sees core slot");
        assert_eq!(inherited.proxy_url, "http://core.proxy:1111");
        assert_eq!(inherited.effective_source, "core");
        assert!(!inherited.has_node_override);

        let overridden = edge_repo
            .patch_admin_proxy_config("proxy-slot-1", AdminProxyConfigPatch {
                proxy_url: Some("http://edge.proxy:2222".to_string()),
                proxy_username: Some(Some("edge-user".to_string())),
                proxy_password: Some(Some("edge-pass".to_string())),
                status: Some("active".to_string()),
                updated_at_ms: 200,
                ..AdminProxyConfigPatch::default()
            })
            .await
            .expect("patch edge proxy override")
            .expect("edge proxy slot exists");
        assert_eq!(overridden.proxy_url, "http://edge.proxy:2222");
        assert_eq!(overridden.proxy_username.as_deref(), Some("edge-user"));
        assert_eq!(overridden.effective_source, "node_override");
        assert!(overridden.has_node_override);

        let core_after_override = core_repo
            .get_admin_proxy_config("proxy-slot-1")
            .await
            .expect("load core proxy")
            .expect("core slot exists");
        assert_eq!(core_after_override.proxy_url, "http://core.proxy:1111");
        assert_eq!(core_after_override.effective_source, "core");

        edge_repo
            .update_admin_proxy_binding("codex", Some("proxy-slot-1".to_string()))
            .await
            .expect("bind codex proxy slot");
        let edge_context = edge_repo
            .load_provider_proxy_resolution_context("codex")
            .await
            .expect("load edge proxy context");
        let fixed_proxy = super::resolve_provider_proxy_config_from_context(
            "fixed",
            Some("proxy-slot-1"),
            &edge_context,
        )
        .expect("resolve fixed edge proxy")
        .expect("fixed proxy present");
        assert_eq!(fixed_proxy.proxy_url, "http://edge.proxy:2222");
        assert_eq!(
            edge_context.binding.effective_proxy_url.as_deref(),
            Some("http://edge.proxy:2222")
        );

        let reset = edge_repo
            .reset_admin_proxy_config_override("proxy-slot-1")
            .await
            .expect("reset edge proxy override")
            .expect("edge proxy slot exists after reset");
        assert_eq!(reset.proxy_url, "http://core.proxy:1111");
        assert_eq!(reset.effective_source, "core");
        assert!(!reset.has_node_override);
    }

    #[tokio::test]
    async fn postgres_repository_records_proxy_endpoint_checks_per_node_scope() {
        let Ok(database_url) = std::env::var("TEST_POSTGRES_URL") else {
            eprintln!("skipping postgres integration test: TEST_POSTGRES_URL is not set");
            return;
        };
        let _guard = test_db_guard().await;
        reset_test_db(&database_url)
            .await
            .expect("reset postgres test database");
        let core_repo = super::PostgresControlRepository::connect_with_proxy_scope(
            &database_url,
            None,
            super::ProxyConfigScope::core(),
        )
        .await
        .expect("connect core postgres repository");
        let edge_repo = super::PostgresControlRepository::connect_with_proxy_scope(
            &database_url,
            None,
            super::ProxyConfigScope::node("edge-a"),
        )
        .await
        .expect("connect edge postgres repository");

        core_repo
            .create_admin_proxy_config(NewAdminProxyConfig {
                id: "proxy-slot-1".to_string(),
                name: "slot 1".to_string(),
                proxy_url: "http://core.proxy:1111".to_string(),
                proxy_username: None,
                proxy_password: None,
                created_at_ms: 100,
            })
            .await
            .expect("create core proxy slot");

        core_repo
            .record_admin_proxy_endpoint_check(
                llm_access_core::store::AdminProxyEndpointCheckUpdate {
                    proxy_config_id: "proxy-slot-1".to_string(),
                    provider_type: "codex".to_string(),
                    target_url: "https://chatgpt.com/backend-api/codex/models".to_string(),
                    reachable: true,
                    status_code: Some(401),
                    latency_ms: 1234,
                    error_message: Some("unauthorized".to_string()),
                    checked_at_ms: 200,
                },
            )
            .await
            .expect("record core codex check");
        edge_repo
            .record_admin_proxy_endpoint_check(
                llm_access_core::store::AdminProxyEndpointCheckUpdate {
                    proxy_config_id: "proxy-slot-1".to_string(),
                    provider_type: "codex".to_string(),
                    target_url: "https://chatgpt.com/backend-api/codex/models".to_string(),
                    reachable: true,
                    status_code: Some(200),
                    latency_ms: 321,
                    error_message: None,
                    checked_at_ms: 250,
                },
            )
            .await
            .expect("record edge codex check");

        let core_checked = core_repo
            .get_admin_proxy_config("proxy-slot-1")
            .await
            .expect("load core checked proxy")
            .expect("core proxy exists");
        assert_eq!(
            core_checked
                .latest_codex_check
                .as_ref()
                .map(|check| check.latency_ms),
            Some(1234)
        );

        let edge_checked = edge_repo
            .get_admin_proxy_config("proxy-slot-1")
            .await
            .expect("load edge checked proxy")
            .expect("edge proxy exists");
        assert_eq!(
            edge_checked
                .latest_codex_check
                .as_ref()
                .map(|check| check.latency_ms),
            Some(321)
        );
        assert_eq!(edge_checked.effective_source, "core");
        assert!(!edge_checked.has_node_override);
    }

    #[tokio::test]
    async fn postgres_repository_updates_key_usage_rollups() {
        let Ok(database_url) = std::env::var("TEST_POSTGRES_URL") else {
            eprintln!("skipping postgres integration test: TEST_POSTGRES_URL is not set");
            return;
        };
        let _guard = test_db_guard().await;
        reset_test_db(&database_url)
            .await
            .expect("reset postgres test database");
        seed_test_key_bundle(&database_url)
            .await
            .expect("seed postgres test key bundle");
        let repo = super::PostgresControlRepository::connect(&database_url, None)
            .await
            .expect("connect postgres repository");

        let event = llm_access_core::usage::UsageEvent {
            event_id: "evt-1".to_string(),
            created_at_ms: 1_700_000_000_001,
            provider_type: ProviderType::Codex,
            protocol_family: ProtocolFamily::OpenAi,
            key_id: "key-1".to_string(),
            key_name: "external".to_string(),
            account_name: Some("acct-1".to_string()),
            account_group_id_at_event: None,
            route_strategy_at_event: Some(RouteStrategy::Auto),
            request_method: "POST".to_string(),
            request_url: "https://ackingliu.top/v1/chat/completions".to_string(),
            endpoint: "/v1/chat/completions".to_string(),
            model: Some("gpt-4.1".to_string()),
            mapped_model: Some("gpt-4.1".to_string()),
            status_code: 200,
            request_body_bytes: Some(256),
            quota_failover_count: 0,
            routing_diagnostics_json: None,
            input_uncached_tokens: 10,
            input_cached_tokens: 2,
            output_tokens: 5,
            billable_tokens: 15,
            credit_usage: Some("1.25".to_string()),
            usage_missing: false,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: None,
            client_request_body_json: None,
            upstream_request_body_json: None,
            full_request_json: None,
            error_message: None,
            error_body: None,
            timing: llm_access_core::usage::UsageTiming {
                latency_ms: Some(120),
                ..Default::default()
            },
            stream: llm_access_core::usage::UsageStreamDetails::default(),
        };
        repo.apply_usage_rollup(&event)
            .await
            .expect("apply usage rollup");

        let key = repo
            .get_public_usage_key_by_secret("secret")
            .await
            .expect("load usage lookup key")
            .expect("public usage lookup row");
        assert_eq!(key.usage_billable_tokens, 15);
        assert_eq!(key.usage_credit_total, 1.25);
        assert_eq!(key.usage_credit_missing_events, 0);
        assert_eq!(key.last_used_at_ms, Some(1_700_000_000_001));
    }

    #[tokio::test]
    async fn postgres_repository_creates_account_contribution_request() {
        let Ok(database_url) = std::env::var("TEST_POSTGRES_URL") else {
            eprintln!("skipping postgres integration test: TEST_POSTGRES_URL is not set");
            return;
        };
        let _guard = test_db_guard().await;
        reset_test_db(&database_url)
            .await
            .expect("reset postgres test database");
        let repo = super::PostgresControlRepository::connect(&database_url, None)
            .await
            .expect("connect postgres repository");

        repo.create_public_account_contribution_request(NewPublicAccountContributionRequest {
            request_id: "req-1".to_string(),
            account_name: "acct-1".to_string(),
            account_id: Some("acct-id-1".to_string()),
            id_token: "id-token".to_string(),
            access_token: "access-token".to_string(),
            refresh_token: "refresh-token".to_string(),
            requester_email: "user@example.com".to_string(),
            contributor_message: "hello".to_string(),
            github_id: None,
            frontend_page_url: None,
            show_on_public_wall: true,
            fingerprint: "fp".to_string(),
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            created_at_ms: 1_700_000_000_100,
        })
        .await
        .expect("create account contribution request");

        let created = repo
            .get_admin_account_contribution_request("req-1")
            .await
            .expect("load request")
            .expect("request row");
        assert_eq!(created.status, "pending");
        assert_eq!(created.account_name, "acct-1");
        assert_eq!(created.account_id.as_deref(), Some("acct-id-1"));
    }

    #[test]
    fn aggregate_usage_rollup_deltas_merges_events() {
        let events = vec![
            llm_access_core::usage::UsageEvent {
                event_id: "evt-1".to_string(),
                created_at_ms: 10,
                provider_type: ProviderType::Codex,
                protocol_family: ProtocolFamily::OpenAi,
                key_id: "key-1".to_string(),
                key_name: "external".to_string(),
                account_name: None,
                account_group_id_at_event: None,
                route_strategy_at_event: None,
                request_method: "POST".to_string(),
                request_url: "https://ackingliu.top/v1/chat/completions".to_string(),
                endpoint: "/v1/chat/completions".to_string(),
                model: None,
                mapped_model: None,
                status_code: 200,
                request_body_bytes: None,
                quota_failover_count: 0,
                routing_diagnostics_json: None,
                input_uncached_tokens: 10,
                input_cached_tokens: 1,
                output_tokens: 5,
                billable_tokens: 15,
                credit_usage: Some("1.25".to_string()),
                usage_missing: false,
                credit_usage_missing: false,
                client_ip: "127.0.0.1".to_string(),
                ip_region: "local".to_string(),
                request_headers_json: "{}".to_string(),
                last_message_content: None,
                client_request_body_json: None,
                upstream_request_body_json: None,
                full_request_json: None,
                error_message: None,
                error_body: None,
                timing: llm_access_core::usage::UsageTiming::default(),
                stream: llm_access_core::usage::UsageStreamDetails::default(),
            },
            llm_access_core::usage::UsageEvent {
                event_id: "evt-2".to_string(),
                created_at_ms: 25,
                provider_type: ProviderType::Codex,
                protocol_family: ProtocolFamily::OpenAi,
                key_id: "key-1".to_string(),
                key_name: "external".to_string(),
                account_name: None,
                account_group_id_at_event: None,
                route_strategy_at_event: None,
                request_method: "POST".to_string(),
                request_url: "https://ackingliu.top/v1/chat/completions".to_string(),
                endpoint: "/v1/chat/completions".to_string(),
                model: None,
                mapped_model: None,
                status_code: 200,
                request_body_bytes: None,
                quota_failover_count: 0,
                routing_diagnostics_json: None,
                input_uncached_tokens: 4,
                input_cached_tokens: 0,
                output_tokens: 1,
                billable_tokens: 5,
                credit_usage: Some("0.5".to_string()),
                usage_missing: false,
                credit_usage_missing: true,
                client_ip: "127.0.0.1".to_string(),
                ip_region: "local".to_string(),
                request_headers_json: "{}".to_string(),
                last_message_content: None,
                client_request_body_json: None,
                upstream_request_body_json: None,
                full_request_json: None,
                error_message: None,
                error_body: None,
                timing: llm_access_core::usage::UsageTiming::default(),
                stream: llm_access_core::usage::UsageStreamDetails::default(),
            },
        ];

        let deltas = super::aggregate_usage_rollup_deltas(&events).expect("aggregate usage deltas");
        assert_eq!(deltas.len(), 1);
        let (key_id, delta) = deltas[0];
        assert_eq!(key_id, "key-1");
        assert_eq!(delta.input_uncached_tokens, 14);
        assert_eq!(delta.input_cached_tokens, 1);
        assert_eq!(delta.output_tokens, 6);
        assert_eq!(delta.billable_tokens, 20);
        assert_eq!(delta.credit_total, 1.75);
        assert_eq!(delta.credit_missing_events, 1);
        assert_eq!(delta.last_used_at_ms, 25);
    }

    #[tokio::test]
    async fn postgres_repository_batches_key_usage_rollups() {
        let Ok(database_url) = std::env::var("TEST_POSTGRES_URL") else {
            eprintln!("skipping postgres integration test: TEST_POSTGRES_URL is not set");
            return;
        };
        let _guard = test_db_guard().await;
        reset_test_db(&database_url)
            .await
            .expect("reset postgres test database");
        seed_test_key_bundle(&database_url)
            .await
            .expect("seed postgres test key bundle");
        let repo = super::PostgresControlRepository::connect(&database_url, None)
            .await
            .expect("connect postgres repository");

        let first = llm_access_core::usage::UsageEvent {
            event_id: "evt-1".to_string(),
            created_at_ms: 1_700_000_000_001,
            provider_type: ProviderType::Codex,
            protocol_family: ProtocolFamily::OpenAi,
            key_id: "key-1".to_string(),
            key_name: "external".to_string(),
            account_name: Some("acct-1".to_string()),
            account_group_id_at_event: None,
            route_strategy_at_event: Some(RouteStrategy::Auto),
            request_method: "POST".to_string(),
            request_url: "https://ackingliu.top/v1/chat/completions".to_string(),
            endpoint: "/v1/chat/completions".to_string(),
            model: Some("gpt-4.1".to_string()),
            mapped_model: Some("gpt-4.1".to_string()),
            status_code: 200,
            request_body_bytes: Some(256),
            quota_failover_count: 0,
            routing_diagnostics_json: None,
            input_uncached_tokens: 10,
            input_cached_tokens: 2,
            output_tokens: 5,
            billable_tokens: 15,
            credit_usage: Some("1.25".to_string()),
            usage_missing: false,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: None,
            client_request_body_json: None,
            upstream_request_body_json: None,
            full_request_json: None,
            error_message: None,
            error_body: None,
            timing: llm_access_core::usage::UsageTiming {
                latency_ms: Some(120),
                ..Default::default()
            },
            stream: llm_access_core::usage::UsageStreamDetails::default(),
        };
        let second = llm_access_core::usage::UsageEvent {
            event_id: "evt-2".to_string(),
            created_at_ms: 1_700_000_000_101,
            input_uncached_tokens: 4,
            input_cached_tokens: 2,
            output_tokens: 1,
            billable_tokens: 5,
            credit_usage: Some("0.50".to_string()),
            ..first.clone()
        };

        repo.append_usage_events(&[first, second])
            .await
            .expect("append usage events");

        let key = repo
            .get_public_usage_key_by_secret("secret")
            .await
            .expect("load usage lookup key")
            .expect("public usage lookup row");
        assert_eq!(key.usage_input_uncached_tokens, 14);
        assert_eq!(key.usage_input_cached_tokens, 4);
        assert_eq!(key.usage_output_tokens, 6);
        assert_eq!(key.usage_billable_tokens, 20);
        assert_eq!(key.usage_credit_total, 1.75);
        assert_eq!(key.usage_credit_missing_events, 0);
        assert_eq!(key.last_used_at_ms, Some(1_700_000_000_101));
    }

    #[tokio::test]
    async fn postgres_repository_lists_kiro_key_pages_with_candidate_credit_summaries() {
        let Ok(database_url) = std::env::var("TEST_POSTGRES_URL") else {
            eprintln!("skipping postgres integration test: TEST_POSTGRES_URL is not set");
            return;
        };
        let _guard = test_db_guard().await;
        reset_test_db(&database_url)
            .await
            .expect("reset postgres test database");
        seed_test_kiro_key_page_fixture(&database_url)
            .await
            .expect("seed postgres kiro key page fixture");
        let repo = super::PostgresControlRepository::connect(&database_url, None)
            .await
            .expect("connect postgres repository");

        let first_page = repo
            .list_admin_keys_page(Some("kiro"), llm_access_core::store::AdminPageRequest {
                limit: 2,
                offset: 0,
            })
            .await
            .expect("list first kiro key page");
        assert_eq!(first_page.total, 3);
        assert!(first_page.has_more);
        assert_eq!(
            first_page
                .keys
                .iter()
                .map(|key| key.id.as_str())
                .collect::<Vec<_>>(),
            ["kiro-key-new", "kiro-key-mid"]
        );
        let newest_summary = first_page.keys[0]
            .kiro_candidate_credit_summary
            .expect("newest key candidate summary");
        assert_eq!(newest_summary.candidate_count, 4);
        assert_eq!(newest_summary.loaded_balance_count, 3);
        assert_eq!(newest_summary.missing_balance_count, 1);
        assert_eq!(newest_summary.total_limit, 600.0);
        assert_eq!(newest_summary.total_remaining, 280.0);
        let middle_summary = first_page.keys[1]
            .kiro_candidate_credit_summary
            .expect("middle key candidate summary");
        assert_eq!(middle_summary.candidate_count, 2);
        assert_eq!(middle_summary.loaded_balance_count, 1);
        assert_eq!(middle_summary.missing_balance_count, 1);
        assert_eq!(middle_summary.total_limit, 200.0);
        assert_eq!(middle_summary.total_remaining, 150.0);

        let second_page = repo
            .list_admin_keys_page(Some("kiro"), llm_access_core::store::AdminPageRequest {
                limit: 2,
                offset: 2,
            })
            .await
            .expect("list second kiro key page");
        assert_eq!(second_page.total, 3);
        assert!(!second_page.has_more);
        assert_eq!(second_page.keys.len(), 1);
        assert_eq!(second_page.keys[0].id, "kiro-key-old");
        let oldest_summary = second_page.keys[0]
            .kiro_candidate_credit_summary
            .expect("oldest key candidate summary");
        assert_eq!(oldest_summary.candidate_count, 2);
        assert_eq!(oldest_summary.loaded_balance_count, 2);
        assert_eq!(oldest_summary.missing_balance_count, 0);
        assert_eq!(oldest_summary.total_limit, 400.0);
        assert_eq!(oldest_summary.total_remaining, 130.0);
    }

    #[tokio::test]
    async fn postgres_repository_lists_filtered_codex_account_pages() {
        let Ok(database_url) = std::env::var("TEST_POSTGRES_URL") else {
            eprintln!("skipping postgres integration test: TEST_POSTGRES_URL is not set");
            return;
        };
        let _guard = test_db_guard().await;
        reset_test_db(&database_url)
            .await
            .expect("reset postgres test database");
        seed_test_codex_account_page_fixture(&database_url)
            .await
            .expect("seed postgres codex account page fixture");
        let repo = super::PostgresControlRepository::connect(&database_url, None)
            .await
            .expect("connect postgres repository");

        let primary_sorted = repo
            .list_admin_codex_accounts_filtered_page(
                &AdminCodexAccountPageQuery {
                    sort: AdminCodexAccountSortMode::PrimaryAsc,
                    ..AdminCodexAccountPageQuery::default()
                },
                llm_access_core::store::AdminPageRequest {
                    limit: 2,
                    offset: 0,
                },
            )
            .await
            .expect("list codex accounts sorted by primary remaining");
        assert_eq!(primary_sorted.total, 3);
        assert!(primary_sorted.has_more);
        assert_eq!(
            primary_sorted
                .accounts
                .iter()
                .map(|account| account.name.as_str())
                .collect::<Vec<_>>(),
            ["codex-old", "codex-new"]
        );
        assert_eq!(primary_sorted.accounts[0].plan_type.as_deref(), Some("Plus"));
        assert_eq!(primary_sorted.accounts[0].primary_remaining_percent, Some(20.0));

        let unhealthy_only = repo
            .list_admin_codex_accounts_filtered_page(
                &AdminCodexAccountPageQuery {
                    unhealthy_only: true,
                    ..AdminCodexAccountPageQuery::default()
                },
                llm_access_core::store::AdminPageRequest {
                    limit: 8,
                    offset: 0,
                },
            )
            .await
            .expect("list unhealthy codex accounts");
        assert_eq!(unhealthy_only.total, 2);
        assert_eq!(
            unhealthy_only
                .accounts
                .iter()
                .map(|account| account.name.as_str())
                .collect::<Vec<_>>(),
            ["codex-mid", "codex-old"]
        );

        let searched = repo
            .list_admin_codex_accounts_filtered_page(
                &AdminCodexAccountPageQuery {
                    search: Some("plus".to_string()),
                    ..AdminCodexAccountPageQuery::default()
                },
                llm_access_core::store::AdminPageRequest {
                    limit: 8,
                    offset: 0,
                },
            )
            .await
            .expect("search codex accounts by plan type");
        assert_eq!(searched.total, 1);
        assert_eq!(searched.accounts[0].name, "codex-old");
    }
}
