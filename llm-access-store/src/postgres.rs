//! Postgres control-plane repository for `llm-access`.
//! ## Module map
//!
//! `postgres.rs` is the facade: it owns the imports, the SQLx
//! client/transaction helpers, the row/record/context helper structs, the
//! `decode_*` and small free helpers, and the `PostgresControlRepository`
//! struct itself. Each storage domain owns one submodule holding that domain's
//! inherent-method `impl` block plus the matching `*Store` trait impl (split
//! across files as separate `impl` blocks on the same type — inherent/trait
//! impls apply crate-wide even from a private `mod`, so no re-export is
//! needed):
//!
//! ```text
//!  PostgresControlRepository (struct + helpers in parent)
//!    +-- [config]        connect/runtime-config/cache + AdminConfigStore, ControlStore
//!    +-- [keys]          admin-key CRUD + AdminKeyStore
//!    +-- [groups]        account groups + AdminAccountGroupStore
//!    +-- [proxy]         proxy config/bindings/checks + AdminProxyStore
//!    +-- [codex_account] Codex accounts/import jobs + AdminCodexAccountStore
//!    +-- [kiro_account]  Kiro accounts/cache views + AdminKiroAccountStore
//!    +-- [routing]       auth-key + request-snapshot caches + ProviderRouteStore
//!    +-- [usage]         usage proxy attribution + rollups + UsageEventSink
//!    +-- [status]        Codex rate-limit status cache + PublicStatusStore
//!    +-- [public]        public access/community/usage/review + their Stores
//! ```

pub(crate) use std::{
    collections::{BTreeMap, HashMap},
    env,
    sync::Arc,
    time::{Duration, Instant},
};

pub(crate) use anyhow::Context;
pub(crate) use llm_access_core::{
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
        AdminKiroKeyCandidateCreditSummary, AdminKiroStatusCacheUpdate,
        AdminLegacyKiroProxyMigration, AdminPageRequest, AdminProxyBinding, AdminProxyConfig,
        AdminProxyConfigPatch, AdminProxyEndpointCheck, AdminProxyEndpointCheckUpdate,
        AdminProxyStore, AdminReviewQueueAction, AdminReviewQueueQuery, AdminReviewQueueStore,
        AdminRuntimeConfig, AdminSponsorRequest, AdminSponsorRequestsPage, AdminTokenRequest,
        AdminTokenRequestsPage, AuthenticatedKey, CodexPublicAccountStatus, CodexRateLimitStatus,
        CodexStatusRefreshTarget, ControlStore, KiroStatusRefreshTarget, NewAdminAccountGroup,
        NewAdminCodexAccount, NewAdminCodexImportJob, NewAdminKey, NewAdminKiroAccount,
        NewAdminProxyConfig, NewPublicAccountContributionRequest, NewPublicSponsorRequest,
        NewPublicTokenRequest, ProviderCodexAuthUpdate, ProviderCodexRoute, ProviderKiroAuthUpdate,
        ProviderKiroRoute, ProviderProxyConfig, ProviderRouteStore, PublicAccessKey,
        PublicAccessStore, PublicAccountContribution, PublicCommunityStore, PublicSponsor,
        PublicStatusStore, PublicSubmissionStore, PublicUsageLookupKey, PublicUsageStore,
        UsageEventSink, DEFAULT_AUTH_CACHE_TTL_SECONDS, DEFAULT_CODEX_STATUS_REFRESH_SECONDS,
        PUBLIC_ACCOUNT_CONTRIBUTION_STATUS_VALIDATED,
        PUBLIC_SPONSOR_REQUEST_STATUS_PAYMENT_EMAIL_SENT, PUBLIC_SPONSOR_REQUEST_STATUS_SUBMITTED,
        PUBLIC_TOKEN_REQUEST_STATUS_PENDING,
    },
    usage::UsageEvent,
};
pub(crate) use llm_access_kiro::cache_policy::{
    resolve_effective_kiro_cache_policy, KiroCachePolicy,
};
pub(crate) use sha2::{Digest, Sha256};
pub(crate) use sqlx_core::{
    arguments::Arguments, column::ColumnIndex, decode::Decode, encode::Encode, query::query_with,
    query_builder::QueryBuilder, row::Row as SqlxRowTrait, types::Type,
};
pub(crate) use sqlx_postgres::{PgArguments, PgPool, PgPoolOptions, PgRow as SqlxPgRow, Postgres};
pub(crate) use tokio::sync::{Mutex, RwLock};

pub(crate) use crate::{
    records::{
        CodexAccountRecord, KeyBundle, KeyRecord, KeyRouteConfig, KeyUsageRollup,
        KiroAccountRecord, RuntimeConfigRecord,
    },
    request_cache::{RequestCache, RequestCacheConfig},
};

mod codex_account;
mod config;
mod groups;
mod keys;
mod kiro_account;
mod proxy;
mod public;
mod routing;
mod status;
mod usage;

pub(crate) trait SqlxBindParam {
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
pub(crate) struct PgRow(SqlxPgRow);
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
pub(crate) struct CodexRouteCandidateRow {
    account_name: String,
    status: String,
    settings_json: String,
    last_refresh_at_ms: Option<i64>,
    last_error: Option<String>,
    access_token: Option<String>,
}
#[derive(Debug, Clone)]
pub(crate) struct KiroRouteCandidateRow {
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
pub(crate) struct UsageRollupDelta {
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
pub(crate) struct SqlxClient {
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
pub(crate) struct SqlxTransaction<'a> {
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
pub(crate) struct CachedCodexRateLimitStatus {
    snapshot: CodexRateLimitStatus,
    loaded_at: Instant,
}
pub(crate) type KiroCachedStatusParts = (Option<AdminKiroBalanceView>, AdminKiroCacheView);
pub(crate) struct KiroAdminAccountViewContext {
    default_cache: AdminKiroCacheView,
    status_by_account: BTreeMap<String, KiroCachedStatusParts>,
    proxy_configs_by_id: BTreeMap<String, AdminProxyConfig>,
    kiro_proxy_binding: AdminProxyBinding,
}
pub(crate) struct CodexAdminAccountViewContext {
    proxy_configs_by_id: BTreeMap<String, AdminProxyConfig>,
    codex_proxy_binding: AdminProxyBinding,
}
pub(crate) struct ProviderProxyResolutionContext {
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
pub(crate) struct ProxyConfigNodeOverride {
    proxy_url: String,
    proxy_username: Option<String>,
    proxy_password: Option<String>,
    status: String,
    created_at_ms: i64,
    updated_at_ms: i64,
}
#[derive(Debug, Clone)]
pub(crate) struct ProxyEndpointCheckRow {
    proxy_config_id: String,
    provider_type: String,
    check: AdminProxyEndpointCheck,
}
#[derive(Debug, Clone)]
pub(crate) struct CodexAdminAccountListRow {
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
pub(crate) struct KiroAdminAccountListRow {
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
pub(crate) struct CodexAccountSettings {
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
fn hash_bearer_secret(secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    format!("{:x}", hasher.finalize())
}
fn decode_runtime_config_row(row: PgRow) -> anyhow::Result<RuntimeConfigRecord> {
    Ok(RuntimeConfigRecord {
        id: row.get(0),
        auth_cache_ttl_seconds: row.get(1),
        max_request_body_bytes: row.get(2),
        account_failure_retry_limit: row.get(3),
        codex_client_version: row.get(4),
        kiro_channel_max_concurrency: row.get(5),
        kiro_channel_min_start_interval_ms: row.get(6),
        codex_status_refresh_min_interval_seconds: row.get(7),
        codex_status_refresh_max_interval_seconds: row.get(8),
        codex_status_account_jitter_max_seconds: row.get(9),
        codex_weight_free: row.get(10),
        codex_weight_plus: row.get(11),
        codex_weight_pro5x: row.get(12),
        codex_weight_pro20x: row.get(13),
        kiro_status_refresh_min_interval_seconds: row.get(14),
        kiro_status_refresh_max_interval_seconds: row.get(15),
        kiro_status_account_jitter_max_seconds: row.get(16),
        usage_event_flush_batch_size: row.get(17),
        usage_event_flush_interval_seconds: row.get(18),
        usage_event_flush_max_buffer_bytes: row.get(19),
        duckdb_usage_memory_limit_mib: row.get(20),
        duckdb_usage_checkpoint_threshold_mib: row.get(21),
        usage_analytics_retention_days: row.get(22),
        usage_journal_enabled: row.get(23),
        usage_journal_max_file_bytes: row.get(24),
        usage_journal_max_file_age_ms: row.get(25),
        usage_journal_max_files: row.get(26),
        usage_journal_block_target_uncompressed_bytes: row.get(27),
        usage_journal_block_max_events: row.get(28),
        usage_journal_fsync_interval_ms: row.get(29),
        usage_journal_zstd_level: row.get(30),
        usage_journal_consumer_lease_ms: row.get(31),
        usage_journal_delete_bad_files: row.get::<_, i64>(32) != 0,
        usage_query_bind_addr: row.get(33),
        usage_query_base_url: row.get(34),
        usage_event_maintenance_enabled: row.get(35),
        usage_event_maintenance_interval_seconds: row.get(36),
        usage_event_detail_retention_days: row.get(37),
        kiro_cache_kmodels_json: row.get(38),
        kiro_billable_model_multipliers_json: row.get(39),
        kiro_cache_policy_json: row.get(40),
        kiro_context_usage_min_request_tokens: row.get(41),
        kiro_prefix_cache_mode: row.get(42),
        kiro_prefix_cache_max_tokens: row.get(43),
        kiro_prefix_cache_entry_ttl_seconds: row.get(44),
        kiro_conversation_anchor_max_entries: row.get(45),
        kiro_conversation_anchor_ttl_seconds: row.get(46),
        updated_at_ms: row.get(47),
    })
}
fn decode_key_bundle(row: &PgRow) -> anyhow::Result<KeyBundle> {
    let key_id: String = row.get(0);
    let credit_total_raw: String = row.get(30);
    let credit_total = credit_total_raw
        .parse::<f64>()
        .with_context(|| format!("parse key rollup credit_total `{credit_total_raw}`"))?;
    Ok(KeyBundle {
        key: KeyRecord {
            key_id: key_id.clone(),
            name: row.get(1),
            secret: row.get(2),
            key_hash: row.get(3),
            status: row.get(4),
            provider_type: row.get(5),
            protocol_family: row.get(6),
            public_visible: row.get(7),
            quota_billable_limit: row.get(8),
            created_at_ms: row.get(9),
            updated_at_ms: row.get(10),
        },
        route: KeyRouteConfig {
            key_id: key_id.clone(),
            route_strategy: row.get(11),
            fixed_account_name: row.get(12),
            auto_account_names_json: row.get(13),
            account_group_id: row.get(14),
            model_name_map_json: row.get(15),
            request_max_concurrency: row.get(16),
            request_min_start_interval_ms: row.get(17),
            codex_fast_enabled: row.get::<_, Option<bool>>(18).unwrap_or(true),
            kiro_request_validation_enabled: row.get::<_, Option<bool>>(19).unwrap_or(false),
            kiro_cache_estimation_enabled: row.get::<_, Option<bool>>(20).unwrap_or(false),
            kiro_zero_cache_debug_enabled: row.get::<_, Option<bool>>(21).unwrap_or(false),
            kiro_full_request_logging_enabled: row.get::<_, Option<bool>>(22).unwrap_or(false),
            kiro_remote_media_resolution_enabled: row.get::<_, Option<bool>>(23).unwrap_or(false),
            kiro_latency_routing_enabled: row
                .get_optional_bool("kiro_latency_routing_enabled")
                .unwrap_or(true),
            kiro_cache_policy_override_json: row.get(24),
            kiro_billable_model_multipliers_override_json: row.get(25),
        },
        rollup: KeyUsageRollup {
            key_id,
            input_uncached_tokens: row.get(26),
            input_cached_tokens: row.get(27),
            output_tokens: row.get(28),
            billable_tokens: row.get(29),
            credit_total,
            credit_missing_events: row.get(31),
            last_used_at_ms: row.get(32),
            updated_at_ms: row.get(33),
        },
    })
}
fn decode_key_bundle_row(row: PgRow) -> anyhow::Result<KeyBundle> {
    decode_key_bundle(&row)
}
fn admin_key_from_bundle(bundle: &KeyBundle) -> AdminKey {
    let quota = bundle.key.quota_billable_limit.max(0) as u64;
    let billable = bundle.rollup.billable_tokens.max(0) as u64;
    AdminKey {
        id: bundle.key.key_id.clone(),
        name: bundle.key.name.clone(),
        secret: bundle.key.secret.clone(),
        key_hash: bundle.key.key_hash.clone(),
        status: bundle.key.status.clone(),
        provider_type: bundle.key.provider_type.clone(),
        public_visible: bundle.key.public_visible,
        quota_billable_limit: quota,
        usage_input_uncached_tokens: bundle.rollup.input_uncached_tokens.max(0) as u64,
        usage_input_cached_tokens: bundle.rollup.input_cached_tokens.max(0) as u64,
        usage_output_tokens: bundle.rollup.output_tokens.max(0) as u64,
        usage_credit_total: bundle.rollup.credit_total,
        usage_credit_missing_events: bundle.rollup.credit_missing_events.max(0) as u64,
        remaining_billable: (quota as i64).saturating_sub(billable as i64),
        last_used_at: bundle.rollup.last_used_at_ms,
        created_at: bundle.key.created_at_ms,
        updated_at: bundle.key.updated_at_ms,
        route_strategy: bundle.route.route_strategy.clone(),
        account_group_id: bundle.route.account_group_id.clone(),
        fixed_account_name: bundle.route.fixed_account_name.clone(),
        auto_account_names: decode_optional_json(bundle.route.auto_account_names_json.as_deref()),
        model_name_map: decode_optional_json(bundle.route.model_name_map_json.as_deref()),
        request_max_concurrency: bundle
            .route
            .request_max_concurrency
            .and_then(non_negative_i64_to_u64),
        request_min_start_interval_ms: bundle
            .route
            .request_min_start_interval_ms
            .and_then(non_negative_i64_to_u64),
        codex_fast_enabled: bundle.route.codex_fast_enabled,
        kiro_request_validation_enabled: bundle.route.kiro_request_validation_enabled,
        kiro_cache_estimation_enabled: bundle.route.kiro_cache_estimation_enabled,
        kiro_zero_cache_debug_enabled: bundle.route.kiro_zero_cache_debug_enabled,
        kiro_full_request_logging_enabled: bundle.route.kiro_full_request_logging_enabled,
        kiro_remote_media_resolution_enabled: bundle.route.kiro_remote_media_resolution_enabled,
        kiro_latency_routing_enabled: bundle.route.kiro_latency_routing_enabled,
        kiro_cache_policy_override_json: bundle.route.kiro_cache_policy_override_json.clone(),
        kiro_billable_model_multipliers_override_json: bundle
            .route
            .kiro_billable_model_multipliers_override_json
            .clone(),
        effective_kiro_cache_policy_json: bundle
            .route
            .kiro_cache_policy_override_json
            .clone()
            .unwrap_or_else(core_store::default_kiro_cache_policy_json),
        uses_global_kiro_cache_policy: bundle.route.kiro_cache_policy_override_json.is_none(),
        effective_kiro_billable_model_multipliers_json: bundle
            .route
            .kiro_billable_model_multipliers_override_json
            .clone()
            .unwrap_or_else(core_store::default_kiro_billable_model_multipliers_json),
        uses_global_kiro_billable_model_multipliers: bundle
            .route
            .kiro_billable_model_multipliers_override_json
            .is_none(),
        kiro_candidate_credit_summary: None,
    }
}
fn decode_kiro_candidate_credit_summary_row(
    row: &PgRow,
    offset: usize,
) -> AdminKiroKeyCandidateCreditSummary {
    AdminKiroKeyCandidateCreditSummary {
        candidate_count: row.get::<_, i64>(offset).max(0) as usize,
        loaded_balance_count: row.get::<_, i64>(offset + 1).max(0) as usize,
        missing_balance_count: row.get::<_, i64>(offset + 2).max(0) as usize,
        total_limit: row.get(offset + 3),
        total_remaining: row.get(offset + 4),
    }
}
fn decode_kiro_admin_key_row(row: PgRow) -> anyhow::Result<AdminKey> {
    let bundle = decode_key_bundle(&row)?;
    let mut key = admin_key_from_bundle(&bundle);
    key.kiro_candidate_credit_summary = Some(decode_kiro_candidate_credit_summary_row(&row, 34));
    Ok(key)
}
fn decode_admin_account_group_row(row: PgRow) -> anyhow::Result<AdminAccountGroup> {
    let account_names_json: String = row.get(3);
    let account_names = serde_json::from_str::<Vec<String>>(&account_names_json)
        .with_context(|| format!("decode account_names_json `{account_names_json}`"))?;
    Ok(AdminAccountGroup {
        id: row.get(0),
        provider_type: row.get(1),
        name: row.get(2),
        account_names,
        created_at: row.get(4),
        updated_at: row.get(5),
    })
}
fn decode_admin_proxy_config_row(row: PgRow) -> AdminProxyConfig {
    AdminProxyConfig {
        id: row.get(0),
        name: row.get(1),
        proxy_url: row.get(2),
        proxy_username: row.get(3),
        proxy_password: row.get(4),
        status: row.get(5),
        created_at: row.get(6),
        updated_at: row.get(7),
        scope_node_id: None,
        effective_source: "core".to_string(),
        has_node_override: false,
        can_edit_slot_metadata: true,
        latest_codex_check: None,
        latest_kiro_check: None,
    }
}
fn decode_proxy_endpoint_check_row(row: PgRow) -> ProxyEndpointCheckRow {
    let status_code = row
        .get::<_, Option<i32>>(4)
        .and_then(|value| u16::try_from(value).ok());
    ProxyEndpointCheckRow {
        proxy_config_id: row.get(0),
        provider_type: row.get(1),
        check: AdminProxyEndpointCheck {
            target_url: row.get(2),
            reachable: row.get(3),
            status_code,
            latency_ms: row.get(5),
            error_message: row.get(6),
            checked_at: row.get(7),
        },
    }
}
fn decode_codex_account_row(row: PgRow) -> CodexAccountRecord {
    CodexAccountRecord {
        account_name: row.get(0),
        account_id: row.get(1),
        email: row.get(2),
        status: row.get(3),
        auth_json: row.get(4),
        settings_json: row.get(5),
        last_refresh_at_ms: row.get(6),
        last_error: row.get(7),
        created_at_ms: row.get(8),
        updated_at_ms: row.get(9),
    }
}
fn decode_kiro_account_row(row: PgRow) -> KiroAccountRecord {
    KiroAccountRecord {
        account_name: row.get(0),
        auth_method: row.get(1),
        account_id: row.get(2),
        profile_arn: row.get(3),
        user_id: row.get(4),
        status: row.get(5),
        auth_json: row.get(6),
        max_concurrency: row.get(7),
        min_start_interval_ms: row.get(8),
        proxy_config_id: row.get(9),
        last_refresh_at_ms: row.get(10),
        last_error: row.get(11),
        created_at_ms: row.get(12),
        updated_at_ms: row.get(13),
    }
}
fn decode_codex_admin_account_list_row(row: PgRow) -> CodexAdminAccountListRow {
    CodexAdminAccountListRow {
        account_name: row.get(0),
        account_id: row.get(1),
        status: row.get(2),
        map_gpt53_codex_to_spark: row.get(3),
        auth_refresh_enabled: row.get(4),
        route_weight_tier: row.get(5),
        proxy_mode: row.get(6),
        proxy_config_id: row.get(7),
        request_max_concurrency: row.get(8),
        request_min_start_interval_ms: row.get(9),
        last_refresh_at_ms: row.get(10),
        last_error: row.get(11),
        access_token: row.get(12),
        plan_type: row.get(13),
        primary_remaining_percent: row.get(14),
        secondary_remaining_percent: row.get(15),
        last_usage_checked_at_ms: row.get(16),
        last_usage_success_at_ms: row.get(17),
        usage_error_message: row.get(18),
    }
}
fn decode_kiro_admin_account_list_row(row: PgRow) -> KiroAdminAccountListRow {
    KiroAdminAccountListRow {
        account_name: row.get(0),
        auth_method: row.get(1),
        profile_arn: row.get(2),
        user_id: row.get(3),
        status: row.get(4),
        provider: row.get(5),
        email: row.get(6),
        expires_at: row.get(7),
        auth_profile_arn: row.get(8),
        has_refresh_token: row.get(9),
        disabled_json: row.get(10),
        disabled_reason: row.get(11),
        source: row.get(12),
        source_db_path: row.get(13),
        last_imported_at: row.get(14),
        subscription_title: row.get(15),
        region: row.get(16),
        auth_region: row.get(17),
        api_region: row.get(18),
        machine_id: row.get(19),
        max_concurrency: row.get(20),
        auth_max_concurrency: row.get(21),
        min_start_interval_ms: row.get(22),
        auth_min_start_interval_ms: row.get(23),
        minimum_remaining_credits_before_block: row.get(24),
        proxy_mode: row.get(25),
        proxy_config_id: row.get(26),
        auth_proxy_config_id: row.get(27),
        proxy_url: row.get(28),
        last_error: row.get(29),
    }
}
fn decode_public_usage_lookup_row(row: PgRow) -> anyhow::Result<PublicUsageLookupKey> {
    let credit_total_raw: String = row.get(10);
    let usage_credit_total = credit_total_raw
        .parse::<f64>()
        .with_context(|| format!("parse usage credit_total `{credit_total_raw}`"))?;
    Ok(PublicUsageLookupKey {
        key_id: row.get(0),
        key_name: row.get(1),
        provider_type: row.get(2),
        status: row.get(3),
        public_visible: row.get(4),
        quota_billable_limit: row.get::<_, i64>(5).max(0) as u64,
        usage_input_uncached_tokens: row.get::<_, i64>(6).max(0) as u64,
        usage_input_cached_tokens: row.get::<_, i64>(7).max(0) as u64,
        usage_output_tokens: row.get::<_, i64>(8).max(0) as u64,
        usage_billable_tokens: row.get::<_, i64>(9).max(0) as u64,
        usage_credit_total,
        usage_credit_missing_events: row.get::<_, i64>(11).max(0) as u64,
        last_used_at_ms: row.get(12),
    })
}
fn decode_admin_token_request_row(row: PgRow) -> AdminTokenRequest {
    AdminTokenRequest {
        request_id: row.get(0),
        requester_email: row.get(1),
        requested_quota_billable_limit: row.get::<_, i64>(2).max(0) as u64,
        request_reason: row.get(3),
        frontend_page_url: row.get(4),
        status: row.get(5),
        client_ip: row.get(6),
        ip_region: row.get(7),
        admin_note: row.get(8),
        failure_reason: row.get(9),
        issued_key_id: row.get(10),
        issued_key_name: row.get(11),
        created_at: row.get(12),
        updated_at: row.get(13),
        processed_at: row.get(14),
    }
}
fn decode_admin_account_contribution_request_row(row: PgRow) -> AdminAccountContributionRequest {
    AdminAccountContributionRequest {
        request_id: row.get(0),
        account_name: row.get(1),
        account_id: row.get(2),
        id_token: row.get(3),
        access_token: row.get(4),
        refresh_token: row.get(5),
        requester_email: row.get(6),
        contributor_message: row.get(7),
        github_id: row.get(8),
        frontend_page_url: row.get(9),
        status: row.get(10),
        client_ip: row.get(11),
        ip_region: row.get(12),
        admin_note: row.get(13),
        failure_reason: row.get(14),
        imported_account_name: row.get(15),
        issued_key_id: row.get(16),
        issued_key_name: row.get(17),
        created_at: row.get(18),
        updated_at: row.get(19),
        processed_at: row.get(20),
    }
}
fn decode_admin_sponsor_request_row(row: PgRow) -> AdminSponsorRequest {
    AdminSponsorRequest {
        request_id: row.get(0),
        requester_email: row.get(1),
        sponsor_message: row.get(2),
        display_name: row.get(3),
        github_id: row.get(4),
        frontend_page_url: row.get(5),
        status: row.get(6),
        client_ip: row.get(7),
        ip_region: row.get(8),
        admin_note: row.get(9),
        failure_reason: row.get(10),
        payment_email_sent_at: row.get(11),
        created_at: row.get(12),
        updated_at: row.get(13),
        processed_at: row.get(14),
    }
}
fn decode_codex_import_job_summary_row(row: PgRow) -> AdminCodexImportJobSummary {
    AdminCodexImportJobSummary {
        job_id: row.get(0),
        provider_type: row.get(1),
        source_type: row.get(2),
        validate_before_import: row.get(3),
        status: row.get(4),
        total_count: row.get::<_, i64>(5).max(0) as usize,
        completed_count: row.get::<_, i64>(6).max(0) as usize,
        succeeded_count: row.get::<_, i64>(7).max(0) as usize,
        skipped_count: row.get::<_, i64>(8).max(0) as usize,
        failed_count: row.get::<_, i64>(9).max(0) as usize,
        batch_error_message: row.get(10),
        created_at_ms: row.get(11),
        updated_at_ms: row.get(12),
        finished_at_ms: row.get(13),
    }
}
fn decode_codex_import_job_item_row(row: PgRow) -> AdminCodexImportJobItem {
    AdminCodexImportJobItem {
        item_index: row.get::<_, i64>(0).max(0) as usize,
        requested_name: row.get(1),
        requested_account_id: row.get(2),
        status: row.get(3),
        error_message: row.get(4),
        imported_account_name: row.get(5),
        final_account_id: row.get(6),
        validated_at_ms: row.get(7),
        imported_at_ms: row.get(8),
    }
}
fn decode_optional_json<T: serde::de::DeserializeOwned>(value: Option<&str>) -> Option<T> {
    value.and_then(|raw| serde_json::from_str(raw).ok())
}
fn optional_json_string(value: &serde_json::Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}
fn optional_json_string_any(value: &serde_json::Value, fields: &[&str]) -> Option<String> {
    fields
        .iter()
        .find_map(|field| optional_json_string(value, field))
}
fn optional_json_bool_any(value: &serde_json::Value, fields: &[&str]) -> Option<bool> {
    fields
        .iter()
        .find_map(|field| value.get(*field).and_then(serde_json::Value::as_bool))
}
fn optional_json_u64_any(value: &serde_json::Value, fields: &[&str]) -> Option<u64> {
    fields.iter().find_map(|field| {
        value
            .get(*field)
            .and_then(serde_json::Value::as_u64)
            .or_else(|| {
                value
                    .get(*field)
                    .and_then(serde_json::Value::as_i64)
                    .and_then(non_negative_i64_to_u64)
            })
    })
}
fn optional_json_i64_any(value: &serde_json::Value, fields: &[&str]) -> Option<i64> {
    fields
        .iter()
        .find_map(|field| value.get(*field).and_then(serde_json::Value::as_i64))
}
fn optional_json_f64_any(value: &serde_json::Value, fields: &[&str]) -> Option<f64> {
    fields
        .iter()
        .find_map(|field| value.get(*field).and_then(serde_json::Value::as_f64))
}
fn set_json_optional_string(
    object: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: Option<String>,
) {
    match value {
        Some(value) => {
            object.insert(key.to_string(), serde_json::Value::String(value));
        },
        None => {
            object.remove(key);
        },
    }
}
fn set_json_optional_bool(
    object: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: Option<bool>,
) {
    match value {
        Some(value) => {
            object.insert(key.to_string(), serde_json::Value::Bool(value));
        },
        None => {
            object.remove(key);
        },
    }
}
fn set_json_optional_u64(
    object: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: Option<u64>,
) {
    match value {
        Some(value) => {
            object.insert(key.to_string(), serde_json::Value::Number(value.into()));
        },
        None => {
            object.remove(key);
        },
    }
}
fn set_json_optional_f64(
    object: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: Option<f64>,
) -> anyhow::Result<()> {
    match value {
        Some(value) => {
            let number =
                serde_json::Number::from_f64(value).context("serialize finite JSON number")?;
            object.insert(key.to_string(), serde_json::Value::Number(number));
        },
        None => {
            object.remove(key);
        },
    }
    Ok(())
}
fn non_negative_i64_to_u64(value: i64) -> Option<u64> {
    u64::try_from(value.max(0)).ok()
}
fn cached_authenticated_key_from_value(
    key: &AuthenticatedKey,
) -> crate::request_cache::CachedAuthenticatedKey {
    crate::request_cache::CachedAuthenticatedKey {
        key_id: key.key_id.clone(),
        key_name: key.key_name.clone(),
        provider_type: key.provider_type.clone(),
        protocol_family: key.protocol_family.clone(),
        status: key.status.clone(),
        quota_billable_limit: key.quota_billable_limit,
        billable_tokens_used: key.billable_tokens_used,
    }
}
fn cached_authenticated_key_from_bundle(
    bundle: &KeyBundle,
) -> crate::request_cache::CachedAuthenticatedKey {
    cached_authenticated_key_from_value(&AuthenticatedKey {
        key_id: bundle.key.key_id.clone(),
        key_name: bundle.key.name.clone(),
        provider_type: bundle.key.provider_type.clone(),
        protocol_family: bundle.key.protocol_family.clone(),
        status: bundle.key.status.clone(),
        quota_billable_limit: bundle.key.quota_billable_limit,
        billable_tokens_used: bundle.rollup.billable_tokens,
    })
}
fn authenticated_key_from_cached(
    key: crate::request_cache::CachedAuthenticatedKey,
) -> AuthenticatedKey {
    AuthenticatedKey {
        key_id: key.key_id,
        key_name: key.key_name,
        provider_type: key.provider_type,
        protocol_family: key.protocol_family,
        status: key.status,
        quota_billable_limit: key.quota_billable_limit,
        billable_tokens_used: key.billable_tokens_used,
    }
}
fn cached_proxy_from_option(
    proxy: Option<ProviderProxyConfig>,
) -> Option<crate::request_cache::CachedProxyConfig> {
    proxy.map(Into::into)
}
fn proxy_from_cached_option(
    proxy: Option<crate::request_cache::CachedProxyConfig>,
) -> Option<ProviderProxyConfig> {
    proxy.map(Into::into)
}
fn build_cached_kiro_account_view(
    row: &KiroRouteCandidateRow,
    cached_status: Option<KiroCachedStatusParts>,
    proxy_context: &ProviderProxyResolutionContext,
    generation: i64,
) -> anyhow::Result<crate::request_cache::CachedKiroAccountView> {
    let cached_balance = cached_status
        .as_ref()
        .and_then(|(balance, _)| balance.as_ref());
    let routing_identity = cached_balance
        .and_then(|balance| balance.user_id.clone())
        .or_else(|| row.user_id.clone())
        .unwrap_or_else(|| row.account_name.clone());
    let proxy_mode = row.proxy_mode.clone().unwrap_or_else(|| {
        if row.proxy_config_id.is_some() {
            "fixed".to_string()
        } else {
            "inherit".to_string()
        }
    });
    let proxy_config_id = row
        .proxy_config_id
        .clone()
        .or_else(|| row.auth_proxy_config_id.clone());
    let proxy = resolve_provider_proxy_config_from_context(
        &proxy_mode,
        proxy_config_id.as_deref(),
        proxy_context,
    )?;
    Ok(crate::request_cache::CachedKiroAccountView {
        account_name: row.account_name.clone(),
        generation,
        profile_arn: row.profile_arn.clone().or(row.auth_profile_arn.clone()),
        user_id: row.user_id.clone(),
        status: row.status.clone(),
        request_max_concurrency: row.max_concurrency.and_then(non_negative_i64_to_u64),
        request_min_start_interval_ms: row.min_start_interval_ms.and_then(non_negative_i64_to_u64),
        disabled: row.disabled,
        minimum_remaining_credits_before_block: row.minimum_remaining_credits_before_block,
        api_region: row
            .api_region
            .clone()
            .unwrap_or_else(|| "us-east-1".to_string()),
        proxy: cached_proxy_from_option(proxy),
        routing_identity,
        cached_balance: cached_status
            .as_ref()
            .and_then(|(balance, _)| balance.clone()),
        cached_cache: cached_status.as_ref().map(|(_, cache)| cache.clone()),
    })
}
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CodexRouteQuotaScore {
    rank: u8,
    remaining: f64,
    last_success_at: i64,
}
fn sort_codex_routes_by_cached_quota(
    routes: &mut [ProviderCodexRoute],
    status: Option<&CodexRateLimitStatus>,
    runtime_config: &RuntimeConfigRecord,
    route_weight_tiers: &BTreeMap<String, Option<String>>,
) {
    let status_by_account = status
        .map(|status| {
            status
                .accounts
                .iter()
                .map(|account| (account.name.as_str(), account))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    routes.sort_by(|left, right| {
        let left_score = codex_route_quota_score(
            &left.account_name,
            &status_by_account,
            runtime_config,
            route_weight_tiers
                .get(&left.account_name)
                .and_then(|value| value.as_deref()),
        );
        let right_score = codex_route_quota_score(
            &right.account_name,
            &status_by_account,
            runtime_config,
            route_weight_tiers
                .get(&right.account_name)
                .and_then(|value| value.as_deref()),
        );
        right_score
            .rank
            .cmp(&left_score.rank)
            .then_with(|| right_score.remaining.total_cmp(&left_score.remaining))
            .then_with(|| right_score.last_success_at.cmp(&left_score.last_success_at))
            .then_with(|| left.account_name.cmp(&right.account_name))
    });
}
fn codex_route_quota_score(
    account_name: &str,
    status_by_account: &BTreeMap<&str, &CodexPublicAccountStatus>,
    runtime_config: &RuntimeConfigRecord,
    route_weight_tier: Option<&str>,
) -> CodexRouteQuotaScore {
    let Some(status) = status_by_account.get(account_name) else {
        return CodexRouteQuotaScore {
            rank: 2,
            remaining: -1.0,
            last_success_at: 0,
        };
    };
    if status.status != core_store::KEY_STATUS_ACTIVE || status.usage_error_message.is_some() {
        return CodexRouteQuotaScore {
            rank: 0,
            remaining: -1.0,
            last_success_at: status.last_usage_success_at.unwrap_or(0),
        };
    }
    let Some(remaining) = codex_remaining_bottleneck(status) else {
        return CodexRouteQuotaScore {
            rank: 2,
            remaining: -1.0,
            last_success_at: status.last_usage_success_at.unwrap_or(0),
        };
    };
    CodexRouteQuotaScore {
        rank: if remaining > 0.0 { 3 } else { 1 },
        remaining: remaining
            * codex_route_weight_multiplier(
                status.plan_type.as_deref(),
                route_weight_tier,
                runtime_config,
            ),
        last_success_at: status.last_usage_success_at.unwrap_or(0),
    }
}
fn codex_route_weight_multiplier(
    plan_type: Option<&str>,
    route_weight_tier: Option<&str>,
    runtime_config: &RuntimeConfigRecord,
) -> f64 {
    match codex_effective_route_weight_tier(plan_type, route_weight_tier) {
        "free" => runtime_config.codex_weight_free.max(0) as f64,
        "plus" => runtime_config.codex_weight_plus.max(0) as f64,
        "pro20x" => runtime_config.codex_weight_pro20x.max(0) as f64,
        _ => runtime_config.codex_weight_pro5x.max(0) as f64,
    }
}
fn codex_effective_route_weight_tier(
    plan_type: Option<&str>,
    route_weight_tier: Option<&str>,
) -> &'static str {
    match route_weight_tier
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        Some("free") => "free",
        Some("plus") => "plus",
        Some("pro5x") => "pro5x",
        Some("pro20x") => "pro20x",
        Some("auto") | None | Some(_) => match plan_type
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase())
            .as_deref()
        {
            Some("free") => "free",
            Some("plus") => "plus",
            Some("pro20x") => "pro20x",
            Some("pro") | Some("pro5x") => "pro5x",
            _ => "free",
        },
    }
}
fn codex_remaining_bottleneck(status: &CodexPublicAccountStatus) -> Option<f64> {
    [status.primary_remaining_percent, status.secondary_remaining_percent]
        .into_iter()
        .flatten()
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(0.0, 100.0))
        .reduce(f64::min)
}
fn codex_cached_error_message(
    account_name: &str,
    record_last_error: Option<&str>,
    record_last_refresh_at_ms: Option<i64>,
    auth_refresh_enabled: bool,
    auth_json: &str,
    status_by_account: &BTreeMap<String, CodexPublicAccountStatus>,
) -> Option<String> {
    let local_auth_error =
        codex_local_auth_error_message(record_last_error, auth_refresh_enabled, auth_json);
    match status_by_account.get(account_name) {
        Some(status) => {
            if status.usage_error_message.is_some() {
                return status.usage_error_message.clone();
            }
            let local_refresh = record_last_refresh_at_ms.unwrap_or(0);
            let status_checked_at = status.last_usage_checked_at.unwrap_or(0);
            if local_refresh > status_checked_at {
                local_auth_error
            } else {
                codex_disabled_expired_auth_error(auth_refresh_enabled, auth_json)
            }
        },
        None => local_auth_error,
    }
}
fn codex_local_auth_error_message(
    record_last_error: Option<&str>,
    auth_refresh_enabled: bool,
    auth_json: &str,
) -> Option<String> {
    if auth_refresh_enabled {
        return record_last_error.map(str::to_string);
    }
    if codex_access_token_is_still_usable(auth_json) {
        return None;
    }
    record_last_error
        .map(str::to_string)
        .or_else(|| codex_disabled_expired_auth_error(auth_refresh_enabled, auth_json))
}
fn codex_disabled_expired_auth_error(
    auth_refresh_enabled: bool,
    auth_json: &str,
) -> Option<String> {
    if auth_refresh_enabled || codex_access_token_is_still_usable(auth_json) {
        return None;
    }
    Some("codex auth refresh disabled and current access token expired".to_string())
}
fn codex_access_token_is_still_usable(auth_json: &str) -> bool {
    let Some(expires_at_ms) = core_store::codex_auth_access_token_expires_at_ms(auth_json) else {
        return true;
    };
    expires_at_ms > now_ms()
}
fn minimal_codex_auth_json_for_access_token(access_token: Option<&str>) -> String {
    match access_token {
        Some(token) if !token.trim().is_empty() => {
            serde_json::json!({ "access_token": token }).to_string()
        },
        _ => "{}".to_string(),
    }
}
fn effective_kiro_cache_policy_json(
    runtime_policy_json: &str,
    override_json: Option<&str>,
) -> anyhow::Result<String> {
    let runtime_policy = serde_json::from_str::<KiroCachePolicy>(runtime_policy_json)
        .context("parse runtime kiro cache policy")?;
    let effective_policy = resolve_effective_kiro_cache_policy(&runtime_policy, override_json)
        .context("resolve effective kiro cache policy")?;
    serde_json::to_string(&effective_policy).context("serialize effective kiro cache policy")
}
fn provider_proxy_from_admin_proxy(proxy: AdminProxyConfig) -> ProviderProxyConfig {
    ProviderProxyConfig {
        proxy_url: proxy.proxy_url,
        proxy_username: proxy.proxy_username,
        proxy_password: proxy.proxy_password,
    }
}
fn apply_proxy_config_node_override(
    proxy: &mut AdminProxyConfig,
    override_row: &ProxyConfigNodeOverride,
) {
    proxy.proxy_url = override_row.proxy_url.clone();
    proxy.proxy_username = override_row.proxy_username.clone();
    proxy.proxy_password = override_row.proxy_password.clone();
    proxy.status = override_row.status.clone();
    proxy.updated_at = override_row.updated_at_ms;
}
fn apply_proxy_endpoint_checks(proxy: &mut AdminProxyConfig, rows: &[ProxyEndpointCheckRow]) {
    proxy.latest_codex_check = None;
    proxy.latest_kiro_check = None;
    for row in rows {
        match row.provider_type.as_str() {
            core_store::PROVIDER_CODEX => proxy.latest_codex_check = Some(row.check.clone()),
            core_store::PROVIDER_KIRO => proxy.latest_kiro_check = Some(row.check.clone()),
            _ => {},
        }
    }
}
fn legacy_proxy_json_string(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| value.get(*key))
        .find_map(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}
fn clear_legacy_kiro_proxy_json(auth_json: &str, proxy_config_id: &str) -> anyhow::Result<String> {
    let mut value = serde_json::from_str::<serde_json::Value>(auth_json)
        .context("parse postgres kiro auth json for legacy proxy cleanup")?;
    if let Some(object) = value.as_object_mut() {
        for key in [
            "proxyUrl",
            "proxy_url",
            "proxyUsername",
            "proxy_username",
            "proxyPassword",
            "proxy_password",
        ] {
            object.remove(key);
        }
        object.insert("proxyMode".to_string(), serde_json::Value::String("fixed".to_string()));
        object.insert(
            "proxyConfigId".to_string(),
            serde_json::Value::String(proxy_config_id.to_string()),
        );
    }
    serde_json::to_string(&value).context("serialize postgres kiro auth json after proxy cleanup")
}
fn resolve_provider_proxy_config_from_context(
    proxy_mode: &str,
    proxy_config_id: Option<&str>,
    context: &ProviderProxyResolutionContext,
) -> anyhow::Result<Option<ProviderProxyConfig>> {
    match proxy_mode {
        "none" | "direct" => Ok(None),
        "fixed" => {
            let Some(proxy_id) = proxy_config_id else {
                anyhow::bail!("fixed proxy mode requires proxy_config_id");
            };
            let Some(proxy) = context.proxy_configs_by_id.get(proxy_id).cloned() else {
                anyhow::bail!("fixed proxy config `{proxy_id}` is missing");
            };
            if proxy.status != core_store::KEY_STATUS_ACTIVE {
                anyhow::bail!("fixed proxy config `{}` is disabled", proxy.name);
            }
            Ok(Some(provider_proxy_from_admin_proxy(proxy)))
        },
        _ => {
            if let Some(message) = context.binding.error_message.clone() {
                anyhow::bail!("provider proxy binding is invalid: {message}");
            }
            match context.binding.effective_proxy_url.clone() {
                Some(proxy_url) => Ok(Some(ProviderProxyConfig {
                    proxy_url,
                    proxy_username: context.binding.effective_proxy_username.clone(),
                    proxy_password: context.binding.effective_proxy_password.clone(),
                })),
                None => Ok(None),
            }
        },
    }
}
fn decode_codex_account_settings(value: &str) -> anyhow::Result<CodexAccountSettings> {
    serde_json::from_str(value).context("decode codex account settings")
}

#[cfg(test)]
mod tests;
