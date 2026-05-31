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
    provider::RouteStrategy,
    store::{
        self as core_store, default_proxy_bindings, AdminAccountContributionRequest,
        AdminAccountContributionRequestsPage, AdminAccountGroup, AdminAccountGroupOption,
        AdminAccountGroupPatch, AdminAccountGroupStore, AdminCodexAccount,
        AdminCodexAccountPageQuery, AdminCodexAccountPatch, AdminCodexAccountSortMode,
        AdminCodexAccountStore, AdminCodexAccountsPage, AdminCodexImportJobDetail,
        AdminCodexImportJobItem, AdminCodexImportJobItemResult, AdminCodexImportJobSummary,
        AdminConfigStore, AdminKey, AdminKeyPageQuery, AdminKeyPatch, AdminKeySortMode,
        AdminKeyStore, AdminKeysPage, AdminKiroAccount, AdminKiroAccountPatch,
        AdminKiroAccountStore, AdminKiroAccountsPage, AdminKiroBalanceView, AdminKiroCacheView,
        AdminKiroStatusCacheUpdate, AdminLegacyKiroProxyMigration, AdminPageRequest,
        AdminProxyBinding, AdminProxyConfig, AdminProxyConfigPatch, AdminProxyEndpointCheck,
        AdminProxyEndpointCheckUpdate, AdminProxyStore, AdminReviewQueueAction,
        AdminReviewQueueQuery, AdminReviewQueueStore, AdminRuntimeConfig, AdminSponsorRequest,
        AdminSponsorRequestsPage, AdminTokenRequest, AdminTokenRequestsPage, AuthenticatedKey,
        CodexRateLimitStatus, CodexStatusRefreshTarget, ControlStore, KiroStatusRefreshTarget,
        NewAdminAccountGroup, NewAdminCodexAccount, NewAdminCodexImportJob, NewAdminKey,
        NewAdminKiroAccount, NewAdminProxyConfig, NewPublicAccountContributionRequest,
        NewPublicSponsorRequest, NewPublicTokenRequest, ProviderCodexAuthUpdate,
        ProviderCodexRoute, ProviderKiroAuthUpdate, ProviderKiroRoute, ProviderRouteStore,
        PublicAccessKey, PublicAccessStore, PublicAccountContribution, PublicCommunityStore,
        PublicSponsor, PublicStatusStore, PublicSubmissionStore, PublicUsageLookupKey,
        PublicUsageStore, UsageEventSink, DEFAULT_AUTH_CACHE_TTL_SECONDS,
        DEFAULT_CODEX_STATUS_REFRESH_SECONDS, PUBLIC_ACCOUNT_CONTRIBUTION_STATUS_VALIDATED,
        PUBLIC_SPONSOR_REQUEST_STATUS_PAYMENT_EMAIL_SENT, PUBLIC_SPONSOR_REQUEST_STATUS_SUBMITTED,
        PUBLIC_TOKEN_REQUEST_STATUS_PENDING,
    },
    usage::UsageEvent,
};
use sha2::{Digest, Sha256};
use sqlx_core::{
    arguments::Arguments, column::ColumnIndex, decode::Decode, encode::Encode, query::query_with,
    query_builder::QueryBuilder, row::Row as SqlxRowTrait, types::Type,
};
use sqlx_postgres::{PgArguments, PgPool, PgPoolOptions, PgRow as SqlxPgRow, Postgres};
use tokio::sync::{Mutex, RwLock};

use crate::{
    records::{
        CodexAccountRecord, KeyBundle, KeyRecord, KeyRouteConfig, KeyUsageRollup,
        KiroAccountRecord, RuntimeConfigRecord,
    },
    request_cache::{RequestCache, RequestCacheConfig},
};

mod cache_convert;
mod codex_routing;
mod decode;
mod json;
mod proxy_support;

use cache_convert::{
    authenticated_key_from_cached, build_cached_kiro_account_view,
    cached_authenticated_key_from_bundle, cached_authenticated_key_from_value,
    cached_proxy_from_option, effective_kiro_cache_policy_json, proxy_from_cached_option,
};
use codex_routing::{
    codex_cached_error_message, minimal_codex_auth_json_for_access_token,
    sort_codex_routes_by_cached_quota,
};
use decode::{
    admin_key_from_bundle, decode_admin_account_contribution_request_row,
    decode_admin_account_group_row, decode_admin_proxy_config_row,
    decode_admin_sponsor_request_row, decode_admin_token_request_row, decode_codex_account_row,
    decode_codex_account_settings, decode_codex_admin_account_list_row,
    decode_codex_import_job_item_row, decode_codex_import_job_summary_row, decode_key_bundle_row,
    decode_kiro_account_row, decode_kiro_admin_account_list_row, decode_kiro_admin_key_row,
    decode_proxy_endpoint_check_row, decode_public_usage_lookup_row, decode_runtime_config_row,
};
use json::{
    decode_optional_json, non_negative_i64_to_u64, optional_json_bool_any, optional_json_f64_any,
    optional_json_i64_any, optional_json_string, optional_json_string_any, optional_json_u64_any,
    set_json_optional_bool, set_json_optional_f64, set_json_optional_string, set_json_optional_u64,
};
use proxy_support::{
    apply_proxy_config_node_override, apply_proxy_endpoint_checks, clear_legacy_kiro_proxy_json,
    legacy_proxy_json_string, resolve_provider_proxy_config_from_context,
};

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

    async fn load_runtime_config_record(&self) -> anyhow::Result<Option<RuntimeConfigRecord>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT
                    id,
                    auth_cache_ttl_seconds,
                    max_request_body_bytes,
                    account_failure_retry_limit,
                    codex_client_version,
                    kiro_channel_max_concurrency,
                    kiro_channel_min_start_interval_ms,
                    codex_status_refresh_min_interval_seconds,
                    codex_status_refresh_max_interval_seconds,
                    codex_status_account_jitter_max_seconds,
                    codex_weight_free,
                    codex_weight_plus,
                    codex_weight_pro5x,
                    codex_weight_pro20x,
                    kiro_status_refresh_min_interval_seconds,
                    kiro_status_refresh_max_interval_seconds,
                    kiro_status_account_jitter_max_seconds,
                    usage_event_flush_batch_size,
                    usage_event_flush_interval_seconds,
                    usage_event_flush_max_buffer_bytes,
                    duckdb_usage_memory_limit_mib,
                    duckdb_usage_checkpoint_threshold_mib,
                    usage_analytics_retention_days,
                    usage_journal_enabled,
                    usage_journal_max_file_bytes,
                    usage_journal_max_file_age_ms,
                    usage_journal_max_files,
                    usage_journal_block_target_uncompressed_bytes,
                    usage_journal_block_max_events,
                    usage_journal_fsync_interval_ms,
                    usage_journal_zstd_level,
                    usage_journal_consumer_lease_ms,
                    usage_journal_delete_bad_files,
                    usage_query_bind_addr,
                    usage_query_base_url,
                    usage_event_maintenance_enabled,
                    usage_event_maintenance_interval_seconds,
                    usage_event_detail_retention_days,
                    kiro_cache_kmodels_json::text AS kiro_cache_kmodels_json,
                    kiro_billable_model_multipliers_json::text
                        AS kiro_billable_model_multipliers_json,
                    kiro_cache_policy_json::text AS kiro_cache_policy_json,
                    kiro_context_usage_min_request_tokens,
                    kiro_prefix_cache_mode,
                    kiro_prefix_cache_max_tokens,
                    kiro_prefix_cache_entry_ttl_seconds,
                    kiro_conversation_anchor_max_entries,
                    kiro_conversation_anchor_ttl_seconds,
                    updated_at_ms
                 FROM llm_runtime_config
                 WHERE id = 'default'",
                &[],
            )
            .await
            .context("load runtime config")?;
        row.map(decode_runtime_config_row).transpose()
    }

    async fn load_runtime_config_record_cached(
        &self,
    ) -> anyhow::Result<Option<RuntimeConfigRecord>> {
        let Some(cache) = self.request_cache.as_ref() else {
            return self.load_runtime_config_record().await;
        };
        let cache_key = cache.runtime_config_key();
        match cache
            .get_json::<crate::request_cache::CachedRuntimeConfigLookup>(&cache_key)
            .await
        {
            Ok(Some(lookup)) => return Ok(lookup.record),
            Ok(None) => {},
            Err(err) => {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache runtime-config read failed; falling back to postgres"
                );
            },
        }
        let record = self.load_runtime_config_record().await?;
        let lookup = crate::request_cache::CachedRuntimeConfigLookup {
            record: record.clone(),
        };
        if let Err(err) = cache
            .set_json(&cache_key, &lookup, cache.runtime_config_ttl())
            .await
        {
            tracing::warn!(
                key = %cache_key,
                error = %err,
                "request cache runtime-config write failed"
            );
        }
        Ok(record)
    }

    async fn store_runtime_config_record_cached(&self, record: Option<&RuntimeConfigRecord>) {
        let Some(cache) = self.request_cache.as_ref() else {
            return;
        };
        let cache_key = cache.runtime_config_key();
        let lookup = crate::request_cache::CachedRuntimeConfigLookup {
            record: record.cloned(),
        };
        if let Err(err) = cache
            .set_json(&cache_key, &lookup, cache.runtime_config_ttl())
            .await
        {
            tracing::warn!(
                key = %cache_key,
                error = %err,
                "request cache runtime-config write-through failed"
            );
        }
    }

    async fn load_admin_proxy_configs_cached(&self) -> anyhow::Result<Vec<AdminProxyConfig>> {
        let Some(cache) = self.request_cache.as_ref() else {
            return self.list_admin_proxy_configs_rows().await;
        };
        let generation = self.current_proxy_metadata_generation().await;
        let scope = self.proxy_scope.cache_key_segment();
        let cache_key = cache.proxy_configs_key(scope);
        match cache
            .get_json::<crate::request_cache::CachedProxyConfigsLookup>(&cache_key)
            .await
        {
            Ok(Some(lookup)) if lookup.generation == generation => return Ok(lookup.configs),
            Ok(Some(_)) => {},
            Ok(None) => {},
            Err(err) => {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache proxy-configs read failed; falling back to postgres"
                );
            },
        }
        let configs = self.list_admin_proxy_configs_rows().await?;
        let lookup = crate::request_cache::CachedProxyConfigsLookup {
            generation,
            configs: configs.clone(),
        };
        if let Err(err) = cache
            .set_json(&cache_key, &lookup, cache.proxy_configs_ttl(scope))
            .await
        {
            tracing::warn!(
                key = %cache_key,
                error = %err,
                "request cache proxy-configs write failed"
            );
        }
        Ok(configs)
    }

    async fn load_admin_proxy_binding_cached(
        &self,
        provider_type: &str,
    ) -> anyhow::Result<AdminProxyBinding> {
        let Some(cache) = self.request_cache.as_ref() else {
            return self.load_admin_proxy_binding_row(provider_type).await;
        };
        let generation = self.current_dispatch_generation(provider_type).await;
        let scope = self.proxy_scope.cache_key_segment();
        let cache_key = cache.proxy_binding_key(provider_type, scope);
        match cache
            .get_json::<crate::request_cache::CachedProxyBindingLookup>(&cache_key)
            .await
        {
            Ok(Some(lookup)) if lookup.generation == generation => return Ok(lookup.binding),
            Ok(Some(_)) => {},
            Ok(None) => {},
            Err(err) => {
                tracing::warn!(
                    provider = provider_type,
                    key = %cache_key,
                    error = %err,
                    "request cache proxy-binding read failed; falling back to postgres"
                );
            },
        }
        let proxy_configs_by_id = self
            .load_admin_proxy_configs_cached()
            .await?
            .into_iter()
            .map(|proxy| (proxy.id.clone(), proxy))
            .collect::<BTreeMap<_, _>>();
        let binding = self
            .load_admin_proxy_binding_from_configs(provider_type, &proxy_configs_by_id)
            .await?;
        let lookup = crate::request_cache::CachedProxyBindingLookup {
            generation,
            binding: binding.clone(),
        };
        if let Err(err) = cache
            .set_json(&cache_key, &lookup, cache.proxy_binding_ttl(provider_type, scope))
            .await
        {
            tracing::warn!(
                provider = provider_type,
                key = %cache_key,
                error = %err,
                "request cache proxy-binding write failed"
            );
        }
        Ok(binding)
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

    async fn load_authenticated_key_by_hash(
        &self,
        key_hash: &str,
    ) -> anyhow::Result<Option<AuthenticatedKey>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT
                    k.key_id,
                    k.name,
                    k.provider_type,
                    k.protocol_family,
                    k.status,
                    k.quota_billable_limit,
                    COALESCE(u.billable_tokens, 0)
                 FROM llm_keys k
                 LEFT JOIN llm_key_usage_rollups u ON u.key_id = k.key_id
                 WHERE k.key_hash = $1",
                &[&key_hash],
            )
            .await
            .context("load authenticated key by hash")?;
        Ok(row.map(|row| AuthenticatedKey {
            key_id: row.get(0),
            key_name: row.get(1),
            provider_type: row.get(2),
            protocol_family: row.get(3),
            status: row.get(4),
            quota_billable_limit: row.get(5),
            billable_tokens_used: row.get::<_, i64>(6),
        }))
    }

    async fn load_key_hashes_by_ids(
        &self,
        key_ids: &[String],
    ) -> anyhow::Result<BTreeMap<String, String>> {
        if key_ids.is_empty() {
            return Ok(BTreeMap::new());
        }
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT key_id, key_hash
                 FROM llm_keys
                 WHERE key_id = ANY($1)",
                &[&key_ids.to_vec()],
            )
            .await
            .context("load key hashes by ids")?;
        Ok(rows
            .into_iter()
            .map(|row| (row.get::<_, String>(0), row.get::<_, String>(1)))
            .collect())
    }

    fn ensure_connection_alive(&self) -> anyhow::Result<()> {
        if self.client.is_closed() {
            anyhow::bail!("sqlx postgres control pool is closed");
        }
        Ok(())
    }

    async fn cached_codex_rate_limit_status(&self) -> Option<CodexRateLimitStatus> {
        let guard = self.codex_status_cache.read().await;
        let cached = guard.as_ref()?;
        if cached.loaded_at.elapsed() > CODEX_STATUS_CACHE_TTL {
            return None;
        }
        Some(cached.snapshot.clone())
    }

    async fn store_cached_codex_rate_limit_status(&self, snapshot: Option<CodexRateLimitStatus>) {
        let mut guard = self.codex_status_cache.write().await;
        *guard = snapshot.map(|snapshot| CachedCodexRateLimitStatus {
            snapshot,
            loaded_at: Instant::now(),
        });
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

    /// Resolve the effective proxy attribution for one consumed usage event.
    pub async fn resolve_usage_proxy_attribution(
        &self,
        provider: &str,
        account_name: &str,
    ) -> anyhow::Result<Option<UsageProxyAttribution>> {
        let provider = provider.trim();
        let account_name = account_name.trim();
        if provider.is_empty() || account_name.is_empty() {
            return Ok(None);
        }
        let Some(cache) = self.request_cache.as_ref() else {
            return self
                .build_usage_proxy_attribution(provider, account_name)
                .await;
        };
        let generation = self.current_dispatch_generation(provider).await;
        let scope = self.proxy_scope.cache_key_segment();
        let cache_key = cache.usage_proxy_attribution_key(provider, account_name, scope);
        match cache
            .get_json::<crate::request_cache::CachedUsageProxyAttributionLookup>(&cache_key)
            .await
        {
            Ok(Some(lookup)) if lookup.generation == generation => {
                return Ok(lookup.attribution.map(|value| UsageProxyAttribution {
                    provider_type: value.provider_type,
                    account_name: value.account_name,
                    proxy_source: value.proxy_source,
                    proxy_config_id: value.proxy_config_id,
                    proxy_config_name: value.proxy_config_name,
                    proxy_url: value.proxy_url,
                }));
            },
            Ok(_) => {},
            Err(err) => tracing::warn!(
                provider,
                account_name,
                key = %cache_key,
                error = %err,
                "request cache usage proxy attribution read failed; falling back to postgres"
            ),
        }
        let attribution = self
            .build_usage_proxy_attribution(provider, account_name)
            .await?;
        let lookup = crate::request_cache::CachedUsageProxyAttributionLookup {
            generation,
            attribution: attribution.clone().map(|value| {
                crate::request_cache::CachedUsageProxyAttributionView {
                    provider_type: value.provider_type,
                    account_name: value.account_name,
                    proxy_source: value.proxy_source,
                    proxy_config_id: value.proxy_config_id,
                    proxy_config_name: value.proxy_config_name,
                    proxy_url: value.proxy_url,
                }
            }),
        };
        if let Err(err) = cache
            .set_json(
                &cache_key,
                &lookup,
                cache.usage_proxy_attribution_ttl(provider, account_name, scope),
            )
            .await
        {
            tracing::warn!(
                provider,
                account_name,
                key = %cache_key,
                error = %err,
                "request cache usage proxy attribution write failed"
            );
        }
        Ok(attribution)
    }

    async fn build_usage_proxy_attribution(
        &self,
        provider: &str,
        account_name: &str,
    ) -> anyhow::Result<Option<UsageProxyAttribution>> {
        match provider {
            core_store::PROVIDER_CODEX => {
                self.build_codex_usage_proxy_attribution(account_name).await
            },
            core_store::PROVIDER_KIRO => {
                self.build_kiro_usage_proxy_attribution(account_name).await
            },
            _ => Ok(None),
        }
    }

    async fn build_codex_usage_proxy_attribution(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<UsageProxyAttribution>> {
        let Some(record) = self.get_codex_account_row(account_name).await? else {
            return Ok(None);
        };
        let settings = decode_codex_account_settings(&record.settings_json)?;
        let context = self.load_codex_admin_account_view_context().await?;
        let (proxy_source, proxy_url, proxy_config_name) =
            self.resolve_codex_account_proxy_view_with_context(&settings, &context);
        let proxy_config_id = match settings.proxy_mode.as_str() {
            "fixed" => settings.proxy_config_id.clone(),
            _ => context.codex_proxy_binding.bound_proxy_config_id.clone(),
        };
        Ok(Some(UsageProxyAttribution {
            provider_type: core_store::PROVIDER_CODEX.to_string(),
            account_name: record.account_name,
            proxy_source,
            proxy_config_id,
            proxy_config_name,
            proxy_url,
        }))
    }

    async fn build_kiro_usage_proxy_attribution(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<UsageProxyAttribution>> {
        let Some(record) = self.get_kiro_account_row(account_name).await? else {
            return Ok(None);
        };
        let auth_json = serde_json::from_str::<serde_json::Value>(&record.auth_json)
            .context("parse kiro account auth json for usage proxy attribution")?;
        let proxy_mode = optional_json_string_any(&auth_json, &["proxyMode", "proxy_mode"])
            .unwrap_or_else(|| {
                if record.proxy_config_id.is_some() {
                    "fixed".to_string()
                } else {
                    "inherit".to_string()
                }
            });
        let proxy_config_id = record.proxy_config_id.clone().or_else(|| {
            optional_json_string_any(&auth_json, &["proxyConfigId", "proxy_config_id"])
        });
        let context = self.load_kiro_admin_account_view_context().await?;
        let (proxy_source, proxy_url, proxy_config_name) = self
            .resolve_kiro_account_proxy_view_with_context(
                &proxy_mode,
                proxy_config_id.as_deref(),
                &context,
            );
        let proxy_config_id = match proxy_mode.as_str() {
            "fixed" => proxy_config_id,
            _ => context.kiro_proxy_binding.bound_proxy_config_id.clone(),
        };
        Ok(Some(UsageProxyAttribution {
            provider_type: core_store::PROVIDER_KIRO.to_string(),
            account_name: record.account_name,
            proxy_source,
            proxy_config_id,
            proxy_config_name,
            proxy_url,
        }))
    }

    async fn load_authenticated_key_cached(
        &self,
        key_hash: &str,
    ) -> anyhow::Result<Option<AuthenticatedKey>> {
        let Some(cache) = self.request_cache.as_ref() else {
            return self.load_authenticated_key_by_hash(key_hash).await;
        };
        let cache_key = cache.auth_key(key_hash);
        match cache
            .get_json::<crate::request_cache::CachedAuthLookup>(&cache_key)
            .await
        {
            Ok(Some(lookup)) => return Ok(lookup.key.map(authenticated_key_from_cached)),
            Ok(None) => {},
            Err(err) => {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache auth read failed; falling back to postgres"
                );
            },
        }
        let key = self.load_authenticated_key_by_hash(key_hash).await?;
        let lookup = crate::request_cache::CachedAuthLookup {
            key: key
                .clone()
                .map(|value| cached_authenticated_key_from_value(&value)),
        };
        let ttl = if key.is_some() {
            cache.auth_ttl(key_hash)
        } else {
            cache.negative_auth_ttl(key_hash)
        };
        if let Err(err) = cache.set_json(&cache_key, &lookup, ttl).await {
            tracing::warn!(
                key = %cache_key,
                error = %err,
                "request cache auth write failed"
            );
        }
        Ok(key)
    }

    async fn invalidate_authenticated_key_cache_by_ids(&self, key_ids: &[String]) {
        let Some(cache) = self.request_cache.as_ref() else {
            return;
        };
        let key_hashes = match self.load_key_hashes_by_ids(key_ids).await {
            Ok(value) => value,
            Err(err) => {
                tracing::warn!(error = %err, "failed to load key hashes for auth-cache invalidation");
                return;
            },
        };
        let cache_keys = key_hashes
            .values()
            .map(|key_hash| cache.auth_key(key_hash))
            .collect::<Vec<_>>();
        let cache_key_refs = cache_keys.iter().map(String::as_str).collect::<Vec<_>>();
        if let Err(err) = cache.delete_many(cache_key_refs).await {
            tracing::warn!(error = %err, "failed to invalidate auth cache keys");
        }
    }

    async fn invalidate_request_snapshot_cache(&self, provider: &str, key_id: &str) {
        let Some(cache) = self.request_cache.as_ref() else {
            return;
        };
        let cache_key = cache.request_snapshot_key(provider, key_id);
        if let Err(err) = cache.delete(&cache_key).await {
            tracing::warn!(
                provider,
                key = %cache_key,
                error = %err,
                "failed to invalidate request snapshot cache"
            );
        }
    }

    async fn invalidate_account_cache(&self, provider: &str, account_name: &str) {
        let Some(cache) = self.request_cache.as_ref() else {
            return;
        };
        let scope = self.proxy_scope.cache_key_segment();
        let view_key = cache.account_view_key(provider, account_name, scope);
        let auth_key = cache.account_auth_key(provider, account_name);
        if let Err(err) = cache
            .delete_many([view_key.as_str(), auth_key.as_str()])
            .await
        {
            tracing::warn!(
                provider,
                account_name,
                error = %err,
                "failed to invalidate account cache entries"
            );
        }
    }

    async fn invalidate_all_account_views_for_provider(&self, provider: &str) {
        let Some(cache) = self.request_cache.as_ref() else {
            return;
        };
        let account_names = match provider {
            core_store::PROVIDER_CODEX => match self.list_codex_route_candidate_rows().await {
                Ok(rows) => rows
                    .into_iter()
                    .map(|row| row.account_name)
                    .collect::<Vec<_>>(),
                Err(err) => {
                    tracing::warn!(provider, error = %err, "failed to list codex accounts for cache invalidation");
                    return;
                },
            },
            core_store::PROVIDER_KIRO => match self.list_kiro_route_candidate_rows().await {
                Ok(rows) => rows
                    .into_iter()
                    .map(|row| row.account_name)
                    .collect::<Vec<_>>(),
                Err(err) => {
                    tracing::warn!(provider, error = %err, "failed to list kiro accounts for cache invalidation");
                    return;
                },
            },
            _ => return,
        };
        let scope = self.proxy_scope.cache_key_segment();
        let view_keys = account_names
            .iter()
            .map(|name| cache.account_view_key(provider, name, scope))
            .collect::<Vec<_>>();
        let view_key_refs = view_keys.iter().map(String::as_str).collect::<Vec<_>>();
        if let Err(err) = cache.delete_many(view_key_refs).await {
            tracing::warn!(provider, error = %err, "failed to invalidate provider account view cache");
        }
    }

    async fn load_codex_request_snapshot_cached(
        &self,
        key_id: &str,
    ) -> anyhow::Result<Option<crate::request_cache::CachedCodexRequestSnapshot>> {
        let Some(cache) = self.request_cache.as_ref() else {
            return self.build_codex_request_snapshot(key_id, 0).await;
        };
        let generation = self
            .current_dispatch_generation(core_store::PROVIDER_CODEX)
            .await;
        let cache_key = cache.request_snapshot_key(core_store::PROVIDER_CODEX, key_id);
        match cache
            .get_json::<crate::request_cache::CachedCodexRequestSnapshot>(&cache_key)
            .await
        {
            Ok(Some(snapshot)) if snapshot.generation == generation => return Ok(Some(snapshot)),
            Ok(_) => {},
            Err(err) => {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache codex snapshot read failed; rebuilding from postgres"
                );
            },
        }
        let snapshot = self
            .build_codex_request_snapshot(key_id, generation)
            .await?;
        if let Some(snapshot_ref) = snapshot.as_ref() {
            if let Err(err) = cache
                .set_json(
                    &cache_key,
                    snapshot_ref,
                    cache.request_snapshot_ttl(core_store::PROVIDER_CODEX, key_id),
                )
                .await
            {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache codex snapshot write failed"
                );
            }
        }
        Ok(snapshot)
    }

    async fn load_kiro_request_snapshot_cached(
        &self,
        key_id: &str,
    ) -> anyhow::Result<Option<crate::request_cache::CachedKiroRequestSnapshot>> {
        let Some(cache) = self.request_cache.as_ref() else {
            return self.build_kiro_request_snapshot(key_id, 0).await;
        };
        let generation = self
            .current_dispatch_generation(core_store::PROVIDER_KIRO)
            .await;
        let cache_key = cache.request_snapshot_key(core_store::PROVIDER_KIRO, key_id);
        match cache
            .get_json::<crate::request_cache::CachedKiroRequestSnapshot>(&cache_key)
            .await
        {
            Ok(Some(snapshot)) if snapshot.generation == generation => return Ok(Some(snapshot)),
            Ok(_) => {},
            Err(err) => {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache kiro snapshot read failed; rebuilding from postgres"
                );
            },
        }
        let snapshot = self.build_kiro_request_snapshot(key_id, generation).await?;
        if let Some(snapshot_ref) = snapshot.as_ref() {
            if let Err(err) = cache
                .set_json(
                    &cache_key,
                    snapshot_ref,
                    cache.request_snapshot_ttl(core_store::PROVIDER_KIRO, key_id),
                )
                .await
            {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache kiro snapshot write failed"
                );
            }
        }
        Ok(snapshot)
    }

    async fn build_codex_request_snapshot(
        &self,
        key_id: &str,
        generation: i64,
    ) -> anyhow::Result<Option<crate::request_cache::CachedCodexRequestSnapshot>> {
        let Some(bundle) = self.load_key_bundle_by_id(key_id).await? else {
            return Ok(None);
        };
        if bundle.key.provider_type != core_store::PROVIDER_CODEX {
            return Ok(None);
        }
        let runtime_config = self
            .load_runtime_config_record_cached()
            .await?
            .unwrap_or_default();
        let records = self.list_codex_route_candidate_rows().await?;
        let selected_account_names = self
            .resolve_route_account_names(
                core_store::PROVIDER_CODEX,
                &bundle.route,
                records
                    .iter()
                    .filter(|record| record.status == core_store::KEY_STATUS_ACTIVE)
                    .map(|record| record.account_name.clone())
                    .collect(),
            )
            .await?;
        Ok(Some(crate::request_cache::CachedCodexRequestSnapshot {
            key: cached_authenticated_key_from_bundle(&bundle),
            generation,
            route_strategy: bundle
                .route
                .route_strategy
                .clone()
                .unwrap_or_else(|| "auto".to_string()),
            account_group_id_at_event: bundle.route.account_group_id.clone(),
            selected_account_names,
            use_all_active_accounts: false,
            request_max_concurrency: bundle
                .route
                .request_max_concurrency
                .and_then(non_negative_i64_to_u64),
            request_min_start_interval_ms: bundle
                .route
                .request_min_start_interval_ms
                .and_then(non_negative_i64_to_u64),
            codex_fast_enabled: bundle.route.codex_fast_enabled,
            codex_weight_free: runtime_config.codex_weight_free,
            codex_weight_plus: runtime_config.codex_weight_plus,
            codex_weight_pro5x: runtime_config.codex_weight_pro5x,
            codex_weight_pro20x: runtime_config.codex_weight_pro20x,
        }))
    }

    async fn build_kiro_request_snapshot(
        &self,
        key_id: &str,
        generation: i64,
    ) -> anyhow::Result<Option<crate::request_cache::CachedKiroRequestSnapshot>> {
        let Some(bundle) = self.load_key_bundle_by_id(key_id).await? else {
            return Ok(None);
        };
        if bundle.key.provider_type != core_store::PROVIDER_KIRO {
            return Ok(None);
        }
        let runtime_config = self
            .load_runtime_config_record_cached()
            .await?
            .unwrap_or_default();
        let records = self.list_kiro_route_candidate_rows().await?;
        let selected_account_names = self
            .resolve_route_account_names(
                core_store::PROVIDER_KIRO,
                &bundle.route,
                records
                    .iter()
                    .filter(|record| record.status == core_store::KEY_STATUS_ACTIVE)
                    .map(|record| record.account_name.clone())
                    .collect(),
            )
            .await?;
        let cache_policy_json = effective_kiro_cache_policy_json(
            &runtime_config.kiro_cache_policy_json,
            bundle.route.kiro_cache_policy_override_json.as_deref(),
        )?;
        Ok(Some(crate::request_cache::CachedKiroRequestSnapshot {
            key: cached_authenticated_key_from_bundle(&bundle),
            generation,
            route_strategy: bundle
                .route
                .route_strategy
                .clone()
                .unwrap_or_else(|| "auto".to_string()),
            account_group_id_at_event: bundle.route.account_group_id.clone(),
            selected_account_names,
            use_all_active_accounts: false,
            request_max_concurrency: bundle
                .route
                .request_max_concurrency
                .and_then(non_negative_i64_to_u64),
            request_min_start_interval_ms: bundle
                .route
                .request_min_start_interval_ms
                .and_then(non_negative_i64_to_u64),
            request_validation_enabled: bundle.route.kiro_request_validation_enabled,
            cache_estimation_enabled: bundle.route.kiro_cache_estimation_enabled,
            zero_cache_debug_enabled: bundle.route.kiro_zero_cache_debug_enabled,
            full_request_logging_enabled: bundle.route.kiro_full_request_logging_enabled,
            remote_media_resolution_enabled: bundle.route.kiro_remote_media_resolution_enabled,
            latency_routing_enabled: bundle.route.kiro_latency_routing_enabled,
            model_name_map_json: bundle
                .route
                .model_name_map_json
                .clone()
                .unwrap_or_else(|| "{}".to_string()),
            cache_kmodels_json: runtime_config.kiro_cache_kmodels_json.clone(),
            cache_policy_json,
            context_usage_min_request_tokens: runtime_config
                .kiro_context_usage_min_request_tokens
                .max(0) as u64,
            prefix_cache_mode: runtime_config.kiro_prefix_cache_mode.clone(),
            prefix_cache_max_tokens: runtime_config.kiro_prefix_cache_max_tokens.max(0) as u64,
            prefix_cache_entry_ttl_seconds: runtime_config
                .kiro_prefix_cache_entry_ttl_seconds
                .max(0) as u64,
            conversation_anchor_max_entries: runtime_config
                .kiro_conversation_anchor_max_entries
                .max(0) as u64,
            conversation_anchor_ttl_seconds: runtime_config
                .kiro_conversation_anchor_ttl_seconds
                .max(0) as u64,
            billable_model_multipliers_json: bundle
                .route
                .kiro_billable_model_multipliers_override_json
                .clone()
                .unwrap_or_else(|| runtime_config.kiro_billable_model_multipliers_json.clone()),
            status_refresh_interval_seconds: runtime_config
                .kiro_status_refresh_max_interval_seconds
                .max(0) as u64,
        }))
    }

    async fn load_codex_account_views_cached(
        &self,
        account_names: &[String],
    ) -> anyhow::Result<BTreeMap<String, crate::request_cache::CachedCodexAccountView>> {
        if account_names.is_empty() {
            return Ok(BTreeMap::new());
        }
        let Some(cache) = self.request_cache.as_ref() else {
            let proxy_context = self
                .load_provider_proxy_resolution_context(core_store::PROVIDER_CODEX)
                .await?;
            return self
                .list_codex_route_candidate_rows_by_names(account_names)
                .await?
                .into_iter()
                .map(|row| {
                    let settings = decode_codex_account_settings(&row.settings_json)?;
                    let proxy = resolve_provider_proxy_config_from_context(
                        &settings.proxy_mode,
                        settings.proxy_config_id.as_deref(),
                        &proxy_context,
                    )?;
                    Ok((row.account_name.clone(), crate::request_cache::CachedCodexAccountView {
                        account_name: row.account_name,
                        generation: 0,
                        status: row.status,
                        map_gpt53_codex_to_spark: settings.map_gpt53_codex_to_spark,
                        auth_refresh_enabled: settings.auth_refresh_enabled,
                        route_weight_tier: settings.route_weight_tier,
                        request_max_concurrency: settings.request_max_concurrency,
                        request_min_start_interval_ms: settings.request_min_start_interval_ms,
                        last_refresh_at_ms: row.last_refresh_at_ms,
                        last_error: row.last_error,
                        access_token: row.access_token,
                        proxy: cached_proxy_from_option(proxy),
                    }))
                })
                .collect();
        };

        let generation = self
            .current_dispatch_generation(core_store::PROVIDER_CODEX)
            .await;
        let scope = self.proxy_scope.cache_key_segment();
        let cache_keys = account_names
            .iter()
            .map(|name| cache.account_view_key(core_store::PROVIDER_CODEX, name, scope))
            .collect::<Vec<_>>();
        let cached_values = match cache
            .mget_json::<crate::request_cache::CachedCodexAccountView>(&cache_keys)
            .await
        {
            Ok(values) => values,
            Err(err) => {
                tracing::warn!(error = %err, "request cache codex account view batch read failed");
                vec![None; account_names.len()]
            },
        };
        let mut views_by_name = BTreeMap::new();
        let mut missing = Vec::new();
        for (account_name, cached) in account_names.iter().cloned().zip(cached_values.into_iter()) {
            if let Some(view) = cached.filter(|view| view.generation == generation) {
                views_by_name.insert(account_name, view);
            } else {
                missing.push(account_name);
            }
        }
        if missing.is_empty() {
            return Ok(views_by_name);
        }
        let proxy_context = self
            .load_provider_proxy_resolution_context(core_store::PROVIDER_CODEX)
            .await?;
        for row in self
            .list_codex_route_candidate_rows_by_names(&missing)
            .await?
        {
            let settings = decode_codex_account_settings(&row.settings_json)?;
            let proxy = resolve_provider_proxy_config_from_context(
                &settings.proxy_mode,
                settings.proxy_config_id.as_deref(),
                &proxy_context,
            )?;
            let view = crate::request_cache::CachedCodexAccountView {
                account_name: row.account_name.clone(),
                generation,
                status: row.status,
                map_gpt53_codex_to_spark: settings.map_gpt53_codex_to_spark,
                auth_refresh_enabled: settings.auth_refresh_enabled,
                route_weight_tier: settings.route_weight_tier,
                request_max_concurrency: settings.request_max_concurrency,
                request_min_start_interval_ms: settings.request_min_start_interval_ms,
                last_refresh_at_ms: row.last_refresh_at_ms,
                last_error: row.last_error,
                access_token: row.access_token,
                proxy: cached_proxy_from_option(proxy),
            };
            let cache_key =
                cache.account_view_key(core_store::PROVIDER_CODEX, &row.account_name, scope);
            if let Err(err) = cache
                .set_json(
                    &cache_key,
                    &view,
                    cache.account_view_ttl(core_store::PROVIDER_CODEX, &row.account_name, scope),
                )
                .await
            {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache codex account view write failed"
                );
            }
            views_by_name.insert(row.account_name, view);
        }
        Ok(views_by_name)
    }

    async fn load_kiro_account_views_cached(
        &self,
        account_names: &[String],
    ) -> anyhow::Result<BTreeMap<String, crate::request_cache::CachedKiroAccountView>> {
        if account_names.is_empty() {
            return Ok(BTreeMap::new());
        }
        let Some(cache) = self.request_cache.as_ref() else {
            let proxy_context = self
                .load_provider_proxy_resolution_context(core_store::PROVIDER_KIRO)
                .await?;
            let status_by_account = self
                .list_kiro_cached_status_parts_rows_by_names(account_names)
                .await?;
            return self
                .list_kiro_route_candidate_rows_by_names(account_names)
                .await?
                .into_iter()
                .map(|row| {
                    build_cached_kiro_account_view(
                        &row,
                        status_by_account.get(&row.account_name).cloned(),
                        &proxy_context,
                        0,
                    )
                })
                .map(|result| result.map(|view| (view.account_name.clone(), view)))
                .collect();
        };

        let generation = self
            .current_dispatch_generation(core_store::PROVIDER_KIRO)
            .await;
        let scope = self.proxy_scope.cache_key_segment();
        let cache_keys = account_names
            .iter()
            .map(|name| cache.account_view_key(core_store::PROVIDER_KIRO, name, scope))
            .collect::<Vec<_>>();
        let cached_values = match cache
            .mget_json::<crate::request_cache::CachedKiroAccountView>(&cache_keys)
            .await
        {
            Ok(values) => values,
            Err(err) => {
                tracing::warn!(error = %err, "request cache kiro account view batch read failed");
                vec![None; account_names.len()]
            },
        };
        let mut views_by_name = BTreeMap::new();
        let mut missing = Vec::new();
        for (account_name, cached) in account_names.iter().cloned().zip(cached_values.into_iter()) {
            if let Some(view) = cached.filter(|view| view.generation == generation) {
                views_by_name.insert(account_name, view);
            } else {
                missing.push(account_name);
            }
        }
        if missing.is_empty() {
            return Ok(views_by_name);
        }
        let proxy_context = self
            .load_provider_proxy_resolution_context(core_store::PROVIDER_KIRO)
            .await?;
        let status_by_account = self
            .list_kiro_cached_status_parts_rows_by_names(&missing)
            .await?;
        for row in self
            .list_kiro_route_candidate_rows_by_names(&missing)
            .await?
        {
            let view = build_cached_kiro_account_view(
                &row,
                status_by_account.get(&row.account_name).cloned(),
                &proxy_context,
                generation,
            )?;
            let cache_key =
                cache.account_view_key(core_store::PROVIDER_KIRO, &view.account_name, scope);
            if let Err(err) = cache
                .set_json(
                    &cache_key,
                    &view,
                    cache.account_view_ttl(core_store::PROVIDER_KIRO, &view.account_name, scope),
                )
                .await
            {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache kiro account view write failed"
                );
            }
            views_by_name.insert(view.account_name.clone(), view);
        }
        Ok(views_by_name)
    }

    async fn load_codex_account_auth_cached(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<crate::request_cache::CachedAccountAuth>> {
        let Some(cache) = self.request_cache.as_ref() else {
            return Ok(self
                .get_codex_account_row(account_name)
                .await?
                .map(|record| crate::request_cache::CachedAccountAuth {
                    auth_json: record.auth_json,
                }));
        };
        let cache_key = cache.account_auth_key(core_store::PROVIDER_CODEX, account_name);
        match cache
            .get_json::<crate::request_cache::CachedAccountAuth>(&cache_key)
            .await
        {
            Ok(Some(value)) => return Ok(Some(value)),
            Ok(None) => {},
            Err(err) => {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache codex auth read failed; falling back to postgres"
                );
            },
        }
        let auth = self
            .get_codex_account_row(account_name)
            .await?
            .map(|record| crate::request_cache::CachedAccountAuth {
                auth_json: record.auth_json,
            });
        if let Some(auth_ref) = auth.as_ref() {
            if let Err(err) = cache
                .set_json(
                    &cache_key,
                    auth_ref,
                    cache.account_auth_ttl(core_store::PROVIDER_CODEX, account_name),
                )
                .await
            {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache codex auth write failed"
                );
            }
        }
        Ok(auth)
    }

    async fn load_kiro_account_auth_cached(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<crate::request_cache::CachedAccountAuth>> {
        let Some(cache) = self.request_cache.as_ref() else {
            return Ok(self
                .get_kiro_account_row(account_name)
                .await?
                .map(|record| crate::request_cache::CachedAccountAuth {
                    auth_json: record.auth_json,
                }));
        };
        let cache_key = cache.account_auth_key(core_store::PROVIDER_KIRO, account_name);
        match cache
            .get_json::<crate::request_cache::CachedAccountAuth>(&cache_key)
            .await
        {
            Ok(Some(value)) => return Ok(Some(value)),
            Ok(None) => {},
            Err(err) => {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache kiro auth read failed; falling back to postgres"
                );
            },
        }
        let auth = self
            .get_kiro_account_row(account_name)
            .await?
            .map(|record| crate::request_cache::CachedAccountAuth {
                auth_json: record.auth_json,
            });
        if let Some(auth_ref) = auth.as_ref() {
            if let Err(err) = cache
                .set_json(
                    &cache_key,
                    auth_ref,
                    cache.account_auth_ttl(core_store::PROVIDER_KIRO, account_name),
                )
                .await
            {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache kiro auth write failed"
                );
            }
        }
        Ok(auth)
    }

    async fn load_codex_rate_limit_status_cached(
        &self,
    ) -> anyhow::Result<Option<CodexRateLimitStatus>> {
        if let Some(snapshot) = self.cached_codex_rate_limit_status().await {
            return Ok(Some(snapshot));
        }
        if let Some(cache) = self.request_cache.as_ref() {
            let cache_key = cache.codex_status_key();
            match cache
                .get_json::<crate::request_cache::CachedCodexStatusLookup>(&cache_key)
                .await
            {
                Ok(Some(lookup)) => {
                    self.store_cached_codex_rate_limit_status(lookup.snapshot.clone())
                        .await;
                    return Ok(lookup.snapshot);
                },
                Ok(None) => {},
                Err(err) => {
                    tracing::warn!(
                        key = %cache_key,
                        error = %err,
                        "request cache codex status read failed; falling back to postgres"
                    );
                },
            }
        }
        let snapshot = self.load_codex_rate_limit_status_row().await?;
        if let Some(cache) = self.request_cache.as_ref() {
            let cache_key = cache.codex_status_key();
            let lookup = crate::request_cache::CachedCodexStatusLookup {
                snapshot: snapshot.clone(),
            };
            if let Err(err) = cache
                .set_json(&cache_key, &lookup, cache.codex_status_ttl())
                .await
            {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache codex status write failed"
                );
            }
        }
        self.store_cached_codex_rate_limit_status(snapshot.clone())
            .await;
        Ok(snapshot)
    }

    async fn apply_usage_rollups_batch(&self, events: &[UsageEvent]) -> anyhow::Result<()> {
        self.ensure_connection_alive()?;
        if events.is_empty() {
            return Ok(());
        }
        let deltas = aggregate_usage_rollup_deltas(events)?;
        for chunk in deltas.chunks(USAGE_ROLLUP_BATCH_ROW_LIMIT.max(1)) {
            let mut builder = QueryBuilder::<Postgres>::new(
                "UPDATE llm_key_usage_rollups AS u
                 SET input_uncached_tokens = u.input_uncached_tokens + v.input_uncached_tokens,
                     input_cached_tokens = u.input_cached_tokens + v.input_cached_tokens,
                     output_tokens = u.output_tokens + v.output_tokens,
                     billable_tokens = u.billable_tokens + v.billable_tokens,
                     credit_total = ((u.credit_total)::numeric + (v.credit_total::double \
                 precision)::numeric)::text,
                     credit_missing_events = u.credit_missing_events + v.credit_missing_events,
                     last_used_at_ms = CASE
                         WHEN u.last_used_at_ms IS NULL THEN v.last_used_at_ms
                         ELSE GREATEST(u.last_used_at_ms, v.last_used_at_ms)
                     END,
                     updated_at_ms = GREATEST(u.updated_at_ms, v.last_used_at_ms)
                 FROM (",
            );
            builder.push_values(chunk.iter(), |mut row, (key_id, delta)| {
                row.push_bind(*key_id)
                    .push_bind(delta.input_uncached_tokens)
                    .push_bind(delta.input_cached_tokens)
                    .push_bind(delta.output_tokens)
                    .push_bind(delta.billable_tokens)
                    .push_bind(delta.credit_total)
                    .push_bind(delta.credit_missing_events)
                    .push_bind(delta.last_used_at_ms);
            });
            builder.push(
                ") AS v(
                    key_id,
                    input_uncached_tokens,
                    input_cached_tokens,
                    output_tokens,
                    billable_tokens,
                    credit_total,
                    credit_missing_events,
                    last_used_at_ms
                 )
                 WHERE u.key_id = v.key_id",
            );
            let changed = builder
                .build()
                .persistent(false)
                .execute(&self.client.pool)
                .await
                .context("batch update postgres usage rollups")?
                .rows_affected();
            if changed != chunk.len() as u64 {
                anyhow::bail!(
                    "usage rollup rows missing for {} key(s) in postgres batch update",
                    chunk.len().saturating_sub(changed as usize)
                );
            }
        }
        let key_ids = deltas
            .iter()
            .map(|(key_id, _)| (*key_id).to_string())
            .collect::<Vec<_>>();
        self.invalidate_authenticated_key_cache_by_ids(&key_ids)
            .await;
        Ok(())
    }

    async fn load_key_bundle_by_id(&self, key_id: &str) -> anyhow::Result<Option<KeyBundle>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT
                    k.key_id, k.name, k.secret, k.key_hash, k.status, k.provider_type,
                    k.protocol_family, k.public_visible, k.quota_billable_limit,
                    k.created_at_ms, k.updated_at_ms,
                    r.route_strategy, r.fixed_account_name, r.auto_account_names_json::text,
                    r.account_group_id, r.model_name_map_json::text,
                    r.request_max_concurrency, r.request_min_start_interval_ms,
                    r.codex_fast_enabled, r.kiro_request_validation_enabled,
                    r.kiro_cache_estimation_enabled,
                    r.kiro_zero_cache_debug_enabled, r.kiro_full_request_logging_enabled,
                    r.kiro_remote_media_resolution_enabled,
                    r.kiro_cache_policy_override_json::text,
                    r.kiro_billable_model_multipliers_override_json::text,
                    COALESCE(u.input_uncached_tokens, 0),
                    COALESCE(u.input_cached_tokens, 0),
                    COALESCE(u.output_tokens, 0),
                    COALESCE(u.billable_tokens, 0),
                    COALESCE(u.credit_total, '0'),
                    COALESCE(u.credit_missing_events, 0),
                    u.last_used_at_ms,
                    COALESCE(u.updated_at_ms, 0),
                    r.kiro_latency_routing_enabled AS kiro_latency_routing_enabled
                 FROM llm_keys k
                 LEFT JOIN llm_key_route_config r ON r.key_id = k.key_id
                 LEFT JOIN llm_key_usage_rollups u ON u.key_id = k.key_id
                 WHERE k.key_id = $1",
                &[&key_id],
            )
            .await
            .context("load key bundle by id")?;
        row.map(decode_key_bundle_row).transpose()
    }

    async fn list_key_bundles(&self) -> anyhow::Result<Vec<KeyBundle>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    k.key_id, k.name, k.secret, k.key_hash, k.status, k.provider_type,
                    k.protocol_family, k.public_visible, k.quota_billable_limit,
                    k.created_at_ms, k.updated_at_ms,
                    r.route_strategy, r.fixed_account_name, r.auto_account_names_json::text,
                    r.account_group_id, r.model_name_map_json::text,
                    r.request_max_concurrency, r.request_min_start_interval_ms,
                    r.codex_fast_enabled, r.kiro_request_validation_enabled,
                    r.kiro_cache_estimation_enabled,
                    r.kiro_zero_cache_debug_enabled, r.kiro_full_request_logging_enabled,
                    r.kiro_remote_media_resolution_enabled,
                    r.kiro_cache_policy_override_json::text,
                    r.kiro_billable_model_multipliers_override_json::text,
                    COALESCE(u.input_uncached_tokens, 0),
                    COALESCE(u.input_cached_tokens, 0),
                    COALESCE(u.output_tokens, 0),
                    COALESCE(u.billable_tokens, 0),
                    COALESCE(u.credit_total, '0'),
                    COALESCE(u.credit_missing_events, 0),
                    u.last_used_at_ms,
                    COALESCE(u.updated_at_ms, k.updated_at_ms),
                    r.kiro_latency_routing_enabled AS kiro_latency_routing_enabled
                 FROM llm_keys k
                 LEFT JOIN llm_key_route_config r ON r.key_id = k.key_id
                 LEFT JOIN llm_key_usage_rollups u ON u.key_id = k.key_id
                 ORDER BY k.created_at_ms DESC, k.key_id DESC",
                &[],
            )
            .await
            .context("list key bundles")?;
        rows.into_iter()
            .map(decode_key_bundle_row)
            .collect::<anyhow::Result<Vec<_>>>()
    }

    async fn admin_keys_summary(
        &self,
        provider_type: Option<&str>,
    ) -> anyhow::Result<core_store::AdminKeysSummary> {
        self.ensure_connection_alive()?;
        let provider = provider_type.map(str::to_string);
        let row = self
            .client
            .query_one(
                "SELECT
                    COUNT(*)::BIGINT,
                    COALESCE(SUM(CASE WHEN k.public_visible THEN 1 ELSE 0 END), 0)::BIGINT,
                    COALESCE(SUM(CASE WHEN k.status = 'active' THEN 1 ELSE 0 END), 0)::BIGINT,
                    COALESCE(SUM(CASE WHEN k.status = 'disabled' THEN 1 ELSE 0 END), 0)::BIGINT,
                    COALESCE(SUM(k.quota_billable_limit), 0)::BIGINT,
                    COALESCE(SUM(k.quota_billable_limit - COALESCE(u.billable_tokens, 0)), \
                 0)::BIGINT,
                    COALESCE(SUM(u.input_uncached_tokens), 0)::BIGINT,
                    COALESCE(SUM(u.input_cached_tokens), 0)::BIGINT,
                    COALESCE(SUM(u.output_tokens), 0)::BIGINT,
                    COALESCE(SUM(u.billable_tokens), 0)::BIGINT,
                    COALESCE(SUM((u.credit_total)::DOUBLE PRECISION), 0)::DOUBLE PRECISION,
                    COALESCE(SUM(u.credit_missing_events), 0)::BIGINT
                 FROM llm_keys k
                 LEFT JOIN llm_key_usage_rollups u ON u.key_id = k.key_id
                 WHERE ($1::text IS NULL OR k.provider_type = $1)",
                &[&provider],
            )
            .await
            .context("summarize postgres key bundles")?;
        Ok(core_store::AdminKeysSummary {
            total: row.get::<_, i64>(0).max(0) as usize,
            public_visible_count: row.get::<_, i64>(1).max(0) as usize,
            active_count: row.get::<_, i64>(2).max(0) as usize,
            disabled_count: row.get::<_, i64>(3).max(0) as usize,
            quota_billable_limit_sum: row.get::<_, i64>(4).max(0) as u64,
            remaining_billable_sum: row.get(5),
            usage_input_uncached_tokens_sum: row.get::<_, i64>(6).max(0) as u64,
            usage_input_cached_tokens_sum: row.get::<_, i64>(7).max(0) as u64,
            usage_output_tokens_sum: row.get::<_, i64>(8).max(0) as u64,
            usage_billable_tokens_sum: row.get::<_, i64>(9).max(0) as u64,
            usage_credit_total: row.get(10),
            usage_credit_missing_events: row.get::<_, i64>(11).max(0) as u64,
        })
    }

    async fn list_key_bundles_page(
        &self,
        provider_type: Option<&str>,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminKeysPage> {
        if provider_type == Some(core_store::PROVIDER_KIRO) {
            return self
                .list_kiro_key_bundles_page_with_candidate_summaries(page)
                .await;
        }
        self.ensure_connection_alive()?;
        let provider = provider_type.map(str::to_string);
        let total = self
            .client
            .query_one(
                "SELECT COUNT(*)
                 FROM llm_keys k
                 WHERE ($1::text IS NULL OR k.provider_type = $1)",
                &[&provider],
            )
            .await
            .context("count postgres key bundles page")?
            .get::<_, i64>(0)
            .max(0) as usize;
        let limit = page.limit.max(1);
        let offset = page.offset;
        let rows = self
            .client
            .query(
                "SELECT
                    k.key_id, k.name, k.secret, k.key_hash, k.status, k.provider_type,
                    k.protocol_family, k.public_visible, k.quota_billable_limit,
                    k.created_at_ms, k.updated_at_ms,
                    r.route_strategy, r.fixed_account_name, r.auto_account_names_json::text,
                    r.account_group_id, r.model_name_map_json::text,
                    r.request_max_concurrency, r.request_min_start_interval_ms,
                    r.codex_fast_enabled, r.kiro_request_validation_enabled,
                    r.kiro_cache_estimation_enabled,
                    r.kiro_zero_cache_debug_enabled, r.kiro_full_request_logging_enabled,
                    r.kiro_remote_media_resolution_enabled,
                    r.kiro_cache_policy_override_json::text,
                    r.kiro_billable_model_multipliers_override_json::text,
                    COALESCE(u.input_uncached_tokens, 0),
                    COALESCE(u.input_cached_tokens, 0),
                    COALESCE(u.output_tokens, 0),
                    COALESCE(u.billable_tokens, 0),
                    COALESCE(u.credit_total, '0'),
                    COALESCE(u.credit_missing_events, 0),
                    u.last_used_at_ms,
                    COALESCE(u.updated_at_ms, k.updated_at_ms),
                    r.kiro_latency_routing_enabled AS kiro_latency_routing_enabled
                 FROM llm_keys k
                 LEFT JOIN llm_key_route_config r ON r.key_id = k.key_id
                 LEFT JOIN llm_key_usage_rollups u ON u.key_id = k.key_id
                 WHERE ($1::text IS NULL OR k.provider_type = $1)
                 ORDER BY k.created_at_ms DESC, k.key_id DESC
                 LIMIT $2 OFFSET $3",
                &[&provider, &(limit as i64), &(offset as i64)],
            )
            .await
            .context("list postgres key bundles page")?;
        let keys = rows
            .into_iter()
            .map(decode_key_bundle_row)
            .map(|bundle| bundle.map(|bundle| admin_key_from_bundle(&bundle)))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let summary = self.admin_keys_summary(provider_type).await?;
        Ok(AdminKeysPage {
            has_more: page.has_more(keys.len(), total),
            keys,
            summary,
            total,
            limit,
            offset,
        })
    }

    async fn list_kiro_key_bundles_page_with_candidate_summaries(
        &self,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminKeysPage> {
        self.ensure_connection_alive()?;
        let summary = self
            .admin_keys_summary(Some(core_store::PROVIDER_KIRO))
            .await?;
        let total = summary.total;
        let limit = page.limit.max(1);
        let offset = page.offset;
        let provider = core_store::PROVIDER_KIRO.to_string();
        let rows = self
            .client
            .query(
                "WITH page_keys AS (
                    SELECT
                        k.key_id, k.name, k.secret, k.key_hash, k.status, k.provider_type,
                        k.protocol_family, k.public_visible, k.quota_billable_limit,
                        k.created_at_ms, k.updated_at_ms,
                        r.route_strategy, r.fixed_account_name, r.auto_account_names_json,
                        r.account_group_id, r.model_name_map_json,
                        r.request_max_concurrency, r.request_min_start_interval_ms,
                        r.codex_fast_enabled, r.kiro_request_validation_enabled,
                        r.kiro_cache_estimation_enabled,
                        r.kiro_zero_cache_debug_enabled, r.kiro_full_request_logging_enabled,
                        r.kiro_remote_media_resolution_enabled,
                        r.kiro_latency_routing_enabled,
                        r.kiro_cache_policy_override_json,
                        r.kiro_billable_model_multipliers_override_json,
                        COALESCE(u.input_uncached_tokens, 0) AS input_uncached_tokens,
                        COALESCE(u.input_cached_tokens, 0) AS input_cached_tokens,
                        COALESCE(u.output_tokens, 0) AS output_tokens,
                        COALESCE(u.billable_tokens, 0) AS billable_tokens,
                        COALESCE(u.credit_total, '0') AS credit_total,
                        COALESCE(u.credit_missing_events, 0) AS credit_missing_events,
                        u.last_used_at_ms,
                        COALESCE(u.updated_at_ms, k.updated_at_ms) AS rollup_updated_at_ms,
                        g.account_names_json AS group_account_names_json,
                        COALESCE(NULLIF(r.route_strategy, ''), 'auto') AS route_strategy_norm
                    FROM llm_keys k
                    LEFT JOIN llm_key_route_config r ON r.key_id = k.key_id
                    LEFT JOIN llm_key_usage_rollups u ON u.key_id = k.key_id
                    LEFT JOIN llm_account_groups g
                        ON g.group_id = r.account_group_id
                       AND g.provider_type = $1
                    WHERE k.provider_type = $1
                    ORDER BY k.created_at_ms DESC, k.key_id DESC
                    LIMIT $2 OFFSET $3
                 ),
                 all_accounts AS (
                    SELECT account_name
                    FROM llm_kiro_accounts
                 ),
                 group_candidates AS (
                    SELECT
                        page_keys.key_id,
                        candidate.account_name AS account_name
                    FROM page_keys
                    CROSS JOIN LATERAL jsonb_array_elements_text(
                        COALESCE(page_keys.group_account_names_json, '[]'::jsonb)
                    ) AS candidate(account_name)
                    WHERE page_keys.route_strategy_norm IN ('auto', 'fixed')
                      AND page_keys.group_account_names_json IS NOT NULL
                 ),
                 fixed_candidates AS (
                    SELECT
                        page_keys.key_id,
                        page_keys.fixed_account_name AS account_name
                    FROM page_keys
                    WHERE page_keys.route_strategy_norm = 'fixed'
                      AND page_keys.group_account_names_json IS NULL
                 ),
                 auto_named_candidates AS (
                    SELECT
                        page_keys.key_id,
                        candidate.account_name AS account_name
                    FROM page_keys
                    CROSS JOIN LATERAL jsonb_array_elements_text(
                        COALESCE(page_keys.auto_account_names_json, '[]'::jsonb)
                    ) AS candidate(account_name)
                    WHERE page_keys.route_strategy_norm = 'auto'
                      AND page_keys.group_account_names_json IS NULL
                      AND COALESCE(
                            jsonb_typeof(page_keys.auto_account_names_json) = 'array'
                            AND jsonb_array_length(page_keys.auto_account_names_json) > 0,
                            FALSE
                        )
                 ),
                 all_auto_candidates AS (
                    SELECT
                        page_keys.key_id,
                        all_accounts.account_name
                    FROM page_keys
                    CROSS JOIN all_accounts
                    WHERE page_keys.route_strategy_norm = 'auto'
                      AND page_keys.group_account_names_json IS NULL
                      AND NOT COALESCE(
                            jsonb_typeof(page_keys.auto_account_names_json) = 'array'
                            AND jsonb_array_length(page_keys.auto_account_names_json) > 0,
                            FALSE
                        )
                 ),
                 key_candidate_names AS (
                    SELECT key_id, account_name FROM group_candidates
                    UNION ALL
                    SELECT key_id, account_name FROM fixed_candidates
                    UNION ALL
                    SELECT key_id, account_name FROM auto_named_candidates
                    UNION ALL
                    SELECT key_id, account_name FROM all_auto_candidates
                 ),
                 valid_key_candidates AS (
                    SELECT DISTINCT candidates.key_id, accounts.account_name
                    FROM key_candidate_names candidates
                    JOIN llm_kiro_accounts accounts
                      ON accounts.account_name = candidates.account_name
                    WHERE candidates.account_name IS NOT NULL
                 ),
                 key_candidate_summary AS (
                    SELECT
                        candidates.key_id,
                        COUNT(*)::BIGINT AS candidate_count,
                        COUNT(*) FILTER (
                            WHERE status.balance_json IS NOT NULL
                              AND status.balance_json <> 'null'::jsonb
                        )::BIGINT AS loaded_balance_count,
                        COUNT(*) FILTER (
                            WHERE status.balance_json IS NULL
                              OR status.balance_json = 'null'::jsonb
                        )::BIGINT AS missing_balance_count,
                        COALESCE(SUM(
                            CASE
                                WHEN status.balance_json IS NOT NULL
                                 AND status.balance_json <> 'null'::jsonb
                                THEN GREATEST(
                                    COALESCE(
                                        (status.balance_json ->> 'usage_limit')::double precision,
                                        0.0
                                    ),
                                    0.0
                                )
                                ELSE 0.0
                            END
                        ), 0.0) AS total_limit,
                        COALESCE(SUM(
                            CASE
                                WHEN status.balance_json IS NOT NULL
                                 AND status.balance_json <> 'null'::jsonb
                                THEN GREATEST(
                                    COALESCE(
                                        (status.balance_json ->> 'remaining')::double precision,
                                        0.0
                                    ),
                                    0.0
                                )
                                ELSE 0.0
                            END
                        ), 0.0) AS total_remaining
                    FROM valid_key_candidates candidates
                    LEFT JOIN llm_kiro_status_cache status
                      ON status.account_name = candidates.account_name
                    GROUP BY candidates.key_id
                 )
                 SELECT
                    page_keys.key_id, page_keys.name, page_keys.secret, page_keys.key_hash,
                    page_keys.status, page_keys.provider_type, page_keys.protocol_family,
                    page_keys.public_visible, page_keys.quota_billable_limit,
                    page_keys.created_at_ms, page_keys.updated_at_ms,
                    page_keys.route_strategy, page_keys.fixed_account_name,
                    page_keys.auto_account_names_json::text,
                    page_keys.account_group_id, page_keys.model_name_map_json::text,
                    page_keys.request_max_concurrency, page_keys.request_min_start_interval_ms,
                    page_keys.codex_fast_enabled, page_keys.kiro_request_validation_enabled,
                    page_keys.kiro_cache_estimation_enabled,
                    page_keys.kiro_zero_cache_debug_enabled,
                    page_keys.kiro_full_request_logging_enabled,
                    page_keys.kiro_remote_media_resolution_enabled,
                    page_keys.kiro_cache_policy_override_json::text,
                    page_keys.kiro_billable_model_multipliers_override_json::text,
                    page_keys.input_uncached_tokens,
                    page_keys.input_cached_tokens,
                    page_keys.output_tokens,
                    page_keys.billable_tokens,
                    page_keys.credit_total,
                    page_keys.credit_missing_events,
                    page_keys.last_used_at_ms,
                    page_keys.rollup_updated_at_ms,
                    COALESCE(summary.candidate_count, 0),
                    COALESCE(summary.loaded_balance_count, 0),
                    COALESCE(summary.missing_balance_count, 0),
                    COALESCE(summary.total_limit, 0.0),
                    COALESCE(summary.total_remaining, 0.0),
                    page_keys.kiro_latency_routing_enabled AS kiro_latency_routing_enabled
                 FROM page_keys
                 LEFT JOIN key_candidate_summary summary
                   ON summary.key_id = page_keys.key_id
                 ORDER BY page_keys.created_at_ms DESC, page_keys.key_id DESC",
                &[&provider, &(limit as i64), &(offset as i64)],
            )
            .await
            .context("list postgres kiro key bundles page with candidate summaries")?;
        let keys = rows
            .into_iter()
            .map(decode_kiro_admin_key_row)
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(AdminKeysPage {
            has_more: page.has_more(keys.len(), total),
            keys,
            summary,
            total,
            limit,
            offset,
        })
    }

    async fn find_key_referencing_account_group(
        &self,
        provider_type: &str,
        group_id: &str,
    ) -> anyhow::Result<Option<AdminKey>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT
                    k.key_id, k.name, k.secret, k.key_hash, k.status, k.provider_type,
                    k.protocol_family, k.public_visible, k.quota_billable_limit,
                    k.created_at_ms, k.updated_at_ms,
                    r.route_strategy, r.fixed_account_name, r.auto_account_names_json::text,
                    r.account_group_id, r.model_name_map_json::text,
                    r.request_max_concurrency, r.request_min_start_interval_ms,
                    r.codex_fast_enabled, r.kiro_request_validation_enabled,
                    r.kiro_cache_estimation_enabled,
                    r.kiro_zero_cache_debug_enabled, r.kiro_full_request_logging_enabled,
                    r.kiro_remote_media_resolution_enabled,
                    r.kiro_cache_policy_override_json::text,
                    r.kiro_billable_model_multipliers_override_json::text,
                    COALESCE(u.input_uncached_tokens, 0),
                    COALESCE(u.input_cached_tokens, 0),
                    COALESCE(u.output_tokens, 0),
                    COALESCE(u.billable_tokens, 0),
                    COALESCE(u.credit_total, '0'),
                    COALESCE(u.credit_missing_events, 0),
                    u.last_used_at_ms,
                    COALESCE(u.updated_at_ms, k.updated_at_ms),
                    r.kiro_latency_routing_enabled AS kiro_latency_routing_enabled
                 FROM llm_keys k
                 JOIN llm_key_route_config r ON r.key_id = k.key_id
                 LEFT JOIN llm_key_usage_rollups u ON u.key_id = k.key_id
                 WHERE k.provider_type = $1 AND r.account_group_id = $2
                 ORDER BY k.created_at_ms DESC, k.key_id DESC
                 LIMIT 1",
                &[&provider_type, &group_id],
            )
            .await
            .context("find postgres key referencing account group")?;
        row.map(decode_key_bundle_row)
            .transpose()
            .map(|bundle| bundle.map(|bundle| admin_key_from_bundle(&bundle)))
    }

    async fn list_public_access_keys_rows(&self) -> anyhow::Result<Vec<PublicAccessKey>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    k.key_id,
                    k.name,
                    k.secret,
                    k.quota_billable_limit,
                    COALESCE(u.input_uncached_tokens, 0),
                    COALESCE(u.input_cached_tokens, 0),
                    COALESCE(u.output_tokens, 0),
                    COALESCE(u.billable_tokens, 0),
                    u.last_used_at_ms
                 FROM llm_keys k
                 LEFT JOIN llm_key_usage_rollups u ON u.key_id = k.key_id
                 WHERE k.status = 'active' AND k.public_visible = TRUE
                 ORDER BY lower(k.name)",
                &[],
            )
            .await
            .context("list public access keys")?;
        Ok(rows
            .into_iter()
            .map(|row| PublicAccessKey {
                key_id: row.get(0),
                key_name: row.get(1),
                secret: row.get(2),
                quota_billable_limit: row.get::<_, i64>(3).max(0) as u64,
                usage_input_uncached_tokens: row.get::<_, i64>(4).max(0) as u64,
                usage_input_cached_tokens: row.get::<_, i64>(5).max(0) as u64,
                usage_output_tokens: row.get::<_, i64>(6).max(0) as u64,
                usage_billable_tokens: row.get::<_, i64>(7).max(0) as u64,
                last_used_at_ms: row.get(8),
            })
            .collect())
    }

    async fn load_public_usage_key_by_hash(
        &self,
        key_hash: &str,
    ) -> anyhow::Result<Option<PublicUsageLookupKey>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT
                    k.key_id,
                    k.name,
                    k.provider_type,
                    k.status,
                    k.public_visible,
                    k.quota_billable_limit,
                    COALESCE(u.input_uncached_tokens, 0),
                    COALESCE(u.input_cached_tokens, 0),
                    COALESCE(u.output_tokens, 0),
                    COALESCE(u.billable_tokens, 0),
                    COALESCE(u.credit_total, '0'),
                    COALESCE(u.credit_missing_events, 0),
                    u.last_used_at_ms
                 FROM llm_keys k
                 LEFT JOIN llm_key_usage_rollups u ON u.key_id = k.key_id
                 WHERE k.key_hash = $1",
                &[&key_hash],
            )
            .await
            .context("load public usage key by hash")?;
        row.map(decode_public_usage_lookup_row).transpose()
    }

    async fn list_public_account_contributions_rows(
        &self,
        limit: usize,
    ) -> anyhow::Result<Vec<PublicAccountContribution>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    request_id,
                    COALESCE(imported_account_name, account_name),
                    contributor_message,
                    github_id,
                    processed_at_ms
                 FROM llm_account_contribution_requests
                 WHERE status = 'issued'
                   AND show_on_public_wall = TRUE
                 ORDER BY COALESCE(processed_at_ms, created_at_ms) DESC
                 LIMIT $1",
                &[&(limit.max(1) as i64)],
            )
            .await
            .context("list public account contributions")?;
        Ok(rows
            .into_iter()
            .map(|row| PublicAccountContribution {
                request_id: row.get(0),
                account_name: row.get(1),
                contributor_message: row.get(2),
                github_id: row.get(3),
                processed_at_ms: row.get(4),
            })
            .collect())
    }

    async fn list_public_sponsors_rows(&self, limit: usize) -> anyhow::Result<Vec<PublicSponsor>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    request_id,
                    display_name,
                    sponsor_message,
                    github_id,
                    processed_at_ms
                 FROM llm_sponsor_requests
                 WHERE status = 'approved'
                 ORDER BY COALESCE(processed_at_ms, created_at_ms) DESC
                 LIMIT $1",
                &[&(limit.max(1) as i64)],
            )
            .await
            .context("list public sponsors")?;
        Ok(rows
            .into_iter()
            .map(|row| PublicSponsor {
                request_id: row.get(0),
                display_name: row.get(1),
                sponsor_message: row.get(2),
                github_id: row.get(3),
                processed_at_ms: row.get(4),
            })
            .collect())
    }

    async fn list_admin_account_groups_rows(
        &self,
        provider_type: &str,
    ) -> anyhow::Result<Vec<AdminAccountGroup>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT group_id, provider_type, name, account_names_json::text,
                    created_at_ms, updated_at_ms
                 FROM llm_account_groups
                 WHERE provider_type = $1
                 ORDER BY created_at_ms DESC, group_id DESC",
                &[&provider_type],
            )
            .await
            .context("list admin account groups")?;
        rows.into_iter()
            .map(decode_admin_account_group_row)
            .collect::<anyhow::Result<Vec<_>>>()
    }

    async fn list_admin_account_groups_page_rows(
        &self,
        provider_type: &str,
        page: AdminPageRequest,
    ) -> anyhow::Result<core_store::AdminAccountGroupsPage> {
        self.ensure_connection_alive()?;
        let total_row = self
            .client
            .query_one(
                "SELECT COUNT(*)::bigint
                 FROM llm_account_groups
                 WHERE provider_type = $1",
                &[&provider_type],
            )
            .await
            .context("count admin account groups")?;
        let total_i64 = total_row.get::<_, i64>(0);
        let total = usize::try_from(total_i64)
            .with_context(|| format!("admin account groups total out of range: {total_i64}"))?;
        let rows = self
            .client
            .query(
                "SELECT group_id, provider_type, name, account_names_json::text,
                    created_at_ms, updated_at_ms
                 FROM llm_account_groups
                 WHERE provider_type = $1
                 ORDER BY created_at_ms DESC, group_id DESC
                 LIMIT $2 OFFSET $3",
                &[&provider_type, &(page.limit.max(1) as i64), &(page.offset as i64)],
            )
            .await
            .context("list admin account groups page")?;
        let groups = rows
            .into_iter()
            .map(decode_admin_account_group_row)
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(core_store::AdminAccountGroupsPage {
            has_more: page.has_more(groups.len(), total),
            groups,
            total,
            limit: page.limit,
            offset: page.offset,
        })
    }

    async fn get_admin_account_group_row(
        &self,
        group_id: &str,
    ) -> anyhow::Result<Option<AdminAccountGroup>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT group_id, provider_type, name, account_names_json::text,
                    created_at_ms, updated_at_ms
                 FROM llm_account_groups
                 WHERE group_id = $1",
                &[&group_id],
            )
            .await
            .context("load admin account group")?;
        row.map(decode_admin_account_group_row).transpose()
    }

    async fn list_admin_proxy_config_base_rows(&self) -> anyhow::Result<Vec<AdminProxyConfig>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT proxy_config_id, name, proxy_url, proxy_username, proxy_password,
                    status, created_at_ms, updated_at_ms
                 FROM llm_proxy_configs
                 ORDER BY created_at_ms DESC, proxy_config_id DESC",
                &[],
            )
            .await
            .context("list admin proxy configs")?;
        Ok(rows
            .into_iter()
            .map(decode_admin_proxy_config_row)
            .collect())
    }

    async fn list_admin_proxy_configs_rows(&self) -> anyhow::Result<Vec<AdminProxyConfig>> {
        let mut proxies = self.list_admin_proxy_config_base_rows().await?;
        if self.proxy_scope.can_edit_slot_metadata() {
            for proxy in &mut proxies {
                self.apply_proxy_scope_metadata(proxy, "core", false);
            }
            self.apply_proxy_endpoint_checks_to_configs(&mut proxies)
                .await?;
            return Ok(proxies);
        }
        let overrides = self.list_proxy_config_node_overrides().await?;
        for proxy in &mut proxies {
            if let Some(override_row) = overrides.get(&proxy.id) {
                apply_proxy_config_node_override(proxy, override_row);
                self.apply_proxy_scope_metadata(proxy, "node_override", true);
            } else {
                self.apply_proxy_scope_metadata(proxy, "core", false);
            }
        }
        self.apply_proxy_endpoint_checks_to_configs(&mut proxies)
            .await?;
        Ok(proxies)
    }

    async fn get_admin_proxy_config_base_row(
        &self,
        proxy_id: &str,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT proxy_config_id, name, proxy_url, proxy_username, proxy_password,
                    status, created_at_ms, updated_at_ms
                 FROM llm_proxy_configs
                 WHERE proxy_config_id = $1",
                &[&proxy_id],
            )
            .await
            .context("load admin proxy config")?;
        Ok(row.map(decode_admin_proxy_config_row))
    }

    async fn get_admin_proxy_config_row(
        &self,
        proxy_id: &str,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        let Some(mut proxy) = self.get_admin_proxy_config_base_row(proxy_id).await? else {
            return Ok(None);
        };
        if self.proxy_scope.can_edit_slot_metadata() {
            self.apply_proxy_scope_metadata(&mut proxy, "core", false);
            self.apply_proxy_endpoint_checks_to_config(&mut proxy)
                .await?;
            return Ok(Some(proxy));
        }
        match self.get_proxy_config_node_override(proxy_id).await? {
            Some(override_row) => {
                apply_proxy_config_node_override(&mut proxy, &override_row);
                self.apply_proxy_scope_metadata(&mut proxy, "node_override", true);
            },
            None => self.apply_proxy_scope_metadata(&mut proxy, "core", false),
        }
        self.apply_proxy_endpoint_checks_to_config(&mut proxy)
            .await?;
        Ok(Some(proxy))
    }

    async fn list_proxy_endpoint_check_rows(&self) -> anyhow::Result<Vec<ProxyEndpointCheckRow>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT proxy_config_id, provider_type, target_url, reachable, status_code,
                    latency_ms, error_message, checked_at_ms
                 FROM llm_proxy_config_endpoint_checks
                 WHERE node_id = $1",
                &[&self.proxy_scope.node_id],
            )
            .await
            .context("list postgres proxy endpoint checks")?;
        Ok(rows
            .into_iter()
            .map(decode_proxy_endpoint_check_row)
            .collect())
    }

    async fn apply_proxy_endpoint_checks_to_configs(
        &self,
        proxies: &mut [AdminProxyConfig],
    ) -> anyhow::Result<()> {
        let mut by_proxy_id = BTreeMap::<String, Vec<ProxyEndpointCheckRow>>::new();
        for row in self.list_proxy_endpoint_check_rows().await? {
            by_proxy_id
                .entry(row.proxy_config_id.clone())
                .or_default()
                .push(row);
        }
        for proxy in proxies {
            if let Some(rows) = by_proxy_id.get(&proxy.id) {
                apply_proxy_endpoint_checks(proxy, rows);
            }
        }
        Ok(())
    }

    async fn apply_proxy_endpoint_checks_to_config(
        &self,
        proxy: &mut AdminProxyConfig,
    ) -> anyhow::Result<()> {
        let rows = self
            .list_proxy_endpoint_check_rows()
            .await?
            .into_iter()
            .filter(|row| row.proxy_config_id == proxy.id)
            .collect::<Vec<_>>();
        apply_proxy_endpoint_checks(proxy, &rows);
        Ok(())
    }

    async fn list_proxy_config_node_overrides(
        &self,
    ) -> anyhow::Result<BTreeMap<String, ProxyConfigNodeOverride>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT proxy_config_id, proxy_url, proxy_username, proxy_password,
                    status, created_at_ms, updated_at_ms
                 FROM llm_proxy_config_node_overrides
                 WHERE node_id = $1",
                &[&self.proxy_scope.node_id],
            )
            .await
            .context("list postgres proxy config node overrides")?;
        Ok(rows
            .into_iter()
            .map(|row| {
                (row.get::<_, String>(0), ProxyConfigNodeOverride {
                    proxy_url: row.get(1),
                    proxy_username: row.get(2),
                    proxy_password: row.get(3),
                    status: row.get(4),
                    created_at_ms: row.get(5),
                    updated_at_ms: row.get(6),
                })
            })
            .collect())
    }

    async fn get_proxy_config_node_override(
        &self,
        proxy_id: &str,
    ) -> anyhow::Result<Option<ProxyConfigNodeOverride>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT proxy_url, proxy_username, proxy_password,
                    status, created_at_ms, updated_at_ms
                 FROM llm_proxy_config_node_overrides
                 WHERE proxy_config_id = $1 AND node_id = $2",
                &[&proxy_id, &self.proxy_scope.node_id],
            )
            .await
            .context("load postgres proxy config node override")?;
        Ok(row.map(|row| ProxyConfigNodeOverride {
            proxy_url: row.get(0),
            proxy_username: row.get(1),
            proxy_password: row.get(2),
            status: row.get(3),
            created_at_ms: row.get(4),
            updated_at_ms: row.get(5),
        }))
    }

    async fn patch_admin_proxy_config_node_override(
        &self,
        proxy_id: &str,
        patch: AdminProxyConfigPatch,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        if patch.name.is_some() {
            anyhow::bail!("proxy slot names can only be changed on the core node");
        }
        let Some(base) = self.get_admin_proxy_config_base_row(proxy_id).await? else {
            return Ok(None);
        };
        let existing = self.get_proxy_config_node_override(proxy_id).await?;
        let proxy_url = patch
            .proxy_url
            .or_else(|| existing.as_ref().map(|row| row.proxy_url.clone()))
            .unwrap_or(base.proxy_url);
        let proxy_username = match patch.proxy_username {
            Some(value) => value,
            None => existing
                .as_ref()
                .map(|row| row.proxy_username.clone())
                .unwrap_or(base.proxy_username),
        };
        let proxy_password = match patch.proxy_password {
            Some(value) => value,
            None => existing
                .as_ref()
                .map(|row| row.proxy_password.clone())
                .unwrap_or(base.proxy_password),
        };
        let status = patch
            .status
            .or_else(|| existing.as_ref().map(|row| row.status.clone()))
            .unwrap_or(base.status);
        let created_at_ms = existing
            .as_ref()
            .map(|row| row.created_at_ms)
            .unwrap_or(patch.updated_at_ms);
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "INSERT INTO llm_proxy_config_node_overrides (
                    proxy_config_id, node_id, proxy_url, proxy_username, proxy_password,
                    status, created_at_ms, updated_at_ms
                 ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                 ON CONFLICT (proxy_config_id, node_id) DO UPDATE SET
                    proxy_url = EXCLUDED.proxy_url,
                    proxy_username = EXCLUDED.proxy_username,
                    proxy_password = EXCLUDED.proxy_password,
                    status = EXCLUDED.status,
                    updated_at_ms = EXCLUDED.updated_at_ms",
                &[
                    &proxy_id,
                    &self.proxy_scope.node_id,
                    &proxy_url,
                    &proxy_username,
                    &proxy_password,
                    &status,
                    &created_at_ms,
                    &patch.updated_at_ms,
                ],
            )
            .await
            .context("patch postgres proxy config node override")?;
        self.invalidate_proxy_metadata_cache().await;
        self.invalidate_all_account_views_for_provider(core_store::PROVIDER_CODEX)
            .await;
        self.invalidate_all_account_views_for_provider(core_store::PROVIDER_KIRO)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_CODEX)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_KIRO)
            .await;
        self.get_admin_proxy_config_row(proxy_id).await
    }

    fn apply_proxy_scope_metadata(
        &self,
        proxy: &mut AdminProxyConfig,
        effective_source: &str,
        has_node_override: bool,
    ) {
        proxy.scope_node_id = self.proxy_scope.scope_node_id();
        proxy.effective_source = effective_source.to_string();
        proxy.has_node_override = has_node_override;
        proxy.can_edit_slot_metadata = self.proxy_scope.can_edit_slot_metadata();
    }

    async fn load_admin_proxy_binding_row(
        &self,
        provider_type: &str,
    ) -> anyhow::Result<AdminProxyBinding> {
        let proxy_configs_by_id = self
            .list_admin_proxy_configs_rows()
            .await?
            .into_iter()
            .map(|proxy| (proxy.id.clone(), proxy))
            .collect::<BTreeMap<_, _>>();
        self.load_admin_proxy_binding_from_configs(provider_type, &proxy_configs_by_id)
            .await
    }

    async fn load_admin_proxy_binding_from_configs(
        &self,
        provider_type: &str,
        proxy_configs_by_id: &BTreeMap<String, AdminProxyConfig>,
    ) -> anyhow::Result<AdminProxyBinding> {
        self.ensure_connection_alive()?;
        let binding = self
            .client
            .query_opt(
                "SELECT provider_type, proxy_config_id, updated_at_ms
                 FROM llm_proxy_bindings
                 WHERE provider_type = $1",
                &[&provider_type],
            )
            .await
            .context("load proxy binding row")?;
        let Some(row) = binding else {
            return Ok(default_proxy_bindings()
                .into_iter()
                .find(|binding| binding.provider_type == provider_type)
                .unwrap_or_else(|| AdminProxyBinding {
                    provider_type: provider_type.to_string(),
                    effective_source: "none".to_string(),
                    bound_proxy_config_id: None,
                    effective_proxy_config_name: None,
                    effective_proxy_url: None,
                    effective_proxy_username: None,
                    effective_proxy_password: None,
                    binding_updated_at: None,
                    error_message: None,
                }));
        };
        let provider_type: String = row.get(0);
        let proxy_config_id: String = row.get(1);
        let updated_at_ms: i64 = row.get(2);
        let Some(proxy) = proxy_configs_by_id.get(&proxy_config_id).cloned() else {
            return Ok(AdminProxyBinding {
                provider_type,
                effective_source: "invalid".to_string(),
                bound_proxy_config_id: Some(proxy_config_id),
                effective_proxy_config_name: None,
                effective_proxy_url: None,
                effective_proxy_username: None,
                effective_proxy_password: None,
                binding_updated_at: Some(updated_at_ms),
                error_message: Some("bound proxy config is missing".to_string()),
            });
        };
        if proxy.status != core_store::KEY_STATUS_ACTIVE {
            return Ok(AdminProxyBinding {
                provider_type,
                effective_source: "invalid".to_string(),
                bound_proxy_config_id: Some(proxy.id),
                effective_proxy_config_name: Some(proxy.name),
                effective_proxy_url: None,
                effective_proxy_username: None,
                effective_proxy_password: None,
                binding_updated_at: Some(updated_at_ms),
                error_message: Some("bound proxy config is disabled".to_string()),
            });
        }
        Ok(AdminProxyBinding {
            provider_type,
            effective_source: "binding".to_string(),
            bound_proxy_config_id: Some(proxy.id),
            effective_proxy_config_name: Some(proxy.name),
            effective_proxy_url: Some(proxy.proxy_url),
            effective_proxy_username: proxy.proxy_username,
            effective_proxy_password: proxy.proxy_password,
            binding_updated_at: Some(updated_at_ms),
            error_message: None,
        })
    }

    async fn load_codex_rate_limit_status_row(
        &self,
    ) -> anyhow::Result<Option<CodexRateLimitStatus>> {
        self.ensure_connection_alive()?;
        let snapshot_json = self
            .client
            .query_opt(
                "SELECT snapshot_json::text FROM llm_codex_status_cache WHERE id = 'default'",
                &[],
            )
            .await
            .context("load codex rate-limit status snapshot")?
            .map(|row| row.get::<_, String>(0));
        snapshot_json
            .map(|json| {
                serde_json::from_str::<CodexRateLimitStatus>(&json)
                    .context("decode codex rate-limit status snapshot")
            })
            .transpose()
    }

    async fn get_admin_token_request_row(
        &self,
        request_id: &str,
    ) -> anyhow::Result<Option<AdminTokenRequest>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT
                    request_id, requester_email, requested_quota_billable_limit,
                    request_reason, frontend_page_url, status, client_ip, ip_region,
                    admin_note, failure_reason, issued_key_id, issued_key_name,
                    created_at_ms, updated_at_ms, processed_at_ms
                 FROM llm_token_requests
                 WHERE request_id = $1",
                &[&request_id],
            )
            .await
            .context("load admin token request")?;
        Ok(row.map(decode_admin_token_request_row))
    }

    async fn get_admin_account_contribution_request_row(
        &self,
        request_id: &str,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT
                    request_id, account_name, account_id, id_token, access_token,
                    refresh_token, requester_email, contributor_message, github_id,
                    frontend_page_url, status, client_ip, ip_region, admin_note,
                    failure_reason, imported_account_name, issued_key_id, issued_key_name,
                    created_at_ms, updated_at_ms, processed_at_ms
                 FROM llm_account_contribution_requests
                 WHERE request_id = $1",
                &[&request_id],
            )
            .await
            .context("load admin account contribution request")?;
        Ok(row.map(decode_admin_account_contribution_request_row))
    }

    async fn get_admin_sponsor_request_row(
        &self,
        request_id: &str,
    ) -> anyhow::Result<Option<AdminSponsorRequest>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT
                    request_id, requester_email, sponsor_message, display_name, github_id,
                    frontend_page_url, status, client_ip, ip_region, admin_note,
                    failure_reason, payment_email_sent_at_ms, created_at_ms, updated_at_ms,
                    processed_at_ms
                 FROM llm_sponsor_requests
                 WHERE request_id = $1",
                &[&request_id],
            )
            .await
            .context("load admin sponsor request")?;
        Ok(row.map(decode_admin_sponsor_request_row))
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

    async fn list_codex_admin_account_rows(&self) -> anyhow::Result<Vec<CodexAdminAccountListRow>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    account_name,
                    account_id,
                    status,
                    COALESCE(
                        CASE
                            WHEN jsonb_typeof(settings_json -> 'map_gpt53_codex_to_spark')
                                = 'boolean'
                            THEN (settings_json ->> 'map_gpt53_codex_to_spark')::boolean
                        END,
                        false
                    ),
                    COALESCE(
                        CASE
                            WHEN jsonb_typeof(settings_json -> 'auth_refresh_enabled')
                                = 'boolean'
                            THEN (settings_json ->> 'auth_refresh_enabled')::boolean
                        END,
                        true
                    ),
                    NULLIF(BTRIM(settings_json ->> 'route_weight_tier'), ''),
                    COALESCE(NULLIF(BTRIM(settings_json ->> 'proxy_mode'), ''), 'inherit'),
                    NULLIF(BTRIM(settings_json ->> 'proxy_config_id'), ''),
                    CASE
                        WHEN jsonb_typeof(settings_json -> 'request_max_concurrency') = 'number'
                        THEN (settings_json ->> 'request_max_concurrency')::bigint
                        ELSE NULL
                    END,
                    CASE
                        WHEN jsonb_typeof(settings_json -> 'request_min_start_interval_ms')
                            = 'number'
                        THEN (settings_json ->> 'request_min_start_interval_ms')::bigint
                        ELSE NULL
                    END,
                    last_refresh_at_ms,
                    last_error,
                    COALESCE(
                        auth_json #>> '{tokens,access_token}',
                        auth_json #>> '{tokens,accessToken}',
                        auth_json ->> 'access_token',
                        auth_json ->> 'accessToken'
                    ),
                    NULL::text,
                    NULL::double precision,
                    NULL::double precision,
                    NULL::bigint,
                    NULL::bigint,
                    NULL::text
                 FROM llm_codex_accounts
                 ORDER BY created_at_ms DESC, account_name DESC",
                &[],
            )
            .await
            .context("list postgres codex admin account rows")?;
        Ok(rows
            .into_iter()
            .map(decode_codex_admin_account_list_row)
            .collect())
    }

    async fn admin_codex_accounts_summary(
        &self,
    ) -> anyhow::Result<core_store::AdminAccountsSummary> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_one(
                "SELECT
                    COUNT(*)::BIGINT,
                    COALESCE(SUM(CASE WHEN status = 'active' THEN 1 ELSE 0 END), 0)::BIGINT,
                    COALESCE(SUM(CASE WHEN status = 'disabled' THEN 1 ELSE 0 END), 0)::BIGINT,
                    COALESCE(SUM(CASE WHEN status = 'unavailable' THEN 1 ELSE 0 END), 0)::BIGINT
                 FROM llm_codex_accounts",
                &[],
            )
            .await
            .context("summarize postgres codex accounts")?;
        Ok(core_store::AdminAccountsSummary {
            total: row.get::<_, i64>(0).max(0) as usize,
            active_count: row.get::<_, i64>(1).max(0) as usize,
            disabled_count: row.get::<_, i64>(2).max(0) as usize,
            unavailable_count: row.get::<_, i64>(3).max(0) as usize,
        })
    }

    async fn list_codex_admin_account_rows_page(
        &self,
        page: AdminPageRequest,
    ) -> anyhow::Result<(Vec<CodexAdminAccountListRow>, usize)> {
        self.ensure_connection_alive()?;
        let total = self
            .client
            .query_one("SELECT COUNT(*) FROM llm_codex_accounts", &[])
            .await
            .context("count postgres codex admin account rows")?
            .get::<_, i64>(0)
            .max(0) as usize;
        let rows = self
            .client
            .query(
                "SELECT
                    account_name,
                    account_id,
                    status,
                    COALESCE(
                        CASE
                            WHEN jsonb_typeof(settings_json -> 'map_gpt53_codex_to_spark')
                                = 'boolean'
                            THEN (settings_json ->> 'map_gpt53_codex_to_spark')::boolean
                        END,
                        false
                    ),
                    COALESCE(
                        CASE
                            WHEN jsonb_typeof(settings_json -> 'auth_refresh_enabled')
                                = 'boolean'
                            THEN (settings_json ->> 'auth_refresh_enabled')::boolean
                        END,
                        true
                    ),
                    NULLIF(BTRIM(settings_json ->> 'route_weight_tier'), ''),
                    COALESCE(NULLIF(BTRIM(settings_json ->> 'proxy_mode'), ''), 'inherit'),
                    NULLIF(BTRIM(settings_json ->> 'proxy_config_id'), ''),
                    CASE
                        WHEN jsonb_typeof(settings_json -> 'request_max_concurrency') = 'number'
                        THEN (settings_json ->> 'request_max_concurrency')::bigint
                        ELSE NULL
                    END,
                    CASE
                        WHEN jsonb_typeof(settings_json -> 'request_min_start_interval_ms')
                            = 'number'
                        THEN (settings_json ->> 'request_min_start_interval_ms')::bigint
                        ELSE NULL
                    END,
                    last_refresh_at_ms,
                    last_error,
                    COALESCE(
                        auth_json #>> '{tokens,access_token}',
                        auth_json #>> '{tokens,accessToken}',
                        auth_json ->> 'access_token',
                        auth_json ->> 'accessToken'
                    ),
                    NULL::text,
                    NULL::double precision,
                    NULL::double precision,
                    NULL::bigint,
                    NULL::bigint,
                    NULL::text
                 FROM llm_codex_accounts
                 ORDER BY created_at_ms DESC, account_name DESC
                 LIMIT $1 OFFSET $2",
                &[&(page.limit.max(1) as i64), &(page.offset as i64)],
            )
            .await
            .context("list postgres codex admin account rows page")?;
        Ok((
            rows.into_iter()
                .map(decode_codex_admin_account_list_row)
                .collect(),
            total,
        ))
    }

    async fn list_codex_admin_account_rows_filtered_page(
        &self,
        query: &AdminCodexAccountPageQuery,
        page: AdminPageRequest,
    ) -> anyhow::Result<(Vec<CodexAdminAccountListRow>, usize)> {
        self.ensure_connection_alive()?;
        let search = query
            .search
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| format!("%{}%", value.to_ascii_lowercase()));
        let base_cte = "
            WITH status_snapshot AS (
                SELECT snapshot_json
                FROM llm_codex_status_cache
                WHERE id = 'default'
            ),
            status_accounts AS (
                SELECT
                    NULLIF(BTRIM(account ->> 'name'), '') AS account_name,
                    COALESCE(NULLIF(BTRIM(account ->> 'status'), ''), 'unknown') AS status,
                    NULLIF(BTRIM(account ->> 'plan_type'), '') AS plan_type,
                    CASE
                        WHEN jsonb_typeof(account -> 'primary_remaining_percent') = 'number'
                        THEN (account ->> 'primary_remaining_percent')::double precision
                        ELSE NULL
                    END AS primary_remaining_percent,
                    CASE
                        WHEN jsonb_typeof(account -> 'secondary_remaining_percent') = 'number'
                        THEN (account ->> 'secondary_remaining_percent')::double precision
                        ELSE NULL
                    END AS secondary_remaining_percent,
                    CASE
                        WHEN jsonb_typeof(account -> 'last_usage_checked_at') = 'number'
                        THEN (account ->> 'last_usage_checked_at')::bigint
                        ELSE NULL
                    END AS last_usage_checked_at_ms,
                    CASE
                        WHEN jsonb_typeof(account -> 'last_usage_success_at') = 'number'
                        THEN (account ->> 'last_usage_success_at')::bigint
                        ELSE NULL
                    END AS last_usage_success_at_ms,
                    NULLIF(BTRIM(account ->> 'usage_error_message'), '') AS usage_error_message
                FROM status_snapshot snapshot
                CROSS JOIN LATERAL jsonb_array_elements(
                    COALESCE(snapshot.snapshot_json -> 'accounts', '[]'::jsonb)
                ) account
            ),
            account_rows AS (
                SELECT
                    a.account_name,
                    a.account_id,
                    a.status,
                    COALESCE(
                        CASE
                            WHEN jsonb_typeof(a.settings_json -> 'map_gpt53_codex_to_spark')
                                = 'boolean'
                            THEN (a.settings_json ->> 'map_gpt53_codex_to_spark')::boolean
                        END,
                        false
                    ) AS map_gpt53_codex_to_spark,
                    COALESCE(
                        CASE
                            WHEN jsonb_typeof(a.settings_json -> 'auth_refresh_enabled')
                                = 'boolean'
                            THEN (a.settings_json ->> 'auth_refresh_enabled')::boolean
                        END,
                        true
                    ) AS auth_refresh_enabled,
                    NULLIF(BTRIM(a.settings_json ->> 'route_weight_tier'), '') AS \
                        route_weight_tier,
                    COALESCE(NULLIF(BTRIM(a.settings_json ->> 'proxy_mode'), ''), 'inherit')
                        AS proxy_mode,
                    NULLIF(BTRIM(a.settings_json ->> 'proxy_config_id'), '') AS proxy_config_id,
                    CASE
                        WHEN jsonb_typeof(a.settings_json -> 'request_max_concurrency') = 'number'
                        THEN (a.settings_json ->> 'request_max_concurrency')::bigint
                        ELSE NULL
                    END AS request_max_concurrency,
                    CASE
                        WHEN jsonb_typeof(a.settings_json -> 'request_min_start_interval_ms')
                            = 'number'
                        THEN (a.settings_json ->> 'request_min_start_interval_ms')::bigint
                        ELSE NULL
                    END AS request_min_start_interval_ms,
                    a.last_refresh_at_ms,
                    a.last_error,
                    COALESCE(
                        a.auth_json #>> '{tokens,access_token}',
                        a.auth_json #>> '{tokens,accessToken}',
                        a.auth_json ->> 'access_token',
                        a.auth_json ->> 'accessToken'
                    ) AS access_token,
                    CASE
                        WHEN a.status = 'active' AND COALESCE(sa.status, '') = 'active'
                        THEN sa.plan_type
                        ELSE NULL
                    END AS plan_type,
                    CASE
                        WHEN a.status = 'active' AND COALESCE(sa.status, '') = 'active'
                        THEN sa.primary_remaining_percent
                        ELSE NULL
                    END AS primary_remaining_percent,
                    CASE
                        WHEN a.status = 'active' AND COALESCE(sa.status, '') = 'active'
                        THEN sa.secondary_remaining_percent
                        ELSE NULL
                    END AS secondary_remaining_percent,
                    CASE
                        WHEN a.status = 'active' AND COALESCE(sa.status, '') = 'active'
                        THEN sa.last_usage_checked_at_ms
                        ELSE NULL
                    END AS last_usage_checked_at_ms,
                    CASE
                        WHEN a.status = 'active' AND COALESCE(sa.status, '') = 'active'
                        THEN sa.last_usage_success_at_ms
                        ELSE NULL
                    END AS last_usage_success_at_ms,
                    CASE
                        WHEN a.status = 'active' AND COALESCE(sa.status, '') = 'active'
                        THEN sa.usage_error_message
                        ELSE NULL
                    END AS usage_error_message,
                    a.created_at_ms
                FROM llm_codex_accounts a
                LEFT JOIN status_accounts sa ON sa.account_name = a.account_name
            )";
        let filter_sql = "
            WHERE ($1::text IS NULL
                   OR lower(account_name) LIKE $1
                   OR lower(status) LIKE $1
                   OR lower(COALESCE(plan_type, '')) LIKE $1
                   OR lower(COALESCE(account_id, '')) LIKE $1
                   OR lower(COALESCE(route_weight_tier, '')) LIKE $1)
              AND ($2::boolean = FALSE OR status <> 'disabled')
              AND ($3::boolean = FALSE
                   OR status = 'disabled'
                   OR last_error IS NOT NULL
                   OR usage_error_message IS NOT NULL)";
        let total_sql = format!(
            "{base_cte}
             SELECT COUNT(*)
             FROM account_rows
             {filter_sql}"
        );
        let total = self
            .client
            .query_one(total_sql.as_str(), &[&search, &query.active_only, &query.unhealthy_only])
            .await
            .context("count postgres filtered codex admin account rows")?
            .get::<_, i64>(0)
            .max(0) as usize;
        let order_by = match query.sort {
            AdminCodexAccountSortMode::Newest => "created_at_ms DESC, account_name DESC",
            AdminCodexAccountSortMode::PrimaryAsc => {
                "COALESCE(primary_remaining_percent, 100.0) ASC, account_name DESC"
            },
            AdminCodexAccountSortMode::PrimaryDesc => {
                "COALESCE(primary_remaining_percent, 100.0) DESC, account_name DESC"
            },
            AdminCodexAccountSortMode::SecondaryAsc => {
                "COALESCE(secondary_remaining_percent, 100.0) ASC, account_name DESC"
            },
            AdminCodexAccountSortMode::SecondaryDesc => {
                "COALESCE(secondary_remaining_percent, 100.0) DESC, account_name DESC"
            },
        };
        let rows_sql = format!(
            "{base_cte}
             SELECT
                account_name,
                account_id,
                status,
                map_gpt53_codex_to_spark,
                auth_refresh_enabled,
                route_weight_tier,
                proxy_mode,
                proxy_config_id,
                request_max_concurrency,
                request_min_start_interval_ms,
                last_refresh_at_ms,
                last_error,
                access_token,
                plan_type,
                primary_remaining_percent,
                secondary_remaining_percent,
                last_usage_checked_at_ms,
                last_usage_success_at_ms,
                usage_error_message
             FROM account_rows
             {filter_sql}
             ORDER BY {order_by}
             LIMIT $4 OFFSET $5"
        );
        let rows = self
            .client
            .query(rows_sql.as_str(), &[
                &search,
                &query.active_only,
                &query.unhealthy_only,
                &(page.limit.max(1) as i64),
                &(page.offset as i64),
            ])
            .await
            .context("list postgres filtered codex admin account rows page")?;
        Ok((
            rows.into_iter()
                .map(decode_codex_admin_account_list_row)
                .collect(),
            total,
        ))
    }

    async fn get_codex_account_row(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<CodexAccountRecord>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT
                    account_name, account_id, email, status, auth_json::text, settings_json::text,
                    last_refresh_at_ms, last_error, created_at_ms, updated_at_ms
                 FROM llm_codex_accounts
                 WHERE account_name = $1",
                &[&name],
            )
            .await
            .context("load codex account")?;
        Ok(row.map(decode_codex_account_row))
    }

    async fn list_codex_route_candidate_rows(&self) -> anyhow::Result<Vec<CodexRouteCandidateRow>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    account_name,
                    status,
                    settings_json::text,
                    last_refresh_at_ms,
                    last_error,
                    COALESCE(
                        auth_json #>> '{tokens,access_token}',
                        auth_json #>> '{tokens,accessToken}',
                        auth_json ->> 'access_token',
                        auth_json ->> 'accessToken'
                    )
                 FROM llm_codex_accounts
                 ORDER BY account_name",
                &[],
            )
            .await
            .context("list postgres codex route candidates")?;
        Ok(rows
            .into_iter()
            .map(|row| CodexRouteCandidateRow {
                account_name: row.get(0),
                status: row.get(1),
                settings_json: row.get(2),
                last_refresh_at_ms: row.get(3),
                last_error: row.get(4),
                access_token: row.get(5),
            })
            .collect())
    }

    async fn list_codex_route_candidate_rows_by_names(
        &self,
        account_names: &[String],
    ) -> anyhow::Result<Vec<CodexRouteCandidateRow>> {
        if account_names.is_empty() {
            return Ok(Vec::new());
        }
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    account_name,
                    status,
                    settings_json::text,
                    last_refresh_at_ms,
                    last_error,
                    COALESCE(
                        auth_json #>> '{tokens,access_token}',
                        auth_json #>> '{tokens,accessToken}',
                        auth_json ->> 'access_token',
                        auth_json ->> 'accessToken'
                    )
                 FROM llm_codex_accounts
                 WHERE account_name = ANY($1)
                 ORDER BY account_name",
                &[&account_names.to_vec()],
            )
            .await
            .context("list postgres codex route candidates by names")?;
        Ok(rows
            .into_iter()
            .map(|row| CodexRouteCandidateRow {
                account_name: row.get(0),
                status: row.get(1),
                settings_json: row.get(2),
                last_refresh_at_ms: row.get(3),
                last_error: row.get(4),
                access_token: row.get(5),
            })
            .collect())
    }

    async fn list_kiro_accounts_rows(&self) -> anyhow::Result<Vec<KiroAccountRecord>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    account_name, auth_method, account_id, profile_arn, user_id, status,
                    auth_json::text, max_concurrency, min_start_interval_ms, proxy_config_id,
                    last_refresh_at_ms, last_error, created_at_ms, updated_at_ms
                 FROM llm_kiro_accounts
                 ORDER BY created_at_ms DESC, account_name DESC",
                &[],
            )
            .await
            .context("list kiro accounts")?;
        Ok(rows.into_iter().map(decode_kiro_account_row).collect())
    }

    async fn list_kiro_admin_account_rows(&self) -> anyhow::Result<Vec<KiroAdminAccountListRow>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    account_name,
                    auth_method,
                    profile_arn,
                    user_id,
                    status,
                    NULLIF(BTRIM(auth_json ->> 'provider'), ''),
                    NULLIF(BTRIM(auth_json ->> 'email'), ''),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'expiresAt',
                                auth_json ->> 'expires_at'
                            )
                        ),
                        ''
                    ),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'profileArn',
                                auth_json ->> 'profile_arn'
                            )
                        ),
                        ''
                    ),
                    COALESCE(
                        NULLIF(BTRIM(auth_json ->> 'refreshToken'), ''),
                        NULLIF(BTRIM(auth_json ->> 'refresh_token'), '')
                    ) IS NOT NULL,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'disabled') = 'boolean'
                        THEN (auth_json ->> 'disabled')::boolean
                        ELSE false
                    END,
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'disabledReason',
                                auth_json ->> 'disabled_reason'
                            )
                        ),
                        ''
                    ),
                    NULLIF(BTRIM(auth_json ->> 'source'), ''),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'sourceDbPath',
                                auth_json ->> 'source_db_path'
                            )
                        ),
                        ''
                    ),
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'lastImportedAt') = 'number'
                        THEN (auth_json ->> 'lastImportedAt')::bigint
                        WHEN jsonb_typeof(auth_json -> 'last_imported_at') = 'number'
                        THEN (auth_json ->> 'last_imported_at')::bigint
                        ELSE NULL
                    END,
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'subscriptionTitle',
                                auth_json ->> 'subscription_title'
                            )
                        ),
                        ''
                    ),
                    NULLIF(BTRIM(auth_json ->> 'region'), ''),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'authRegion',
                                auth_json ->> 'auth_region'
                            )
                        ),
                        ''
                    ),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'apiRegion',
                                auth_json ->> 'api_region'
                            )
                        ),
                        ''
                    ),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'machineId',
                                auth_json ->> 'machine_id'
                            )
                        ),
                        ''
                    ),
                    max_concurrency,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'kiroChannelMaxConcurrency') = 'number'
                        THEN (auth_json ->> 'kiroChannelMaxConcurrency')::bigint
                        WHEN jsonb_typeof(auth_json -> 'kiro_channel_max_concurrency')
                            = 'number'
                        THEN (auth_json ->> 'kiro_channel_max_concurrency')::bigint
                        ELSE NULL
                    END,
                    min_start_interval_ms,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'kiroChannelMinStartIntervalMs')
                            = 'number'
                        THEN (auth_json ->> 'kiroChannelMinStartIntervalMs')::bigint
                        WHEN jsonb_typeof(auth_json -> 'kiro_channel_min_start_interval_ms')
                            = 'number'
                        THEN (auth_json ->> 'kiro_channel_min_start_interval_ms')::bigint
                        ELSE NULL
                    END,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'minimumRemainingCreditsBeforeBlock')
                            = 'number'
                        THEN (auth_json ->> 'minimumRemainingCreditsBeforeBlock')::double precision
                        WHEN jsonb_typeof(
                            auth_json -> 'minimum_remaining_credits_before_block'
                        ) = 'number'
                        THEN (
                            auth_json ->> 'minimum_remaining_credits_before_block'
                        )::double precision
                        ELSE NULL
                    END,
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'proxyMode',
                                auth_json ->> 'proxy_mode'
                            )
                        ),
                        ''
                    ),
                    proxy_config_id,
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'proxyConfigId',
                                auth_json ->> 'proxy_config_id'
                            )
                        ),
                        ''
                    ),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'proxyUrl',
                                auth_json ->> 'proxy_url'
                            )
                        ),
                        ''
                    ),
                    last_error
                 FROM llm_kiro_accounts
                 ORDER BY created_at_ms DESC, account_name DESC",
                &[],
            )
            .await
            .context("list postgres kiro admin account rows")?;
        Ok(rows
            .into_iter()
            .map(decode_kiro_admin_account_list_row)
            .collect())
    }

    async fn admin_kiro_accounts_summary(
        &self,
    ) -> anyhow::Result<core_store::AdminAccountsSummary> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_one(
                "SELECT
                    COUNT(*)::BIGINT,
                    COALESCE(SUM(CASE
                        WHEN status = 'active'
                            AND NOT CASE
                                WHEN jsonb_typeof(auth_json -> 'disabled') = 'boolean'
                                THEN (auth_json ->> 'disabled')::boolean
                                ELSE false
                            END
                        THEN 1 ELSE 0 END), 0)::BIGINT,
                    COALESCE(SUM(CASE
                        WHEN status <> 'active'
                            OR CASE
                                WHEN jsonb_typeof(auth_json -> 'disabled') = 'boolean'
                                THEN (auth_json ->> 'disabled')::boolean
                                ELSE false
                            END
                        THEN 1 ELSE 0 END), 0)::BIGINT,
                    COALESCE(SUM(CASE WHEN status = 'unavailable' THEN 1 ELSE 0 END), 0)::BIGINT
                 FROM llm_kiro_accounts",
                &[],
            )
            .await
            .context("summarize postgres kiro accounts")?;
        Ok(core_store::AdminAccountsSummary {
            total: row.get::<_, i64>(0).max(0) as usize,
            active_count: row.get::<_, i64>(1).max(0) as usize,
            disabled_count: row.get::<_, i64>(2).max(0) as usize,
            unavailable_count: row.get::<_, i64>(3).max(0) as usize,
        })
    }

    async fn list_kiro_admin_account_rows_page_filtered(
        &self,
        page: AdminPageRequest,
        prefix: Option<&str>,
    ) -> anyhow::Result<(Vec<KiroAdminAccountListRow>, usize)> {
        self.ensure_connection_alive()?;
        let normalized_prefix = prefix
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase);
        let total = if let Some(prefix) = normalized_prefix.as_deref() {
            self.client
                .query_one(
                    "SELECT COUNT(*)
                     FROM llm_kiro_accounts
                     WHERE lower(account_name) LIKE $1 || '%'",
                    &[&prefix],
                )
                .await
                .context("count filtered postgres kiro admin account rows")?
                .get::<_, i64>(0)
                .max(0) as usize
        } else {
            self.client
                .query_one("SELECT COUNT(*) FROM llm_kiro_accounts", &[])
                .await
                .context("count postgres kiro admin account rows")?
                .get::<_, i64>(0)
                .max(0) as usize
        };
        let sql = if normalized_prefix.is_some() {
            "SELECT
                    account_name,
                    auth_method,
                    profile_arn,
                    user_id,
                    status,
                    NULLIF(BTRIM(auth_json ->> 'provider'), ''),
                    NULLIF(BTRIM(auth_json ->> 'email'), ''),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'expiresAt',
                                auth_json ->> 'expires_at'
                            )
                        ),
                        ''
                    ),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'profileArn',
                                auth_json ->> 'profile_arn'
                            )
                        ),
                        ''
                    ),
                    COALESCE(
                        NULLIF(BTRIM(auth_json ->> 'refreshToken'), ''),
                        NULLIF(BTRIM(auth_json ->> 'refresh_token'), '')
                    ) IS NOT NULL,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'disabled') = 'boolean'
                        THEN (auth_json ->> 'disabled')::boolean
                        ELSE false
                    END,
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'disabledReason',
                                auth_json ->> 'disabled_reason'
                            )
                        ),
                        ''
                    ),
                    NULLIF(BTRIM(auth_json ->> 'source'), ''),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'sourceDbPath',
                                auth_json ->> 'source_db_path'
                            )
                        ),
                        ''
                    ),
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'lastImportedAt') = 'number'
                        THEN (auth_json ->> 'lastImportedAt')::bigint
                        WHEN jsonb_typeof(auth_json -> 'last_imported_at') = 'number'
                        THEN (auth_json ->> 'last_imported_at')::bigint
                        ELSE NULL
                    END,
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'subscriptionTitle',
                                auth_json ->> 'subscription_title'
                            )
                        ),
                        ''
                    ),
                    NULLIF(BTRIM(auth_json ->> 'region'), ''),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'authRegion',
                                auth_json ->> 'auth_region'
                            )
                        ),
                        ''
                    ),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'apiRegion',
                                auth_json ->> 'api_region'
                            )
                        ),
                        ''
                    ),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'machineId',
                                auth_json ->> 'machine_id'
                            )
                        ),
                        ''
                    ),
                    max_concurrency,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'kiroChannelMaxConcurrency') = 'number'
                        THEN (auth_json ->> 'kiroChannelMaxConcurrency')::bigint
                        WHEN jsonb_typeof(auth_json -> 'kiro_channel_max_concurrency')
                            = 'number'
                        THEN (auth_json ->> 'kiro_channel_max_concurrency')::bigint
                        ELSE NULL
                    END,
                    min_start_interval_ms,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'kiroChannelMinStartIntervalMs')
                            = 'number'
                        THEN (auth_json ->> 'kiroChannelMinStartIntervalMs')::bigint
                        WHEN jsonb_typeof(auth_json -> 'kiro_channel_min_start_interval_ms')
                            = 'number'
                        THEN (auth_json ->> 'kiro_channel_min_start_interval_ms')::bigint
                        ELSE NULL
                    END,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'minimumRemainingCreditsBeforeBlock')
                            = 'number'
                        THEN (auth_json ->> 'minimumRemainingCreditsBeforeBlock')::double precision
                        WHEN jsonb_typeof(
                            auth_json -> 'minimum_remaining_credits_before_block'
                        ) = 'number'
                        THEN (
                            auth_json ->> 'minimum_remaining_credits_before_block'
                        )::double precision
                        ELSE NULL
                    END,
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'proxyMode',
                                auth_json ->> 'proxy_mode'
                            )
                        ),
                        ''
                    ),
                    proxy_config_id,
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'proxyConfigId',
                                auth_json ->> 'proxy_config_id'
                            )
                        ),
                        ''
                    ),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'proxyUrl',
                                auth_json ->> 'proxy_url'
                            )
                        ),
                        ''
                    ),
                    last_error
                 FROM llm_kiro_accounts
                 WHERE lower(account_name) LIKE $1 || '%'
                 ORDER BY created_at_ms DESC, account_name DESC
                 LIMIT $2 OFFSET $3"
        } else {
            "SELECT
                    account_name,
                    auth_method,
                    profile_arn,
                    user_id,
                    status,
                    NULLIF(BTRIM(auth_json ->> 'provider'), ''),
                    NULLIF(BTRIM(auth_json ->> 'email'), ''),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'expiresAt',
                                auth_json ->> 'expires_at'
                            )
                        ),
                        ''
                    ),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'profileArn',
                                auth_json ->> 'profile_arn'
                            )
                        ),
                        ''
                    ),
                    COALESCE(
                        NULLIF(BTRIM(auth_json ->> 'refreshToken'), ''),
                        NULLIF(BTRIM(auth_json ->> 'refresh_token'), '')
                    ) IS NOT NULL,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'disabled') = 'boolean'
                        THEN (auth_json ->> 'disabled')::boolean
                        ELSE false
                    END,
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'disabledReason',
                                auth_json ->> 'disabled_reason'
                            )
                        ),
                        ''
                    ),
                    NULLIF(BTRIM(auth_json ->> 'source'), ''),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'sourceDbPath',
                                auth_json ->> 'source_db_path'
                            )
                        ),
                        ''
                    ),
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'lastImportedAt') = 'number'
                        THEN (auth_json ->> 'lastImportedAt')::bigint
                        WHEN jsonb_typeof(auth_json -> 'last_imported_at') = 'number'
                        THEN (auth_json ->> 'last_imported_at')::bigint
                        ELSE NULL
                    END,
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'subscriptionTitle',
                                auth_json ->> 'subscription_title'
                            )
                        ),
                        ''
                    ),
                    NULLIF(BTRIM(auth_json ->> 'region'), ''),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'authRegion',
                                auth_json ->> 'auth_region'
                            )
                        ),
                        ''
                    ),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'apiRegion',
                                auth_json ->> 'api_region'
                            )
                        ),
                        ''
                    ),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'machineId',
                                auth_json ->> 'machine_id'
                            )
                        ),
                        ''
                    ),
                    max_concurrency,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'kiroChannelMaxConcurrency') = 'number'
                        THEN (auth_json ->> 'kiroChannelMaxConcurrency')::bigint
                        WHEN jsonb_typeof(auth_json -> 'kiro_channel_max_concurrency')
                            = 'number'
                        THEN (auth_json ->> 'kiro_channel_max_concurrency')::bigint
                        ELSE NULL
                    END,
                    min_start_interval_ms,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'kiroChannelMinStartIntervalMs')
                            = 'number'
                        THEN (auth_json ->> 'kiroChannelMinStartIntervalMs')::bigint
                        WHEN jsonb_typeof(auth_json -> 'kiro_channel_min_start_interval_ms')
                            = 'number'
                        THEN (auth_json ->> 'kiro_channel_min_start_interval_ms')::bigint
                        ELSE NULL
                    END,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'minimumRemainingCreditsBeforeBlock')
                            = 'number'
                        THEN (auth_json ->> 'minimumRemainingCreditsBeforeBlock')::double precision
                        WHEN jsonb_typeof(
                            auth_json -> 'minimum_remaining_credits_before_block'
                        ) = 'number'
                        THEN (
                            auth_json ->> 'minimum_remaining_credits_before_block'
                        )::double precision
                        ELSE NULL
                    END,
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'proxyMode',
                                auth_json ->> 'proxy_mode'
                            )
                        ),
                        ''
                    ),
                    proxy_config_id,
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'proxyConfigId',
                                auth_json ->> 'proxy_config_id'
                            )
                        ),
                        ''
                    ),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'proxyUrl',
                                auth_json ->> 'proxy_url'
                            )
                        ),
                        ''
                    ),
                    last_error
                 FROM llm_kiro_accounts
                 ORDER BY created_at_ms DESC, account_name DESC
                 LIMIT $1 OFFSET $2"
        };
        let rows = if let Some(prefix) = normalized_prefix.as_deref() {
            self.client
                .query(sql, &[&prefix, &(page.limit.max(1) as i64), &(page.offset as i64)])
                .await
                .context("list filtered postgres kiro admin account rows page")?
        } else {
            self.client
                .query(sql, &[&(page.limit.max(1) as i64), &(page.offset as i64)])
                .await
                .context("list postgres kiro admin account rows page")?
        };
        Ok((
            rows.into_iter()
                .map(decode_kiro_admin_account_list_row)
                .collect(),
            total,
        ))
    }

    async fn get_kiro_account_row(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<KiroAccountRecord>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT
                    account_name, auth_method, account_id, profile_arn, user_id, status,
                    auth_json::text, max_concurrency, min_start_interval_ms, proxy_config_id,
                    last_refresh_at_ms, last_error, created_at_ms, updated_at_ms
                 FROM llm_kiro_accounts
                 WHERE account_name = $1",
                &[&account_name],
            )
            .await
            .context("load kiro account")?;
        Ok(row.map(decode_kiro_account_row))
    }

    async fn list_kiro_route_candidate_rows(&self) -> anyhow::Result<Vec<KiroRouteCandidateRow>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    account_name,
                    profile_arn,
                    user_id,
                    status,
                    max_concurrency,
                    min_start_interval_ms,
                    proxy_config_id,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'disabled') = 'boolean'
                        THEN (auth_json ->> 'disabled')::boolean
                        ELSE FALSE
                    END,
                    COALESCE(
                        CASE
                            WHEN jsonb_typeof(auth_json -> 'minimumRemainingCreditsBeforeBlock') = \
                 'number'
                            THEN (auth_json ->> 'minimumRemainingCreditsBeforeBlock')::double \
                 precision
                        END,
                        CASE
                            WHEN jsonb_typeof(auth_json -> \
                 'minimum_remaining_credits_before_block') = 'number'
                            THEN (auth_json ->> 'minimum_remaining_credits_before_block')::double \
                 precision
                        END,
                        0.0
                    ),
                    NULLIF(COALESCE(auth_json ->> 'profileArn', auth_json ->> 'profile_arn'), ''),
                    NULLIF(COALESCE(auth_json ->> 'apiRegion', auth_json ->> 'api_region', \
                 auth_json ->> 'region'), ''),
                    NULLIF(COALESCE(auth_json ->> 'proxyMode', auth_json ->> 'proxy_mode'), ''),
                    NULLIF(COALESCE(auth_json ->> 'proxyConfigId', auth_json ->> \
                 'proxy_config_id'), '')
                 FROM llm_kiro_accounts
                 ORDER BY account_name",
                &[],
            )
            .await
            .context("list postgres kiro route candidates")?;
        Ok(rows
            .into_iter()
            .map(|row| KiroRouteCandidateRow {
                account_name: row.get(0),
                profile_arn: row.get(1),
                user_id: row.get(2),
                status: row.get(3),
                max_concurrency: row.get(4),
                min_start_interval_ms: row.get(5),
                proxy_config_id: row.get(6),
                disabled: row.get(7),
                minimum_remaining_credits_before_block: row.get::<_, f64>(8).max(0.0),
                auth_profile_arn: row.get(9),
                api_region: row.get(10),
                proxy_mode: row.get(11),
                auth_proxy_config_id: row.get(12),
            })
            .collect())
    }

    async fn list_kiro_route_candidate_rows_by_names(
        &self,
        account_names: &[String],
    ) -> anyhow::Result<Vec<KiroRouteCandidateRow>> {
        if account_names.is_empty() {
            return Ok(Vec::new());
        }
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    account_name,
                    profile_arn,
                    user_id,
                    status,
                    max_concurrency,
                    min_start_interval_ms,
                    proxy_config_id,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'disabled') = 'boolean'
                        THEN (auth_json ->> 'disabled')::boolean
                        ELSE FALSE
                    END,
                    COALESCE(
                        CASE
                            WHEN jsonb_typeof(auth_json -> 'minimumRemainingCreditsBeforeBlock') = \
                 'number'
                            THEN (auth_json ->> 'minimumRemainingCreditsBeforeBlock')::double \
                 precision
                        END,
                        CASE
                            WHEN jsonb_typeof(auth_json -> \
                 'minimum_remaining_credits_before_block') = 'number'
                            THEN (auth_json ->> 'minimum_remaining_credits_before_block')::double \
                 precision
                        END,
                        0.0
                    ),
                    NULLIF(COALESCE(auth_json ->> 'profileArn', auth_json ->> 'profile_arn'), ''),
                    NULLIF(COALESCE(auth_json ->> 'apiRegion', auth_json ->> 'api_region', \
                 auth_json ->> 'region'), ''),
                    NULLIF(COALESCE(auth_json ->> 'proxyMode', auth_json ->> 'proxy_mode'), ''),
                    NULLIF(COALESCE(auth_json ->> 'proxyConfigId', auth_json ->> \
                 'proxy_config_id'), '')
                 FROM llm_kiro_accounts
                 WHERE account_name = ANY($1)
                 ORDER BY account_name",
                &[&account_names.to_vec()],
            )
            .await
            .context("list postgres kiro route candidates by names")?;
        Ok(rows
            .into_iter()
            .map(|row| KiroRouteCandidateRow {
                account_name: row.get(0),
                profile_arn: row.get(1),
                user_id: row.get(2),
                status: row.get(3),
                max_concurrency: row.get(4),
                min_start_interval_ms: row.get(5),
                proxy_config_id: row.get(6),
                disabled: row.get(7),
                minimum_remaining_credits_before_block: row.get::<_, f64>(8).max(0.0),
                auth_profile_arn: row.get(9),
                api_region: row.get(10),
                proxy_mode: row.get(11),
                auth_proxy_config_id: row.get(12),
            })
            .collect())
    }

    async fn list_kiro_cached_status_parts_rows(
        &self,
    ) -> anyhow::Result<BTreeMap<String, KiroCachedStatusParts>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT account_name, balance_json::text, cache_json::text
                 FROM llm_kiro_status_cache",
                &[],
            )
            .await
            .context("list kiro cached status")?;
        let mut status_by_account = BTreeMap::new();
        for row in rows {
            let account_name: String = row.get(0);
            let balance_json: String = row.get(1);
            let cache_json: String = row.get(2);
            let balance = serde_json::from_str::<Option<AdminKiroBalanceView>>(&balance_json)
                .context("decode kiro cached balance")?;
            let cache = serde_json::from_str::<AdminKiroCacheView>(&cache_json)
                .context("decode kiro cached cache view")?;
            status_by_account.insert(account_name, (balance, cache));
        }
        Ok(status_by_account)
    }

    async fn list_kiro_cached_status_parts_rows_by_names(
        &self,
        account_names: &[String],
    ) -> anyhow::Result<BTreeMap<String, KiroCachedStatusParts>> {
        if account_names.is_empty() {
            return Ok(BTreeMap::new());
        }
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT account_name, balance_json::text, cache_json::text
                 FROM llm_kiro_status_cache
                 WHERE account_name = ANY($1)",
                &[&account_names.to_vec()],
            )
            .await
            .context("list kiro cached status by names")?;
        let mut status_by_account = BTreeMap::new();
        for row in rows {
            let account_name: String = row.get(0);
            let balance_json: String = row.get(1);
            let cache_json: String = row.get(2);
            let balance = serde_json::from_str::<Option<AdminKiroBalanceView>>(&balance_json)
                .context("decode kiro cached balance")?;
            let cache = serde_json::from_str::<AdminKiroCacheView>(&cache_json)
                .context("decode kiro cached cache view")?;
            status_by_account.insert(account_name, (balance, cache));
        }
        Ok(status_by_account)
    }

    async fn get_kiro_cached_status_parts_row(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<KiroCachedStatusParts>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT balance_json::text, cache_json::text
                 FROM llm_kiro_status_cache
                 WHERE account_name = $1",
                &[&account_name],
            )
            .await
            .context("load kiro cached status")?;
        row.map(|row| {
            let balance_json: String = row.get(0);
            let cache_json: String = row.get(1);
            let balance = serde_json::from_str::<Option<AdminKiroBalanceView>>(&balance_json)
                .context("decode kiro cached balance")?;
            let cache = serde_json::from_str::<AdminKiroCacheView>(&cache_json)
                .context("decode kiro cached cache view")?;
            Ok((balance, cache))
        })
        .transpose()
    }

    async fn load_provider_proxy_resolution_context(
        &self,
        provider_type: &str,
    ) -> anyhow::Result<ProviderProxyResolutionContext> {
        let proxy_configs_by_id = self
            .load_admin_proxy_configs_cached()
            .await?
            .into_iter()
            .map(|proxy| (proxy.id.clone(), proxy))
            .collect::<BTreeMap<_, _>>();
        let binding = self.load_admin_proxy_binding_cached(provider_type).await?;
        Ok(ProviderProxyResolutionContext {
            proxy_configs_by_id,
            binding,
        })
    }

    async fn resolve_route_account_names(
        &self,
        provider_type: &str,
        route: &KeyRouteConfig,
        default_active_account_names: Vec<String>,
    ) -> anyhow::Result<Vec<String>> {
        let strategy = route.route_strategy.as_deref().unwrap_or("auto");
        match strategy {
            "fixed" => {
                let account_name = if let Some(group_id) = route.account_group_id.as_deref() {
                    let group = self
                        .get_admin_account_group_row(group_id)
                        .await?
                        .with_context(|| {
                            format!("configured account_group_id `{group_id}` does not exist")
                        })?;
                    if group.provider_type != provider_type {
                        anyhow::bail!(
                            "configured account_group_id belongs to a different provider"
                        );
                    }
                    if group.account_names.len() != 1 {
                        anyhow::bail!(
                            "fixed route_strategy requires an account group with exactly one \
                             account"
                        );
                    }
                    group.account_names[0].clone()
                } else {
                    route
                        .fixed_account_name
                        .clone()
                        .filter(|value| !value.trim().is_empty())
                        .context("fixed route_strategy requires account_group_id")?
                };
                Ok(vec![account_name])
            },
            "auto" => {
                if let Some(group_id) = route.account_group_id.as_deref() {
                    let group = self
                        .get_admin_account_group_row(group_id)
                        .await?
                        .with_context(|| {
                            format!("configured account_group_id `{group_id}` does not exist")
                        })?;
                    if group.provider_type != provider_type {
                        anyhow::bail!(
                            "configured account_group_id belongs to a different provider"
                        );
                    }
                    if !group.account_names.is_empty() {
                        return Ok(group.account_names);
                    }
                }
                if let Some(account_names) =
                    decode_optional_json::<Vec<String>>(route.auto_account_names_json.as_deref())
                {
                    if !account_names.is_empty() {
                        return Ok(account_names);
                    }
                }
                Ok(default_active_account_names)
            },
            other => anyhow::bail!("unsupported route strategy `{other}`"),
        }
    }

    async fn load_codex_admin_account_view_context(
        &self,
    ) -> anyhow::Result<CodexAdminAccountViewContext> {
        let proxy_configs_by_id = self
            .list_admin_proxy_configs_rows()
            .await?
            .into_iter()
            .map(|proxy| (proxy.id.clone(), proxy))
            .collect::<BTreeMap<_, _>>();
        let codex_proxy_binding = self
            .load_admin_proxy_binding_from_configs(core_store::PROVIDER_CODEX, &proxy_configs_by_id)
            .await?;
        Ok(CodexAdminAccountViewContext {
            proxy_configs_by_id,
            codex_proxy_binding,
        })
    }

    fn resolve_codex_account_proxy_view_with_context(
        &self,
        settings: &CodexAccountSettings,
        context: &CodexAdminAccountViewContext,
    ) -> (String, Option<String>, Option<String>) {
        match settings.proxy_mode.as_str() {
            "none" => ("none".to_string(), None, None),
            "fixed" => {
                let Some(proxy_id) = settings.proxy_config_id.as_deref() else {
                    return ("invalid".to_string(), None, None);
                };
                match context.proxy_configs_by_id.get(proxy_id) {
                    Some(proxy) if proxy.status == core_store::KEY_STATUS_ACTIVE => (
                        "fixed".to_string(),
                        Some(proxy.proxy_url.clone()),
                        Some(proxy.name.clone()),
                    ),
                    Some(proxy) => ("invalid".to_string(), None, Some(proxy.name.clone())),
                    None => ("invalid".to_string(), None, None),
                }
            },
            _ => (
                context.codex_proxy_binding.effective_source.clone(),
                context.codex_proxy_binding.effective_proxy_url.clone(),
                context
                    .codex_proxy_binding
                    .effective_proxy_config_name
                    .clone(),
            ),
        }
    }

    fn admin_codex_account_from_list_row_with_context(
        &self,
        row: &CodexAdminAccountListRow,
        context: &CodexAdminAccountViewContext,
    ) -> AdminCodexAccount {
        let settings = CodexAccountSettings {
            map_gpt53_codex_to_spark: row.map_gpt53_codex_to_spark,
            auth_refresh_enabled: row.auth_refresh_enabled,
            route_weight_tier: row.route_weight_tier.clone(),
            proxy_mode: row.proxy_mode.clone(),
            proxy_config_id: row.proxy_config_id.clone(),
            request_max_concurrency: row
                .request_max_concurrency
                .and_then(non_negative_i64_to_u64),
            request_min_start_interval_ms: row
                .request_min_start_interval_ms
                .and_then(non_negative_i64_to_u64),
        };
        let (effective_proxy_source, effective_proxy_url, effective_proxy_config_name) =
            self.resolve_codex_account_proxy_view_with_context(&settings, context);
        AdminCodexAccount {
            name: row.account_name.clone(),
            status: row.status.clone(),
            account_id: row.account_id.clone(),
            plan_type: row.plan_type.clone(),
            route_weight_tier: settings
                .route_weight_tier
                .clone()
                .unwrap_or_else(|| "auto".to_string()),
            primary_remaining_percent: row.primary_remaining_percent,
            secondary_remaining_percent: row.secondary_remaining_percent,
            map_gpt53_codex_to_spark: settings.map_gpt53_codex_to_spark,
            auto_refresh_enabled: settings.auth_refresh_enabled,
            request_max_concurrency: settings.request_max_concurrency,
            request_min_start_interval_ms: settings.request_min_start_interval_ms,
            proxy_mode: settings.proxy_mode,
            proxy_config_id: settings.proxy_config_id,
            effective_proxy_source,
            effective_proxy_url,
            effective_proxy_config_name,
            last_refresh: row.last_refresh_at_ms,
            access_token_expires_at: core_store::codex_access_token_expires_at_ms(
                row.access_token.as_deref(),
            ),
            auth_refresh_error_message: row.last_error.clone(),
            last_usage_checked_at: row.last_usage_checked_at_ms,
            last_usage_success_at: row.last_usage_success_at_ms,
            usage_error_message: row.usage_error_message.clone(),
        }
    }

    fn admin_codex_account_from_record_with_context(
        &self,
        record: &CodexAccountRecord,
        context: &CodexAdminAccountViewContext,
    ) -> anyhow::Result<AdminCodexAccount> {
        let settings = decode_codex_account_settings(&record.settings_json)?;
        let (effective_proxy_source, effective_proxy_url, effective_proxy_config_name) =
            self.resolve_codex_account_proxy_view_with_context(&settings, context);
        Ok(AdminCodexAccount {
            name: record.account_name.clone(),
            status: record.status.clone(),
            account_id: record.account_id.clone(),
            plan_type: None,
            route_weight_tier: settings
                .route_weight_tier
                .clone()
                .unwrap_or_else(|| "auto".to_string()),
            primary_remaining_percent: None,
            secondary_remaining_percent: None,
            map_gpt53_codex_to_spark: settings.map_gpt53_codex_to_spark,
            auto_refresh_enabled: settings.auth_refresh_enabled,
            request_max_concurrency: settings.request_max_concurrency,
            request_min_start_interval_ms: settings.request_min_start_interval_ms,
            proxy_mode: settings.proxy_mode,
            proxy_config_id: settings.proxy_config_id,
            effective_proxy_source,
            effective_proxy_url,
            effective_proxy_config_name,
            last_refresh: record.last_refresh_at_ms,
            access_token_expires_at: core_store::codex_auth_access_token_expires_at_ms(
                &record.auth_json,
            ),
            auth_refresh_error_message: record.last_error.clone(),
            last_usage_checked_at: None,
            last_usage_success_at: None,
            usage_error_message: None,
        })
    }

    async fn admin_codex_account_from_record(
        &self,
        record: &CodexAccountRecord,
    ) -> anyhow::Result<AdminCodexAccount> {
        let context = self.load_codex_admin_account_view_context().await?;
        self.admin_codex_account_from_record_with_context(record, &context)
    }

    async fn load_kiro_admin_account_view_context(
        &self,
    ) -> anyhow::Result<KiroAdminAccountViewContext> {
        let refresh_interval_seconds = self
            .load_runtime_config_record_cached()
            .await?
            .map(|config| config.kiro_status_refresh_max_interval_seconds.max(0) as u64)
            .unwrap_or(core_store::DEFAULT_KIRO_STATUS_REFRESH_MAX_INTERVAL_SECONDS);
        let default_cache = AdminKiroCacheView {
            refresh_interval_seconds,
            ..AdminKiroCacheView::default()
        };
        let status_by_account = self.list_kiro_cached_status_parts_rows().await?;
        let proxy_configs_by_id = self
            .list_admin_proxy_configs_rows()
            .await?
            .into_iter()
            .map(|proxy| (proxy.id.clone(), proxy))
            .collect::<BTreeMap<_, _>>();
        let kiro_proxy_binding = self
            .load_admin_proxy_binding_from_configs(core_store::PROVIDER_KIRO, &proxy_configs_by_id)
            .await?;
        Ok(KiroAdminAccountViewContext {
            default_cache,
            status_by_account,
            proxy_configs_by_id,
            kiro_proxy_binding,
        })
    }

    fn resolve_kiro_account_proxy_view_with_context(
        &self,
        proxy_mode: &str,
        proxy_config_id: Option<&str>,
        context: &KiroAdminAccountViewContext,
    ) -> (String, Option<String>, Option<String>) {
        match proxy_mode {
            "none" => ("none".to_string(), None, None),
            "fixed" => {
                let Some(proxy_id) = proxy_config_id else {
                    return ("invalid".to_string(), None, None);
                };
                match context.proxy_configs_by_id.get(proxy_id) {
                    Some(proxy) if proxy.status == core_store::KEY_STATUS_ACTIVE => (
                        "fixed".to_string(),
                        Some(proxy.proxy_url.clone()),
                        Some(proxy.name.clone()),
                    ),
                    Some(proxy) => ("invalid".to_string(), None, Some(proxy.name.clone())),
                    None => ("invalid".to_string(), None, None),
                }
            },
            _ => (
                context.kiro_proxy_binding.effective_source.clone(),
                context.kiro_proxy_binding.effective_proxy_url.clone(),
                context
                    .kiro_proxy_binding
                    .effective_proxy_config_name
                    .clone(),
            ),
        }
    }

    fn admin_kiro_account_from_list_row_with_context(
        &self,
        row: &KiroAdminAccountListRow,
        context: &KiroAdminAccountViewContext,
    ) -> AdminKiroAccount {
        let (balance, cache) = context
            .status_by_account
            .get(&row.account_name)
            .cloned()
            .unwrap_or_else(|| (None, context.default_cache.clone()));
        let proxy_mode = row.proxy_mode.clone().unwrap_or_else(|| {
            if row
                .proxy_config_id
                .as_deref()
                .or(row.auth_proxy_config_id.as_deref())
                .is_some()
            {
                "fixed".to_string()
            } else {
                "inherit".to_string()
            }
        });
        let proxy_config_id = row
            .proxy_config_id
            .clone()
            .or_else(|| row.auth_proxy_config_id.clone());
        let (effective_proxy_source, effective_proxy_url, effective_proxy_config_name) = self
            .resolve_kiro_account_proxy_view_with_context(
                &proxy_mode,
                proxy_config_id.as_deref(),
                context,
            );
        let disabled = row.disabled_json || row.status != core_store::KEY_STATUS_ACTIVE;
        let disabled_reason = row
            .disabled_reason
            .clone()
            .or_else(|| row.last_error.clone());
        let balance = if disabled { None } else { balance };
        let subscription_title = balance
            .as_ref()
            .and_then(|value| value.subscription_title.clone())
            .or_else(|| row.subscription_title.clone());
        AdminKiroAccount {
            name: row.account_name.clone(),
            auth_method: row.auth_method.clone(),
            provider: row.provider.clone(),
            upstream_user_id: balance
                .as_ref()
                .and_then(|value| value.user_id.clone())
                .or_else(|| row.user_id.clone()),
            email: row.email.clone(),
            expires_at: row.expires_at.clone(),
            profile_arn: row
                .profile_arn
                .clone()
                .or_else(|| row.auth_profile_arn.clone()),
            has_refresh_token: row.has_refresh_token,
            disabled,
            disabled_reason,
            source: row.source.clone(),
            source_db_path: row.source_db_path.clone(),
            last_imported_at: row.last_imported_at,
            subscription_title,
            region: row.region.clone(),
            auth_region: row.auth_region.clone(),
            api_region: row.api_region.clone(),
            machine_id: row.machine_id.clone(),
            kiro_channel_max_concurrency: row
                .max_concurrency
                .and_then(non_negative_i64_to_u64)
                .or_else(|| row.auth_max_concurrency.and_then(non_negative_i64_to_u64))
                .unwrap_or(core_store::DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY)
                .max(1),
            kiro_channel_min_start_interval_ms: row
                .min_start_interval_ms
                .and_then(non_negative_i64_to_u64)
                .or_else(|| {
                    row.auth_min_start_interval_ms
                        .and_then(non_negative_i64_to_u64)
                })
                .unwrap_or(core_store::DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS),
            minimum_remaining_credits_before_block: row
                .minimum_remaining_credits_before_block
                .filter(|value| value.is_finite())
                .unwrap_or(0.0)
                .max(0.0),
            proxy_mode,
            proxy_config_id,
            effective_proxy_source,
            effective_proxy_url,
            effective_proxy_config_name,
            proxy_url: row.proxy_url.clone(),
            balance,
            cache,
        }
    }

    fn admin_kiro_account_from_record_with_context(
        &self,
        record: &KiroAccountRecord,
        context: &KiroAdminAccountViewContext,
    ) -> anyhow::Result<AdminKiroAccount> {
        let auth = serde_json::from_str::<serde_json::Value>(&record.auth_json)
            .context("parse kiro auth json for admin view")?;
        let (balance, cache) = context
            .status_by_account
            .get(&record.account_name)
            .cloned()
            .unwrap_or_else(|| (None, context.default_cache.clone()));
        let proxy_mode = optional_json_string_any(&auth, &["proxyMode", "proxy_mode"])
            .unwrap_or_else(|| {
                if record.proxy_config_id.is_some() {
                    "fixed".to_string()
                } else {
                    "inherit".to_string()
                }
            });
        let proxy_config_id = record
            .proxy_config_id
            .clone()
            .or_else(|| optional_json_string_any(&auth, &["proxyConfigId", "proxy_config_id"]));
        let (effective_proxy_source, effective_proxy_url, effective_proxy_config_name) = self
            .resolve_kiro_account_proxy_view_with_context(
                &proxy_mode,
                proxy_config_id.as_deref(),
                context,
            );
        let disabled_json = optional_json_bool_any(&auth, &["disabled"]).unwrap_or(false);
        let disabled = disabled_json || record.status != core_store::KEY_STATUS_ACTIVE;
        let disabled_reason =
            optional_json_string_any(&auth, &["disabledReason", "disabled_reason"])
                .or_else(|| record.last_error.clone());
        let balance = if disabled { None } else { balance };
        let subscription_title = balance
            .as_ref()
            .and_then(|value| value.subscription_title.clone())
            .or_else(|| {
                optional_json_string_any(&auth, &["subscriptionTitle", "subscription_title"])
            });
        Ok(AdminKiroAccount {
            name: record.account_name.clone(),
            auth_method: record.auth_method.clone(),
            provider: optional_json_string_any(&auth, &["provider"]),
            upstream_user_id: balance
                .as_ref()
                .and_then(|value| value.user_id.clone())
                .or_else(|| record.user_id.clone()),
            email: optional_json_string_any(&auth, &["email"]),
            expires_at: optional_json_string_any(&auth, &["expiresAt", "expires_at"]),
            profile_arn: record
                .profile_arn
                .clone()
                .or_else(|| optional_json_string_any(&auth, &["profileArn", "profile_arn"])),
            has_refresh_token: optional_json_string_any(&auth, &["refreshToken", "refresh_token"])
                .is_some(),
            disabled,
            disabled_reason,
            source: optional_json_string_any(&auth, &["source"]),
            source_db_path: optional_json_string_any(&auth, &["sourceDbPath", "source_db_path"]),
            last_imported_at: optional_json_i64_any(&auth, &["lastImportedAt", "last_imported_at"]),
            subscription_title,
            region: optional_json_string_any(&auth, &["region"]),
            auth_region: optional_json_string_any(&auth, &["authRegion", "auth_region"]),
            api_region: optional_json_string_any(&auth, &["apiRegion", "api_region"]),
            machine_id: optional_json_string_any(&auth, &["machineId", "machine_id"]),
            kiro_channel_max_concurrency: record
                .max_concurrency
                .and_then(non_negative_i64_to_u64)
                .or_else(|| {
                    optional_json_u64_any(&auth, &[
                        "kiroChannelMaxConcurrency",
                        "kiro_channel_max_concurrency",
                    ])
                })
                .unwrap_or(core_store::DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY)
                .max(1),
            kiro_channel_min_start_interval_ms: record
                .min_start_interval_ms
                .and_then(non_negative_i64_to_u64)
                .or_else(|| {
                    optional_json_u64_any(&auth, &[
                        "kiroChannelMinStartIntervalMs",
                        "kiro_channel_min_start_interval_ms",
                    ])
                })
                .unwrap_or(core_store::DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS),
            minimum_remaining_credits_before_block: optional_json_f64_any(&auth, &[
                "minimumRemainingCreditsBeforeBlock",
                "minimum_remaining_credits_before_block",
            ])
            .filter(|value| value.is_finite())
            .unwrap_or(0.0)
            .max(0.0),
            proxy_mode,
            proxy_config_id,
            effective_proxy_source,
            effective_proxy_url,
            effective_proxy_config_name,
            proxy_url: optional_json_string_any(&auth, &["proxyUrl", "proxy_url"]),
            balance,
            cache,
        })
    }

    async fn admin_kiro_account_from_record(
        &self,
        record: &KiroAccountRecord,
    ) -> anyhow::Result<AdminKiroAccount> {
        let context = self.load_kiro_admin_account_view_context().await?;
        self.admin_kiro_account_from_record_with_context(record, &context)
    }

    async fn upsert_key_bundle_client(
        client: &SqlxClient,
        key: &KeyRecord,
        route: &KeyRouteConfig,
        rollup: &KeyUsageRollup,
    ) -> anyhow::Result<()> {
        client
            .execute(
                "INSERT INTO llm_keys (
                    key_id, name, secret, key_hash, status, provider_type, protocol_family,
                    public_visible, quota_billable_limit, created_at_ms, updated_at_ms
                 ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                 ON CONFLICT(key_id) DO UPDATE SET
                    name = EXCLUDED.name,
                    secret = EXCLUDED.secret,
                    key_hash = EXCLUDED.key_hash,
                    status = EXCLUDED.status,
                    provider_type = EXCLUDED.provider_type,
                    protocol_family = EXCLUDED.protocol_family,
                    public_visible = EXCLUDED.public_visible,
                    quota_billable_limit = EXCLUDED.quota_billable_limit,
                    created_at_ms = EXCLUDED.created_at_ms,
                    updated_at_ms = EXCLUDED.updated_at_ms",
                &[
                    &key.key_id,
                    &key.name,
                    &key.secret,
                    &key.key_hash,
                    &key.status,
                    &key.provider_type,
                    &key.protocol_family,
                    &key.public_visible,
                    &key.quota_billable_limit,
                    &key.created_at_ms,
                    &key.updated_at_ms,
                ],
            )
            .await
            .context("upsert postgres llm key")?;
        client
            .execute(
                "INSERT INTO llm_key_route_config (
                    key_id, route_strategy, fixed_account_name, auto_account_names_json,
                    account_group_id, model_name_map_json, request_max_concurrency,
                    request_min_start_interval_ms, codex_fast_enabled,
                    kiro_request_validation_enabled, kiro_cache_estimation_enabled,
                    kiro_zero_cache_debug_enabled, kiro_full_request_logging_enabled,
                    kiro_remote_media_resolution_enabled, kiro_latency_routing_enabled,
                    kiro_cache_policy_override_json,
                    kiro_billable_model_multipliers_override_json
                 ) VALUES (
                    $1, $2, $3, $4::jsonb, $5, $6::jsonb, $7, $8, $9, $10, $11, $12,
                    $13, $14, $15, $16::jsonb, $17::jsonb
                 )
                 ON CONFLICT(key_id) DO UPDATE SET
                    route_strategy = EXCLUDED.route_strategy,
                    fixed_account_name = EXCLUDED.fixed_account_name,
                    auto_account_names_json = EXCLUDED.auto_account_names_json,
                    account_group_id = EXCLUDED.account_group_id,
                    model_name_map_json = EXCLUDED.model_name_map_json,
                    request_max_concurrency = EXCLUDED.request_max_concurrency,
                    request_min_start_interval_ms = EXCLUDED.request_min_start_interval_ms,
                    codex_fast_enabled = EXCLUDED.codex_fast_enabled,
                    kiro_request_validation_enabled = EXCLUDED.kiro_request_validation_enabled,
                    kiro_cache_estimation_enabled = EXCLUDED.kiro_cache_estimation_enabled,
                    kiro_zero_cache_debug_enabled = EXCLUDED.kiro_zero_cache_debug_enabled,
                    kiro_full_request_logging_enabled =
                        EXCLUDED.kiro_full_request_logging_enabled,
                    kiro_remote_media_resolution_enabled =
                        EXCLUDED.kiro_remote_media_resolution_enabled,
                    kiro_latency_routing_enabled =
                        EXCLUDED.kiro_latency_routing_enabled,
                    kiro_cache_policy_override_json =
                        EXCLUDED.kiro_cache_policy_override_json,
                    kiro_billable_model_multipliers_override_json =
                        EXCLUDED.kiro_billable_model_multipliers_override_json",
                &[
                    &route.key_id,
                    &route.route_strategy,
                    &route.fixed_account_name,
                    &route.auto_account_names_json,
                    &route.account_group_id,
                    &route.model_name_map_json,
                    &route.request_max_concurrency,
                    &route.request_min_start_interval_ms,
                    &route.codex_fast_enabled,
                    &route.kiro_request_validation_enabled,
                    &route.kiro_cache_estimation_enabled,
                    &route.kiro_zero_cache_debug_enabled,
                    &route.kiro_full_request_logging_enabled,
                    &route.kiro_remote_media_resolution_enabled,
                    &route.kiro_latency_routing_enabled,
                    &route.kiro_cache_policy_override_json,
                    &route.kiro_billable_model_multipliers_override_json,
                ],
            )
            .await
            .context("upsert postgres key route config")?;
        client
            .execute(
                "INSERT INTO llm_key_usage_rollups (
                    key_id, input_uncached_tokens, input_cached_tokens, output_tokens,
                    billable_tokens, credit_total, credit_missing_events, last_used_at_ms,
                    updated_at_ms
                 ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                 ON CONFLICT(key_id) DO UPDATE SET
                    input_uncached_tokens = EXCLUDED.input_uncached_tokens,
                    input_cached_tokens = EXCLUDED.input_cached_tokens,
                    output_tokens = EXCLUDED.output_tokens,
                    billable_tokens = EXCLUDED.billable_tokens,
                    credit_total = EXCLUDED.credit_total,
                    credit_missing_events = EXCLUDED.credit_missing_events,
                    last_used_at_ms = EXCLUDED.last_used_at_ms,
                    updated_at_ms = EXCLUDED.updated_at_ms",
                &[
                    &rollup.key_id,
                    &rollup.input_uncached_tokens,
                    &rollup.input_cached_tokens,
                    &rollup.output_tokens,
                    &rollup.billable_tokens,
                    &rollup.credit_total.to_string(),
                    &rollup.credit_missing_events,
                    &rollup.last_used_at_ms,
                    &rollup.updated_at_ms,
                ],
            )
            .await
            .context("upsert postgres key usage rollup")?;
        Ok(())
    }

    async fn upsert_runtime_config_record(
        &self,
        record: &RuntimeConfigRecord,
    ) -> anyhow::Result<()> {
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "INSERT INTO llm_runtime_config (
                    id, auth_cache_ttl_seconds, max_request_body_bytes,
                    account_failure_retry_limit, codex_client_version,
                    kiro_channel_max_concurrency, kiro_channel_min_start_interval_ms,
                    codex_status_refresh_min_interval_seconds,
                    codex_status_refresh_max_interval_seconds,
                    codex_status_account_jitter_max_seconds,
                    codex_weight_free, codex_weight_plus, codex_weight_pro5x,
                    codex_weight_pro20x, kiro_status_refresh_min_interval_seconds,
                    kiro_status_refresh_max_interval_seconds,
                    kiro_status_account_jitter_max_seconds,
                    usage_event_flush_batch_size, usage_event_flush_interval_seconds,
                    usage_event_flush_max_buffer_bytes, duckdb_usage_memory_limit_mib,
                    duckdb_usage_checkpoint_threshold_mib, usage_analytics_retention_days,
                    usage_journal_enabled, usage_journal_max_file_bytes,
                    usage_journal_max_file_age_ms, usage_journal_max_files,
                    usage_journal_block_target_uncompressed_bytes,
                    usage_journal_block_max_events, usage_journal_fsync_interval_ms,
                    usage_journal_zstd_level, usage_journal_consumer_lease_ms,
                    usage_journal_delete_bad_files, usage_query_bind_addr,
                    usage_query_base_url, usage_event_maintenance_enabled,
                    usage_event_maintenance_interval_seconds,
                    usage_event_detail_retention_days, kiro_cache_kmodels_json,
                    kiro_billable_model_multipliers_json, kiro_cache_policy_json,
                    kiro_context_usage_min_request_tokens,
                    kiro_prefix_cache_mode, kiro_prefix_cache_max_tokens,
                    kiro_prefix_cache_entry_ttl_seconds,
                    kiro_conversation_anchor_max_entries,
                    kiro_conversation_anchor_ttl_seconds, updated_at_ms
                ) VALUES (
                    $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13,
                    $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24,
                    $25, $26, $27, $28, $29, $30, $31, $32, $33, $34, $35,
                    $36, $37, $38, $39::jsonb, $40::jsonb, $41::jsonb, $42,
                    $43, $44, $45, $46, $47, $48
                )
                ON CONFLICT(id) DO UPDATE SET
                    auth_cache_ttl_seconds = EXCLUDED.auth_cache_ttl_seconds,
                    max_request_body_bytes = EXCLUDED.max_request_body_bytes,
                    account_failure_retry_limit = EXCLUDED.account_failure_retry_limit,
                    codex_client_version = EXCLUDED.codex_client_version,
                    kiro_channel_max_concurrency = EXCLUDED.kiro_channel_max_concurrency,
                    kiro_channel_min_start_interval_ms =
                        EXCLUDED.kiro_channel_min_start_interval_ms,
                    codex_status_refresh_min_interval_seconds =
                        EXCLUDED.codex_status_refresh_min_interval_seconds,
                    codex_status_refresh_max_interval_seconds =
                        EXCLUDED.codex_status_refresh_max_interval_seconds,
                    codex_status_account_jitter_max_seconds =
                        EXCLUDED.codex_status_account_jitter_max_seconds,
                    codex_weight_free = EXCLUDED.codex_weight_free,
                    codex_weight_plus = EXCLUDED.codex_weight_plus,
                    codex_weight_pro5x = EXCLUDED.codex_weight_pro5x,
                    codex_weight_pro20x = EXCLUDED.codex_weight_pro20x,
                    kiro_status_refresh_min_interval_seconds =
                        EXCLUDED.kiro_status_refresh_min_interval_seconds,
                    kiro_status_refresh_max_interval_seconds =
                        EXCLUDED.kiro_status_refresh_max_interval_seconds,
                    kiro_status_account_jitter_max_seconds =
                        EXCLUDED.kiro_status_account_jitter_max_seconds,
                    usage_event_flush_batch_size = EXCLUDED.usage_event_flush_batch_size,
                    usage_event_flush_interval_seconds =
                        EXCLUDED.usage_event_flush_interval_seconds,
                    usage_event_flush_max_buffer_bytes =
                        EXCLUDED.usage_event_flush_max_buffer_bytes,
                    duckdb_usage_memory_limit_mib =
                        EXCLUDED.duckdb_usage_memory_limit_mib,
                    duckdb_usage_checkpoint_threshold_mib =
                        EXCLUDED.duckdb_usage_checkpoint_threshold_mib,
                    usage_analytics_retention_days =
                        EXCLUDED.usage_analytics_retention_days,
                    usage_journal_enabled = EXCLUDED.usage_journal_enabled,
                    usage_journal_max_file_bytes = EXCLUDED.usage_journal_max_file_bytes,
                    usage_journal_max_file_age_ms = EXCLUDED.usage_journal_max_file_age_ms,
                    usage_journal_max_files = EXCLUDED.usage_journal_max_files,
                    usage_journal_block_target_uncompressed_bytes =
                        EXCLUDED.usage_journal_block_target_uncompressed_bytes,
                    usage_journal_block_max_events =
                        EXCLUDED.usage_journal_block_max_events,
                    usage_journal_fsync_interval_ms =
                        EXCLUDED.usage_journal_fsync_interval_ms,
                    usage_journal_zstd_level = EXCLUDED.usage_journal_zstd_level,
                    usage_journal_consumer_lease_ms =
                        EXCLUDED.usage_journal_consumer_lease_ms,
                    usage_journal_delete_bad_files =
                        EXCLUDED.usage_journal_delete_bad_files,
                    usage_query_bind_addr = EXCLUDED.usage_query_bind_addr,
                    usage_query_base_url = EXCLUDED.usage_query_base_url,
                    usage_event_maintenance_enabled =
                        EXCLUDED.usage_event_maintenance_enabled,
                    usage_event_maintenance_interval_seconds =
                        EXCLUDED.usage_event_maintenance_interval_seconds,
                    usage_event_detail_retention_days =
                        EXCLUDED.usage_event_detail_retention_days,
                    kiro_cache_kmodels_json = EXCLUDED.kiro_cache_kmodels_json,
                    kiro_billable_model_multipliers_json =
                        EXCLUDED.kiro_billable_model_multipliers_json,
                    kiro_cache_policy_json = EXCLUDED.kiro_cache_policy_json,
                    kiro_context_usage_min_request_tokens =
                        EXCLUDED.kiro_context_usage_min_request_tokens,
                    kiro_prefix_cache_mode = EXCLUDED.kiro_prefix_cache_mode,
                    kiro_prefix_cache_max_tokens = EXCLUDED.kiro_prefix_cache_max_tokens,
                    kiro_prefix_cache_entry_ttl_seconds =
                        EXCLUDED.kiro_prefix_cache_entry_ttl_seconds,
                    kiro_conversation_anchor_max_entries =
                        EXCLUDED.kiro_conversation_anchor_max_entries,
                    kiro_conversation_anchor_ttl_seconds =
                        EXCLUDED.kiro_conversation_anchor_ttl_seconds,
                    updated_at_ms = EXCLUDED.updated_at_ms",
                &[
                    &record.id,
                    &record.auth_cache_ttl_seconds,
                    &record.max_request_body_bytes,
                    &record.account_failure_retry_limit,
                    &record.codex_client_version,
                    &record.kiro_channel_max_concurrency,
                    &record.kiro_channel_min_start_interval_ms,
                    &record.codex_status_refresh_min_interval_seconds,
                    &record.codex_status_refresh_max_interval_seconds,
                    &record.codex_status_account_jitter_max_seconds,
                    &record.codex_weight_free,
                    &record.codex_weight_plus,
                    &record.codex_weight_pro5x,
                    &record.codex_weight_pro20x,
                    &record.kiro_status_refresh_min_interval_seconds,
                    &record.kiro_status_refresh_max_interval_seconds,
                    &record.kiro_status_account_jitter_max_seconds,
                    &record.usage_event_flush_batch_size,
                    &record.usage_event_flush_interval_seconds,
                    &record.usage_event_flush_max_buffer_bytes,
                    &record.duckdb_usage_memory_limit_mib,
                    &record.duckdb_usage_checkpoint_threshold_mib,
                    &record.usage_analytics_retention_days,
                    &record.usage_journal_enabled,
                    &record.usage_journal_max_file_bytes,
                    &record.usage_journal_max_file_age_ms,
                    &record.usage_journal_max_files,
                    &record.usage_journal_block_target_uncompressed_bytes,
                    &record.usage_journal_block_max_events,
                    &record.usage_journal_fsync_interval_ms,
                    &record.usage_journal_zstd_level,
                    &record.usage_journal_consumer_lease_ms,
                    &(record.usage_journal_delete_bad_files as i64),
                    &record.usage_query_bind_addr,
                    &record.usage_query_base_url,
                    &record.usage_event_maintenance_enabled,
                    &record.usage_event_maintenance_interval_seconds,
                    &record.usage_event_detail_retention_days,
                    &record.kiro_cache_kmodels_json,
                    &record.kiro_billable_model_multipliers_json,
                    &record.kiro_cache_policy_json,
                    &record.kiro_context_usage_min_request_tokens,
                    &record.kiro_prefix_cache_mode,
                    &record.kiro_prefix_cache_max_tokens,
                    &record.kiro_prefix_cache_entry_ttl_seconds,
                    &record.kiro_conversation_anchor_max_entries,
                    &record.kiro_conversation_anchor_ttl_seconds,
                    &record.updated_at_ms,
                ],
            )
            .await
            .context("upsert postgres runtime config")?;
        self.store_runtime_config_record_cached(Some(record)).await;
        self.bump_dispatch_generation(core_store::PROVIDER_CODEX)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_KIRO)
            .await;
        Ok(())
    }

    async fn upsert_codex_account(&self, record: &CodexAccountRecord) -> anyhow::Result<()> {
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "INSERT INTO llm_codex_accounts (
                    account_name, account_id, email, status, auth_json, settings_json,
                    last_refresh_at_ms, last_error, created_at_ms, updated_at_ms
                 ) VALUES ($1, $2, $3, $4, $5::jsonb, $6::jsonb, $7, $8, $9, $10)
                 ON CONFLICT(account_name) DO UPDATE SET
                    account_id = EXCLUDED.account_id,
                    email = EXCLUDED.email,
                    status = EXCLUDED.status,
                    auth_json = EXCLUDED.auth_json,
                    settings_json = EXCLUDED.settings_json,
                    last_refresh_at_ms = EXCLUDED.last_refresh_at_ms,
                    last_error = EXCLUDED.last_error,
                    created_at_ms = EXCLUDED.created_at_ms,
                    updated_at_ms = EXCLUDED.updated_at_ms",
                &[
                    &record.account_name,
                    &record.account_id,
                    &record.email,
                    &record.status,
                    &record.auth_json,
                    &record.settings_json,
                    &record.last_refresh_at_ms,
                    &record.last_error,
                    &record.created_at_ms,
                    &record.updated_at_ms,
                ],
            )
            .await
            .context("upsert postgres codex account")?;
        Ok(())
    }

    async fn upsert_kiro_account(&self, record: &KiroAccountRecord) -> anyhow::Result<()> {
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "INSERT INTO llm_kiro_accounts (
                    account_name, auth_method, account_id, profile_arn, user_id,
                    status, auth_json, max_concurrency, min_start_interval_ms,
                    proxy_config_id, last_refresh_at_ms, last_error, created_at_ms,
                    updated_at_ms
                 ) VALUES (
                    $1, $2, $3, $4, $5, $6, $7::jsonb, $8, $9, $10, $11, $12, $13, $14
                 )
                 ON CONFLICT(account_name) DO UPDATE SET
                    auth_method = EXCLUDED.auth_method,
                    account_id = EXCLUDED.account_id,
                    profile_arn = EXCLUDED.profile_arn,
                    user_id = EXCLUDED.user_id,
                    status = EXCLUDED.status,
                    auth_json = EXCLUDED.auth_json,
                    max_concurrency = EXCLUDED.max_concurrency,
                    min_start_interval_ms = EXCLUDED.min_start_interval_ms,
                    proxy_config_id = EXCLUDED.proxy_config_id,
                    last_refresh_at_ms = EXCLUDED.last_refresh_at_ms,
                    last_error = EXCLUDED.last_error,
                    created_at_ms = EXCLUDED.created_at_ms,
                    updated_at_ms = EXCLUDED.updated_at_ms",
                &[
                    &record.account_name,
                    &record.auth_method,
                    &record.account_id,
                    &record.profile_arn,
                    &record.user_id,
                    &record.status,
                    &record.auth_json,
                    &record.max_concurrency,
                    &record.min_start_interval_ms,
                    &record.proxy_config_id,
                    &record.last_refresh_at_ms,
                    &record.last_error,
                    &record.created_at_ms,
                    &record.updated_at_ms,
                ],
            )
            .await
            .context("upsert postgres kiro account")?;
        Ok(())
    }

    async fn disable_admin_key_if_present(
        &self,
        key_id: &str,
        updated_at_ms: i64,
    ) -> anyhow::Result<()> {
        if self.load_key_bundle_by_id(key_id).await?.is_some() {
            self.patch_admin_key(key_id, AdminKeyPatch {
                status: Some(core_store::KEY_STATUS_DISABLED.to_string()),
                updated_at_ms,
                ..AdminKeyPatch::default()
            })
            .await?;
        }
        Ok(())
    }

    async fn load_codex_import_job_summary_row(
        &self,
        job_id: &str,
    ) -> anyhow::Result<Option<AdminCodexImportJobSummary>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT
                    job_id, provider_type, source_type, validate_before_import, status,
                    total_count, completed_count, succeeded_count, skipped_count, failed_count,
                    batch_error_message, created_at_ms, updated_at_ms, finished_at_ms
                 FROM llm_account_import_jobs
                 WHERE job_id = $1",
                &[&job_id],
            )
            .await
            .context("load postgres codex import job summary")?;
        Ok(row.map(decode_codex_import_job_summary_row))
    }

    async fn load_codex_import_job_items_rows(
        &self,
        job_id: &str,
    ) -> anyhow::Result<Vec<AdminCodexImportJobItem>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    item_index, requested_name, requested_account_id, status,
                    error_message, imported_account_name, final_account_id,
                    validated_at_ms, imported_at_ms
                 FROM llm_account_import_job_items
                 WHERE job_id = $1
                 ORDER BY item_index",
                &[&job_id],
            )
            .await
            .context("load postgres codex import job items")?;
        Ok(rows
            .into_iter()
            .map(decode_codex_import_job_item_row)
            .collect())
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
impl AdminConfigStore for PostgresControlRepository {
    async fn get_admin_runtime_config(&self) -> anyhow::Result<AdminRuntimeConfig> {
        let record = self
            .load_runtime_config_record_cached()
            .await?
            .unwrap_or_default();
        Ok(record.to_admin_runtime_config())
    }

    async fn update_admin_runtime_config(
        &self,
        config: AdminRuntimeConfig,
    ) -> anyhow::Result<AdminRuntimeConfig> {
        let mut record = self
            .load_runtime_config_record_cached()
            .await?
            .unwrap_or_default();
        record.apply_admin_runtime_config(&config);
        self.upsert_runtime_config_record(&record).await?;
        Ok(record.to_admin_runtime_config())
    }
}

#[async_trait]
impl AdminKeyStore for PostgresControlRepository {
    async fn list_admin_keys(&self) -> anyhow::Result<Vec<AdminKey>> {
        Ok(self
            .list_key_bundles()
            .await?
            .iter()
            .map(admin_key_from_bundle)
            .collect())
    }

    async fn get_admin_key(&self, key_id: &str) -> anyhow::Result<Option<AdminKey>> {
        Ok(self
            .load_key_bundle_by_id(key_id)
            .await?
            .map(|bundle| admin_key_from_bundle(&bundle)))
    }

    async fn list_admin_keys_page(
        &self,
        provider_type: Option<&str>,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminKeysPage> {
        self.list_key_bundles_page(provider_type, page).await
    }

    async fn list_admin_keys_filtered_page(
        &self,
        provider_type: Option<&str>,
        query: &AdminKeyPageQuery,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminKeysPage> {
        if provider_type == Some(core_store::PROVIDER_KIRO)
            && query == &AdminKeyPageQuery::default()
        {
            return self
                .list_kiro_key_bundles_page_with_candidate_summaries(page)
                .await;
        }
        self.ensure_connection_alive()?;
        let provider = provider_type.map(str::to_string);
        let search = query
            .search
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| format!("%{}%", value.to_ascii_lowercase()));
        let total = self
            .client
            .query_one(
                "SELECT COUNT(*)
                 FROM llm_keys k
                 WHERE ($1::text IS NULL OR k.provider_type = $1)
                   AND ($2::text IS NULL
                        OR lower(k.key_id) LIKE $2
                        OR lower(k.name) LIKE $2
                        OR lower(k.provider_type) LIKE $2
                        OR lower(k.status) LIKE $2)
                   AND ($3::boolean = FALSE OR k.status <> 'disabled')",
                &[&provider, &search, &query.active_only],
            )
            .await
            .context("count postgres filtered key bundles page")?
            .get::<_, i64>(0)
            .max(0) as usize;
        let order_by = match query.sort {
            AdminKeySortMode::Newest => "k.created_at_ms DESC, k.key_id DESC",
            AdminKeySortMode::QuotaAsc => {
                "(k.quota_billable_limit - COALESCE(u.billable_tokens, 0)) ASC, k.created_at_ms \
                 DESC, k.key_id DESC"
            },
            AdminKeySortMode::QuotaDesc => {
                "(k.quota_billable_limit - COALESCE(u.billable_tokens, 0)) DESC, k.created_at_ms \
                 DESC, k.key_id DESC"
            },
            AdminKeySortMode::UsageAsc => {
                "COALESCE(u.credit_total, '0') ASC, k.created_at_ms DESC, k.key_id DESC"
            },
            AdminKeySortMode::UsageDesc => {
                "COALESCE(u.credit_total, '0') DESC, k.created_at_ms DESC, k.key_id DESC"
            },
        };
        let sql = format!(
            "SELECT
                k.key_id, k.name, k.secret, k.key_hash, k.status, k.provider_type,
                k.protocol_family, k.public_visible, k.quota_billable_limit,
                k.created_at_ms, k.updated_at_ms,
                r.route_strategy, r.fixed_account_name, r.auto_account_names_json::text,
                r.account_group_id, r.model_name_map_json::text,
                r.request_max_concurrency, r.request_min_start_interval_ms,
                r.codex_fast_enabled, r.kiro_request_validation_enabled,
                r.kiro_cache_estimation_enabled,
                r.kiro_zero_cache_debug_enabled, r.kiro_full_request_logging_enabled,
                r.kiro_remote_media_resolution_enabled,
                r.kiro_cache_policy_override_json::text,
                r.kiro_billable_model_multipliers_override_json::text,
                COALESCE(u.input_uncached_tokens, 0),
                COALESCE(u.input_cached_tokens, 0),
                COALESCE(u.output_tokens, 0),
                COALESCE(u.billable_tokens, 0),
                COALESCE(u.credit_total, '0'),
                COALESCE(u.credit_missing_events, 0),
                u.last_used_at_ms,
                COALESCE(u.updated_at_ms, k.updated_at_ms),
                r.kiro_latency_routing_enabled AS kiro_latency_routing_enabled
             FROM llm_keys k
             LEFT JOIN llm_key_route_config r ON r.key_id = k.key_id
             LEFT JOIN llm_key_usage_rollups u ON u.key_id = k.key_id
             WHERE ($1::text IS NULL OR k.provider_type = $1)
               AND ($2::text IS NULL
                    OR lower(k.key_id) LIKE $2
                    OR lower(k.name) LIKE $2
                    OR lower(k.provider_type) LIKE $2
                    OR lower(k.status) LIKE $2)
               AND ($3::boolean = FALSE OR k.status <> 'disabled')
             ORDER BY {order_by}
             LIMIT $4 OFFSET $5"
        );
        let rows = self
            .client
            .query(sql.as_str(), &[
                &provider,
                &search,
                &query.active_only,
                &(page.limit.max(1) as i64),
                &(page.offset as i64),
            ])
            .await
            .context("list postgres filtered key bundles page")?;
        let keys = rows
            .into_iter()
            .map(decode_key_bundle_row)
            .map(|bundle| bundle.map(|bundle| admin_key_from_bundle(&bundle)))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let summary = self.admin_keys_summary(provider_type).await?;
        Ok(AdminKeysPage {
            has_more: page.has_more(keys.len(), total),
            keys,
            summary,
            total,
            limit: page.limit.max(1),
            offset: page.offset,
        })
    }

    async fn find_admin_key_referencing_account_group(
        &self,
        provider_type: &str,
        group_id: &str,
    ) -> anyhow::Result<Option<AdminKey>> {
        self.find_key_referencing_account_group(provider_type, group_id)
            .await
    }

    async fn create_admin_key(&self, key: NewAdminKey) -> anyhow::Result<AdminKey> {
        let key_record = KeyRecord {
            key_id: key.id.clone(),
            name: key.name.clone(),
            secret: key.secret.clone(),
            key_hash: key.key_hash.clone(),
            status: core_store::KEY_STATUS_ACTIVE.to_string(),
            provider_type: key.provider_type.clone(),
            protocol_family: key.protocol_family.clone(),
            public_visible: key.public_visible,
            quota_billable_limit: key.quota_billable_limit as i64,
            created_at_ms: key.created_at_ms,
            updated_at_ms: key.created_at_ms,
        };
        let route = KeyRouteConfig {
            key_id: key.id.clone(),
            route_strategy: None,
            fixed_account_name: None,
            auto_account_names_json: None,
            account_group_id: None,
            model_name_map_json: None,
            request_max_concurrency: key.request_max_concurrency.map(|value| value as i64),
            request_min_start_interval_ms: key
                .request_min_start_interval_ms
                .map(|value| value as i64),
            codex_fast_enabled: true,
            kiro_request_validation_enabled: true,
            kiro_cache_estimation_enabled: true,
            kiro_zero_cache_debug_enabled: false,
            kiro_full_request_logging_enabled: false,
            kiro_remote_media_resolution_enabled: false,
            kiro_latency_routing_enabled: true,
            kiro_cache_policy_override_json: None,
            kiro_billable_model_multipliers_override_json: None,
        };
        let rollup = KeyUsageRollup {
            key_id: key.id.clone(),
            input_uncached_tokens: 0,
            input_cached_tokens: 0,
            output_tokens: 0,
            billable_tokens: 0,
            credit_total: 0.0,
            credit_missing_events: 0,
            last_used_at_ms: None,
            updated_at_ms: key.created_at_ms,
        };
        Self::upsert_key_bundle_client(&self.client, &key_record, &route, &rollup).await?;
        self.bump_dispatch_generation(&key.provider_type).await;
        self.load_key_bundle_by_id(&key.id)
            .await?
            .map(|bundle| admin_key_from_bundle(&bundle))
            .context("created postgres admin key disappeared")
    }

    async fn patch_admin_key(
        &self,
        key_id: &str,
        patch: AdminKeyPatch,
    ) -> anyhow::Result<Option<AdminKey>> {
        let Some(mut bundle) = self.load_key_bundle_by_id(key_id).await? else {
            return Ok(None);
        };
        if let Some(name) = patch.name.as_ref() {
            bundle.key.name = name.clone();
        }
        if let Some(status) = patch.status.as_ref() {
            bundle.key.status = status.clone();
        }
        if let Some(public_visible) = patch.public_visible {
            bundle.key.public_visible = public_visible;
        }
        if let Some(limit) = patch.quota_billable_limit {
            bundle.key.quota_billable_limit = limit as i64;
        }
        if let Some(value) = patch.route_strategy.as_ref() {
            bundle.route.route_strategy = value.clone();
        }
        if let Some(value) = patch.account_group_id.as_ref() {
            bundle.route.account_group_id = value.clone();
        }
        if let Some(value) = patch.fixed_account_name.as_ref() {
            bundle.route.fixed_account_name = value.clone();
        }
        if let Some(value) = patch.auto_account_names.as_ref() {
            bundle.route.auto_account_names_json = value
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .context("serialize postgres auto account names")?;
        }
        if let Some(value) = patch.model_name_map.as_ref() {
            bundle.route.model_name_map_json = value
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .context("serialize postgres model name map")?;
        }
        if let Some(value) = patch.request_max_concurrency {
            bundle.route.request_max_concurrency = value.map(|value| value as i64);
        }
        if let Some(value) = patch.request_min_start_interval_ms {
            bundle.route.request_min_start_interval_ms = value.map(|value| value as i64);
        }
        if let Some(value) = patch.codex_fast_enabled {
            bundle.route.codex_fast_enabled = value;
        }
        if let Some(value) = patch.kiro_request_validation_enabled {
            bundle.route.kiro_request_validation_enabled = value;
        }
        if let Some(value) = patch.kiro_cache_estimation_enabled {
            bundle.route.kiro_cache_estimation_enabled = value;
        }
        if let Some(value) = patch.kiro_zero_cache_debug_enabled {
            bundle.route.kiro_zero_cache_debug_enabled = value;
        }
        if let Some(value) = patch.kiro_full_request_logging_enabled {
            bundle.route.kiro_full_request_logging_enabled = value;
        }
        if let Some(value) = patch.kiro_remote_media_resolution_enabled {
            bundle.route.kiro_remote_media_resolution_enabled = value;
        }
        if let Some(value) = patch.kiro_latency_routing_enabled {
            bundle.route.kiro_latency_routing_enabled = value;
        }
        if let Some(value) = patch.kiro_cache_policy_override_json.as_ref() {
            bundle.route.kiro_cache_policy_override_json = value.clone();
        }
        if let Some(value) = patch.kiro_billable_model_multipliers_override_json.as_ref() {
            bundle.route.kiro_billable_model_multipliers_override_json = value.clone();
        }
        bundle.key.updated_at_ms = patch.updated_at_ms;
        bundle.rollup.updated_at_ms = bundle.rollup.updated_at_ms.max(patch.updated_at_ms);
        Self::upsert_key_bundle_client(&self.client, &bundle.key, &bundle.route, &bundle.rollup)
            .await?;
        self.invalidate_authenticated_key_cache_by_ids(std::slice::from_ref(&bundle.key.key_id))
            .await;
        self.invalidate_request_snapshot_cache(&bundle.key.provider_type, &bundle.key.key_id)
            .await;
        self.bump_dispatch_generation(&bundle.key.provider_type)
            .await;
        Ok(Some(admin_key_from_bundle(&bundle)))
    }

    async fn delete_admin_key(&self, key_id: &str) -> anyhow::Result<Option<AdminKey>> {
        let Some(bundle) = self.load_key_bundle_by_id(key_id).await? else {
            return Ok(None);
        };
        self.ensure_connection_alive()?;
        self.client
            .execute("DELETE FROM llm_keys WHERE key_id = $1", &[&key_id])
            .await
            .context("delete postgres admin key")?;
        self.invalidate_authenticated_key_cache_by_ids(std::slice::from_ref(&bundle.key.key_id))
            .await;
        self.invalidate_request_snapshot_cache(&bundle.key.provider_type, &bundle.key.key_id)
            .await;
        self.bump_dispatch_generation(&bundle.key.provider_type)
            .await;
        Ok(Some(admin_key_from_bundle(&bundle)))
    }
}

#[async_trait]
impl AdminAccountGroupStore for PostgresControlRepository {
    async fn list_admin_account_groups(
        &self,
        provider_type: &str,
    ) -> anyhow::Result<Vec<AdminAccountGroup>> {
        self.list_admin_account_groups_rows(provider_type).await
    }

    async fn list_admin_account_groups_page(
        &self,
        provider_type: &str,
        page: AdminPageRequest,
    ) -> anyhow::Result<core_store::AdminAccountGroupsPage> {
        self.list_admin_account_groups_page_rows(provider_type, page)
            .await
    }

    async fn list_admin_account_group_options(
        &self,
        provider_type: &str,
    ) -> anyhow::Result<Vec<AdminAccountGroupOption>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    group_id,
                    provider_type,
                    name,
                    CASE
                        WHEN jsonb_typeof(account_names_json) = 'array'
                        THEN jsonb_array_length(account_names_json)
                        ELSE 0
                    END,
                    CASE
                        WHEN jsonb_typeof(account_names_json) = 'array'
                             AND jsonb_array_length(account_names_json) = 1
                        THEN account_names_json ->> 0
                        ELSE NULL
                    END
                 FROM llm_account_groups
                 WHERE provider_type = $1
                 ORDER BY lower(name), group_id",
                &[&provider_type],
            )
            .await
            .context("list postgres admin account group options")?;
        Ok(rows
            .into_iter()
            .map(|row| AdminAccountGroupOption {
                id: row.get(0),
                provider_type: row.get(1),
                name: row.get(2),
                account_count: row.get::<_, i32>(3).max(0) as usize,
                single_account_name: row.get(4),
            })
            .collect())
    }

    async fn create_admin_account_group(
        &self,
        group: NewAdminAccountGroup,
    ) -> anyhow::Result<AdminAccountGroup> {
        let account_names_json =
            serde_json::to_string(&group.account_names).context("serialize account group names")?;
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "INSERT INTO llm_account_groups (
                    group_id, provider_type, name, account_names_json, created_at_ms, updated_at_ms
                 ) VALUES ($1, $2, $3, $4::jsonb, $5, $6)",
                &[
                    &group.id,
                    &group.provider_type,
                    &group.name,
                    &account_names_json,
                    &group.created_at_ms,
                    &group.created_at_ms,
                ],
            )
            .await
            .context("create postgres account group")?;
        self.bump_dispatch_generation(&group.provider_type).await;
        self.get_admin_account_group_row(&group.id)
            .await?
            .context("created postgres account group disappeared")
    }

    async fn patch_admin_account_group(
        &self,
        group_id: &str,
        patch: AdminAccountGroupPatch,
    ) -> anyhow::Result<Option<AdminAccountGroup>> {
        let Some(mut group) = self.get_admin_account_group_row(group_id).await? else {
            return Ok(None);
        };
        if let Some(name) = patch.name.as_ref() {
            group.name = name.clone();
        }
        if let Some(account_names) = patch.account_names.as_ref() {
            group.account_names = account_names.clone();
        }
        group.updated_at = patch.updated_at_ms;
        let account_names_json =
            serde_json::to_string(&group.account_names).context("serialize account group names")?;
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "UPDATE llm_account_groups
                 SET name = $2, account_names_json = $3::jsonb, updated_at_ms = $4
                 WHERE group_id = $1",
                &[&group_id, &group.name, &account_names_json, &group.updated_at],
            )
            .await
            .context("patch postgres account group")?;
        self.bump_dispatch_generation(&group.provider_type).await;
        Ok(Some(group))
    }

    async fn delete_admin_account_group(
        &self,
        group_id: &str,
    ) -> anyhow::Result<Option<AdminAccountGroup>> {
        let Some(group) = self.get_admin_account_group_row(group_id).await? else {
            return Ok(None);
        };
        self.ensure_connection_alive()?;
        self.client
            .execute("DELETE FROM llm_account_groups WHERE group_id = $1", &[&group_id])
            .await
            .context("delete postgres account group")?;
        self.bump_dispatch_generation(&group.provider_type).await;
        Ok(Some(group))
    }
}

#[async_trait]
impl AdminProxyStore for PostgresControlRepository {
    async fn list_admin_proxy_configs(&self) -> anyhow::Result<Vec<AdminProxyConfig>> {
        self.load_admin_proxy_configs_cached().await
    }

    async fn get_admin_proxy_config(
        &self,
        proxy_id: &str,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        self.get_admin_proxy_config_row(proxy_id).await
    }

    async fn create_admin_proxy_config(
        &self,
        proxy: NewAdminProxyConfig,
    ) -> anyhow::Result<AdminProxyConfig> {
        if !self.proxy_scope.can_edit_slot_metadata() {
            anyhow::bail!("proxy slots can only be created on the core node");
        }
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "INSERT INTO llm_proxy_configs (
                    proxy_config_id, name, proxy_url, proxy_username, proxy_password,
                    status, created_at_ms, updated_at_ms
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                &[
                    &proxy.id,
                    &proxy.name,
                    &proxy.proxy_url,
                    &proxy.proxy_username,
                    &proxy.proxy_password,
                    &core_store::KEY_STATUS_ACTIVE,
                    &proxy.created_at_ms,
                    &proxy.created_at_ms,
                ],
            )
            .await
            .context("create postgres proxy config")?;
        self.invalidate_proxy_metadata_cache().await;
        self.bump_dispatch_generation(core_store::PROVIDER_CODEX)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_KIRO)
            .await;
        self.get_admin_proxy_config_row(&proxy.id)
            .await?
            .context("created postgres proxy config disappeared")
    }

    async fn patch_admin_proxy_config(
        &self,
        proxy_id: &str,
        patch: AdminProxyConfigPatch,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        if !self.proxy_scope.can_edit_slot_metadata() {
            return self
                .patch_admin_proxy_config_node_override(proxy_id, patch)
                .await;
        }
        let Some(mut proxy) = self.get_admin_proxy_config_base_row(proxy_id).await? else {
            return Ok(None);
        };
        if let Some(name) = patch.name.as_ref() {
            proxy.name = name.clone();
        }
        if let Some(proxy_url) = patch.proxy_url.as_ref() {
            proxy.proxy_url = proxy_url.clone();
        }
        if let Some(proxy_username) = patch.proxy_username.as_ref() {
            proxy.proxy_username = proxy_username.clone();
        }
        if let Some(proxy_password) = patch.proxy_password.as_ref() {
            proxy.proxy_password = proxy_password.clone();
        }
        if let Some(status) = patch.status.as_ref() {
            proxy.status = status.clone();
        }
        proxy.updated_at = patch.updated_at_ms;
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "UPDATE llm_proxy_configs
                 SET name = $2, proxy_url = $3, proxy_username = $4,
                     proxy_password = $5, status = $6, updated_at_ms = $7
                 WHERE proxy_config_id = $1",
                &[
                    &proxy_id,
                    &proxy.name,
                    &proxy.proxy_url,
                    &proxy.proxy_username,
                    &proxy.proxy_password,
                    &proxy.status,
                    &proxy.updated_at,
                ],
            )
            .await
            .context("patch postgres proxy config")?;
        self.invalidate_proxy_metadata_cache().await;
        self.invalidate_all_account_views_for_provider(core_store::PROVIDER_CODEX)
            .await;
        self.invalidate_all_account_views_for_provider(core_store::PROVIDER_KIRO)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_CODEX)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_KIRO)
            .await;
        self.get_admin_proxy_config_row(proxy_id).await
    }

    async fn record_admin_proxy_endpoint_check(
        &self,
        update: AdminProxyEndpointCheckUpdate,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        if update.provider_type != core_store::PROVIDER_CODEX
            && update.provider_type != core_store::PROVIDER_KIRO
        {
            anyhow::bail!("unsupported proxy endpoint check provider `{}`", update.provider_type);
        }
        if self
            .get_admin_proxy_config_base_row(&update.proxy_config_id)
            .await?
            .is_none()
        {
            return Ok(None);
        }
        let status_code = update.status_code.map(i32::from);
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "INSERT INTO llm_proxy_config_endpoint_checks (
                    proxy_config_id, node_id, provider_type, target_url, reachable,
                    status_code, latency_ms, error_message, checked_at_ms
                 ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                 ON CONFLICT (proxy_config_id, node_id, provider_type) DO UPDATE SET
                    target_url = EXCLUDED.target_url,
                    reachable = EXCLUDED.reachable,
                    status_code = EXCLUDED.status_code,
                    latency_ms = EXCLUDED.latency_ms,
                    error_message = EXCLUDED.error_message,
                    checked_at_ms = EXCLUDED.checked_at_ms",
                &[
                    &update.proxy_config_id,
                    &self.proxy_scope.node_id,
                    &update.provider_type,
                    &update.target_url,
                    &update.reachable,
                    &status_code,
                    &update.latency_ms,
                    &update.error_message,
                    &update.checked_at_ms,
                ],
            )
            .await
            .context("record postgres proxy endpoint check")?;
        self.invalidate_proxy_metadata_cache().await;
        self.get_admin_proxy_config_row(&update.proxy_config_id)
            .await
    }

    async fn delete_admin_proxy_config(
        &self,
        proxy_id: &str,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        if !self.proxy_scope.can_edit_slot_metadata() {
            anyhow::bail!("proxy slots can only be deleted on the core node");
        }
        let Some(proxy) = self.get_admin_proxy_config_row(proxy_id).await? else {
            return Ok(None);
        };
        self.ensure_connection_alive()?;
        self.client
            .execute("DELETE FROM llm_proxy_configs WHERE proxy_config_id = $1", &[&proxy_id])
            .await
            .context("delete postgres proxy config")?;
        self.invalidate_proxy_metadata_cache().await;
        self.invalidate_all_account_views_for_provider(core_store::PROVIDER_CODEX)
            .await;
        self.invalidate_all_account_views_for_provider(core_store::PROVIDER_KIRO)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_CODEX)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_KIRO)
            .await;
        Ok(Some(proxy))
    }

    async fn reset_admin_proxy_config_override(
        &self,
        proxy_id: &str,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        let Some(_) = self.get_admin_proxy_config_base_row(proxy_id).await? else {
            return Ok(None);
        };
        if !self.proxy_scope.can_edit_slot_metadata() {
            self.ensure_connection_alive()?;
            self.client
                .execute(
                    "DELETE FROM llm_proxy_config_node_overrides
                     WHERE proxy_config_id = $1 AND node_id = $2",
                    &[&proxy_id, &self.proxy_scope.node_id],
                )
                .await
                .context("reset postgres proxy config node override")?;
            self.invalidate_proxy_metadata_cache().await;
            self.invalidate_all_account_views_for_provider(core_store::PROVIDER_CODEX)
                .await;
            self.invalidate_all_account_views_for_provider(core_store::PROVIDER_KIRO)
                .await;
            self.bump_dispatch_generation(core_store::PROVIDER_CODEX)
                .await;
            self.bump_dispatch_generation(core_store::PROVIDER_KIRO)
                .await;
        }
        self.get_admin_proxy_config_row(proxy_id).await
    }

    async fn list_admin_proxy_bindings(&self) -> anyhow::Result<Vec<AdminProxyBinding>> {
        let mut bindings = Vec::new();
        for provider in [core_store::PROVIDER_CODEX, core_store::PROVIDER_KIRO] {
            bindings.push(self.load_admin_proxy_binding_cached(provider).await?);
        }
        Ok(bindings)
    }

    async fn update_admin_proxy_binding(
        &self,
        provider_type: &str,
        proxy_config_id: Option<String>,
    ) -> anyhow::Result<AdminProxyBinding> {
        self.ensure_connection_alive()?;
        match proxy_config_id {
            Some(proxy_config_id) => {
                self.client
                    .execute(
                        "INSERT INTO llm_proxy_bindings (
                            provider_type, proxy_config_id, updated_at_ms
                        ) VALUES ($1, $2, $3)
                        ON CONFLICT(provider_type) DO UPDATE SET
                            proxy_config_id = EXCLUDED.proxy_config_id,
                            updated_at_ms = EXCLUDED.updated_at_ms",
                        &[&provider_type, &proxy_config_id, &now_ms()],
                    )
                    .await
                    .context("upsert postgres proxy binding")?;
            },
            None => {
                self.client
                    .execute("DELETE FROM llm_proxy_bindings WHERE provider_type = $1", &[
                        &provider_type,
                    ])
                    .await
                    .context("delete postgres proxy binding")?;
            },
        }
        self.invalidate_proxy_metadata_cache().await;
        self.invalidate_all_account_views_for_provider(provider_type)
            .await;
        self.bump_dispatch_generation(provider_type).await;
        self.load_admin_proxy_binding_cached(provider_type).await
    }

    async fn import_legacy_kiro_proxy_configs(
        &self,
    ) -> anyhow::Result<AdminLegacyKiroProxyMigration> {
        let mut tuples_to_accounts =
            BTreeMap::<(String, Option<String>, Option<String>), Vec<KiroAccountRecord>>::new();
        for account in self.list_kiro_accounts_rows().await? {
            let auth_json = serde_json::from_str::<serde_json::Value>(&account.auth_json)
                .context("parse postgres kiro auth json for legacy proxy migration")?;
            let Some(proxy_url) = legacy_proxy_json_string(&auth_json, &["proxyUrl", "proxy_url"])
            else {
                continue;
            };
            let proxy_username =
                legacy_proxy_json_string(&auth_json, &["proxyUsername", "proxy_username"]);
            let proxy_password =
                legacy_proxy_json_string(&auth_json, &["proxyPassword", "proxy_password"]);
            tuples_to_accounts
                .entry((proxy_url, proxy_username, proxy_password))
                .or_default()
                .push(account);
        }

        if tuples_to_accounts.is_empty() {
            return Ok(AdminLegacyKiroProxyMigration {
                created_configs: Vec::new(),
                reused_configs: Vec::new(),
                migrated_account_names: Vec::new(),
            });
        }

        let mut existing_by_tuple =
            BTreeMap::<(String, Option<String>, Option<String>), AdminProxyConfig>::new();
        for proxy in self.list_admin_proxy_configs().await? {
            existing_by_tuple.insert(
                (
                    proxy.proxy_url.clone(),
                    proxy.proxy_username.clone(),
                    proxy.proxy_password.clone(),
                ),
                proxy,
            );
        }

        let mut created_configs = Vec::new();
        let mut reused_configs = Vec::new();
        let mut migrated_account_names = Vec::new();
        for (index, (tuple, mut accounts)) in tuples_to_accounts.into_iter().enumerate() {
            let proxy = if let Some(proxy) = existing_by_tuple.get(&tuple).cloned() {
                reused_configs.push(proxy.clone());
                proxy
            } else {
                let now = now_ms();
                let base = format!("llm-proxy-legacy-{}-{}", now, index + 1);
                let mut suffix = 0usize;
                let proxy_id = loop {
                    let candidate =
                        if suffix == 0 { base.clone() } else { format!("{base}-{suffix}") };
                    if !existing_by_tuple
                        .values()
                        .any(|proxy| proxy.id == candidate)
                    {
                        break candidate;
                    }
                    suffix += 1;
                    if suffix >= 1_000 {
                        anyhow::bail!("failed to allocate postgres legacy proxy config id");
                    }
                };
                let proxy = NewAdminProxyConfig {
                    id: proxy_id,
                    name: format!("legacy-kiro-{}", index + 1),
                    proxy_url: tuple.0.clone(),
                    proxy_username: tuple.1.clone(),
                    proxy_password: tuple.2.clone(),
                    created_at_ms: now,
                };
                let created = self.create_admin_proxy_config(proxy).await?;
                existing_by_tuple.insert(tuple.clone(), created.clone());
                created_configs.push(created.clone());
                created
            };

            accounts.sort_by_cached_key(|account| account.account_name.to_ascii_lowercase());
            for mut account in accounts {
                account.proxy_config_id = Some(proxy.id.clone());
                account.updated_at_ms = now_ms();
                account.auth_json = clear_legacy_kiro_proxy_json(&account.auth_json, &proxy.id)?;
                self.upsert_kiro_account(&account).await?;
                self.invalidate_account_cache(core_store::PROVIDER_KIRO, &account.account_name)
                    .await;
                migrated_account_names.push(account.account_name);
            }
        }
        migrated_account_names.sort();
        migrated_account_names.dedup();
        self.invalidate_proxy_metadata_cache().await;
        self.bump_dispatch_generation(core_store::PROVIDER_KIRO)
            .await;
        Ok(AdminLegacyKiroProxyMigration {
            created_configs,
            reused_configs,
            migrated_account_names,
        })
    }
}

#[async_trait]
impl AdminCodexAccountStore for PostgresControlRepository {
    async fn list_admin_codex_accounts(&self) -> anyhow::Result<Vec<AdminCodexAccount>> {
        let rows = self.list_codex_admin_account_rows().await?;
        let context = self.load_codex_admin_account_view_context().await?;
        Ok(rows
            .iter()
            .map(|row| self.admin_codex_account_from_list_row_with_context(row, &context))
            .collect())
    }

    async fn list_admin_codex_accounts_page(
        &self,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminCodexAccountsPage> {
        let page = AdminPageRequest {
            limit: page.limit.max(1),
            offset: page.offset,
        };
        let (rows, total) = self.list_codex_admin_account_rows_page(page).await?;
        let context = self.load_codex_admin_account_view_context().await?;
        let accounts = rows
            .iter()
            .map(|row| self.admin_codex_account_from_list_row_with_context(row, &context))
            .collect::<Vec<_>>();
        let summary = self.admin_codex_accounts_summary().await?;
        Ok(AdminCodexAccountsPage {
            has_more: page.has_more(accounts.len(), total),
            accounts,
            summary,
            total,
            limit: page.limit,
            offset: page.offset,
        })
    }

    async fn list_admin_codex_accounts_filtered_page(
        &self,
        query: &AdminCodexAccountPageQuery,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminCodexAccountsPage> {
        if query == &AdminCodexAccountPageQuery::default() {
            return self.list_admin_codex_accounts_page(page).await;
        }
        let page = AdminPageRequest {
            limit: page.limit.max(1),
            offset: page.offset,
        };
        let (rows, total) = self
            .list_codex_admin_account_rows_filtered_page(query, page)
            .await?;
        let context = self.load_codex_admin_account_view_context().await?;
        let accounts = rows
            .iter()
            .map(|row| self.admin_codex_account_from_list_row_with_context(row, &context))
            .collect::<Vec<_>>();
        let summary = self.admin_codex_accounts_summary().await?;
        Ok(AdminCodexAccountsPage {
            has_more: page.has_more(accounts.len(), total),
            accounts,
            summary,
            total,
            limit: page.limit,
            offset: page.offset,
        })
    }

    async fn list_codex_status_refresh_targets(
        &self,
    ) -> anyhow::Result<Vec<CodexStatusRefreshTarget>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT account_name, status
                 FROM llm_codex_accounts
                 ORDER BY account_name",
                &[],
            )
            .await
            .context("list postgres codex status refresh targets")?;
        Ok(rows
            .into_iter()
            .map(|row| CodexStatusRefreshTarget {
                name: row.get(0),
                status: row.get(1),
            })
            .collect())
    }

    async fn get_admin_codex_account(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<AdminCodexAccount>> {
        match self.get_codex_account_row(name).await? {
            Some(record) => self
                .admin_codex_account_from_record(&record)
                .await
                .map(Some),
            None => Ok(None),
        }
    }

    async fn find_admin_codex_account_name_by_account_id(
        &self,
        account_id: &str,
    ) -> anyhow::Result<Option<String>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT account_name
                 FROM llm_codex_accounts
                 WHERE account_id = $1
                 ORDER BY account_name
                 LIMIT 1",
                &[&account_id],
            )
            .await
            .context("load codex account name by account id")?;
        Ok(row.map(|row| row.get(0)))
    }

    async fn create_admin_codex_account(
        &self,
        account: NewAdminCodexAccount,
    ) -> anyhow::Result<AdminCodexAccount> {
        let settings = CodexAccountSettings {
            map_gpt53_codex_to_spark: account.map_gpt53_codex_to_spark,
            auth_refresh_enabled: account.auto_refresh_enabled,
            route_weight_tier: account.route_weight_tier.clone(),
            ..CodexAccountSettings::default()
        };
        let record = CodexAccountRecord {
            account_name: account.name.clone(),
            account_id: account.account_id.clone(),
            email: None,
            status: core_store::KEY_STATUS_ACTIVE.to_string(),
            auth_json: account.auth_json.clone(),
            settings_json: serde_json::to_string(&settings)
                .context("serialize postgres codex settings")?,
            last_refresh_at_ms: Some(account.created_at_ms),
            last_error: None,
            created_at_ms: account.created_at_ms,
            updated_at_ms: account.created_at_ms,
        };
        self.upsert_codex_account(&record).await?;
        self.invalidate_account_cache(core_store::PROVIDER_CODEX, &account.name)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_CODEX)
            .await;
        self.get_admin_codex_account(&account.name)
            .await?
            .context("created postgres codex account disappeared")
    }

    async fn patch_admin_codex_account(
        &self,
        name: &str,
        patch: AdminCodexAccountPatch,
    ) -> anyhow::Result<Option<AdminCodexAccount>> {
        let Some(mut record) = self.get_codex_account_row(name).await? else {
            return Ok(None);
        };
        if let Some(value) = patch.status.as_ref() {
            record.status = value.clone();
        }
        let mut settings = decode_codex_account_settings(&record.settings_json)?;
        if let Some(value) = patch.map_gpt53_codex_to_spark {
            settings.map_gpt53_codex_to_spark = value;
        }
        if let Some(value) = patch.auto_refresh_enabled {
            settings.auth_refresh_enabled = value;
        }
        if let Some(value) = patch.route_weight_tier.as_ref() {
            settings.route_weight_tier = Some(value.clone());
        }
        if let Some(value) = patch.proxy_mode.as_ref() {
            settings.proxy_mode = value.clone();
        }
        if let Some(value) = patch.proxy_config_id.as_ref() {
            settings.proxy_config_id = value.clone();
        }
        if let Some(value) = patch.request_max_concurrency {
            settings.request_max_concurrency = value;
        }
        if let Some(value) = patch.request_min_start_interval_ms {
            settings.request_min_start_interval_ms = value;
        }
        record.settings_json =
            serde_json::to_string(&settings).context("serialize postgres codex settings")?;
        record.updated_at_ms = patch.updated_at_ms;
        self.upsert_codex_account(&record).await?;
        self.invalidate_account_cache(core_store::PROVIDER_CODEX, &record.account_name)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_CODEX)
            .await;
        Ok(Some(self.admin_codex_account_from_record(&record).await?))
    }

    async fn delete_admin_codex_account(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<AdminCodexAccount>> {
        let Some(record) = self.get_codex_account_row(name).await? else {
            return Ok(None);
        };
        let view = self.admin_codex_account_from_record(&record).await?;
        self.ensure_connection_alive()?;
        self.client
            .execute("DELETE FROM llm_codex_accounts WHERE account_name = $1", &[&name])
            .await
            .context("delete postgres codex account")?;
        self.invalidate_account_cache(core_store::PROVIDER_CODEX, &record.account_name)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_CODEX)
            .await;
        Ok(Some(view))
    }

    async fn refresh_admin_codex_account(
        &self,
        name: &str,
        refreshed_at_ms: i64,
    ) -> anyhow::Result<Option<AdminCodexAccount>> {
        let Some(mut record) = self.get_codex_account_row(name).await? else {
            return Ok(None);
        };
        record.last_refresh_at_ms = Some(refreshed_at_ms);
        record.last_error = None;
        record.updated_at_ms = refreshed_at_ms;
        self.upsert_codex_account(&record).await?;
        self.invalidate_account_cache(core_store::PROVIDER_CODEX, &record.account_name)
            .await;
        Ok(Some(self.admin_codex_account_from_record(&record).await?))
    }

    async fn resolve_admin_codex_account_route(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        let Some(record) = self.get_codex_account_row(name).await? else {
            return Ok(None);
        };
        if record.status != core_store::KEY_STATUS_ACTIVE {
            return Ok(None);
        }
        let settings = decode_codex_account_settings(&record.settings_json)?;
        let proxy_context = self
            .load_provider_proxy_resolution_context(core_store::PROVIDER_CODEX)
            .await?;
        let proxy = resolve_provider_proxy_config_from_context(
            &settings.proxy_mode,
            settings.proxy_config_id.as_deref(),
            &proxy_context,
        )?;
        let status_by_account = self
            .load_codex_rate_limit_status_cached()
            .await?
            .map(|status| {
                status
                    .accounts
                    .into_iter()
                    .map(|account| (account.name.clone(), account))
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        let cached_error_message = codex_cached_error_message(
            &record.account_name,
            record.last_error.as_deref(),
            record.last_refresh_at_ms,
            settings.auth_refresh_enabled,
            &record.auth_json,
            &status_by_account,
        );
        Ok(Some(ProviderCodexRoute {
            account_name: record.account_name.clone(),
            account_group_id_at_event: None,
            route_strategy_at_event: RouteStrategy::Auto,
            auth_json: record.auth_json,
            map_gpt53_codex_to_spark: settings.map_gpt53_codex_to_spark,
            auth_refresh_enabled: settings.auth_refresh_enabled,
            codex_fast_enabled: true,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            account_request_max_concurrency: settings.request_max_concurrency,
            account_request_min_start_interval_ms: settings.request_min_start_interval_ms,
            cached_error_message,
            proxy,
        }))
    }

    async fn create_admin_codex_import_job(
        &self,
        job: NewAdminCodexImportJob,
    ) -> anyhow::Result<AdminCodexImportJobDetail> {
        let client = self.connect_fresh_client().await?;
        let tx = client
            .transaction()
            .await
            .context("begin postgres codex import job transaction")?;
        tx.execute(
            "INSERT INTO llm_account_import_jobs (
                job_id, provider_type, source_type, validate_before_import, status,
                total_count, completed_count, succeeded_count, skipped_count, failed_count,
                batch_error_message, created_at_ms, updated_at_ms, finished_at_ms
            ) VALUES (
                $1, $2, $3, $4, 'pending',
                $5, 0, 0, 0, 0,
                NULL, $6, $6, NULL
            )",
            &[
                &job.job_id,
                &job.provider_type,
                &job.source_type,
                &job.validate_before_import,
                &(job.items.len() as i64),
                &job.created_at_ms,
            ],
        )
        .await
        .context("insert postgres codex import job")?;
        for (item_index, item) in job.items.iter().enumerate() {
            tx.execute(
                "INSERT INTO llm_account_import_job_items (
                    job_id, item_index, requested_name, requested_account_id, raw_auth_json,
                    status, error_message, imported_account_name, final_account_id,
                    validated_at_ms, imported_at_ms, created_at_ms, updated_at_ms
                ) VALUES (
                    $1, $2, $3, $4, $5::jsonb,
                    'pending', NULL, NULL, NULL,
                    NULL, NULL, $6, $6
                )",
                &[
                    &job.job_id,
                    &(item_index as i64),
                    &item.requested_name,
                    &item.requested_account_id,
                    &item.raw_auth_json,
                    &job.created_at_ms,
                ],
            )
            .await
            .with_context(|| format!("insert postgres codex import job item {item_index}"))?;
        }
        tx.commit()
            .await
            .context("commit postgres codex import job transaction")?;
        self.get_admin_codex_import_job(&job.job_id)
            .await?
            .context("created postgres codex import job disappeared")
    }

    async fn list_admin_codex_import_jobs(
        &self,
        limit: usize,
    ) -> anyhow::Result<Vec<AdminCodexImportJobSummary>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    job_id, provider_type, source_type, validate_before_import, status,
                    total_count, completed_count, succeeded_count, skipped_count, failed_count,
                    batch_error_message, created_at_ms, updated_at_ms, finished_at_ms
                 FROM llm_account_import_jobs
                 ORDER BY created_at_ms DESC, job_id DESC
                 LIMIT $1",
                &[&(limit as i64)],
            )
            .await
            .context("list postgres codex import jobs")?;
        Ok(rows
            .into_iter()
            .map(decode_codex_import_job_summary_row)
            .collect())
    }

    async fn get_admin_codex_import_job(
        &self,
        job_id: &str,
    ) -> anyhow::Result<Option<AdminCodexImportJobDetail>> {
        let Some(summary) = self.load_codex_import_job_summary_row(job_id).await? else {
            return Ok(None);
        };
        let items = self.load_codex_import_job_items_rows(job_id).await?;
        Ok(Some(AdminCodexImportJobDetail {
            summary,
            items,
        }))
    }

    async fn mark_admin_codex_import_job_running(
        &self,
        job_id: &str,
        updated_at_ms: i64,
    ) -> anyhow::Result<()> {
        self.ensure_connection_alive()?;
        let changed = self
            .client
            .execute(
                "UPDATE llm_account_import_jobs
                 SET status = 'running', updated_at_ms = $2
                 WHERE job_id = $1",
                &[&job_id, &updated_at_ms],
            )
            .await
            .context("mark postgres codex import job running")?;
        if changed == 0 {
            anyhow::bail!("codex import job `{job_id}` not found");
        }
        Ok(())
    }

    async fn mark_admin_codex_import_job_item_running(
        &self,
        job_id: &str,
        item_index: usize,
        updated_at_ms: i64,
    ) -> anyhow::Result<()> {
        self.ensure_connection_alive()?;
        let changed = self
            .client
            .execute(
                "UPDATE llm_account_import_job_items
                 SET status = 'running', updated_at_ms = $3
                 WHERE job_id = $1 AND item_index = $2",
                &[&job_id, &(item_index as i64), &updated_at_ms],
            )
            .await
            .context("mark postgres codex import job item running")?;
        if changed == 0 {
            anyhow::bail!("codex import job item `{job_id}`:{item_index} not found");
        }
        Ok(())
    }

    async fn complete_admin_codex_import_job_item(
        &self,
        job_id: &str,
        result: AdminCodexImportJobItemResult,
    ) -> anyhow::Result<Option<AdminCodexImportJobSummary>> {
        if self
            .load_codex_import_job_summary_row(job_id)
            .await?
            .is_none()
        {
            return Ok(None);
        }
        let client = self.connect_fresh_client().await?;
        let tx = client
            .transaction()
            .await
            .context("begin postgres codex import job item completion transaction")?;
        let item_rows = tx
            .execute(
                "UPDATE llm_account_import_job_items
                 SET
                    raw_auth_json = NULL,
                    status = $3,
                    error_message = $4,
                    imported_account_name = $5,
                    final_account_id = $6,
                    validated_at_ms = $7,
                    imported_at_ms = $8,
                    updated_at_ms = $9
                 WHERE job_id = $1 AND item_index = $2",
                &[
                    &job_id,
                    &(result.item_index as i64),
                    &result.status,
                    &result.error_message,
                    &result.imported_account_name,
                    &result.final_account_id,
                    &result.validated_at_ms,
                    &result.imported_at_ms,
                    &result.updated_at_ms,
                ],
            )
            .await
            .context("update postgres codex import job item terminal state")?;
        if item_rows == 0 {
            anyhow::bail!("codex import job item `{job_id}`:{} not found", result.item_index);
        }
        let job_rows = tx
            .execute(
                "UPDATE llm_account_import_jobs
                 SET
                    completed_count = completed_count + $2,
                    succeeded_count = succeeded_count + $3,
                    skipped_count = skipped_count + $4,
                    failed_count = failed_count + $5,
                    status = CASE
                        WHEN completed_count + $2 >= total_count THEN 'completed'
                        ELSE status
                    END,
                    updated_at_ms = $6,
                    finished_at_ms = CASE
                        WHEN completed_count + $2 >= total_count THEN $6
                        ELSE finished_at_ms
                    END
                 WHERE job_id = $1",
                &[
                    &job_id,
                    &(result.completed_delta as i64),
                    &(result.succeeded_delta as i64),
                    &(result.skipped_delta as i64),
                    &(result.failed_delta as i64),
                    &result.updated_at_ms,
                ],
            )
            .await
            .context("roll up postgres codex import job counters")?;
        if job_rows == 0 {
            anyhow::bail!("codex import job `{job_id}` not found");
        }
        tx.commit()
            .await
            .context("commit postgres codex import job item completion transaction")?;
        self.load_codex_import_job_summary_row(job_id).await
    }

    async fn fail_admin_codex_import_job(
        &self,
        job_id: &str,
        error_message: &str,
        finished_at_ms: i64,
    ) -> anyhow::Result<()> {
        self.ensure_connection_alive()?;
        let changed = self
            .client
            .execute(
                "UPDATE llm_account_import_jobs
                 SET
                    status = 'failed',
                    batch_error_message = $2,
                    updated_at_ms = $3,
                    finished_at_ms = $3
                 WHERE job_id = $1",
                &[&job_id, &error_message, &finished_at_ms],
            )
            .await
            .context("mark postgres codex import job failed")?;
        if changed == 0 {
            anyhow::bail!("codex import job `{job_id}` not found");
        }
        Ok(())
    }
}

#[async_trait]
impl AdminKiroAccountStore for PostgresControlRepository {
    async fn list_admin_kiro_accounts(&self) -> anyhow::Result<Vec<AdminKiroAccount>> {
        let rows = self.list_kiro_admin_account_rows().await?;
        let context = self.load_kiro_admin_account_view_context().await?;
        Ok(rows
            .iter()
            .map(|row| self.admin_kiro_account_from_list_row_with_context(row, &context))
            .collect())
    }

    async fn list_admin_kiro_accounts_page(
        &self,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminKiroAccountsPage> {
        self.list_admin_kiro_accounts_filtered_page(None, page)
            .await
    }

    async fn list_admin_kiro_accounts_filtered_page(
        &self,
        prefix: Option<&str>,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminKiroAccountsPage> {
        let page = AdminPageRequest {
            limit: page.limit.max(1),
            offset: page.offset,
        };
        let (rows, total) = self
            .list_kiro_admin_account_rows_page_filtered(page, prefix)
            .await?;
        let context = self.load_kiro_admin_account_view_context().await?;
        let accounts = rows
            .iter()
            .map(|row| self.admin_kiro_account_from_list_row_with_context(row, &context))
            .collect::<Vec<_>>();
        let summary = self.admin_kiro_accounts_summary().await?;
        Ok(AdminKiroAccountsPage {
            has_more: page.has_more(accounts.len(), total),
            accounts,
            summary,
            total,
            limit: page.limit,
            offset: page.offset,
        })
    }

    async fn list_kiro_status_refresh_targets(
        &self,
    ) -> anyhow::Result<Vec<KiroStatusRefreshTarget>> {
        self.ensure_connection_alive()?;
        let refresh_interval_seconds = self
            .load_runtime_config_record_cached()
            .await?
            .map(|config| config.kiro_status_refresh_max_interval_seconds.max(0) as u64)
            .unwrap_or(core_store::DEFAULT_KIRO_STATUS_REFRESH_MAX_INTERVAL_SECONDS);
        let default_cache = AdminKiroCacheView {
            refresh_interval_seconds,
            ..AdminKiroCacheView::default()
        };
        let mut status_by_account = self.list_kiro_cached_status_parts_rows().await?;
        let rows = self
            .client
            .query(
                "SELECT
                    account_name,
                    status,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'disabled') = 'boolean'
                        THEN (auth_json ->> 'disabled')::boolean
                        ELSE false
                    END
                 FROM llm_kiro_accounts
                 ORDER BY account_name",
                &[],
            )
            .await
            .context("list postgres kiro status refresh targets")?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let name: String = row.get(0);
                let status: String = row.get(1);
                let disabled_json: bool = row.get(2);
                let cache = status_by_account
                    .remove(&name)
                    .map(|(_, cache)| cache)
                    .unwrap_or_else(|| default_cache.clone());
                KiroStatusRefreshTarget {
                    name,
                    disabled: disabled_json || status != core_store::KEY_STATUS_ACTIVE,
                    cache,
                }
            })
            .collect())
    }

    async fn create_admin_kiro_account(
        &self,
        account: NewAdminKiroAccount,
    ) -> anyhow::Result<AdminKiroAccount> {
        let record = KiroAccountRecord {
            account_name: account.name.clone(),
            auth_method: account.auth_method.clone(),
            account_id: account.account_id.clone(),
            profile_arn: account.profile_arn.clone(),
            user_id: account.user_id.clone(),
            status: account.status.clone(),
            auth_json: account.auth_json.clone(),
            max_concurrency: account.max_concurrency.map(|value| value as i64),
            min_start_interval_ms: account.min_start_interval_ms.map(|value| value as i64),
            proxy_config_id: account.proxy_config_id.clone(),
            last_refresh_at_ms: Some(account.created_at_ms),
            last_error: None,
            created_at_ms: account.created_at_ms,
            updated_at_ms: account.created_at_ms,
        };
        self.upsert_kiro_account(&record).await?;
        self.invalidate_account_cache(core_store::PROVIDER_KIRO, &account.name)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_KIRO)
            .await;
        let Some(record) = self.get_kiro_account_row(&account.name).await? else {
            anyhow::bail!("created postgres kiro account disappeared");
        };
        self.admin_kiro_account_from_record(&record).await
    }

    async fn patch_admin_kiro_account(
        &self,
        name: &str,
        patch: AdminKiroAccountPatch,
    ) -> anyhow::Result<Option<AdminKiroAccount>> {
        let Some(mut record) = self.get_kiro_account_row(name).await? else {
            return Ok(None);
        };
        let mut auth_value = serde_json::from_str::<serde_json::Value>(&record.auth_json)
            .context("parse postgres kiro auth json for patch")?;
        let object = auth_value
            .as_object_mut()
            .context("kiro auth json must be an object")?;
        if let Some(status) = patch.status.as_ref() {
            record.status = status.clone();
            set_json_optional_bool(
                object,
                "disabled",
                Some(status == core_store::KEY_STATUS_DISABLED),
            );
            object.remove("disabledReason");
            object.remove("disabled_reason");
        }
        if let Some(value) = patch.max_concurrency {
            record.max_concurrency = Some(value as i64);
            set_json_optional_u64(object, "kiroChannelMaxConcurrency", Some(value));
        }
        if let Some(value) = patch.min_start_interval_ms {
            record.min_start_interval_ms = Some(value as i64);
            set_json_optional_u64(object, "kiroChannelMinStartIntervalMs", Some(value));
        }
        if let Some(value) = patch.minimum_remaining_credits_before_block {
            set_json_optional_f64(
                object,
                "minimumRemainingCreditsBeforeBlock",
                Some(value.max(0.0)),
            )?;
        }
        if let Some(proxy_mode) = patch.proxy_mode.as_ref() {
            set_json_optional_string(object, "proxyMode", Some(proxy_mode.clone()));
        }
        if let Some(proxy_config_id) = patch.proxy_config_id.as_ref() {
            record.proxy_config_id = proxy_config_id.clone();
            set_json_optional_string(object, "proxyConfigId", proxy_config_id.clone());
        }
        record.auth_json =
            serde_json::to_string(&auth_value).context("serialize postgres kiro auth json")?;
        record.updated_at_ms = patch.updated_at_ms;
        self.upsert_kiro_account(&record).await?;
        self.invalidate_account_cache(core_store::PROVIDER_KIRO, &record.account_name)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_KIRO)
            .await;
        Ok(Some(self.admin_kiro_account_from_record(&record).await?))
    }

    async fn delete_admin_kiro_account(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<AdminKiroAccount>> {
        let Some(record) = self.get_kiro_account_row(name).await? else {
            return Ok(None);
        };
        let view = self.admin_kiro_account_from_record(&record).await?;
        self.ensure_connection_alive()?;
        self.client
            .execute("DELETE FROM llm_kiro_accounts WHERE account_name = $1", &[&name])
            .await
            .context("delete postgres kiro account")?;
        self.invalidate_account_cache(core_store::PROVIDER_KIRO, &record.account_name)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_KIRO)
            .await;
        Ok(Some(view))
    }

    async fn get_admin_kiro_balance(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<AdminKiroBalanceView>> {
        let Some((balance, _cache)) = self.get_kiro_cached_status_parts_row(name).await? else {
            return Ok(None);
        };
        Ok(balance)
    }

    async fn resolve_admin_kiro_account_route(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<ProviderKiroRoute>> {
        let Some(record) = self.get_kiro_account_row(account_name).await? else {
            return Ok(None);
        };
        if record.status != core_store::KEY_STATUS_ACTIVE {
            return Ok(None);
        }
        let runtime_config = self
            .load_runtime_config_record_cached()
            .await?
            .unwrap_or_default();
        let auth_json = serde_json::from_str::<serde_json::Value>(&record.auth_json)
            .context("parse kiro account auth json")?;
        let profile_arn = record
            .profile_arn
            .clone()
            .or_else(|| optional_json_string(&auth_json, "profileArn"))
            .or_else(|| optional_json_string(&auth_json, "profile_arn"));
        let api_region = optional_json_string(&auth_json, "apiRegion")
            .or_else(|| optional_json_string(&auth_json, "api_region"))
            .or_else(|| optional_json_string(&auth_json, "region"))
            .unwrap_or_else(|| "us-east-1".to_string());
        let minimum_remaining_credits_before_block = optional_json_f64_any(&auth_json, &[
            "minimumRemainingCreditsBeforeBlock",
            "minimum_remaining_credits_before_block",
        ])
        .filter(|value| value.is_finite())
        .unwrap_or(0.0)
        .max(0.0);
        let cached_status = self
            .get_kiro_cached_status_parts_row(&record.account_name)
            .await?;
        let cached_balance = cached_status
            .as_ref()
            .and_then(|(balance, _)| balance.as_ref());
        let cached_balance_view = cached_balance.cloned();
        let cached_cache_view = cached_status.as_ref().map(|(_, cache)| cache.clone());
        let cached_status_label = cached_status
            .as_ref()
            .map(|(_, cache)| cache.status.clone());
        let cached_remaining_credits = cached_balance.map(|balance| balance.remaining);
        let routing_identity = cached_balance
            .and_then(|balance| balance.user_id.clone())
            .or_else(|| record.user_id.clone())
            .unwrap_or_else(|| record.account_name.clone());
        let proxy_mode = optional_json_string_any(&auth_json, &["proxyMode", "proxy_mode"])
            .unwrap_or_else(|| {
                if record.proxy_config_id.is_some() {
                    "fixed".to_string()
                } else {
                    "inherit".to_string()
                }
            });
        let proxy_config_id = record.proxy_config_id.clone().or_else(|| {
            optional_json_string_any(&auth_json, &["proxyConfigId", "proxy_config_id"])
        });
        let proxy_context = self
            .load_provider_proxy_resolution_context(core_store::PROVIDER_KIRO)
            .await?;
        let proxy = resolve_provider_proxy_config_from_context(
            &proxy_mode,
            proxy_config_id.as_deref(),
            &proxy_context,
        )?;
        Ok(Some(ProviderKiroRoute {
            account_name: record.account_name,
            account_group_id_at_event: None,
            route_strategy_at_event: RouteStrategy::Auto,
            auth_json: record.auth_json,
            profile_arn,
            api_region,
            request_validation_enabled: true,
            cache_estimation_enabled: true,
            zero_cache_debug_enabled: false,
            full_request_logging_enabled: false,
            remote_media_resolution_enabled: false,
            latency_routing_enabled: true,
            model_name_map_json: "{}".to_string(),
            cache_kmodels_json: runtime_config.kiro_cache_kmodels_json,
            cache_policy_json: runtime_config.kiro_cache_policy_json,
            context_usage_min_request_tokens: runtime_config
                .kiro_context_usage_min_request_tokens
                .max(0) as u64,
            prefix_cache_mode: runtime_config.kiro_prefix_cache_mode,
            prefix_cache_max_tokens: runtime_config.kiro_prefix_cache_max_tokens.max(0) as u64,
            prefix_cache_entry_ttl_seconds: runtime_config
                .kiro_prefix_cache_entry_ttl_seconds
                .max(0) as u64,
            conversation_anchor_max_entries: runtime_config
                .kiro_conversation_anchor_max_entries
                .max(0) as u64,
            conversation_anchor_ttl_seconds: runtime_config
                .kiro_conversation_anchor_ttl_seconds
                .max(0) as u64,
            billable_model_multipliers_json: runtime_config.kiro_billable_model_multipliers_json,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            account_request_max_concurrency: record
                .max_concurrency
                .and_then(non_negative_i64_to_u64),
            account_request_min_start_interval_ms: record
                .min_start_interval_ms
                .and_then(non_negative_i64_to_u64),
            proxy,
            routing_identity,
            cached_status: cached_status_label,
            cached_remaining_credits,
            cached_balance: cached_balance_view,
            cached_cache: cached_cache_view,
            status_refresh_interval_seconds: runtime_config
                .kiro_status_refresh_max_interval_seconds
                .max(0) as u64,
            minimum_remaining_credits_before_block,
        }))
    }

    async fn save_admin_kiro_status_cache(
        &self,
        update: AdminKiroStatusCacheUpdate,
    ) -> anyhow::Result<()> {
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "INSERT INTO llm_kiro_status_cache (
                    account_name, status, balance_json, cache_json, refreshed_at_ms,
                    expires_at_ms, last_error
                ) VALUES ($1, $2, $3::jsonb, $4::jsonb, $5, $6, $7)
                ON CONFLICT(account_name) DO UPDATE SET
                    status = EXCLUDED.status,
                    balance_json = EXCLUDED.balance_json,
                    cache_json = EXCLUDED.cache_json,
                    refreshed_at_ms = EXCLUDED.refreshed_at_ms,
                    expires_at_ms = EXCLUDED.expires_at_ms,
                    last_error = EXCLUDED.last_error",
                &[
                    &update.account_name,
                    &update.cache.status,
                    &serde_json::to_string(&update.balance)
                        .context("encode postgres kiro balance cache")?,
                    &serde_json::to_string(&update.cache)
                        .context("encode postgres kiro cache view")?,
                    &update.refreshed_at_ms,
                    &update.expires_at_ms,
                    &update.last_error,
                ],
            )
            .await
            .context("upsert postgres kiro status cache")?;
        self.invalidate_account_cache(core_store::PROVIDER_KIRO, &update.account_name)
            .await;
        Ok(())
    }
}

#[async_trait]
impl ProviderRouteStore for PostgresControlRepository {
    async fn resolve_codex_route(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        Ok(self
            .resolve_codex_route_candidates(key)
            .await?
            .into_iter()
            .next())
    }

    async fn resolve_codex_route_candidates(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Vec<ProviderCodexRoute>> {
        let Some(snapshot) = self.load_codex_request_snapshot_cached(&key.key_id).await? else {
            return Ok(Vec::new());
        };
        if snapshot.key.provider_type != core_store::PROVIDER_CODEX {
            return Ok(Vec::new());
        }
        let route_strategy_at_event = match snapshot.route_strategy.as_str() {
            "fixed" => RouteStrategy::Fixed,
            _ => RouteStrategy::Auto,
        };
        let account_group_id_at_event = snapshot.account_group_id_at_event.clone();
        let status_by_account = self
            .load_codex_rate_limit_status_cached()
            .await?
            .map(|status| {
                status
                    .accounts
                    .into_iter()
                    .map(|account| (account.name.clone(), account))
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        let views_by_name = self
            .load_codex_account_views_cached(&snapshot.selected_account_names)
            .await?;
        let route_weight_tiers = views_by_name
            .iter()
            .map(|(name, view)| (name.clone(), view.route_weight_tier.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut routes = Vec::new();
        for account_name in snapshot.selected_account_names {
            let Some(view) = views_by_name.get(&account_name).cloned() else {
                continue;
            };
            if view.status != core_store::KEY_STATUS_ACTIVE {
                continue;
            }
            let minimal_auth_json =
                minimal_codex_auth_json_for_access_token(view.access_token.as_deref());
            let cached_error_message = codex_cached_error_message(
                &account_name,
                view.last_error.as_deref(),
                view.last_refresh_at_ms,
                view.auth_refresh_enabled,
                &minimal_auth_json,
                &status_by_account,
            );
            routes.push(ProviderCodexRoute {
                account_name: view.account_name,
                account_group_id_at_event: account_group_id_at_event.clone(),
                route_strategy_at_event,
                auth_json: String::new(),
                map_gpt53_codex_to_spark: view.map_gpt53_codex_to_spark,
                auth_refresh_enabled: view.auth_refresh_enabled,
                codex_fast_enabled: snapshot.codex_fast_enabled,
                request_max_concurrency: snapshot.request_max_concurrency,
                request_min_start_interval_ms: snapshot.request_min_start_interval_ms,
                account_request_max_concurrency: view.request_max_concurrency,
                account_request_min_start_interval_ms: view.request_min_start_interval_ms,
                cached_error_message,
                proxy: proxy_from_cached_option(view.proxy),
            });
        }
        let codex_status = self.load_codex_rate_limit_status_cached().await?;
        let runtime_config = RuntimeConfigRecord {
            codex_weight_free: snapshot.codex_weight_free,
            codex_weight_plus: snapshot.codex_weight_plus,
            codex_weight_pro5x: snapshot.codex_weight_pro5x,
            codex_weight_pro20x: snapshot.codex_weight_pro20x,
            ..RuntimeConfigRecord::default()
        };
        sort_codex_routes_by_cached_quota(
            &mut routes,
            codex_status.as_ref(),
            &runtime_config,
            &route_weight_tiers,
        );
        Ok(routes)
    }

    async fn resolve_codex_account_route(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        if self.request_cache.is_none() {
            return self.resolve_admin_codex_account_route(account_name).await;
        }
        let Some(view) = self
            .load_codex_account_views_cached(&[account_name.to_string()])
            .await?
            .remove(account_name)
        else {
            return Ok(None);
        };
        if view.status != core_store::KEY_STATUS_ACTIVE {
            return Ok(None);
        }
        let Some(auth) = self.load_codex_account_auth_cached(account_name).await? else {
            return Ok(None);
        };
        let status_by_account = self
            .load_codex_rate_limit_status_cached()
            .await?
            .map(|status| {
                status
                    .accounts
                    .into_iter()
                    .map(|account| (account.name.clone(), account))
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        let cached_error_message = codex_cached_error_message(
            account_name,
            view.last_error.as_deref(),
            view.last_refresh_at_ms,
            view.auth_refresh_enabled,
            &auth.auth_json,
            &status_by_account,
        );
        Ok(Some(ProviderCodexRoute {
            account_name: view.account_name,
            account_group_id_at_event: None,
            route_strategy_at_event: RouteStrategy::Auto,
            auth_json: auth.auth_json,
            map_gpt53_codex_to_spark: view.map_gpt53_codex_to_spark,
            auth_refresh_enabled: view.auth_refresh_enabled,
            codex_fast_enabled: true,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            account_request_max_concurrency: view.request_max_concurrency,
            account_request_min_start_interval_ms: view.request_min_start_interval_ms,
            cached_error_message,
            proxy: proxy_from_cached_option(view.proxy),
        }))
    }

    async fn resolve_kiro_route(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<ProviderKiroRoute>> {
        Ok(self
            .resolve_kiro_route_candidates(key)
            .await?
            .into_iter()
            .next())
    }

    async fn resolve_kiro_route_candidates(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Vec<ProviderKiroRoute>> {
        let Some(snapshot) = self.load_kiro_request_snapshot_cached(&key.key_id).await? else {
            return Ok(Vec::new());
        };
        if snapshot.key.provider_type != core_store::PROVIDER_KIRO {
            return Ok(Vec::new());
        }
        let route_strategy_at_event = match snapshot.route_strategy.as_str() {
            "fixed" => RouteStrategy::Fixed,
            _ => RouteStrategy::Auto,
        };
        let account_group_id_at_event = snapshot.account_group_id_at_event.clone();
        let views_by_name = self
            .load_kiro_account_views_cached(&snapshot.selected_account_names)
            .await?;
        let mut routes = Vec::new();
        for account_name in snapshot.selected_account_names {
            let Some(view) = views_by_name.get(&account_name).cloned() else {
                continue;
            };
            if view.status != core_store::KEY_STATUS_ACTIVE {
                continue;
            }
            if view.disabled {
                continue;
            }
            if let Some(cache_view) = &view.cached_cache {
                if matches!(cache_view.status.as_str(), "disabled" | "quota_exhausted") {
                    continue;
                }
            }
            if view.cached_balance.as_ref().is_some_and(|balance| {
                balance.remaining <= 0.0
                    || balance.remaining <= view.minimum_remaining_credits_before_block
            }) {
                continue;
            }
            routes.push(ProviderKiroRoute {
                account_name: view.account_name,
                account_group_id_at_event: account_group_id_at_event.clone(),
                route_strategy_at_event,
                auth_json: String::new(),
                profile_arn: view.profile_arn,
                api_region: view.api_region,
                request_validation_enabled: snapshot.request_validation_enabled,
                cache_estimation_enabled: snapshot.cache_estimation_enabled,
                zero_cache_debug_enabled: snapshot.zero_cache_debug_enabled,
                full_request_logging_enabled: snapshot.full_request_logging_enabled,
                remote_media_resolution_enabled: snapshot.remote_media_resolution_enabled,
                latency_routing_enabled: snapshot.latency_routing_enabled,
                model_name_map_json: snapshot.model_name_map_json.clone(),
                cache_kmodels_json: snapshot.cache_kmodels_json.clone(),
                cache_policy_json: snapshot.cache_policy_json.clone(),
                context_usage_min_request_tokens: snapshot.context_usage_min_request_tokens,
                prefix_cache_mode: snapshot.prefix_cache_mode.clone(),
                prefix_cache_max_tokens: snapshot.prefix_cache_max_tokens,
                prefix_cache_entry_ttl_seconds: snapshot.prefix_cache_entry_ttl_seconds,
                conversation_anchor_max_entries: snapshot.conversation_anchor_max_entries,
                conversation_anchor_ttl_seconds: snapshot.conversation_anchor_ttl_seconds,
                billable_model_multipliers_json: snapshot.billable_model_multipliers_json.clone(),
                request_max_concurrency: snapshot.request_max_concurrency,
                request_min_start_interval_ms: snapshot.request_min_start_interval_ms,
                account_request_max_concurrency: view.request_max_concurrency,
                account_request_min_start_interval_ms: view.request_min_start_interval_ms,
                proxy: proxy_from_cached_option(view.proxy),
                routing_identity: view.routing_identity,
                cached_status: view.cached_cache.as_ref().map(|cache| cache.status.clone()),
                cached_remaining_credits: view
                    .cached_balance
                    .as_ref()
                    .map(|balance| balance.remaining),
                cached_balance: view.cached_balance,
                cached_cache: view.cached_cache,
                status_refresh_interval_seconds: snapshot.status_refresh_interval_seconds,
                minimum_remaining_credits_before_block: view.minimum_remaining_credits_before_block,
            });
        }
        Ok(routes)
    }

    async fn resolve_kiro_account_route(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<ProviderKiroRoute>> {
        if self.request_cache.is_none() {
            return self.resolve_admin_kiro_account_route(account_name).await;
        }
        let Some(view) = self
            .load_kiro_account_views_cached(&[account_name.to_string()])
            .await?
            .remove(account_name)
        else {
            return Ok(None);
        };
        if view.status != core_store::KEY_STATUS_ACTIVE {
            return Ok(None);
        }
        let Some(auth) = self.load_kiro_account_auth_cached(account_name).await? else {
            return Ok(None);
        };
        let runtime_config = self
            .load_runtime_config_record_cached()
            .await?
            .unwrap_or_default();
        Ok(Some(ProviderKiroRoute {
            account_name: view.account_name,
            account_group_id_at_event: None,
            route_strategy_at_event: RouteStrategy::Auto,
            auth_json: auth.auth_json,
            profile_arn: view.profile_arn,
            api_region: view.api_region,
            request_validation_enabled: true,
            cache_estimation_enabled: true,
            zero_cache_debug_enabled: false,
            full_request_logging_enabled: false,
            remote_media_resolution_enabled: false,
            latency_routing_enabled: true,
            model_name_map_json: "{}".to_string(),
            cache_kmodels_json: runtime_config.kiro_cache_kmodels_json,
            cache_policy_json: runtime_config.kiro_cache_policy_json,
            context_usage_min_request_tokens: runtime_config
                .kiro_context_usage_min_request_tokens
                .max(0) as u64,
            prefix_cache_mode: runtime_config.kiro_prefix_cache_mode,
            prefix_cache_max_tokens: runtime_config.kiro_prefix_cache_max_tokens.max(0) as u64,
            prefix_cache_entry_ttl_seconds: runtime_config
                .kiro_prefix_cache_entry_ttl_seconds
                .max(0) as u64,
            conversation_anchor_max_entries: runtime_config
                .kiro_conversation_anchor_max_entries
                .max(0) as u64,
            conversation_anchor_ttl_seconds: runtime_config
                .kiro_conversation_anchor_ttl_seconds
                .max(0) as u64,
            billable_model_multipliers_json: runtime_config.kiro_billable_model_multipliers_json,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            account_request_max_concurrency: view.request_max_concurrency,
            account_request_min_start_interval_ms: view.request_min_start_interval_ms,
            proxy: proxy_from_cached_option(view.proxy),
            routing_identity: view.routing_identity,
            cached_status: view.cached_cache.as_ref().map(|cache| cache.status.clone()),
            cached_remaining_credits: view
                .cached_balance
                .as_ref()
                .map(|balance| balance.remaining),
            cached_balance: view.cached_balance,
            cached_cache: view.cached_cache,
            status_refresh_interval_seconds: runtime_config
                .kiro_status_refresh_max_interval_seconds
                .max(0) as u64,
            minimum_remaining_credits_before_block: view.minimum_remaining_credits_before_block,
        }))
    }

    async fn save_kiro_auth_update(&self, _update: ProviderKiroAuthUpdate) -> anyhow::Result<()> {
        let Some(mut record) = self.get_kiro_account_row(&_update.account_name).await? else {
            anyhow::bail!("kiro account `{}` is not configured", _update.account_name);
        };
        record.auth_json = _update.auth_json.clone();
        record.auth_method = _update.auth_method.clone();
        record.account_id = _update.account_id.clone();
        record.profile_arn = _update.profile_arn.clone();
        record.user_id = _update.user_id.clone();
        record.status = _update.status.clone();
        record.last_refresh_at_ms = Some(_update.refreshed_at_ms);
        record.last_error = _update.last_error.clone();
        record.updated_at_ms = _update.refreshed_at_ms;
        self.upsert_kiro_account(&record).await?;
        self.invalidate_account_cache(core_store::PROVIDER_KIRO, &record.account_name)
            .await;
        Ok(())
    }

    async fn save_codex_auth_update(&self, update: ProviderCodexAuthUpdate) -> anyhow::Result<()> {
        let Some(mut record) = self.get_codex_account_row(&update.account_name).await? else {
            anyhow::bail!("codex account `{}` is not configured", update.account_name);
        };
        record.auth_json = update.auth_json.clone();
        if update.account_id.is_some() {
            record.account_id = update.account_id.clone();
        }
        record.status = update.status.clone();
        record.last_refresh_at_ms = Some(update.refreshed_at_ms);
        record.last_error = update.last_error.clone();
        record.updated_at_ms = update.refreshed_at_ms;
        self.upsert_codex_account(&record).await?;
        self.invalidate_account_cache(core_store::PROVIDER_CODEX, &record.account_name)
            .await;
        Ok(())
    }

    async fn set_codex_account_auto_refresh_enabled(
        &self,
        account_name: &str,
        enabled: bool,
        updated_at_ms: i64,
    ) -> anyhow::Result<()> {
        let Some(mut record) = self.get_codex_account_row(account_name).await? else {
            anyhow::bail!("codex account `{account_name}` is not configured");
        };
        let mut settings = decode_codex_account_settings(&record.settings_json)?;
        if settings.auth_refresh_enabled == enabled {
            return Ok(());
        }
        settings.auth_refresh_enabled = enabled;
        record.settings_json =
            serde_json::to_string(&settings).context("serialize postgres codex settings")?;
        record.updated_at_ms = updated_at_ms;
        self.upsert_codex_account(&record).await?;
        self.invalidate_account_cache(core_store::PROVIDER_CODEX, &record.account_name)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_CODEX)
            .await;
        Ok(())
    }

    async fn mark_kiro_account_quota_exhausted(
        &self,
        account_name: &str,
        error_message: &str,
        checked_at_ms: i64,
    ) -> anyhow::Result<()> {
        let refresh_interval_seconds = self
            .load_runtime_config_record_cached()
            .await?
            .unwrap_or_default()
            .kiro_status_refresh_max_interval_seconds
            .max(0) as u64;
        self.save_admin_kiro_status_cache(AdminKiroStatusCacheUpdate {
            account_name: account_name.to_string(),
            balance: None,
            cache: AdminKiroCacheView {
                status: "quota_exhausted".to_string(),
                refresh_interval_seconds,
                last_checked_at: Some(checked_at_ms),
                last_success_at: Some(checked_at_ms),
                error_message: Some(error_message.to_string()),
            },
            refreshed_at_ms: checked_at_ms,
            expires_at_ms: checked_at_ms
                .saturating_add((refresh_interval_seconds as i64).saturating_mul(1000)),
            last_error: Some(error_message.to_string()),
        })
        .await
    }

    async fn save_kiro_status_cache_update(
        &self,
        update: AdminKiroStatusCacheUpdate,
    ) -> anyhow::Result<()> {
        self.save_admin_kiro_status_cache(update).await
    }
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

#[async_trait]
impl UsageEventSink for PostgresControlRepository {
    async fn append_usage_events(&self, events: &[UsageEvent]) -> anyhow::Result<()> {
        self.apply_usage_rollups_batch(events).await
    }

    async fn append_usage_events_owned(&self, events: Vec<UsageEvent>) -> anyhow::Result<()> {
        self.apply_usage_rollups_batch(&events).await
    }
}

#[async_trait]
impl PublicSubmissionStore for PostgresControlRepository {
    async fn create_public_token_request(
        &self,
        request: NewPublicTokenRequest,
    ) -> anyhow::Result<()> {
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "INSERT INTO llm_token_requests (
                    request_id, requester_email, requested_quota_billable_limit, request_reason,
                    frontend_page_url, status, fingerprint, client_ip, ip_region, admin_note,
                    failure_reason, issued_key_id, issued_key_name, created_at_ms,
                    updated_at_ms, processed_at_ms
                ) VALUES (
                    $1, $2, $3, $4, $5, $6, $7, $8, $9, NULL, NULL, NULL, NULL, $10, $10, NULL
                )",
                &[
                    &request.request_id,
                    &request.requester_email,
                    &(request.requested_quota_billable_limit as i64),
                    &request.request_reason,
                    &request.frontend_page_url,
                    &PUBLIC_TOKEN_REQUEST_STATUS_PENDING,
                    &request.fingerprint,
                    &request.client_ip,
                    &request.ip_region,
                    &request.created_at_ms,
                ],
            )
            .await
            .context("create postgres public token request")?;
        Ok(())
    }

    async fn create_public_account_contribution_request(
        &self,
        request: NewPublicAccountContributionRequest,
    ) -> anyhow::Result<()> {
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "INSERT INTO llm_account_contribution_requests (
                    request_id, account_name, account_id, id_token, access_token, refresh_token,
                    requester_email, contributor_message, github_id, frontend_page_url,
                    show_on_public_wall, status, fingerprint, client_ip, ip_region,
                    admin_note, failure_reason, imported_account_name, issued_key_id,
                    issued_key_name, created_at_ms, updated_at_ms, processed_at_ms
                ) VALUES (
                    $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                    $15, NULL, NULL, NULL, NULL, NULL, $16, $16, NULL
                )",
                &[
                    &request.request_id,
                    &request.account_name,
                    &request.account_id,
                    &request.id_token,
                    &request.access_token,
                    &request.refresh_token,
                    &request.requester_email,
                    &request.contributor_message,
                    &request.github_id,
                    &request.frontend_page_url,
                    &request.show_on_public_wall,
                    &PUBLIC_TOKEN_REQUEST_STATUS_PENDING,
                    &request.fingerprint,
                    &request.client_ip,
                    &request.ip_region,
                    &request.created_at_ms,
                ],
            )
            .await
            .context("create postgres public account contribution request")?;
        Ok(())
    }

    async fn public_account_contribution_name_exists(
        &self,
        account_name: &str,
    ) -> anyhow::Result<bool> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_one(
                "SELECT EXISTS(
                    SELECT 1 FROM llm_codex_accounts WHERE account_name = $1
                    UNION ALL
                    SELECT 1 FROM llm_account_contribution_requests
                     WHERE account_name = $1
                       AND status IN ($2, $3, 'issued')
                )",
                &[
                    &account_name,
                    &PUBLIC_TOKEN_REQUEST_STATUS_PENDING,
                    &PUBLIC_ACCOUNT_CONTRIBUTION_STATUS_VALIDATED,
                ],
            )
            .await
            .context("check postgres public account contribution name")?;
        Ok(row.get(0))
    }

    async fn create_public_sponsor_request(
        &self,
        request: NewPublicSponsorRequest,
    ) -> anyhow::Result<()> {
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "INSERT INTO llm_sponsor_requests (
                    request_id, requester_email, sponsor_message, display_name, github_id,
                    frontend_page_url, status, fingerprint, client_ip, ip_region, admin_note,
                    failure_reason, payment_email_sent_at_ms, created_at_ms, updated_at_ms,
                    processed_at_ms
                ) VALUES (
                    $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NULL, NULL, NULL, $11, $11, NULL
                )",
                &[
                    &request.request_id,
                    &request.requester_email,
                    &request.sponsor_message,
                    &request.display_name,
                    &request.github_id,
                    &request.frontend_page_url,
                    &PUBLIC_SPONSOR_REQUEST_STATUS_SUBMITTED,
                    &request.fingerprint,
                    &request.client_ip,
                    &request.ip_region,
                    &request.created_at_ms,
                ],
            )
            .await
            .context("create postgres public sponsor request")?;
        Ok(())
    }

    async fn record_public_sponsor_payment_email_result(
        &self,
        request_id: &str,
        sent_at_ms: Option<i64>,
        failure_reason: Option<String>,
    ) -> anyhow::Result<()> {
        self.ensure_connection_alive()?;
        let status = if sent_at_ms.is_some() {
            PUBLIC_SPONSOR_REQUEST_STATUS_PAYMENT_EMAIL_SENT
        } else {
            PUBLIC_SPONSOR_REQUEST_STATUS_SUBMITTED
        };
        let updated_at_ms = sent_at_ms.unwrap_or_else(now_ms);
        self.client
            .execute(
                "UPDATE llm_sponsor_requests
                 SET status = $2,
                     failure_reason = $3,
                     payment_email_sent_at_ms = $4,
                     updated_at_ms = $5
                 WHERE request_id = $1",
                &[&request_id, &status, &failure_reason, &sent_at_ms, &updated_at_ms],
            )
            .await
            .context("record postgres sponsor payment email result")?;
        Ok(())
    }
}

#[async_trait]
impl PublicAccessStore for PostgresControlRepository {
    async fn auth_cache_ttl_seconds(&self) -> anyhow::Result<u64> {
        Ok(self
            .load_runtime_config_record_cached()
            .await?
            .map_or(DEFAULT_AUTH_CACHE_TTL_SECONDS, |record| {
                record.auth_cache_ttl_seconds.max(0) as u64
            }))
    }

    async fn list_public_access_keys(&self) -> anyhow::Result<Vec<PublicAccessKey>> {
        self.list_public_access_keys_rows().await
    }
}

#[async_trait]
impl PublicCommunityStore for PostgresControlRepository {
    async fn list_public_account_contributions(
        &self,
        limit: usize,
    ) -> anyhow::Result<Vec<PublicAccountContribution>> {
        self.list_public_account_contributions_rows(limit).await
    }

    async fn list_public_sponsors(&self, limit: usize) -> anyhow::Result<Vec<PublicSponsor>> {
        self.list_public_sponsors_rows(limit).await
    }
}

#[async_trait]
impl PublicUsageStore for PostgresControlRepository {
    async fn get_public_usage_key_by_secret(
        &self,
        secret: &str,
    ) -> anyhow::Result<Option<PublicUsageLookupKey>> {
        self.load_public_usage_key_by_hash(&hash_bearer_secret(secret))
            .await
    }
}

#[async_trait]
impl AdminReviewQueueStore for PostgresControlRepository {
    async fn get_admin_token_request(
        &self,
        request_id: &str,
    ) -> anyhow::Result<Option<AdminTokenRequest>> {
        self.get_admin_token_request_row(request_id).await
    }

    async fn list_admin_token_requests(
        &self,
        query: AdminReviewQueueQuery,
    ) -> anyhow::Result<AdminTokenRequestsPage> {
        let total = self
            .count_rows(
                "SELECT COUNT(*) FROM llm_token_requests",
                "SELECT COUNT(*) FROM llm_token_requests WHERE status = $1",
                query.status.as_deref(),
            )
            .await?;
        if total == 0 || query.offset >= total {
            return Ok(AdminTokenRequestsPage {
                total,
                offset: query.offset,
                limit: query.limit,
                has_more: false,
                requests: Vec::new(),
            });
        }
        let rows = if let Some(status) = query.status.as_deref() {
            self.client
                .query(
                    "SELECT
                        request_id, requester_email, requested_quota_billable_limit,
                        request_reason, frontend_page_url, status, client_ip, ip_region,
                        admin_note, failure_reason, issued_key_id, issued_key_name,
                        created_at_ms, updated_at_ms, processed_at_ms
                     FROM llm_token_requests
                     WHERE status = $1
                     ORDER BY created_at_ms DESC, request_id DESC
                     LIMIT $2 OFFSET $3",
                    &[&status, &(query.limit as i64), &(query.offset as i64)],
                )
                .await
                .context("list admin token requests by status")?
        } else {
            self.client
                .query(
                    "SELECT
                        request_id, requester_email, requested_quota_billable_limit,
                        request_reason, frontend_page_url, status, client_ip, ip_region,
                        admin_note, failure_reason, issued_key_id, issued_key_name,
                        created_at_ms, updated_at_ms, processed_at_ms
                     FROM llm_token_requests
                     ORDER BY created_at_ms DESC, request_id DESC
                     LIMIT $1 OFFSET $2",
                    &[&(query.limit as i64), &(query.offset as i64)],
                )
                .await
                .context("list admin token requests")?
        };
        let requests = rows
            .into_iter()
            .map(decode_admin_token_request_row)
            .collect::<Vec<_>>();
        Ok(AdminTokenRequestsPage {
            total,
            offset: query.offset,
            limit: query.limit,
            has_more: query.offset.saturating_add(requests.len()) < total,
            requests,
        })
    }

    async fn get_admin_account_contribution_request(
        &self,
        request_id: &str,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>> {
        self.get_admin_account_contribution_request_row(request_id)
            .await
    }

    async fn list_admin_account_contribution_requests(
        &self,
        query: AdminReviewQueueQuery,
    ) -> anyhow::Result<AdminAccountContributionRequestsPage> {
        let total = self
            .count_rows(
                "SELECT COUNT(*) FROM llm_account_contribution_requests",
                "SELECT COUNT(*) FROM llm_account_contribution_requests WHERE status = $1",
                query.status.as_deref(),
            )
            .await?;
        if total == 0 || query.offset >= total {
            return Ok(AdminAccountContributionRequestsPage {
                total,
                offset: query.offset,
                limit: query.limit,
                has_more: false,
                requests: Vec::new(),
            });
        }
        let rows = if let Some(status) = query.status.as_deref() {
            self.client
                .query(
                    "SELECT
                        request_id, account_name, account_id, id_token, access_token,
                        refresh_token, requester_email, contributor_message, github_id,
                        frontend_page_url, status, client_ip, ip_region, admin_note,
                        failure_reason, imported_account_name, issued_key_id, issued_key_name,
                        created_at_ms, updated_at_ms, processed_at_ms
                     FROM llm_account_contribution_requests
                     WHERE status = $1
                     ORDER BY created_at_ms DESC, request_id DESC
                     LIMIT $2 OFFSET $3",
                    &[&status, &(query.limit as i64), &(query.offset as i64)],
                )
                .await
                .context("list admin account contribution requests by status")?
        } else {
            self.client
                .query(
                    "SELECT
                        request_id, account_name, account_id, id_token, access_token,
                        refresh_token, requester_email, contributor_message, github_id,
                        frontend_page_url, status, client_ip, ip_region, admin_note,
                        failure_reason, imported_account_name, issued_key_id, issued_key_name,
                        created_at_ms, updated_at_ms, processed_at_ms
                     FROM llm_account_contribution_requests
                     ORDER BY created_at_ms DESC, request_id DESC
                     LIMIT $1 OFFSET $2",
                    &[&(query.limit as i64), &(query.offset as i64)],
                )
                .await
                .context("list admin account contribution requests")?
        };
        let requests = rows
            .into_iter()
            .map(decode_admin_account_contribution_request_row)
            .collect::<Vec<_>>();
        Ok(AdminAccountContributionRequestsPage {
            total,
            offset: query.offset,
            limit: query.limit,
            has_more: query.offset.saturating_add(requests.len()) < total,
            requests,
        })
    }

    async fn get_admin_sponsor_request(
        &self,
        request_id: &str,
    ) -> anyhow::Result<Option<AdminSponsorRequest>> {
        self.get_admin_sponsor_request_row(request_id).await
    }

    async fn list_admin_sponsor_requests(
        &self,
        query: AdminReviewQueueQuery,
    ) -> anyhow::Result<AdminSponsorRequestsPage> {
        let total = self
            .count_rows(
                "SELECT COUNT(*) FROM llm_sponsor_requests",
                "SELECT COUNT(*) FROM llm_sponsor_requests WHERE status = $1",
                query.status.as_deref(),
            )
            .await?;
        if total == 0 || query.offset >= total {
            return Ok(AdminSponsorRequestsPage {
                total,
                offset: query.offset,
                limit: query.limit,
                has_more: false,
                requests: Vec::new(),
            });
        }
        let rows = if let Some(status) = query.status.as_deref() {
            self.client
                .query(
                    "SELECT
                        request_id, requester_email, sponsor_message, display_name, github_id,
                        frontend_page_url, status, client_ip, ip_region, admin_note,
                        failure_reason, payment_email_sent_at_ms, created_at_ms, updated_at_ms,
                        processed_at_ms
                     FROM llm_sponsor_requests
                     WHERE status = $1
                     ORDER BY created_at_ms DESC, request_id DESC
                     LIMIT $2 OFFSET $3",
                    &[&status, &(query.limit as i64), &(query.offset as i64)],
                )
                .await
                .context("list admin sponsor requests by status")?
        } else {
            self.client
                .query(
                    "SELECT
                        request_id, requester_email, sponsor_message, display_name, github_id,
                        frontend_page_url, status, client_ip, ip_region, admin_note,
                        failure_reason, payment_email_sent_at_ms, created_at_ms, updated_at_ms,
                        processed_at_ms
                     FROM llm_sponsor_requests
                     ORDER BY created_at_ms DESC, request_id DESC
                     LIMIT $1 OFFSET $2",
                    &[&(query.limit as i64), &(query.offset as i64)],
                )
                .await
                .context("list admin sponsor requests")?
        };
        let requests = rows
            .into_iter()
            .map(decode_admin_sponsor_request_row)
            .collect::<Vec<_>>();
        Ok(AdminSponsorRequestsPage {
            total,
            offset: query.offset,
            limit: query.limit,
            has_more: query.offset.saturating_add(requests.len()) < total,
            requests,
        })
    }

    async fn issue_admin_token_request(
        &self,
        request_id: &str,
        key: Option<NewAdminKey>,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminTokenRequest>> {
        let Some(current) = self.get_admin_token_request_row(request_id).await? else {
            return Ok(None);
        };
        let (issued_key_id, issued_key_name) = match (current.issued_key_id, key) {
            (Some(id), _) => (Some(id), current.issued_key_name),
            (None, Some(key)) => {
                let created = self.create_admin_key(key).await?;
                (Some(created.id), Some(created.name))
            },
            (None, None) => (None, None),
        };
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "UPDATE llm_token_requests
                 SET status = 'issued',
                     admin_note = $2,
                     failure_reason = NULL,
                     issued_key_id = $3,
                     issued_key_name = $4,
                     updated_at_ms = $5,
                     processed_at_ms = $5
                 WHERE request_id = $1",
                &[
                    &request_id,
                    &action.admin_note,
                    &issued_key_id,
                    &issued_key_name,
                    &action.updated_at_ms,
                ],
            )
            .await
            .context("issue postgres admin token request")?;
        self.get_admin_token_request_row(request_id).await
    }

    async fn reject_admin_token_request(
        &self,
        request_id: &str,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminTokenRequest>> {
        let Some(current) = self.get_admin_token_request_row(request_id).await? else {
            return Ok(None);
        };
        if let Some(key_id) = current.issued_key_id.as_deref() {
            self.disable_admin_key_if_present(key_id, action.updated_at_ms)
                .await?;
        }
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "UPDATE llm_token_requests
                 SET status = 'rejected',
                     admin_note = $2,
                     failure_reason = NULL,
                     updated_at_ms = $3,
                     processed_at_ms = $3
                 WHERE request_id = $1",
                &[&request_id, &action.admin_note, &action.updated_at_ms],
            )
            .await
            .context("reject postgres admin token request")?;
        self.get_admin_token_request_row(request_id).await
    }

    async fn issue_admin_account_contribution_request(
        &self,
        request_id: &str,
        account: Option<NewAdminCodexAccount>,
        account_group: Option<NewAdminAccountGroup>,
        key: Option<NewAdminKey>,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>> {
        let Some(current) = self
            .get_admin_account_contribution_request_row(request_id)
            .await?
        else {
            return Ok(None);
        };
        let imported_account_name = match (current.imported_account_name, account) {
            (Some(name), _) => Some(name),
            (None, Some(account)) => {
                let created = self.create_admin_codex_account(account).await?;
                Some(created.name)
            },
            (None, None) => None,
        };
        if let Some(group) = account_group.clone() {
            self.create_admin_account_group(group).await?;
        }
        let (issued_key_id, issued_key_name) = match (current.issued_key_id, key) {
            (Some(id), _) => (Some(id), current.issued_key_name),
            (None, Some(key)) => {
                let created = self.create_admin_key(key).await?;
                if let Some(group) = account_group {
                    self.patch_admin_key(&created.id, AdminKeyPatch {
                        route_strategy: Some(Some("fixed".to_string())),
                        account_group_id: Some(Some(group.id.clone())),
                        updated_at_ms: action.updated_at_ms,
                        ..AdminKeyPatch::default()
                    })
                    .await?;
                }
                (Some(created.id), Some(created.name))
            },
            (None, None) => (None, None),
        };
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "UPDATE llm_account_contribution_requests
                 SET status = 'issued',
                     admin_note = $2,
                     failure_reason = NULL,
                     imported_account_name = $3,
                     issued_key_id = $4,
                     issued_key_name = $5,
                     updated_at_ms = $6,
                     processed_at_ms = $6
                 WHERE request_id = $1",
                &[
                    &request_id,
                    &action.admin_note,
                    &imported_account_name,
                    &issued_key_id,
                    &issued_key_name,
                    &action.updated_at_ms,
                ],
            )
            .await
            .context("issue postgres admin account contribution request")?;
        self.get_admin_account_contribution_request_row(request_id)
            .await
    }

    async fn validate_admin_account_contribution_request(
        &self,
        request_id: &str,
        account_id: Option<String>,
        id_token: String,
        access_token: String,
        refresh_token: String,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>> {
        if self
            .get_admin_account_contribution_request_row(request_id)
            .await?
            .is_none()
        {
            return Ok(None);
        }
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "UPDATE llm_account_contribution_requests
                 SET status = $2,
                     account_id = $3,
                     id_token = $4,
                     access_token = $5,
                     refresh_token = $6,
                     admin_note = $7,
                     failure_reason = NULL,
                     updated_at_ms = $8,
                     processed_at_ms = NULL
                 WHERE request_id = $1",
                &[
                    &request_id,
                    &PUBLIC_ACCOUNT_CONTRIBUTION_STATUS_VALIDATED,
                    &account_id,
                    &id_token,
                    &access_token,
                    &refresh_token,
                    &action.admin_note,
                    &action.updated_at_ms,
                ],
            )
            .await
            .context("validate postgres admin account contribution request")?;
        self.get_admin_account_contribution_request_row(request_id)
            .await
    }

    async fn fail_admin_account_contribution_request(
        &self,
        request_id: &str,
        failure_reason: String,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>> {
        if self
            .get_admin_account_contribution_request_row(request_id)
            .await?
            .is_none()
        {
            return Ok(None);
        }
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "UPDATE llm_account_contribution_requests
                 SET status = 'failed',
                     admin_note = $2,
                     failure_reason = $3,
                     updated_at_ms = $4,
                     processed_at_ms = NULL
                 WHERE request_id = $1",
                &[&request_id, &action.admin_note, &failure_reason, &action.updated_at_ms],
            )
            .await
            .context("fail postgres admin account contribution request")?;
        self.get_admin_account_contribution_request_row(request_id)
            .await
    }

    async fn reject_admin_account_contribution_request(
        &self,
        request_id: &str,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>> {
        let Some(current) = self
            .get_admin_account_contribution_request_row(request_id)
            .await?
        else {
            return Ok(None);
        };
        if let Some(key_id) = current.issued_key_id.as_deref() {
            self.disable_admin_key_if_present(key_id, action.updated_at_ms)
                .await?;
        }
        if let Some(account_name) = current.imported_account_name.as_deref() {
            self.delete_admin_codex_account(account_name).await?;
        }
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "UPDATE llm_account_contribution_requests
                 SET status = 'rejected',
                     admin_note = $2,
                     failure_reason = NULL,
                     updated_at_ms = $3,
                     processed_at_ms = $3
                 WHERE request_id = $1",
                &[&request_id, &action.admin_note, &action.updated_at_ms],
            )
            .await
            .context("reject postgres admin account contribution request")?;
        self.get_admin_account_contribution_request_row(request_id)
            .await
    }

    async fn approve_admin_sponsor_request(
        &self,
        request_id: &str,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminSponsorRequest>> {
        if self
            .get_admin_sponsor_request_row(request_id)
            .await?
            .is_none()
        {
            return Ok(None);
        }
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "UPDATE llm_sponsor_requests
                 SET status = 'approved',
                     admin_note = $2,
                     failure_reason = NULL,
                     updated_at_ms = $3,
                     processed_at_ms = $3
                 WHERE request_id = $1",
                &[&request_id, &action.admin_note, &action.updated_at_ms],
            )
            .await
            .context("approve postgres sponsor request")?;
        self.get_admin_sponsor_request_row(request_id).await
    }

    async fn delete_admin_sponsor_request(&self, request_id: &str) -> anyhow::Result<bool> {
        self.ensure_connection_alive()?;
        let changed = self
            .client
            .execute("DELETE FROM llm_sponsor_requests WHERE request_id = $1", &[&request_id])
            .await
            .context("delete postgres sponsor request")?;
        Ok(changed > 0)
    }
}

#[async_trait]
impl PublicStatusStore for PostgresControlRepository {
    async fn codex_rate_limit_status(&self) -> anyhow::Result<CodexRateLimitStatus> {
        if let Some(snapshot) = self.load_codex_rate_limit_status_cached().await? {
            return Ok(snapshot);
        }
        let refresh_interval_seconds = self
            .load_runtime_config_record_cached()
            .await?
            .map(|record| record.codex_status_refresh_max_interval_seconds.max(0) as u64)
            .unwrap_or(DEFAULT_CODEX_STATUS_REFRESH_SECONDS);
        Ok(CodexRateLimitStatus::loading(refresh_interval_seconds))
    }

    async fn save_codex_rate_limit_status(
        &self,
        snapshot: CodexRateLimitStatus,
    ) -> anyhow::Result<()> {
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "INSERT INTO llm_codex_status_cache (id, snapshot_json, updated_at_ms)
                 VALUES ('default', $1::jsonb, $2)
                 ON CONFLICT(id) DO UPDATE SET
                    snapshot_json = EXCLUDED.snapshot_json,
                    updated_at_ms = EXCLUDED.updated_at_ms",
                &[
                    &serde_json::to_string(&snapshot)
                        .context("serialize postgres codex rate-limit snapshot")?,
                    &now_ms(),
                ],
            )
            .await
            .context("upsert postgres codex rate-limit status snapshot")?;
        if let Some(cache) = self.request_cache.as_ref() {
            let cache_key = cache.codex_status_key();
            let lookup = crate::request_cache::CachedCodexStatusLookup {
                snapshot: Some(snapshot.clone()),
            };
            if let Err(err) = cache
                .set_json(&cache_key, &lookup, cache.codex_status_ttl())
                .await
            {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache codex status write-through failed"
                );
            }
        }
        self.store_cached_codex_rate_limit_status(Some(snapshot))
            .await;
        Ok(())
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
