//! SQLite control-plane repository for `llm-access`.

use anyhow::Context;
use llm_access_core::store::{CodexRateLimitStatus, PublicAccessKey};
use rusqlite::{params, types::Type, Connection, OptionalExtension};

/// SQLite-backed control-plane store.
pub struct SqliteControlStore {
    conn: Connection,
}

/// Complete key state loaded from the control-plane store.
pub struct KeyBundle {
    /// API key row.
    pub key: KeyRecord,
    /// Route configuration row.
    pub route: KeyRouteConfig,
    /// Accumulated usage rollup row.
    pub rollup: KeyUsageRollup,
}

/// API key current-state row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRecord {
    /// Stable key id.
    pub key_id: String,
    /// Human-readable key name.
    pub name: String,
    /// Plaintext secret retained for source-compatible admin behavior.
    pub secret: String,
    /// SHA-256 hash of the bearer secret.
    pub key_hash: String,
    /// Key status.
    pub status: String,
    /// Provider type.
    pub provider_type: String,
    /// Client protocol family.
    pub protocol_family: String,
    /// Whether this key is public-visible.
    pub public_visible: bool,
    /// Billable quota limit.
    pub quota_billable_limit: i64,
    /// Creation timestamp in Unix milliseconds.
    pub created_at_ms: i64,
    /// Update timestamp in Unix milliseconds.
    pub updated_at_ms: i64,
}

/// API key route configuration row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRouteConfig {
    /// Owning key id.
    pub key_id: String,
    /// Account route strategy.
    pub route_strategy: Option<String>,
    /// Fixed account name for fixed routing.
    pub fixed_account_name: Option<String>,
    /// JSON array of account names for auto routing.
    pub auto_account_names_json: Option<String>,
    /// Account group id selected by the key.
    pub account_group_id: Option<String>,
    /// JSON object mapping public model names to upstream model names.
    pub model_name_map_json: Option<String>,
    /// Optional per-key concurrency cap.
    pub request_max_concurrency: Option<i64>,
    /// Optional per-key pacing interval.
    pub request_min_start_interval_ms: Option<i64>,
    /// Whether Kiro public request validation is enabled.
    pub kiro_request_validation_enabled: bool,
    /// Whether Kiro cache estimation is enabled.
    pub kiro_cache_estimation_enabled: bool,
    /// Whether zero-cache diagnostic capture is enabled.
    pub kiro_zero_cache_debug_enabled: bool,
    /// Optional Kiro cache policy override JSON.
    pub kiro_cache_policy_override_json: Option<String>,
    /// Optional Kiro billable multiplier override JSON.
    pub kiro_billable_model_multipliers_override_json: Option<String>,
}

/// API key accumulated usage rollup row.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyUsageRollup {
    /// Owning key id.
    pub key_id: String,
    /// Accumulated uncached input tokens.
    pub input_uncached_tokens: i64,
    /// Accumulated cached input tokens.
    pub input_cached_tokens: i64,
    /// Accumulated output tokens.
    pub output_tokens: i64,
    /// Accumulated billable tokens.
    pub billable_tokens: i64,
    /// Accumulated credit usage.
    pub credit_total: f64,
    /// Number of events missing credit usage.
    pub credit_missing_events: i64,
    /// Last usage timestamp.
    pub last_used_at_ms: Option<i64>,
    /// Update timestamp in Unix milliseconds.
    pub updated_at_ms: i64,
}

/// Runtime configuration row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfigRecord {
    /// Singleton id.
    pub id: String,
    /// Auth cache TTL in seconds.
    pub auth_cache_ttl_seconds: i64,
    /// Maximum request body size.
    pub max_request_body_bytes: i64,
    /// Account failure retry limit.
    pub account_failure_retry_limit: i64,
    /// Codex client version.
    pub codex_client_version: String,
    /// Default Kiro per-account concurrency.
    pub kiro_channel_max_concurrency: i64,
    /// Default Kiro per-account pacing interval.
    pub kiro_channel_min_start_interval_ms: i64,
    /// Codex minimum status refresh interval.
    pub codex_status_refresh_min_interval_seconds: i64,
    /// Codex maximum status refresh interval.
    pub codex_status_refresh_max_interval_seconds: i64,
    /// Codex per-account refresh jitter.
    pub codex_status_account_jitter_max_seconds: i64,
    /// Kiro minimum status refresh interval.
    pub kiro_status_refresh_min_interval_seconds: i64,
    /// Kiro maximum status refresh interval.
    pub kiro_status_refresh_max_interval_seconds: i64,
    /// Kiro per-account refresh jitter.
    pub kiro_status_account_jitter_max_seconds: i64,
    /// Usage event flush batch size.
    pub usage_event_flush_batch_size: i64,
    /// Usage event flush interval.
    pub usage_event_flush_interval_seconds: i64,
    /// Usage event flush max buffer bytes.
    pub usage_event_flush_max_buffer_bytes: i64,
    /// Whether usage maintenance is enabled.
    pub usage_event_maintenance_enabled: bool,
    /// Usage maintenance interval.
    pub usage_event_maintenance_interval_seconds: i64,
    /// Heavy usage detail retention in days.
    pub usage_event_detail_retention_days: i64,
    /// Kiro cache k-models JSON.
    pub kiro_cache_kmodels_json: String,
    /// Kiro billable model multipliers JSON.
    pub kiro_billable_model_multipliers_json: String,
    /// Kiro cache policy JSON.
    pub kiro_cache_policy_json: String,
    /// Kiro prefix cache mode.
    pub kiro_prefix_cache_mode: String,
    /// Kiro prefix cache max tokens.
    pub kiro_prefix_cache_max_tokens: i64,
    /// Kiro prefix cache entry TTL.
    pub kiro_prefix_cache_entry_ttl_seconds: i64,
    /// Kiro conversation anchor max entries.
    pub kiro_conversation_anchor_max_entries: i64,
    /// Kiro conversation anchor TTL.
    pub kiro_conversation_anchor_ttl_seconds: i64,
    /// Update timestamp in Unix milliseconds.
    pub updated_at_ms: i64,
}

/// Codex account control-plane row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexAccountRecord {
    /// Account display name.
    pub account_name: String,
    /// Upstream account id when known.
    pub account_id: Option<String>,
    /// Account email when known.
    pub email: Option<String>,
    /// Runtime status.
    pub status: String,
    /// Persisted auth payload JSON.
    pub auth_json: String,
    /// Persisted settings JSON.
    pub settings_json: String,
    /// Last refresh timestamp.
    pub last_refresh_at_ms: Option<i64>,
    /// Last refresh or runtime error.
    pub last_error: Option<String>,
    /// Creation timestamp.
    pub created_at_ms: i64,
    /// Update timestamp.
    pub updated_at_ms: i64,
}

/// Kiro account control-plane row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KiroAccountRecord {
    /// Account display name.
    pub account_name: String,
    /// Kiro auth method.
    pub auth_method: String,
    /// Upstream account id when known.
    pub account_id: Option<String>,
    /// Kiro profile ARN when known.
    pub profile_arn: Option<String>,
    /// Upstream user id from usage limits when known.
    pub user_id: Option<String>,
    /// Runtime status.
    pub status: String,
    /// Persisted auth payload JSON.
    pub auth_json: String,
    /// Per-account concurrency cap.
    pub max_concurrency: Option<i64>,
    /// Per-account pacing interval.
    pub min_start_interval_ms: Option<i64>,
    /// Optional proxy config id.
    pub proxy_config_id: Option<String>,
    /// Last refresh timestamp.
    pub last_refresh_at_ms: Option<i64>,
    /// Last refresh or runtime error.
    pub last_error: Option<String>,
    /// Creation timestamp.
    pub created_at_ms: i64,
    /// Update timestamp.
    pub updated_at_ms: i64,
}

impl SqliteControlStore {
    /// Create a store from an initialized SQLite connection.
    pub fn new(conn: Connection) -> Self {
        Self {
            conn,
        }
    }

    /// Insert or update the key, route, and usage rollup rows atomically.
    pub fn upsert_key_bundle(
        &self,
        key: &KeyRecord,
        route: &KeyRouteConfig,
        rollup: &KeyUsageRollup,
    ) -> anyhow::Result<()> {
        let tx = self
            .conn
            .unchecked_transaction()
            .context("begin key bundle transaction")?;
        tx.execute(
            "INSERT INTO llm_keys (
                key_id, name, secret, key_hash, status, provider_type, protocol_family,
                public_visible, quota_billable_limit, created_at_ms, updated_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            ON CONFLICT(key_id) DO UPDATE SET
                name = excluded.name,
                secret = excluded.secret,
                key_hash = excluded.key_hash,
                status = excluded.status,
                provider_type = excluded.provider_type,
                protocol_family = excluded.protocol_family,
                public_visible = excluded.public_visible,
                quota_billable_limit = excluded.quota_billable_limit,
                created_at_ms = excluded.created_at_ms,
                updated_at_ms = excluded.updated_at_ms",
            params![
                &key.key_id,
                &key.name,
                &key.secret,
                &key.key_hash,
                &key.status,
                &key.provider_type,
                &key.protocol_family,
                key.public_visible as i64,
                key.quota_billable_limit,
                key.created_at_ms,
                key.updated_at_ms,
            ],
        )
        .context("upsert llm key")?;
        tx.execute(
            "INSERT INTO llm_key_route_config (
                key_id, route_strategy, fixed_account_name, auto_account_names_json,
                account_group_id, model_name_map_json, request_max_concurrency,
                request_min_start_interval_ms, kiro_request_validation_enabled,
                kiro_cache_estimation_enabled, kiro_zero_cache_debug_enabled,
                kiro_cache_policy_override_json,
                kiro_billable_model_multipliers_override_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
            ON CONFLICT(key_id) DO UPDATE SET
                route_strategy = excluded.route_strategy,
                fixed_account_name = excluded.fixed_account_name,
                auto_account_names_json = excluded.auto_account_names_json,
                account_group_id = excluded.account_group_id,
                model_name_map_json = excluded.model_name_map_json,
                request_max_concurrency = excluded.request_max_concurrency,
                request_min_start_interval_ms = excluded.request_min_start_interval_ms,
                kiro_request_validation_enabled = excluded.kiro_request_validation_enabled,
                kiro_cache_estimation_enabled = excluded.kiro_cache_estimation_enabled,
                kiro_zero_cache_debug_enabled = excluded.kiro_zero_cache_debug_enabled,
                kiro_cache_policy_override_json = excluded.kiro_cache_policy_override_json,
                kiro_billable_model_multipliers_override_json =
                    excluded.kiro_billable_model_multipliers_override_json",
            params![
                &route.key_id,
                &route.route_strategy,
                &route.fixed_account_name,
                &route.auto_account_names_json,
                &route.account_group_id,
                &route.model_name_map_json,
                route.request_max_concurrency,
                route.request_min_start_interval_ms,
                route.kiro_request_validation_enabled as i64,
                route.kiro_cache_estimation_enabled as i64,
                route.kiro_zero_cache_debug_enabled as i64,
                &route.kiro_cache_policy_override_json,
                &route.kiro_billable_model_multipliers_override_json,
            ],
        )
        .context("upsert key route config")?;
        tx.execute(
            "INSERT INTO llm_key_usage_rollups (
                key_id, input_uncached_tokens, input_cached_tokens, output_tokens,
                billable_tokens, credit_total, credit_missing_events, last_used_at_ms,
                updated_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(key_id) DO UPDATE SET
                input_uncached_tokens = excluded.input_uncached_tokens,
                input_cached_tokens = excluded.input_cached_tokens,
                output_tokens = excluded.output_tokens,
                billable_tokens = excluded.billable_tokens,
                credit_total = excluded.credit_total,
                credit_missing_events = excluded.credit_missing_events,
                last_used_at_ms = excluded.last_used_at_ms,
                updated_at_ms = excluded.updated_at_ms",
            params![
                &rollup.key_id,
                rollup.input_uncached_tokens,
                rollup.input_cached_tokens,
                rollup.output_tokens,
                rollup.billable_tokens,
                rollup.credit_total.to_string(),
                rollup.credit_missing_events,
                rollup.last_used_at_ms,
                rollup.updated_at_ms,
            ],
        )
        .context("upsert key usage rollup")?;
        tx.commit().context("commit key bundle transaction")?;
        Ok(())
    }

    /// Load one key bundle by key id.
    pub fn get_key(&self, key_id: &str) -> anyhow::Result<Option<KeyBundle>> {
        self.conn
            .query_row(
                "SELECT
                    k.key_id, k.name, k.secret, k.key_hash, k.status, k.provider_type,
                    k.protocol_family, k.public_visible, k.quota_billable_limit,
                    k.created_at_ms, k.updated_at_ms,
                    r.route_strategy, r.fixed_account_name, r.auto_account_names_json,
                    r.account_group_id, r.model_name_map_json,
                    r.request_max_concurrency, r.request_min_start_interval_ms,
                    r.kiro_request_validation_enabled, r.kiro_cache_estimation_enabled,
                    r.kiro_zero_cache_debug_enabled, r.kiro_cache_policy_override_json,
                    r.kiro_billable_model_multipliers_override_json,
                    u.input_uncached_tokens, u.input_cached_tokens, u.output_tokens,
                    u.billable_tokens, u.credit_total, u.credit_missing_events,
                    u.last_used_at_ms, u.updated_at_ms
                 FROM llm_keys k
                 LEFT JOIN llm_key_route_config r ON r.key_id = k.key_id
                 LEFT JOIN llm_key_usage_rollups u ON u.key_id = k.key_id
                 WHERE k.key_id = ?1",
                [key_id],
                decode_key_bundle,
            )
            .optional()
            .context("load key bundle")
    }

    /// Load one key bundle by bearer secret hash.
    pub fn get_key_by_hash(&self, key_hash: &str) -> anyhow::Result<Option<KeyBundle>> {
        self.conn
            .query_row(
                "SELECT
                    k.key_id, k.name, k.secret, k.key_hash, k.status, k.provider_type,
                    k.protocol_family, k.public_visible, k.quota_billable_limit,
                    k.created_at_ms, k.updated_at_ms,
                    r.route_strategy, r.fixed_account_name, r.auto_account_names_json,
                    r.account_group_id, r.model_name_map_json,
                    r.request_max_concurrency, r.request_min_start_interval_ms,
                    r.kiro_request_validation_enabled, r.kiro_cache_estimation_enabled,
                    r.kiro_zero_cache_debug_enabled, r.kiro_cache_policy_override_json,
                    r.kiro_billable_model_multipliers_override_json,
                    u.input_uncached_tokens, u.input_cached_tokens, u.output_tokens,
                    u.billable_tokens, u.credit_total, u.credit_missing_events,
                    u.last_used_at_ms, u.updated_at_ms
                 FROM llm_keys k
                 LEFT JOIN llm_key_route_config r ON r.key_id = k.key_id
                 LEFT JOIN llm_key_usage_rollups u ON u.key_id = k.key_id
                 WHERE k.key_hash = ?1",
                [key_hash],
                decode_key_bundle,
            )
            .optional()
            .context("load key bundle by hash")
    }

    /// Add one accepted usage event to the hot-path key rollup counters.
    pub fn increment_key_usage_rollup(
        &self,
        event: &llm_access_core::usage::UsageEvent,
    ) -> anyhow::Result<()> {
        let credit_delta = event
            .credit_usage
            .as_deref()
            .unwrap_or("0")
            .parse::<f64>()
            .context("parse usage event credit usage")?;
        let changed = self
            .conn
            .execute(
                "UPDATE llm_key_usage_rollups
                 SET input_uncached_tokens = input_uncached_tokens + ?2,
                     input_cached_tokens = input_cached_tokens + ?3,
                     output_tokens = output_tokens + ?4,
                     billable_tokens = billable_tokens + ?5,
                     credit_total = CAST((CAST(credit_total AS REAL) + ?6) AS TEXT),
                     credit_missing_events = credit_missing_events + ?7,
                     last_used_at_ms = ?8,
                     updated_at_ms = ?8
                 WHERE key_id = ?1",
                params![
                    &event.key_id,
                    event.input_uncached_tokens,
                    event.input_cached_tokens,
                    event.output_tokens,
                    event.billable_tokens,
                    credit_delta,
                    event.credit_usage_missing as i64,
                    event.created_at_ms,
                ],
            )
            .context("increment key usage rollup")?;
        if changed == 0 {
            anyhow::bail!("usage rollup not found for key `{}`", event.key_id);
        }
        Ok(())
    }

    /// Insert or update the singleton runtime config row.
    pub fn upsert_runtime_config(&self, record: &RuntimeConfigRecord) -> anyhow::Result<()> {
        self.conn
            .execute(
                "INSERT INTO llm_runtime_config (
                    id, auth_cache_ttl_seconds, max_request_body_bytes,
                    account_failure_retry_limit, codex_client_version,
                    kiro_channel_max_concurrency, kiro_channel_min_start_interval_ms,
                    codex_status_refresh_min_interval_seconds,
                    codex_status_refresh_max_interval_seconds,
                    codex_status_account_jitter_max_seconds,
                    kiro_status_refresh_min_interval_seconds,
                    kiro_status_refresh_max_interval_seconds,
                    kiro_status_account_jitter_max_seconds,
                    usage_event_flush_batch_size,
                    usage_event_flush_interval_seconds,
                    usage_event_flush_max_buffer_bytes,
                    usage_event_maintenance_enabled,
                    usage_event_maintenance_interval_seconds,
                    usage_event_detail_retention_days,
                    kiro_cache_kmodels_json,
                    kiro_billable_model_multipliers_json,
                    kiro_cache_policy_json,
                    kiro_prefix_cache_mode,
                    kiro_prefix_cache_max_tokens,
                    kiro_prefix_cache_entry_ttl_seconds,
                    kiro_conversation_anchor_max_entries,
                    kiro_conversation_anchor_ttl_seconds,
                    updated_at_ms
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                    ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24,
                    ?25, ?26, ?27, ?28
                )
                ON CONFLICT(id) DO UPDATE SET
                    auth_cache_ttl_seconds = excluded.auth_cache_ttl_seconds,
                    max_request_body_bytes = excluded.max_request_body_bytes,
                    account_failure_retry_limit = excluded.account_failure_retry_limit,
                    codex_client_version = excluded.codex_client_version,
                    kiro_channel_max_concurrency = excluded.kiro_channel_max_concurrency,
                    kiro_channel_min_start_interval_ms =
                        excluded.kiro_channel_min_start_interval_ms,
                    codex_status_refresh_min_interval_seconds =
                        excluded.codex_status_refresh_min_interval_seconds,
                    codex_status_refresh_max_interval_seconds =
                        excluded.codex_status_refresh_max_interval_seconds,
                    codex_status_account_jitter_max_seconds =
                        excluded.codex_status_account_jitter_max_seconds,
                    kiro_status_refresh_min_interval_seconds =
                        excluded.kiro_status_refresh_min_interval_seconds,
                    kiro_status_refresh_max_interval_seconds =
                        excluded.kiro_status_refresh_max_interval_seconds,
                    kiro_status_account_jitter_max_seconds =
                        excluded.kiro_status_account_jitter_max_seconds,
                    usage_event_flush_batch_size = excluded.usage_event_flush_batch_size,
                    usage_event_flush_interval_seconds =
                        excluded.usage_event_flush_interval_seconds,
                    usage_event_flush_max_buffer_bytes =
                        excluded.usage_event_flush_max_buffer_bytes,
                    usage_event_maintenance_enabled =
                        excluded.usage_event_maintenance_enabled,
                    usage_event_maintenance_interval_seconds =
                        excluded.usage_event_maintenance_interval_seconds,
                    usage_event_detail_retention_days =
                        excluded.usage_event_detail_retention_days,
                    kiro_cache_kmodels_json = excluded.kiro_cache_kmodels_json,
                    kiro_billable_model_multipliers_json =
                        excluded.kiro_billable_model_multipliers_json,
                    kiro_cache_policy_json = excluded.kiro_cache_policy_json,
                    kiro_prefix_cache_mode = excluded.kiro_prefix_cache_mode,
                    kiro_prefix_cache_max_tokens = excluded.kiro_prefix_cache_max_tokens,
                    kiro_prefix_cache_entry_ttl_seconds =
                        excluded.kiro_prefix_cache_entry_ttl_seconds,
                    kiro_conversation_anchor_max_entries =
                        excluded.kiro_conversation_anchor_max_entries,
                    kiro_conversation_anchor_ttl_seconds =
                        excluded.kiro_conversation_anchor_ttl_seconds,
                    updated_at_ms = excluded.updated_at_ms",
                params![
                    &record.id,
                    record.auth_cache_ttl_seconds,
                    record.max_request_body_bytes,
                    record.account_failure_retry_limit,
                    &record.codex_client_version,
                    record.kiro_channel_max_concurrency,
                    record.kiro_channel_min_start_interval_ms,
                    record.codex_status_refresh_min_interval_seconds,
                    record.codex_status_refresh_max_interval_seconds,
                    record.codex_status_account_jitter_max_seconds,
                    record.kiro_status_refresh_min_interval_seconds,
                    record.kiro_status_refresh_max_interval_seconds,
                    record.kiro_status_account_jitter_max_seconds,
                    record.usage_event_flush_batch_size,
                    record.usage_event_flush_interval_seconds,
                    record.usage_event_flush_max_buffer_bytes,
                    record.usage_event_maintenance_enabled as i64,
                    record.usage_event_maintenance_interval_seconds,
                    record.usage_event_detail_retention_days,
                    &record.kiro_cache_kmodels_json,
                    &record.kiro_billable_model_multipliers_json,
                    &record.kiro_cache_policy_json,
                    &record.kiro_prefix_cache_mode,
                    record.kiro_prefix_cache_max_tokens,
                    record.kiro_prefix_cache_entry_ttl_seconds,
                    record.kiro_conversation_anchor_max_entries,
                    record.kiro_conversation_anchor_ttl_seconds,
                    record.updated_at_ms,
                ],
            )
            .context("upsert runtime config")?;
        Ok(())
    }

    /// Load the singleton runtime config row.
    pub fn get_runtime_config(&self) -> anyhow::Result<Option<RuntimeConfigRecord>> {
        self.conn
            .query_row(
                "SELECT
                    id, auth_cache_ttl_seconds, max_request_body_bytes,
                    account_failure_retry_limit, codex_client_version,
                    kiro_channel_max_concurrency, kiro_channel_min_start_interval_ms,
                    codex_status_refresh_min_interval_seconds,
                    codex_status_refresh_max_interval_seconds,
                    codex_status_account_jitter_max_seconds,
                    kiro_status_refresh_min_interval_seconds,
                    kiro_status_refresh_max_interval_seconds,
                    kiro_status_account_jitter_max_seconds,
                    usage_event_flush_batch_size,
                    usage_event_flush_interval_seconds,
                    usage_event_flush_max_buffer_bytes,
                    usage_event_maintenance_enabled,
                    usage_event_maintenance_interval_seconds,
                    usage_event_detail_retention_days,
                    kiro_cache_kmodels_json,
                    kiro_billable_model_multipliers_json,
                    kiro_cache_policy_json,
                    kiro_prefix_cache_mode,
                    kiro_prefix_cache_max_tokens,
                    kiro_prefix_cache_entry_ttl_seconds,
                    kiro_conversation_anchor_max_entries,
                    kiro_conversation_anchor_ttl_seconds,
                    updated_at_ms
                 FROM llm_runtime_config
                 WHERE id = 'default'",
                [],
                decode_runtime_config,
            )
            .optional()
            .context("load runtime config")
    }

    /// Insert or update the cached Codex public rate-limit snapshot.
    pub fn upsert_codex_rate_limit_status(
        &self,
        snapshot: &CodexRateLimitStatus,
        updated_at_ms: i64,
    ) -> anyhow::Result<()> {
        let snapshot_json =
            serde_json::to_string(snapshot).context("serialize codex rate-limit snapshot")?;
        self.conn
            .execute(
                "INSERT INTO llm_codex_status_cache (id, snapshot_json, updated_at_ms)
                 VALUES ('default', ?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET
                    snapshot_json = excluded.snapshot_json,
                    updated_at_ms = excluded.updated_at_ms",
                params![snapshot_json, updated_at_ms],
            )
            .context("upsert codex rate-limit status snapshot")?;
        Ok(())
    }

    /// Load the cached Codex public rate-limit snapshot, if present.
    pub fn get_codex_rate_limit_status(&self) -> anyhow::Result<Option<CodexRateLimitStatus>> {
        let snapshot_json = self
            .conn
            .query_row(
                "SELECT snapshot_json FROM llm_codex_status_cache WHERE id = 'default'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .context("load codex rate-limit status snapshot")?;
        snapshot_json
            .map(|json| {
                serde_json::from_str::<CodexRateLimitStatus>(&json)
                    .context("decode codex rate-limit status snapshot")
            })
            .transpose()
    }

    /// List active public keys with accumulated rollup counters.
    pub fn list_public_access_keys(&self) -> anyhow::Result<Vec<PublicAccessKey>> {
        let mut stmt = self
            .conn
            .prepare(
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
                 WHERE k.status = 'active' AND k.public_visible = 1
                 ORDER BY lower(k.name)",
            )
            .context("prepare list public access keys")?;
        let rows = stmt
            .query_map([], |row| {
                Ok(PublicAccessKey {
                    key_id: row.get(0)?,
                    key_name: row.get(1)?,
                    secret: row.get(2)?,
                    quota_billable_limit: row.get::<_, i64>(3)? as u64,
                    usage_input_uncached_tokens: row.get::<_, i64>(4)? as u64,
                    usage_input_cached_tokens: row.get::<_, i64>(5)? as u64,
                    usage_output_tokens: row.get::<_, i64>(6)? as u64,
                    usage_billable_tokens: row.get::<_, i64>(7)? as u64,
                    last_used_at_ms: row.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .context("list public access keys")?;
        Ok(rows)
    }

    /// Insert or update a Codex account row.
    pub fn upsert_codex_account(&self, record: &CodexAccountRecord) -> anyhow::Result<()> {
        self.conn
            .execute(
                "INSERT INTO llm_codex_accounts (
                    account_name, account_id, email, status, auth_json, settings_json,
                    last_refresh_at_ms, last_error, created_at_ms, updated_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ON CONFLICT(account_name) DO UPDATE SET
                    account_id = excluded.account_id,
                    email = excluded.email,
                    status = excluded.status,
                    auth_json = excluded.auth_json,
                    settings_json = excluded.settings_json,
                    last_refresh_at_ms = excluded.last_refresh_at_ms,
                    last_error = excluded.last_error,
                    created_at_ms = excluded.created_at_ms,
                    updated_at_ms = excluded.updated_at_ms",
                params![
                    &record.account_name,
                    &record.account_id,
                    &record.email,
                    &record.status,
                    &record.auth_json,
                    &record.settings_json,
                    record.last_refresh_at_ms,
                    &record.last_error,
                    record.created_at_ms,
                    record.updated_at_ms,
                ],
            )
            .context("upsert codex account")?;
        Ok(())
    }

    /// List Codex account rows ordered by account name.
    pub fn list_codex_accounts(&self) -> anyhow::Result<Vec<CodexAccountRecord>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT
                    account_name, account_id, email, status, auth_json, settings_json,
                    last_refresh_at_ms, last_error, created_at_ms, updated_at_ms
                 FROM llm_codex_accounts
                 ORDER BY account_name",
            )
            .context("prepare list codex accounts")?;
        let rows = stmt
            .query_map([], decode_codex_account)?
            .collect::<Result<Vec<_>, _>>()
            .context("list codex accounts")?;
        Ok(rows)
    }

    /// Insert or update a Kiro account row.
    pub fn upsert_kiro_account(&self, record: &KiroAccountRecord) -> anyhow::Result<()> {
        self.conn
            .execute(
                "INSERT INTO llm_kiro_accounts (
                    account_name, auth_method, account_id, profile_arn, user_id,
                    status, auth_json, max_concurrency, min_start_interval_ms,
                    proxy_config_id, last_refresh_at_ms, last_error, created_at_ms,
                    updated_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                ON CONFLICT(account_name) DO UPDATE SET
                    auth_method = excluded.auth_method,
                    account_id = excluded.account_id,
                    profile_arn = excluded.profile_arn,
                    user_id = excluded.user_id,
                    status = excluded.status,
                    auth_json = excluded.auth_json,
                    max_concurrency = excluded.max_concurrency,
                    min_start_interval_ms = excluded.min_start_interval_ms,
                    proxy_config_id = excluded.proxy_config_id,
                    last_refresh_at_ms = excluded.last_refresh_at_ms,
                    last_error = excluded.last_error,
                    created_at_ms = excluded.created_at_ms,
                    updated_at_ms = excluded.updated_at_ms",
                params![
                    &record.account_name,
                    &record.auth_method,
                    &record.account_id,
                    &record.profile_arn,
                    &record.user_id,
                    &record.status,
                    &record.auth_json,
                    record.max_concurrency,
                    record.min_start_interval_ms,
                    &record.proxy_config_id,
                    record.last_refresh_at_ms,
                    &record.last_error,
                    record.created_at_ms,
                    record.updated_at_ms,
                ],
            )
            .context("upsert kiro account")?;
        Ok(())
    }

    /// List Kiro account rows ordered by account name.
    pub fn list_kiro_accounts(&self) -> anyhow::Result<Vec<KiroAccountRecord>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT
                    account_name, auth_method, account_id, profile_arn, user_id,
                    status, auth_json, max_concurrency, min_start_interval_ms,
                    proxy_config_id, last_refresh_at_ms, last_error, created_at_ms,
                    updated_at_ms
                 FROM llm_kiro_accounts
                 ORDER BY account_name",
            )
            .context("prepare list kiro accounts")?;
        let rows = stmt
            .query_map([], decode_kiro_account)?
            .collect::<Result<Vec<_>, _>>()
            .context("list kiro accounts")?;
        Ok(rows)
    }
}

fn decode_key_bundle(row: &rusqlite::Row<'_>) -> rusqlite::Result<KeyBundle> {
    let key_id: String = row.get(0)?;
    let credit_total_raw: String = row.get(27)?;
    let credit_total = credit_total_raw
        .parse::<f64>()
        .map_err(|err| rusqlite::Error::FromSqlConversionFailure(27, Type::Text, Box::new(err)))?;
    Ok(KeyBundle {
        key: KeyRecord {
            key_id: key_id.clone(),
            name: row.get(1)?,
            secret: row.get(2)?,
            key_hash: row.get(3)?,
            status: row.get(4)?,
            provider_type: row.get(5)?,
            protocol_family: row.get(6)?,
            public_visible: row.get::<_, i64>(7)? != 0,
            quota_billable_limit: row.get(8)?,
            created_at_ms: row.get(9)?,
            updated_at_ms: row.get(10)?,
        },
        route: KeyRouteConfig {
            key_id: key_id.clone(),
            route_strategy: row.get(11)?,
            fixed_account_name: row.get(12)?,
            auto_account_names_json: row.get(13)?,
            account_group_id: row.get(14)?,
            model_name_map_json: row.get(15)?,
            request_max_concurrency: row.get(16)?,
            request_min_start_interval_ms: row.get(17)?,
            kiro_request_validation_enabled: row.get::<_, Option<i64>>(18)?.unwrap_or(0) != 0,
            kiro_cache_estimation_enabled: row.get::<_, Option<i64>>(19)?.unwrap_or(0) != 0,
            kiro_zero_cache_debug_enabled: row.get::<_, Option<i64>>(20)?.unwrap_or(0) != 0,
            kiro_cache_policy_override_json: row.get(21)?,
            kiro_billable_model_multipliers_override_json: row.get(22)?,
        },
        rollup: KeyUsageRollup {
            key_id,
            input_uncached_tokens: row.get(23)?,
            input_cached_tokens: row.get(24)?,
            output_tokens: row.get(25)?,
            billable_tokens: row.get(26)?,
            credit_total,
            credit_missing_events: row.get(28)?,
            last_used_at_ms: row.get(29)?,
            updated_at_ms: row.get(30)?,
        },
    })
}

fn decode_codex_account(row: &rusqlite::Row<'_>) -> rusqlite::Result<CodexAccountRecord> {
    Ok(CodexAccountRecord {
        account_name: row.get(0)?,
        account_id: row.get(1)?,
        email: row.get(2)?,
        status: row.get(3)?,
        auth_json: row.get(4)?,
        settings_json: row.get(5)?,
        last_refresh_at_ms: row.get(6)?,
        last_error: row.get(7)?,
        created_at_ms: row.get(8)?,
        updated_at_ms: row.get(9)?,
    })
}

fn decode_kiro_account(row: &rusqlite::Row<'_>) -> rusqlite::Result<KiroAccountRecord> {
    Ok(KiroAccountRecord {
        account_name: row.get(0)?,
        auth_method: row.get(1)?,
        account_id: row.get(2)?,
        profile_arn: row.get(3)?,
        user_id: row.get(4)?,
        status: row.get(5)?,
        auth_json: row.get(6)?,
        max_concurrency: row.get(7)?,
        min_start_interval_ms: row.get(8)?,
        proxy_config_id: row.get(9)?,
        last_refresh_at_ms: row.get(10)?,
        last_error: row.get(11)?,
        created_at_ms: row.get(12)?,
        updated_at_ms: row.get(13)?,
    })
}

fn decode_runtime_config(row: &rusqlite::Row<'_>) -> rusqlite::Result<RuntimeConfigRecord> {
    Ok(RuntimeConfigRecord {
        id: row.get(0)?,
        auth_cache_ttl_seconds: row.get(1)?,
        max_request_body_bytes: row.get(2)?,
        account_failure_retry_limit: row.get(3)?,
        codex_client_version: row.get(4)?,
        kiro_channel_max_concurrency: row.get(5)?,
        kiro_channel_min_start_interval_ms: row.get(6)?,
        codex_status_refresh_min_interval_seconds: row.get(7)?,
        codex_status_refresh_max_interval_seconds: row.get(8)?,
        codex_status_account_jitter_max_seconds: row.get(9)?,
        kiro_status_refresh_min_interval_seconds: row.get(10)?,
        kiro_status_refresh_max_interval_seconds: row.get(11)?,
        kiro_status_account_jitter_max_seconds: row.get(12)?,
        usage_event_flush_batch_size: row.get(13)?,
        usage_event_flush_interval_seconds: row.get(14)?,
        usage_event_flush_max_buffer_bytes: row.get(15)?,
        usage_event_maintenance_enabled: row.get::<_, i64>(16)? != 0,
        usage_event_maintenance_interval_seconds: row.get(17)?,
        usage_event_detail_retention_days: row.get(18)?,
        kiro_cache_kmodels_json: row.get(19)?,
        kiro_billable_model_multipliers_json: row.get(20)?,
        kiro_cache_policy_json: row.get(21)?,
        kiro_prefix_cache_mode: row.get(22)?,
        kiro_prefix_cache_max_tokens: row.get(23)?,
        kiro_prefix_cache_entry_ttl_seconds: row.get(24)?,
        kiro_conversation_anchor_max_entries: row.get(25)?,
        kiro_conversation_anchor_ttl_seconds: row.get(26)?,
        updated_at_ms: row.get(27)?,
    })
}

#[cfg(test)]
impl RuntimeConfigRecord {
    fn test_default() -> Self {
        Self {
            id: "default".to_string(),
            auth_cache_ttl_seconds: 60,
            max_request_body_bytes: 1_048_576,
            account_failure_retry_limit: 3,
            codex_client_version: "0.124.0".to_string(),
            kiro_channel_max_concurrency: 4,
            kiro_channel_min_start_interval_ms: 100,
            codex_status_refresh_min_interval_seconds: 240,
            codex_status_refresh_max_interval_seconds: 300,
            codex_status_account_jitter_max_seconds: 10,
            kiro_status_refresh_min_interval_seconds: 240,
            kiro_status_refresh_max_interval_seconds: 300,
            kiro_status_account_jitter_max_seconds: 10,
            usage_event_flush_batch_size: 32,
            usage_event_flush_interval_seconds: 5,
            usage_event_flush_max_buffer_bytes: 1_048_576,
            usage_event_maintenance_enabled: true,
            usage_event_maintenance_interval_seconds: 3600,
            usage_event_detail_retention_days: 30,
            kiro_cache_kmodels_json: "[]".to_string(),
            kiro_billable_model_multipliers_json: "{}".to_string(),
            kiro_cache_policy_json: "{}".to_string(),
            kiro_prefix_cache_mode: "formula".to_string(),
            kiro_prefix_cache_max_tokens: 100_000,
            kiro_prefix_cache_entry_ttl_seconds: 3600,
            kiro_conversation_anchor_max_entries: 1024,
            kiro_conversation_anchor_ttl_seconds: 3600,
            updated_at_ms: 100,
        }
    }
}

#[cfg(test)]
mod schema_tests {
    use rusqlite::Connection;

    fn table_names(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .expect("prepare table query");
        stmt.query_map([], |row| row.get::<_, String>(0))
            .expect("query table names")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect table names")
    }

    #[test]
    fn sqlite_schema_contains_full_parity_control_tables() {
        let conn = Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("initialize sqlite");
        let tables = table_names(&conn);

        for required in [
            "llm_keys",
            "llm_key_route_config",
            "llm_key_usage_rollups",
            "llm_runtime_config",
            "llm_account_groups",
            "llm_proxy_configs",
            "llm_proxy_bindings",
            "llm_codex_accounts",
            "llm_kiro_accounts",
            "llm_kiro_status_cache",
            "llm_token_requests",
            "llm_account_contribution_requests",
            "llm_sponsor_requests",
            "cdc_consumer_offsets",
            "cdc_apply_state",
            "cdc_applied_events_recent",
        ] {
            assert!(tables.contains(&required.to_string()), "missing table {required}");
        }
    }

    #[test]
    fn key_lookup_by_hash_is_indexed() {
        let conn = Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("initialize sqlite");
        let mut stmt = conn
            .prepare("PRAGMA index_list('llm_keys')")
            .expect("prepare index query");
        let indexes = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .expect("query indexes")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect indexes");
        assert!(indexes
            .iter()
            .any(|name| name.contains("key_hash") || name.contains("sqlite_autoindex")));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn key_repository_round_trips_key_route_and_rollup() {
        let conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("init schema");
        let repo = super::SqliteControlStore::new(conn);

        let key = super::KeyRecord {
            key_id: "key-1".to_string(),
            name: "primary".to_string(),
            secret: "sk-test".to_string(),
            key_hash: "hash".to_string(),
            status: "active".to_string(),
            provider_type: "kiro".to_string(),
            protocol_family: "anthropic".to_string(),
            public_visible: true,
            quota_billable_limit: 1000,
            created_at_ms: 10,
            updated_at_ms: 20,
        };
        let route = super::KeyRouteConfig {
            key_id: "key-1".to_string(),
            route_strategy: Some("auto".to_string()),
            fixed_account_name: None,
            auto_account_names_json: Some(r#"["a","b"]"#.to_string()),
            account_group_id: Some("group-1".to_string()),
            model_name_map_json: None,
            request_max_concurrency: Some(2),
            request_min_start_interval_ms: Some(100),
            kiro_request_validation_enabled: true,
            kiro_cache_estimation_enabled: true,
            kiro_zero_cache_debug_enabled: false,
            kiro_cache_policy_override_json: Some(r#"{"enabled":true}"#.to_string()),
            kiro_billable_model_multipliers_override_json: None,
        };
        let rollup = super::KeyUsageRollup {
            key_id: "key-1".to_string(),
            input_uncached_tokens: 11,
            input_cached_tokens: 22,
            output_tokens: 33,
            billable_tokens: 44,
            credit_total: 55.5,
            credit_missing_events: 1,
            last_used_at_ms: Some(30),
            updated_at_ms: 40,
        };

        repo.upsert_key_bundle(&key, &route, &rollup)
            .expect("upsert key");
        let loaded = repo
            .get_key("key-1")
            .expect("load key")
            .expect("key exists");

        assert_eq!(loaded.key.name, "primary");
        assert_eq!(loaded.route.account_group_id.as_deref(), Some("group-1"));
        assert_eq!(loaded.route.request_max_concurrency, Some(2));
        assert!(loaded.route.kiro_request_validation_enabled);
        assert!(loaded.route.kiro_cache_estimation_enabled);
        assert!(!loaded.route.kiro_zero_cache_debug_enabled);
        assert_eq!(loaded.rollup.output_tokens, 33);
    }

    #[test]
    fn key_repository_loads_key_bundle_by_hash() {
        let conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("init schema");
        let repo = super::SqliteControlStore::new(conn);
        let key = super::KeyRecord {
            key_id: "key-by-hash".to_string(),
            name: "hash target".to_string(),
            secret: "sk-test".to_string(),
            key_hash: "hash-target".to_string(),
            status: "active".to_string(),
            provider_type: "codex".to_string(),
            protocol_family: "openai".to_string(),
            public_visible: false,
            quota_billable_limit: 10_000,
            created_at_ms: 10,
            updated_at_ms: 20,
        };
        let route = super::KeyRouteConfig {
            key_id: key.key_id.clone(),
            route_strategy: Some("fixed".to_string()),
            fixed_account_name: Some("account-a".to_string()),
            auto_account_names_json: None,
            account_group_id: None,
            model_name_map_json: None,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            kiro_request_validation_enabled: false,
            kiro_cache_estimation_enabled: false,
            kiro_zero_cache_debug_enabled: false,
            kiro_cache_policy_override_json: None,
            kiro_billable_model_multipliers_override_json: None,
        };
        let rollup = super::KeyUsageRollup {
            key_id: key.key_id.clone(),
            input_uncached_tokens: 1,
            input_cached_tokens: 2,
            output_tokens: 3,
            billable_tokens: 4,
            credit_total: 0.0,
            credit_missing_events: 0,
            last_used_at_ms: None,
            updated_at_ms: 20,
        };

        repo.upsert_key_bundle(&key, &route, &rollup)
            .expect("upsert key");

        let loaded = repo
            .get_key_by_hash("hash-target")
            .expect("load key by hash")
            .expect("key exists");
        assert_eq!(loaded.key.key_id, "key-by-hash");
        assert_eq!(loaded.route.fixed_account_name.as_deref(), Some("account-a"));
        assert_eq!(loaded.rollup.billable_tokens, 4);
    }

    #[test]
    fn key_usage_rollup_increments_from_usage_event() {
        let conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("init schema");
        let repo = super::SqliteControlStore::new(conn);
        let key = super::KeyRecord {
            key_id: "key-rollup".to_string(),
            name: "rollup".to_string(),
            secret: "sk-test".to_string(),
            key_hash: "hash-rollup".to_string(),
            status: "active".to_string(),
            provider_type: "kiro".to_string(),
            protocol_family: "anthropic".to_string(),
            public_visible: true,
            quota_billable_limit: 10_000,
            created_at_ms: 10,
            updated_at_ms: 20,
        };
        let route = super::KeyRouteConfig {
            key_id: key.key_id.clone(),
            route_strategy: Some("auto".to_string()),
            fixed_account_name: None,
            auto_account_names_json: None,
            account_group_id: None,
            model_name_map_json: None,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            kiro_request_validation_enabled: true,
            kiro_cache_estimation_enabled: true,
            kiro_zero_cache_debug_enabled: false,
            kiro_cache_policy_override_json: None,
            kiro_billable_model_multipliers_override_json: None,
        };
        let rollup = super::KeyUsageRollup {
            key_id: key.key_id.clone(),
            input_uncached_tokens: 10,
            input_cached_tokens: 20,
            output_tokens: 30,
            billable_tokens: 40,
            credit_total: 1.5,
            credit_missing_events: 2,
            last_used_at_ms: Some(100),
            updated_at_ms: 100,
        };
        repo.upsert_key_bundle(&key, &route, &rollup)
            .expect("upsert key");

        let event = llm_access_core::usage::UsageEvent {
            event_id: "event-1".to_string(),
            created_at_ms: 500,
            provider_type: llm_access_core::provider::ProviderType::Kiro,
            protocol_family: llm_access_core::provider::ProtocolFamily::Anthropic,
            key_id: key.key_id.clone(),
            key_name: key.name.clone(),
            account_name: Some("account-a".to_string()),
            route_strategy_at_event: Some(llm_access_core::provider::RouteStrategy::Auto),
            endpoint: "/v1/messages".to_string(),
            model: Some("claude-sonnet-4-5".to_string()),
            mapped_model: None,
            status_code: 200,
            request_body_bytes: Some(1024),
            input_uncached_tokens: 7,
            input_cached_tokens: 8,
            output_tokens: 9,
            billable_tokens: 10,
            credit_usage: Some("0.25".to_string()),
            usage_missing: false,
            credit_usage_missing: false,
            timing: llm_access_core::usage::UsageTiming::default(),
        };

        repo.increment_key_usage_rollup(&event)
            .expect("increment rollup");

        let loaded = repo
            .get_key("key-rollup")
            .expect("load key")
            .expect("key exists");
        assert_eq!(loaded.rollup.input_uncached_tokens, 17);
        assert_eq!(loaded.rollup.input_cached_tokens, 28);
        assert_eq!(loaded.rollup.output_tokens, 39);
        assert_eq!(loaded.rollup.billable_tokens, 50);
        assert_eq!(loaded.rollup.credit_total, 1.75);
        assert_eq!(loaded.rollup.credit_missing_events, 2);
        assert_eq!(loaded.rollup.last_used_at_ms, Some(500));
    }

    #[test]
    fn runtime_config_repository_upserts_single_default_record() {
        let conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("init schema");
        let repo = super::SqliteControlStore::new(conn);
        let mut config = super::RuntimeConfigRecord::test_default();
        config.codex_client_version = "0.124.0".to_string();
        config.updated_at_ms = 100;

        repo.upsert_runtime_config(&config).expect("upsert config");
        config.codex_client_version = "0.125.0".to_string();
        config.updated_at_ms = 200;
        repo.upsert_runtime_config(&config).expect("upsert config");

        let value = repo
            .get_runtime_config()
            .expect("load config")
            .expect("config exists");
        assert_eq!(value.codex_client_version, "0.125.0");
        assert_eq!(value.updated_at_ms, 200);
    }

    #[test]
    fn account_repositories_round_trip_codex_and_kiro_accounts() {
        let conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("init schema");
        let repo = super::SqliteControlStore::new(conn);

        repo.upsert_codex_account(&super::CodexAccountRecord {
            account_name: "codex-a".to_string(),
            account_id: Some("acct-1".to_string()),
            email: Some("codex@example.com".to_string()),
            status: "active".to_string(),
            auth_json: r#"{"tokens":{"access_token":"a"}}"#.to_string(),
            settings_json: r#"{"tier":"plus"}"#.to_string(),
            last_refresh_at_ms: Some(100),
            last_error: None,
            created_at_ms: 10,
            updated_at_ms: 20,
        })
        .expect("upsert codex account");

        repo.upsert_kiro_account(&super::KiroAccountRecord {
            account_name: "kiro-a".to_string(),
            auth_method: "idc".to_string(),
            account_id: Some("kiro-account".to_string()),
            profile_arn: Some("arn:aws:kiro:test".to_string()),
            user_id: Some("user-1".to_string()),
            status: "active".to_string(),
            auth_json: r#"{"accessToken":"a"}"#.to_string(),
            max_concurrency: Some(1),
            min_start_interval_ms: Some(100),
            proxy_config_id: Some("proxy-a".to_string()),
            last_refresh_at_ms: Some(200),
            last_error: None,
            created_at_ms: 30,
            updated_at_ms: 40,
        })
        .expect("upsert kiro account");

        let codex_accounts = repo.list_codex_accounts().expect("list codex accounts");
        let kiro_accounts = repo.list_kiro_accounts().expect("list kiro accounts");

        assert_eq!(codex_accounts.len(), 1);
        assert_eq!(codex_accounts[0].account_name, "codex-a");
        assert_eq!(codex_accounts[0].email.as_deref(), Some("codex@example.com"));
        assert_eq!(kiro_accounts.len(), 1);
        assert_eq!(kiro_accounts[0].account_name, "kiro-a");
        assert_eq!(kiro_accounts[0].auth_method, "idc");
        assert_eq!(kiro_accounts[0].user_id.as_deref(), Some("user-1"));
    }
}
