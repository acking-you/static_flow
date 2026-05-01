//! SQLite control-plane repository for `llm-access`.

use std::collections::BTreeMap;

use anyhow::Context;
use llm_access_core::{
    provider::RouteStrategy,
    store::{
        self as core_store, AdminAccountContributionRequest, AdminAccountContributionRequestsPage,
        AdminAccountGroup, AdminAccountGroupPatch, AdminCodexAccount, AdminCodexAccountPatch,
        AdminKey, AdminKeyPatch, AdminKiroAccount, AdminKiroAccountPatch, AdminKiroBalanceView,
        AdminKiroCacheView, AdminKiroStatusCacheUpdate, AdminLegacyKiroProxyMigration,
        AdminProxyBinding, AdminProxyConfig, AdminProxyConfigPatch, AdminReviewQueueAction,
        AdminReviewQueueQuery, AdminRuntimeConfig, AdminSponsorRequest, AdminSponsorRequestsPage,
        AdminTokenRequest, AdminTokenRequestsPage, AuthenticatedKey, CodexRateLimitStatus,
        NewAdminAccountGroup, NewAdminCodexAccount, NewAdminKey, NewAdminKiroAccount,
        NewAdminProxyConfig, NewPublicAccountContributionRequest, NewPublicSponsorRequest,
        NewPublicTokenRequest, ProviderCodexAuthUpdate, ProviderCodexRoute, ProviderKiroAuthUpdate,
        ProviderProxyConfig, PublicAccessKey, PublicAccountContribution, PublicSponsor,
        PublicUsageLookupKey, PUBLIC_SPONSOR_REQUEST_STATUS_SUBMITTED,
        PUBLIC_TOKEN_REQUEST_STATUS_PENDING,
    },
};
use rusqlite::{params, types::Type, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
struct CodexAccountSettings {
    map_gpt53_codex_to_spark: bool,
    proxy_mode: String,
    proxy_config_id: Option<String>,
    request_max_concurrency: Option<u64>,
    request_min_start_interval_ms: Option<u64>,
}

impl Default for CodexAccountSettings {
    fn default() -> Self {
        Self {
            map_gpt53_codex_to_spark: false,
            proxy_mode: "inherit".to_string(),
            proxy_config_id: None,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn count_review_rows(
    conn: &Connection,
    count_all_sql: &str,
    count_status_sql: &str,
    status: Option<&str>,
) -> anyhow::Result<usize> {
    let count: i64 = if let Some(status) = status {
        conn.query_row(count_status_sql, [status], |row| row.get(0))
    } else {
        conn.query_row(count_all_sql, [], |row| row.get(0))
    }
    .context("count review rows")?;
    Ok(count.max(0) as usize)
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

    /// List all admin-visible key rows with route config and usage rollup.
    pub fn list_admin_keys(&self) -> anyhow::Result<Vec<AdminKey>> {
        let mut stmt = self
            .conn
            .prepare(
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
                 JOIN llm_key_route_config r ON r.key_id = k.key_id
                 JOIN llm_key_usage_rollups u ON u.key_id = k.key_id
                 ORDER BY k.created_at_ms DESC, k.key_id DESC",
            )
            .context("prepare admin key list")?;
        let keys = stmt
            .query_map([], decode_key_bundle)
            .context("query admin key list")?
            .map(|row| row.map(|bundle| admin_key_from_bundle(&bundle)))
            .collect::<Result<Vec<_>, _>>()
            .context("collect admin key list")?;
        Ok(keys)
    }

    /// Create one admin-managed provider key.
    pub fn create_admin_key(&self, key: &NewAdminKey) -> anyhow::Result<AdminKey> {
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
            kiro_request_validation_enabled: true,
            kiro_cache_estimation_enabled: true,
            kiro_zero_cache_debug_enabled: false,
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
        self.upsert_key_bundle(&key_record, &route, &rollup)?;
        self.get_key(&key.id)?
            .map(|bundle| admin_key_from_bundle(&bundle))
            .context("created admin key disappeared")
    }

    /// Resolve the Codex account route for one authenticated key.
    pub fn resolve_provider_codex_route(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        Ok(self.resolve_provider_codex_routes(key)?.into_iter().next())
    }

    /// Resolve all Codex account route candidates for one authenticated key.
    pub fn resolve_provider_codex_routes(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Vec<ProviderCodexRoute>> {
        let Some(bundle) = self.get_key(&key.key_id)? else {
            return Ok(Vec::new());
        };
        if bundle.key.provider_type != core_store::PROVIDER_CODEX {
            return Ok(Vec::new());
        }

        let account_names = self.resolve_route_account_names(
            core_store::PROVIDER_CODEX,
            &bundle.route,
            self.list_codex_accounts()?
                .into_iter()
                .filter(|record| record.status == core_store::KEY_STATUS_ACTIVE)
                .map(|record| record.account_name)
                .collect(),
        )?;
        let route_strategy_at_event = route_strategy_from_config(&bundle.route)?;
        let account_group_id_at_event = bundle.route.account_group_id.clone();
        let mut routes = Vec::new();
        for account_name in account_names {
            let Some(record) = self.get_codex_account(&account_name)? else {
                continue;
            };
            if record.status != core_store::KEY_STATUS_ACTIVE {
                continue;
            }
            let settings = decode_codex_account_settings(&record.settings_json)?;
            let proxy = self.resolve_provider_proxy_config(
                core_store::PROVIDER_CODEX,
                &settings.proxy_mode,
                settings.proxy_config_id.as_deref(),
            )?;
            routes.push(ProviderCodexRoute {
                account_name: record.account_name,
                account_group_id_at_event: account_group_id_at_event.clone(),
                route_strategy_at_event,
                auth_json: record.auth_json,
                map_gpt53_codex_to_spark: settings.map_gpt53_codex_to_spark,
                request_max_concurrency: bundle
                    .route
                    .request_max_concurrency
                    .and_then(non_negative_i64_to_u64),
                request_min_start_interval_ms: bundle
                    .route
                    .request_min_start_interval_ms
                    .and_then(non_negative_i64_to_u64),
                account_request_max_concurrency: settings.request_max_concurrency,
                account_request_min_start_interval_ms: settings.request_min_start_interval_ms,
                proxy,
            });
        }
        Ok(routes)
    }

    /// Resolve the Kiro account route for one authenticated key.
    pub fn resolve_provider_kiro_route(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<core_store::ProviderKiroRoute>> {
        Ok(self.resolve_provider_kiro_routes(key)?.into_iter().next())
    }

    /// Resolve all Kiro account route candidates for one authenticated key.
    pub fn resolve_provider_kiro_routes(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Vec<core_store::ProviderKiroRoute>> {
        let Some(bundle) = self.get_key(&key.key_id)? else {
            return Ok(Vec::new());
        };
        if bundle.key.provider_type != core_store::PROVIDER_KIRO {
            return Ok(Vec::new());
        }

        let runtime_config = self.get_runtime_config_or_default()?;
        let account_names = self.resolve_route_account_names(
            core_store::PROVIDER_KIRO,
            &bundle.route,
            self.list_kiro_accounts()?
                .into_iter()
                .filter(|record| record.status == core_store::KEY_STATUS_ACTIVE)
                .map(|record| record.account_name)
                .collect(),
        )?;
        let route_strategy_at_event = route_strategy_from_config(&bundle.route)?;
        let account_group_id_at_event = bundle.route.account_group_id.clone();
        let mut routes = Vec::new();
        for account_name in account_names {
            let Some(record) = self.get_kiro_account(&account_name)? else {
                continue;
            };
            if record.status != core_store::KEY_STATUS_ACTIVE {
                continue;
            }
            let auth_json = serde_json::from_str::<serde_json::Value>(&record.auth_json)
                .context("parse kiro account auth json")?;
            if optional_json_bool_any(&auth_json, &["disabled"]).unwrap_or(false) {
                continue;
            }
            let minimum_remaining_credits_before_block = optional_json_f64_any(&auth_json, &[
                "minimumRemainingCreditsBeforeBlock",
                "minimum_remaining_credits_before_block",
            ])
            .unwrap_or(0.0)
            .max(0.0);
            let cached_status = self.get_kiro_cached_status_parts(&record.account_name)?;
            if let Some((balance, cache)) = &cached_status {
                if matches!(cache.status.as_str(), "disabled" | "quota_exhausted") {
                    continue;
                }
                if balance.as_ref().is_some_and(|balance| {
                    balance.remaining <= 0.0
                        || balance.remaining <= minimum_remaining_credits_before_block
                }) {
                    continue;
                }
            }
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
            let profile_arn = record
                .profile_arn
                .clone()
                .or_else(|| optional_json_string(&auth_json, "profileArn"))
                .or_else(|| optional_json_string(&auth_json, "profile_arn"));
            let api_region = optional_json_string(&auth_json, "apiRegion")
                .or_else(|| optional_json_string(&auth_json, "api_region"))
                .or_else(|| optional_json_string(&auth_json, "region"))
                .unwrap_or_else(|| "us-east-1".to_string());
            let cache_policy_json = bundle
                .route
                .kiro_cache_policy_override_json
                .clone()
                .unwrap_or_else(|| runtime_config.kiro_cache_policy_json.clone());
            let billable_model_multipliers_json = bundle
                .route
                .kiro_billable_model_multipliers_override_json
                .clone()
                .unwrap_or_else(|| runtime_config.kiro_billable_model_multipliers_json.clone());
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
            let proxy = self.resolve_provider_proxy_config(
                core_store::PROVIDER_KIRO,
                &proxy_mode,
                proxy_config_id.as_deref(),
            )?;
            routes.push(core_store::ProviderKiroRoute {
                account_name: record.account_name,
                account_group_id_at_event: account_group_id_at_event.clone(),
                route_strategy_at_event,
                auth_json: record.auth_json,
                profile_arn,
                api_region,
                request_validation_enabled: bundle.route.kiro_request_validation_enabled,
                cache_estimation_enabled: bundle.route.kiro_cache_estimation_enabled,
                zero_cache_debug_enabled: bundle.route.kiro_zero_cache_debug_enabled,
                model_name_map_json: bundle
                    .route
                    .model_name_map_json
                    .clone()
                    .unwrap_or_else(|| "{}".to_string()),
                cache_kmodels_json: runtime_config.kiro_cache_kmodels_json.clone(),
                cache_policy_json,
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
                billable_model_multipliers_json,
                request_max_concurrency: bundle
                    .route
                    .request_max_concurrency
                    .and_then(non_negative_i64_to_u64),
                request_min_start_interval_ms: bundle
                    .route
                    .request_min_start_interval_ms
                    .and_then(non_negative_i64_to_u64),
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
            });
        }
        Ok(routes)
    }

    fn resolve_route_account_names(
        &self,
        provider_type: &str,
        route: &KeyRouteConfig,
        default_active_account_names: Vec<String>,
    ) -> anyhow::Result<Vec<String>> {
        let strategy = route.route_strategy.as_deref().unwrap_or("auto");
        match strategy {
            "fixed" => {
                let account_name = if let Some(group_id) = route.account_group_id.as_deref() {
                    let group = self.get_admin_account_group(group_id)?.with_context(|| {
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
                    let group = self.get_admin_account_group(group_id)?.with_context(|| {
                        format!("configured account_group_id `{group_id}` does not exist")
                    })?;
                    if group.provider_type != provider_type {
                        anyhow::bail!(
                            "configured account_group_id belongs to a different provider"
                        );
                    }
                    return Ok(group.account_names);
                }
                if let Some(names) =
                    decode_optional_json::<Vec<String>>(route.auto_account_names_json.as_deref())
                {
                    return Ok(names);
                }
                Ok(default_active_account_names)
            },
            other => anyhow::bail!("unsupported route strategy `{other}`"),
        }
    }

    /// Patch one admin-managed key.
    pub fn patch_admin_key(
        &self,
        key_id: &str,
        patch: &AdminKeyPatch,
    ) -> anyhow::Result<Option<AdminKey>> {
        let Some(mut bundle) = self.get_key(key_id)? else {
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
                .context("serialize auto account names")?;
        }
        if let Some(value) = patch.model_name_map.as_ref() {
            bundle.route.model_name_map_json = value
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .context("serialize model name map")?;
        }
        if let Some(value) = patch.request_max_concurrency {
            bundle.route.request_max_concurrency = value.map(|value| value as i64);
        }
        if let Some(value) = patch.request_min_start_interval_ms {
            bundle.route.request_min_start_interval_ms = value.map(|value| value as i64);
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
        if let Some(value) = patch.kiro_cache_policy_override_json.as_ref() {
            bundle.route.kiro_cache_policy_override_json = value.clone();
        }
        if let Some(value) = patch.kiro_billable_model_multipliers_override_json.as_ref() {
            bundle.route.kiro_billable_model_multipliers_override_json = value.clone();
        }
        bundle.key.updated_at_ms = patch.updated_at_ms;
        bundle.rollup.updated_at_ms = bundle.rollup.updated_at_ms.max(patch.updated_at_ms);
        self.upsert_key_bundle(&bundle.key, &bundle.route, &bundle.rollup)?;
        Ok(Some(admin_key_from_bundle(&bundle)))
    }

    /// Delete one admin-managed key.
    pub fn delete_admin_key(&self, key_id: &str) -> anyhow::Result<Option<AdminKey>> {
        let Some(bundle) = self.get_key(key_id)? else {
            return Ok(None);
        };
        self.conn
            .execute("DELETE FROM llm_keys WHERE key_id = ?1", [key_id])
            .context("delete admin key")?;
        Ok(Some(admin_key_from_bundle(&bundle)))
    }

    /// List admin account groups for one provider.
    pub fn list_admin_account_groups(
        &self,
        provider_type: &str,
    ) -> anyhow::Result<Vec<AdminAccountGroup>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT group_id, provider_type, name, account_names_json,
                    created_at_ms, updated_at_ms
                 FROM llm_account_groups
                 WHERE provider_type = ?1
                 ORDER BY name ASC, group_id ASC",
            )
            .context("prepare account group list")?;
        let groups = stmt
            .query_map([provider_type], decode_admin_account_group)
            .context("query account group list")?
            .collect::<Result<Vec<_>, _>>()
            .context("collect account group list")?;
        Ok(groups)
    }

    /// Create one admin account group.
    pub fn create_admin_account_group(
        &self,
        group: &NewAdminAccountGroup,
    ) -> anyhow::Result<AdminAccountGroup> {
        let account_names_json =
            serde_json::to_string(&group.account_names).context("serialize account group names")?;
        self.conn
            .execute(
                "INSERT INTO llm_account_groups (
                    group_id, provider_type, name, account_names_json,
                    created_at_ms, updated_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    &group.id,
                    &group.provider_type,
                    &group.name,
                    &account_names_json,
                    group.created_at_ms,
                    group.created_at_ms,
                ],
            )
            .context("create admin account group")?;
        self.get_admin_account_group(&group.id)?
            .context("created account group disappeared")
    }

    /// Patch one admin account group.
    pub fn patch_admin_account_group(
        &self,
        group_id: &str,
        patch: &AdminAccountGroupPatch,
    ) -> anyhow::Result<Option<AdminAccountGroup>> {
        let Some(mut group) = self.get_admin_account_group(group_id)? else {
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
        self.conn
            .execute(
                "UPDATE llm_account_groups
                 SET name = ?2, account_names_json = ?3, updated_at_ms = ?4
                 WHERE group_id = ?1",
                params![group_id, &group.name, &account_names_json, group.updated_at],
            )
            .context("patch admin account group")?;
        Ok(Some(group))
    }

    /// Delete one admin account group.
    pub fn delete_admin_account_group(
        &self,
        group_id: &str,
    ) -> anyhow::Result<Option<AdminAccountGroup>> {
        let Some(group) = self.get_admin_account_group(group_id)? else {
            return Ok(None);
        };
        self.conn
            .execute("DELETE FROM llm_account_groups WHERE group_id = ?1", [group_id])
            .context("delete admin account group")?;
        Ok(Some(group))
    }

    fn get_admin_account_group(&self, group_id: &str) -> anyhow::Result<Option<AdminAccountGroup>> {
        self.conn
            .query_row(
                "SELECT group_id, provider_type, name, account_names_json,
                    created_at_ms, updated_at_ms
                 FROM llm_account_groups
                 WHERE group_id = ?1",
                [group_id],
                decode_admin_account_group,
            )
            .optional()
            .context("load admin account group")
    }

    /// List reusable proxy configs.
    pub fn list_admin_proxy_configs(&self) -> anyhow::Result<Vec<AdminProxyConfig>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT proxy_config_id, name, proxy_url, proxy_username, proxy_password,
                    status, created_at_ms, updated_at_ms
                 FROM llm_proxy_configs
                 ORDER BY created_at_ms DESC, proxy_config_id DESC",
            )
            .context("prepare proxy config list")?;
        let proxy_configs = stmt
            .query_map([], decode_admin_proxy_config)
            .context("query proxy config list")?
            .collect::<Result<Vec<_>, _>>()
            .context("collect proxy config list")?;
        Ok(proxy_configs)
    }

    /// Load one reusable proxy config by id.
    pub fn get_admin_proxy_config(
        &self,
        proxy_id: &str,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        self.conn
            .query_row(
                "SELECT proxy_config_id, name, proxy_url, proxy_username, proxy_password,
                    status, created_at_ms, updated_at_ms
                 FROM llm_proxy_configs
                 WHERE proxy_config_id = ?1",
                [proxy_id],
                decode_admin_proxy_config,
            )
            .optional()
            .context("load admin proxy config")
    }

    /// Create one reusable proxy config.
    pub fn create_admin_proxy_config(
        &self,
        proxy: &NewAdminProxyConfig,
    ) -> anyhow::Result<AdminProxyConfig> {
        self.conn
            .execute(
                "INSERT INTO llm_proxy_configs (
                    proxy_config_id, name, proxy_url, proxy_username, proxy_password,
                    status, created_at_ms, updated_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    &proxy.id,
                    &proxy.name,
                    &proxy.proxy_url,
                    &proxy.proxy_username,
                    &proxy.proxy_password,
                    core_store::KEY_STATUS_ACTIVE,
                    proxy.created_at_ms,
                    proxy.created_at_ms,
                ],
            )
            .context("create admin proxy config")?;
        self.get_admin_proxy_config(&proxy.id)?
            .context("created proxy config disappeared")
    }

    /// Patch one reusable proxy config.
    pub fn patch_admin_proxy_config(
        &self,
        proxy_id: &str,
        patch: &AdminProxyConfigPatch,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        let Some(mut proxy) = self.get_admin_proxy_config(proxy_id)? else {
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
        self.conn
            .execute(
                "UPDATE llm_proxy_configs
                 SET name = ?2, proxy_url = ?3, proxy_username = ?4,
                     proxy_password = ?5, status = ?6, updated_at_ms = ?7
                 WHERE proxy_config_id = ?1",
                params![
                    proxy_id,
                    &proxy.name,
                    &proxy.proxy_url,
                    &proxy.proxy_username,
                    &proxy.proxy_password,
                    &proxy.status,
                    proxy.updated_at,
                ],
            )
            .context("patch admin proxy config")?;
        Ok(Some(proxy))
    }

    /// Delete one reusable proxy config.
    pub fn delete_admin_proxy_config(
        &self,
        proxy_id: &str,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        let Some(proxy) = self.get_admin_proxy_config(proxy_id)? else {
            return Ok(None);
        };
        self.conn
            .execute("DELETE FROM llm_proxy_configs WHERE proxy_config_id = ?1", [proxy_id])
            .context("delete admin proxy config")?;
        Ok(Some(proxy))
    }

    /// List effective provider-level proxy bindings.
    pub fn list_admin_proxy_bindings(&self) -> anyhow::Result<Vec<AdminProxyBinding>> {
        [core_store::PROVIDER_CODEX, core_store::PROVIDER_KIRO]
            .into_iter()
            .map(|provider_type| self.load_admin_proxy_binding(provider_type))
            .collect()
    }

    /// Update or clear one provider-level proxy binding.
    pub fn update_admin_proxy_binding(
        &self,
        provider_type: &str,
        proxy_config_id: Option<String>,
    ) -> anyhow::Result<AdminProxyBinding> {
        match proxy_config_id {
            Some(proxy_config_id) => {
                self.conn
                    .execute(
                        "INSERT INTO llm_proxy_bindings (
                            provider_type, proxy_config_id, updated_at_ms
                        ) VALUES (?1, ?2, ?3)
                        ON CONFLICT(provider_type) DO UPDATE SET
                            proxy_config_id = excluded.proxy_config_id,
                            updated_at_ms = excluded.updated_at_ms",
                        params![provider_type, &proxy_config_id, now_ms()],
                    )
                    .context("upsert admin proxy binding")?;
            },
            None => {
                self.conn
                    .execute("DELETE FROM llm_proxy_bindings WHERE provider_type = ?1", [
                        provider_type,
                    ])
                    .context("delete admin proxy binding")?;
            },
        }
        self.load_admin_proxy_binding(provider_type)
    }

    /// Import legacy proxy fields embedded in Kiro auth JSON into shared proxy
    /// configs.
    pub fn import_legacy_kiro_proxy_configs(
        &self,
    ) -> anyhow::Result<AdminLegacyKiroProxyMigration> {
        let mut tuples_to_accounts =
            BTreeMap::<(String, Option<String>, Option<String>), Vec<KiroAccountRecord>>::new();
        for account in self.list_kiro_accounts()? {
            let auth_json = serde_json::from_str::<serde_json::Value>(&account.auth_json)
                .context("parse kiro auth json for legacy proxy migration")?;
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
        for proxy in self.list_admin_proxy_configs()? {
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
                let proxy = NewAdminProxyConfig {
                    id: self.generate_legacy_proxy_id(index)?,
                    name: format!("legacy-kiro-{}", index + 1),
                    proxy_url: tuple.0.clone(),
                    proxy_username: tuple.1.clone(),
                    proxy_password: tuple.2.clone(),
                    created_at_ms: now,
                };
                let created = self.create_admin_proxy_config(&proxy)?;
                existing_by_tuple.insert(tuple.clone(), created.clone());
                created_configs.push(created.clone());
                created
            };

            accounts.sort_by_cached_key(|account| account.account_name.to_ascii_lowercase());
            for mut account in accounts {
                account.proxy_config_id = Some(proxy.id.clone());
                account.updated_at_ms = now_ms();
                account.auth_json = clear_legacy_kiro_proxy_json(&account.auth_json, &proxy.id)?;
                self.upsert_kiro_account(&account)?;
                migrated_account_names.push(account.account_name);
            }
        }
        migrated_account_names.sort();
        migrated_account_names.dedup();
        Ok(AdminLegacyKiroProxyMigration {
            created_configs,
            reused_configs,
            migrated_account_names,
        })
    }

    fn generate_legacy_proxy_id(&self, index: usize) -> anyhow::Result<String> {
        let base = format!("llm-proxy-legacy-{}-{}", now_ms(), index + 1);
        if self.get_admin_proxy_config(&base)?.is_none() {
            return Ok(base);
        }
        for suffix in 1..1000 {
            let id = format!("{base}-{suffix}");
            if self.get_admin_proxy_config(&id)?.is_none() {
                return Ok(id);
            }
        }
        anyhow::bail!("failed to allocate legacy proxy config id")
    }

    fn load_admin_proxy_binding(&self, provider_type: &str) -> anyhow::Result<AdminProxyBinding> {
        let binding = self
            .conn
            .query_row(
                "SELECT provider_type, proxy_config_id, updated_at_ms
                 FROM llm_proxy_bindings
                 WHERE provider_type = ?1",
                [provider_type],
                |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?))
                },
            )
            .optional()
            .context("load proxy binding row")?;
        let Some((provider_type, proxy_config_id, updated_at_ms)) = binding else {
            return Ok(core_store::default_proxy_bindings()
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
        let proxy = self.get_admin_proxy_config(&proxy_config_id)?;
        let Some(proxy) = proxy else {
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

    fn resolve_provider_proxy_config(
        &self,
        provider_type: &str,
        proxy_mode: &str,
        proxy_config_id: Option<&str>,
    ) -> anyhow::Result<Option<ProviderProxyConfig>> {
        match proxy_mode {
            "none" | "direct" => Ok(None),
            "fixed" => {
                let Some(proxy_id) = proxy_config_id else {
                    anyhow::bail!("fixed proxy mode requires proxy_config_id");
                };
                let Some(proxy) = self.get_admin_proxy_config(proxy_id)? else {
                    anyhow::bail!("fixed proxy config `{proxy_id}` is missing");
                };
                if proxy.status != core_store::KEY_STATUS_ACTIVE {
                    anyhow::bail!("fixed proxy config `{}` is disabled", proxy.name);
                }
                Ok(Some(provider_proxy_from_admin_proxy(proxy)))
            },
            _ => {
                let binding = self.load_admin_proxy_binding(provider_type)?;
                if let Some(message) = binding.error_message {
                    anyhow::bail!("provider proxy binding is invalid: {message}");
                }
                match binding.effective_proxy_url {
                    Some(proxy_url) => Ok(Some(ProviderProxyConfig {
                        proxy_url,
                        proxy_username: binding.effective_proxy_username,
                        proxy_password: binding.effective_proxy_password,
                    })),
                    None => Ok(None),
                }
            },
        }
    }

    /// List imported Codex accounts for the admin UI.
    pub fn list_admin_codex_accounts(&self) -> anyhow::Result<Vec<AdminCodexAccount>> {
        let records = self.list_codex_accounts()?;
        records
            .iter()
            .map(|record| self.admin_codex_account_from_record(record))
            .collect()
    }

    /// Import one Codex account.
    pub fn create_admin_codex_account(
        &self,
        account: &NewAdminCodexAccount,
    ) -> anyhow::Result<AdminCodexAccount> {
        let settings = CodexAccountSettings {
            map_gpt53_codex_to_spark: account.map_gpt53_codex_to_spark,
            ..CodexAccountSettings::default()
        };
        let record = CodexAccountRecord {
            account_name: account.name.clone(),
            account_id: account.account_id.clone(),
            email: None,
            status: core_store::KEY_STATUS_ACTIVE.to_string(),
            auth_json: account.auth_json.clone(),
            settings_json: serde_json::to_string(&settings).context("serialize codex settings")?,
            last_refresh_at_ms: Some(account.created_at_ms),
            last_error: None,
            created_at_ms: account.created_at_ms,
            updated_at_ms: account.created_at_ms,
        };
        self.upsert_codex_account(&record)?;
        self.get_codex_account(&account.name)?
            .map(|record| self.admin_codex_account_from_record(&record))
            .transpose()?
            .context("created codex account disappeared")
    }

    /// Patch one imported Codex account.
    pub fn patch_admin_codex_account(
        &self,
        name: &str,
        patch: &AdminCodexAccountPatch,
    ) -> anyhow::Result<Option<AdminCodexAccount>> {
        let Some(mut record) = self.get_codex_account(name)? else {
            return Ok(None);
        };
        let mut settings = decode_codex_account_settings(&record.settings_json)?;
        if let Some(value) = patch.map_gpt53_codex_to_spark {
            settings.map_gpt53_codex_to_spark = value;
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
            serde_json::to_string(&settings).context("serialize codex settings")?;
        record.updated_at_ms = patch.updated_at_ms;
        self.upsert_codex_account(&record)?;
        Ok(Some(self.admin_codex_account_from_record(&record)?))
    }

    /// Delete one imported Codex account.
    pub fn delete_admin_codex_account(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<AdminCodexAccount>> {
        let Some(record) = self.get_codex_account(name)? else {
            return Ok(None);
        };
        self.conn
            .execute("DELETE FROM llm_codex_accounts WHERE account_name = ?1", [name])
            .context("delete admin codex account")?;
        Ok(Some(self.admin_codex_account_from_record(&record)?))
    }

    /// Mark one Codex account as refreshed.
    pub fn refresh_admin_codex_account(
        &self,
        name: &str,
        refreshed_at_ms: i64,
    ) -> anyhow::Result<Option<AdminCodexAccount>> {
        let Some(mut record) = self.get_codex_account(name)? else {
            return Ok(None);
        };
        record.last_refresh_at_ms = Some(refreshed_at_ms);
        record.last_error = None;
        record.updated_at_ms = refreshed_at_ms;
        self.upsert_codex_account(&record)?;
        Ok(Some(self.admin_codex_account_from_record(&record)?))
    }

    /// Persist refreshed Codex credential fields without changing settings.
    pub fn save_codex_auth_update(&self, update: &ProviderCodexAuthUpdate) -> anyhow::Result<()> {
        let Some(mut record) = self.get_codex_account(&update.account_name)? else {
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
        self.upsert_codex_account(&record)
    }

    /// Resolve one Codex account as a provider route for admin refresh.
    pub fn resolve_admin_codex_account_route(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        let Some(record) = self.get_codex_account(name)? else {
            return Ok(None);
        };
        if record.status != core_store::KEY_STATUS_ACTIVE {
            return Ok(None);
        }
        let settings = decode_codex_account_settings(&record.settings_json)?;
        let proxy = self.resolve_provider_proxy_config(
            core_store::PROVIDER_CODEX,
            &settings.proxy_mode,
            settings.proxy_config_id.as_deref(),
        )?;
        Ok(Some(ProviderCodexRoute {
            account_name: record.account_name,
            account_group_id_at_event: None,
            route_strategy_at_event: RouteStrategy::Auto,
            auth_json: record.auth_json,
            map_gpt53_codex_to_spark: settings.map_gpt53_codex_to_spark,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            account_request_max_concurrency: settings.request_max_concurrency,
            account_request_min_start_interval_ms: settings.request_min_start_interval_ms,
            proxy,
        }))
    }

    fn get_codex_account(&self, name: &str) -> anyhow::Result<Option<CodexAccountRecord>> {
        self.conn
            .query_row(
                "SELECT
                    account_name, account_id, email, status, auth_json, settings_json,
                    last_refresh_at_ms, last_error, created_at_ms, updated_at_ms
                 FROM llm_codex_accounts
                 WHERE account_name = ?1",
                [name],
                decode_codex_account,
            )
            .optional()
            .context("load codex account")
    }

    fn admin_codex_account_from_record(
        &self,
        record: &CodexAccountRecord,
    ) -> anyhow::Result<AdminCodexAccount> {
        let settings = decode_codex_account_settings(&record.settings_json)?;
        let (effective_proxy_source, effective_proxy_url, effective_proxy_config_name) =
            self.resolve_codex_account_proxy_view(&settings)?;
        Ok(AdminCodexAccount {
            name: record.account_name.clone(),
            status: record.status.clone(),
            account_id: record.account_id.clone(),
            plan_type: None,
            primary_remaining_percent: None,
            secondary_remaining_percent: None,
            map_gpt53_codex_to_spark: settings.map_gpt53_codex_to_spark,
            request_max_concurrency: settings.request_max_concurrency,
            request_min_start_interval_ms: settings.request_min_start_interval_ms,
            proxy_mode: settings.proxy_mode,
            proxy_config_id: settings.proxy_config_id,
            effective_proxy_source,
            effective_proxy_url,
            effective_proxy_config_name,
            last_refresh: record.last_refresh_at_ms,
            last_usage_checked_at: None,
            last_usage_success_at: None,
            usage_error_message: record.last_error.clone(),
        })
    }

    fn resolve_codex_account_proxy_view(
        &self,
        settings: &CodexAccountSettings,
    ) -> anyhow::Result<(String, Option<String>, Option<String>)> {
        match settings.proxy_mode.as_str() {
            "none" => Ok(("none".to_string(), None, None)),
            "fixed" => {
                let Some(proxy_id) = settings.proxy_config_id.as_deref() else {
                    return Ok(("invalid".to_string(), None, None));
                };
                match self.get_admin_proxy_config(proxy_id)? {
                    Some(proxy) if proxy.status == core_store::KEY_STATUS_ACTIVE => {
                        Ok(("fixed".to_string(), Some(proxy.proxy_url), Some(proxy.name)))
                    },
                    Some(proxy) => Ok(("invalid".to_string(), None, Some(proxy.name))),
                    None => Ok(("invalid".to_string(), None, None)),
                }
            },
            _ => {
                let binding = self.load_admin_proxy_binding(core_store::PROVIDER_CODEX)?;
                Ok((
                    binding.effective_source,
                    binding.effective_proxy_url,
                    binding.effective_proxy_config_name,
                ))
            },
        }
    }

    /// List persisted Kiro accounts for the admin UI.
    pub fn list_admin_kiro_accounts(&self) -> anyhow::Result<Vec<AdminKiroAccount>> {
        let records = self.list_kiro_accounts()?;
        records
            .iter()
            .map(|record| self.admin_kiro_account_from_record(record))
            .collect()
    }

    /// Create or replace one Kiro account.
    pub fn create_admin_kiro_account(
        &self,
        account: &NewAdminKiroAccount,
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
        self.upsert_kiro_account(&record)?;
        self.get_kiro_account(&account.name)?
            .map(|record| self.admin_kiro_account_from_record(&record))
            .transpose()?
            .context("created kiro account disappeared")
    }

    /// Patch mutable Kiro account routing/scheduler settings.
    pub fn patch_admin_kiro_account(
        &self,
        name: &str,
        patch: &AdminKiroAccountPatch,
    ) -> anyhow::Result<Option<AdminKiroAccount>> {
        let Some(mut record) = self.get_kiro_account(name)? else {
            return Ok(None);
        };
        let mut auth_value = serde_json::from_str::<serde_json::Value>(&record.auth_json)
            .context("parse kiro auth json for patch")?;
        let object = auth_value
            .as_object_mut()
            .context("kiro auth json must be an object")?;
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
            serde_json::to_string(&auth_value).context("serialize patched kiro auth json")?;
        record.updated_at_ms = patch.updated_at_ms;
        self.upsert_kiro_account(&record)?;
        Ok(Some(self.admin_kiro_account_from_record(&record)?))
    }

    /// Delete one persisted Kiro account.
    pub fn delete_admin_kiro_account(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<AdminKiroAccount>> {
        let Some(record) = self.get_kiro_account(name)? else {
            return Ok(None);
        };
        let view = self.admin_kiro_account_from_record(&record)?;
        self.conn
            .execute("DELETE FROM llm_kiro_accounts WHERE account_name = ?1", [name])
            .context("delete kiro account")?;
        Ok(Some(view))
    }

    /// Return cached Kiro account balance when available.
    pub fn get_admin_kiro_balance(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<AdminKiroBalanceView>> {
        let Some((balance, _cache)) = self.get_kiro_cached_status_parts(name)? else {
            return Ok(None);
        };
        Ok(balance)
    }

    /// Resolve one Kiro account as a provider route for admin balance refresh.
    pub fn resolve_admin_kiro_account_route(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<core_store::ProviderKiroRoute>> {
        let Some(record) = self.get_kiro_account(account_name)? else {
            return Ok(None);
        };
        if record.status != core_store::KEY_STATUS_ACTIVE {
            return Ok(None);
        }
        let runtime_config = self.get_runtime_config_or_default()?;
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
        let cached_status = self.get_kiro_cached_status_parts(&record.account_name)?;
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
        let proxy = self.resolve_provider_proxy_config(
            core_store::PROVIDER_KIRO,
            &proxy_mode,
            proxy_config_id.as_deref(),
        )?;
        Ok(Some(core_store::ProviderKiroRoute {
            account_name: record.account_name,
            account_group_id_at_event: None,
            route_strategy_at_event: RouteStrategy::Auto,
            auth_json: record.auth_json,
            profile_arn,
            api_region,
            request_validation_enabled: true,
            cache_estimation_enabled: true,
            zero_cache_debug_enabled: false,
            model_name_map_json: "{}".to_string(),
            cache_kmodels_json: runtime_config.kiro_cache_kmodels_json,
            cache_policy_json: runtime_config.kiro_cache_policy_json,
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

    /// Persist one Kiro status-cache update.
    pub fn save_admin_kiro_status_cache(
        &self,
        update: &AdminKiroStatusCacheUpdate,
    ) -> anyhow::Result<()> {
        self.conn
            .execute(
                "INSERT INTO llm_kiro_status_cache (
                    account_name, status, balance_json, cache_json, refreshed_at_ms,
                    expires_at_ms, last_error
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(account_name) DO UPDATE SET
                    status = excluded.status,
                    balance_json = excluded.balance_json,
                    cache_json = excluded.cache_json,
                    refreshed_at_ms = excluded.refreshed_at_ms,
                    expires_at_ms = excluded.expires_at_ms,
                    last_error = excluded.last_error",
                params![
                    &update.account_name,
                    &update.cache.status,
                    serde_json::to_string(&update.balance).context("encode kiro balance cache")?,
                    serde_json::to_string(&update.cache).context("encode kiro cache view")?,
                    update.refreshed_at_ms,
                    update.expires_at_ms,
                    &update.last_error,
                ],
            )
            .context("upsert kiro status cache")?;
        Ok(())
    }

    /// Mark one Kiro account as quota-exhausted after the hot provider path
    /// receives an authoritative upstream quota response.
    pub fn mark_kiro_account_quota_exhausted(
        &self,
        account_name: &str,
        error_message: &str,
        checked_at_ms: i64,
    ) -> anyhow::Result<()> {
        let refresh_interval_seconds = self
            .get_runtime_config_or_default()?
            .kiro_status_refresh_max_interval_seconds
            .max(0) as u64;
        let update = AdminKiroStatusCacheUpdate {
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
        };
        self.save_admin_kiro_status_cache(&update)
    }

    fn admin_kiro_account_from_record(
        &self,
        record: &KiroAccountRecord,
    ) -> anyhow::Result<AdminKiroAccount> {
        let auth = serde_json::from_str::<serde_json::Value>(&record.auth_json)
            .context("parse kiro auth json for admin view")?;
        let (balance, cache) = self
            .get_kiro_cached_status_parts(&record.account_name)?
            .unwrap_or_else(|| {
                let refresh_interval_seconds = self
                    .get_runtime_config_or_default()
                    .map(|config| config.kiro_status_refresh_max_interval_seconds.max(0) as u64)
                    .unwrap_or(core_store::DEFAULT_KIRO_STATUS_REFRESH_MAX_INTERVAL_SECONDS);
                (None, AdminKiroCacheView {
                    refresh_interval_seconds,
                    ..AdminKiroCacheView::default()
                })
            });
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
        let (effective_proxy_source, effective_proxy_url, effective_proxy_config_name) =
            self.resolve_kiro_account_proxy_view(&proxy_mode, proxy_config_id.as_deref())?;
        let disabled_json = optional_json_bool_any(&auth, &["disabled"]).unwrap_or(false);
        let disabled = disabled_json || record.status != core_store::KEY_STATUS_ACTIVE;
        let disabled_reason =
            optional_json_string_any(&auth, &["disabledReason", "disabled_reason"])
                .or_else(|| record.last_error.clone());
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

    fn get_kiro_cached_status_parts(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<(Option<AdminKiroBalanceView>, AdminKiroCacheView)>> {
        self.conn
            .query_row(
                "SELECT balance_json, cache_json
                 FROM llm_kiro_status_cache
                 WHERE account_name = ?1",
                [account_name],
                |row| {
                    let balance_json: String = row.get(0)?;
                    let cache_json: String = row.get(1)?;
                    Ok((balance_json, cache_json))
                },
            )
            .optional()
            .context("load kiro cached status")?
            .map(|(balance_json, cache_json)| {
                let balance = serde_json::from_str::<Option<AdminKiroBalanceView>>(&balance_json)
                    .context("decode kiro cached balance")?;
                let cache = serde_json::from_str::<AdminKiroCacheView>(&cache_json)
                    .context("decode kiro cached cache view")?;
                Ok((balance, cache))
            })
            .transpose()
    }

    fn resolve_kiro_account_proxy_view(
        &self,
        proxy_mode: &str,
        proxy_config_id: Option<&str>,
    ) -> anyhow::Result<(String, Option<String>, Option<String>)> {
        match proxy_mode {
            "none" | "direct" => Ok(("none".to_string(), None, None)),
            "fixed" => {
                let Some(proxy_id) = proxy_config_id else {
                    return Ok(("invalid".to_string(), None, None));
                };
                match self.get_admin_proxy_config(proxy_id)? {
                    Some(proxy) if proxy.status == core_store::KEY_STATUS_ACTIVE => {
                        Ok(("fixed".to_string(), Some(proxy.proxy_url), Some(proxy.name)))
                    },
                    Some(proxy) => Ok(("invalid".to_string(), None, Some(proxy.name))),
                    None => Ok(("invalid".to_string(), None, None)),
                }
            },
            _ => {
                let binding = self.load_admin_proxy_binding(core_store::PROVIDER_KIRO)?;
                Ok((
                    binding.effective_source,
                    binding.effective_proxy_url,
                    binding.effective_proxy_config_name,
                ))
            },
        }
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

    /// Load the singleton runtime config row or the built-in default.
    pub fn get_runtime_config_or_default(&self) -> anyhow::Result<RuntimeConfigRecord> {
        Ok(self.get_runtime_config()?.unwrap_or_default())
    }

    /// Persist the admin-facing runtime config while preserving internal-only
    /// storage fields.
    pub fn update_admin_runtime_config(
        &self,
        config: &AdminRuntimeConfig,
    ) -> anyhow::Result<AdminRuntimeConfig> {
        let mut record = self.get_runtime_config_or_default()?;
        record.apply_admin_runtime_config(config);
        self.upsert_runtime_config(&record)?;
        Ok(record.to_admin_runtime_config())
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

    /// Load one usage-lookup key by presented secret hash.
    pub fn get_public_usage_key_by_hash(
        &self,
        key_hash: &str,
    ) -> anyhow::Result<Option<PublicUsageLookupKey>> {
        self.conn
            .query_row(
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
                 WHERE k.key_hash = ?1",
                [key_hash],
                |row| {
                    let credit_total_raw: String = row.get(10)?;
                    let usage_credit_total = credit_total_raw.parse::<f64>().map_err(|err| {
                        rusqlite::Error::FromSqlConversionFailure(10, Type::Text, Box::new(err))
                    })?;
                    Ok(PublicUsageLookupKey {
                        key_id: row.get(0)?,
                        key_name: row.get(1)?,
                        provider_type: row.get(2)?,
                        status: row.get(3)?,
                        public_visible: row.get::<_, i64>(4)? != 0,
                        quota_billable_limit: row.get::<_, i64>(5)? as u64,
                        usage_input_uncached_tokens: row.get::<_, i64>(6)? as u64,
                        usage_input_cached_tokens: row.get::<_, i64>(7)? as u64,
                        usage_output_tokens: row.get::<_, i64>(8)? as u64,
                        usage_billable_tokens: row.get::<_, i64>(9)? as u64,
                        usage_credit_total,
                        usage_credit_missing_events: row.get::<_, i64>(11)? as u64,
                        last_used_at_ms: row.get(12)?,
                    })
                },
            )
            .optional()
            .context("load public usage key by hash")
    }

    /// List issued account contributions for the public thank-you wall.
    pub fn list_public_account_contributions(
        &self,
        limit: usize,
    ) -> anyhow::Result<Vec<PublicAccountContribution>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT
                    request_id,
                    COALESCE(imported_account_name, account_name),
                    contributor_message,
                    github_id,
                    processed_at_ms
                 FROM llm_account_contribution_requests
                 WHERE status = 'issued'
                 ORDER BY COALESCE(processed_at_ms, created_at_ms) DESC
                 LIMIT ?1",
            )
            .context("prepare list public account contributions")?;
        let rows = stmt
            .query_map([limit.max(1) as i64], |row| {
                Ok(PublicAccountContribution {
                    request_id: row.get(0)?,
                    account_name: row.get(1)?,
                    contributor_message: row.get(2)?,
                    github_id: row.get(3)?,
                    processed_at_ms: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .context("list public account contributions")?;
        Ok(rows)
    }

    /// List approved sponsors for the public thank-you wall.
    pub fn list_public_sponsors(&self, limit: usize) -> anyhow::Result<Vec<PublicSponsor>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT
                    request_id,
                    display_name,
                    sponsor_message,
                    github_id,
                    processed_at_ms
                 FROM llm_sponsor_requests
                 WHERE status = 'approved'
                 ORDER BY COALESCE(processed_at_ms, created_at_ms) DESC
                 LIMIT ?1",
            )
            .context("prepare list public sponsors")?;
        let rows = stmt
            .query_map([limit.max(1) as i64], |row| {
                Ok(PublicSponsor {
                    request_id: row.get(0)?,
                    display_name: row.get(1)?,
                    sponsor_message: row.get(2)?,
                    github_id: row.get(3)?,
                    processed_at_ms: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .context("list public sponsors")?;
        Ok(rows)
    }

    /// List token requests for admin review.
    pub fn list_admin_token_requests(
        &self,
        query: &AdminReviewQueueQuery,
    ) -> anyhow::Result<AdminTokenRequestsPage> {
        let total = self.count_admin_token_requests(query.status.as_deref())?;
        if total == 0 || query.offset >= total {
            return Ok(AdminTokenRequestsPage {
                total,
                offset: query.offset,
                limit: query.limit,
                has_more: false,
                requests: Vec::new(),
            });
        }
        let requests = if let Some(status) = query.status.as_deref() {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT
                        request_id, requester_email, requested_quota_billable_limit,
                        request_reason, frontend_page_url, status, client_ip, ip_region,
                        admin_note, failure_reason, issued_key_id, issued_key_name,
                        created_at_ms, updated_at_ms, processed_at_ms
                     FROM llm_token_requests
                     WHERE status = ?1
                     ORDER BY created_at_ms DESC, request_id DESC
                     LIMIT ?2 OFFSET ?3",
                )
                .context("prepare list admin token requests by status")?;
            let rows = stmt
                .query_map(params![status, query.limit as i64, query.offset as i64], |row| {
                    Ok(AdminTokenRequest {
                        request_id: row.get(0)?,
                        requester_email: row.get(1)?,
                        requested_quota_billable_limit: row.get::<_, i64>(2)? as u64,
                        request_reason: row.get(3)?,
                        frontend_page_url: row.get(4)?,
                        status: row.get(5)?,
                        client_ip: row.get(6)?,
                        ip_region: row.get(7)?,
                        admin_note: row.get(8)?,
                        failure_reason: row.get(9)?,
                        issued_key_id: row.get(10)?,
                        issued_key_name: row.get(11)?,
                        created_at: row.get(12)?,
                        updated_at: row.get(13)?,
                        processed_at: row.get(14)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
                .context("list admin token requests by status")?;
            rows
        } else {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT
                        request_id, requester_email, requested_quota_billable_limit,
                        request_reason, frontend_page_url, status, client_ip, ip_region,
                        admin_note, failure_reason, issued_key_id, issued_key_name,
                        created_at_ms, updated_at_ms, processed_at_ms
                     FROM llm_token_requests
                     ORDER BY created_at_ms DESC, request_id DESC
                     LIMIT ?1 OFFSET ?2",
                )
                .context("prepare list admin token requests")?;
            let rows = stmt
                .query_map(params![query.limit as i64, query.offset as i64], |row| {
                    Ok(AdminTokenRequest {
                        request_id: row.get(0)?,
                        requester_email: row.get(1)?,
                        requested_quota_billable_limit: row.get::<_, i64>(2)? as u64,
                        request_reason: row.get(3)?,
                        frontend_page_url: row.get(4)?,
                        status: row.get(5)?,
                        client_ip: row.get(6)?,
                        ip_region: row.get(7)?,
                        admin_note: row.get(8)?,
                        failure_reason: row.get(9)?,
                        issued_key_id: row.get(10)?,
                        issued_key_name: row.get(11)?,
                        created_at: row.get(12)?,
                        updated_at: row.get(13)?,
                        processed_at: row.get(14)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
                .context("list admin token requests")?;
            rows
        };
        Ok(AdminTokenRequestsPage {
            total,
            offset: query.offset,
            limit: query.limit,
            has_more: query.offset.saturating_add(requests.len()) < total,
            requests,
        })
    }

    /// List account contribution requests for admin review.
    pub fn list_admin_account_contribution_requests(
        &self,
        query: &AdminReviewQueueQuery,
    ) -> anyhow::Result<AdminAccountContributionRequestsPage> {
        let total = self.count_admin_account_contribution_requests(query.status.as_deref())?;
        if total == 0 || query.offset >= total {
            return Ok(AdminAccountContributionRequestsPage {
                total,
                offset: query.offset,
                limit: query.limit,
                has_more: false,
                requests: Vec::new(),
            });
        }
        let requests = if let Some(status) = query.status.as_deref() {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT
                        request_id, account_name, account_id, id_token, access_token,
                        refresh_token, requester_email, contributor_message, github_id,
                        frontend_page_url, status, client_ip, ip_region, admin_note,
                        failure_reason, imported_account_name, issued_key_id, issued_key_name,
                        created_at_ms, updated_at_ms, processed_at_ms
                     FROM llm_account_contribution_requests
                     WHERE status = ?1
                     ORDER BY created_at_ms DESC, request_id DESC
                     LIMIT ?2 OFFSET ?3",
                )
                .context("prepare list admin account contribution requests by status")?;
            let rows = stmt
                .query_map(params![status, query.limit as i64, query.offset as i64], |row| {
                    Ok(AdminAccountContributionRequest {
                        request_id: row.get(0)?,
                        account_name: row.get(1)?,
                        account_id: row.get(2)?,
                        id_token: row.get(3)?,
                        access_token: row.get(4)?,
                        refresh_token: row.get(5)?,
                        requester_email: row.get(6)?,
                        contributor_message: row.get(7)?,
                        github_id: row.get(8)?,
                        frontend_page_url: row.get(9)?,
                        status: row.get(10)?,
                        client_ip: row.get(11)?,
                        ip_region: row.get(12)?,
                        admin_note: row.get(13)?,
                        failure_reason: row.get(14)?,
                        imported_account_name: row.get(15)?,
                        issued_key_id: row.get(16)?,
                        issued_key_name: row.get(17)?,
                        created_at: row.get(18)?,
                        updated_at: row.get(19)?,
                        processed_at: row.get(20)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
                .context("list admin account contribution requests by status")?;
            rows
        } else {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT
                        request_id, account_name, account_id, id_token, access_token,
                        refresh_token, requester_email, contributor_message, github_id,
                        frontend_page_url, status, client_ip, ip_region, admin_note,
                        failure_reason, imported_account_name, issued_key_id, issued_key_name,
                        created_at_ms, updated_at_ms, processed_at_ms
                     FROM llm_account_contribution_requests
                     ORDER BY created_at_ms DESC, request_id DESC
                     LIMIT ?1 OFFSET ?2",
                )
                .context("prepare list admin account contribution requests")?;
            let rows = stmt
                .query_map(params![query.limit as i64, query.offset as i64], |row| {
                    Ok(AdminAccountContributionRequest {
                        request_id: row.get(0)?,
                        account_name: row.get(1)?,
                        account_id: row.get(2)?,
                        id_token: row.get(3)?,
                        access_token: row.get(4)?,
                        refresh_token: row.get(5)?,
                        requester_email: row.get(6)?,
                        contributor_message: row.get(7)?,
                        github_id: row.get(8)?,
                        frontend_page_url: row.get(9)?,
                        status: row.get(10)?,
                        client_ip: row.get(11)?,
                        ip_region: row.get(12)?,
                        admin_note: row.get(13)?,
                        failure_reason: row.get(14)?,
                        imported_account_name: row.get(15)?,
                        issued_key_id: row.get(16)?,
                        issued_key_name: row.get(17)?,
                        created_at: row.get(18)?,
                        updated_at: row.get(19)?,
                        processed_at: row.get(20)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
                .context("list admin account contribution requests")?;
            rows
        };
        Ok(AdminAccountContributionRequestsPage {
            total,
            offset: query.offset,
            limit: query.limit,
            has_more: query.offset.saturating_add(requests.len()) < total,
            requests,
        })
    }

    /// List sponsor requests for admin review.
    pub fn list_admin_sponsor_requests(
        &self,
        query: &AdminReviewQueueQuery,
    ) -> anyhow::Result<AdminSponsorRequestsPage> {
        let total = self.count_admin_sponsor_requests(query.status.as_deref())?;
        if total == 0 || query.offset >= total {
            return Ok(AdminSponsorRequestsPage {
                total,
                offset: query.offset,
                limit: query.limit,
                has_more: false,
                requests: Vec::new(),
            });
        }
        let requests = if let Some(status) = query.status.as_deref() {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT
                        request_id, requester_email, sponsor_message, display_name, github_id,
                        frontend_page_url, status, client_ip, ip_region, admin_note,
                        failure_reason, payment_email_sent_at_ms, created_at_ms, updated_at_ms,
                        processed_at_ms
                     FROM llm_sponsor_requests
                     WHERE status = ?1
                     ORDER BY created_at_ms DESC, request_id DESC
                     LIMIT ?2 OFFSET ?3",
                )
                .context("prepare list admin sponsor requests by status")?;
            let rows = stmt
                .query_map(params![status, query.limit as i64, query.offset as i64], |row| {
                    Ok(AdminSponsorRequest {
                        request_id: row.get(0)?,
                        requester_email: row.get(1)?,
                        sponsor_message: row.get(2)?,
                        display_name: row.get(3)?,
                        github_id: row.get(4)?,
                        frontend_page_url: row.get(5)?,
                        status: row.get(6)?,
                        client_ip: row.get(7)?,
                        ip_region: row.get(8)?,
                        admin_note: row.get(9)?,
                        failure_reason: row.get(10)?,
                        payment_email_sent_at: row.get(11)?,
                        created_at: row.get(12)?,
                        updated_at: row.get(13)?,
                        processed_at: row.get(14)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
                .context("list admin sponsor requests by status")?;
            rows
        } else {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT
                        request_id, requester_email, sponsor_message, display_name, github_id,
                        frontend_page_url, status, client_ip, ip_region, admin_note,
                        failure_reason, payment_email_sent_at_ms, created_at_ms, updated_at_ms,
                        processed_at_ms
                     FROM llm_sponsor_requests
                     ORDER BY created_at_ms DESC, request_id DESC
                     LIMIT ?1 OFFSET ?2",
                )
                .context("prepare list admin sponsor requests")?;
            let rows = stmt
                .query_map(params![query.limit as i64, query.offset as i64], |row| {
                    Ok(AdminSponsorRequest {
                        request_id: row.get(0)?,
                        requester_email: row.get(1)?,
                        sponsor_message: row.get(2)?,
                        display_name: row.get(3)?,
                        github_id: row.get(4)?,
                        frontend_page_url: row.get(5)?,
                        status: row.get(6)?,
                        client_ip: row.get(7)?,
                        ip_region: row.get(8)?,
                        admin_note: row.get(9)?,
                        failure_reason: row.get(10)?,
                        payment_email_sent_at: row.get(11)?,
                        created_at: row.get(12)?,
                        updated_at: row.get(13)?,
                        processed_at: row.get(14)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
                .context("list admin sponsor requests")?;
            rows
        };
        Ok(AdminSponsorRequestsPage {
            total,
            offset: query.offset,
            limit: query.limit,
            has_more: query.offset.saturating_add(requests.len()) < total,
            requests,
        })
    }

    fn count_admin_token_requests(&self, status: Option<&str>) -> anyhow::Result<usize> {
        count_review_rows(
            &self.conn,
            "SELECT COUNT(*) FROM llm_token_requests",
            "SELECT COUNT(*) FROM llm_token_requests WHERE status = ?1",
            status,
        )
    }

    fn count_admin_account_contribution_requests(
        &self,
        status: Option<&str>,
    ) -> anyhow::Result<usize> {
        count_review_rows(
            &self.conn,
            "SELECT COUNT(*) FROM llm_account_contribution_requests",
            "SELECT COUNT(*) FROM llm_account_contribution_requests WHERE status = ?1",
            status,
        )
    }

    fn count_admin_sponsor_requests(&self, status: Option<&str>) -> anyhow::Result<usize> {
        count_review_rows(
            &self.conn,
            "SELECT COUNT(*) FROM llm_sponsor_requests",
            "SELECT COUNT(*) FROM llm_sponsor_requests WHERE status = ?1",
            status,
        )
    }

    /// Issue a token request and create its key if supplied.
    pub fn issue_admin_token_request(
        &self,
        request_id: &str,
        key: Option<&NewAdminKey>,
        action: &AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminTokenRequest>> {
        let Some(current) = self.get_admin_token_request(request_id)? else {
            return Ok(None);
        };
        let (issued_key_id, issued_key_name) = match (current.issued_key_id, key) {
            (Some(id), _) => (Some(id), current.issued_key_name),
            (None, Some(key)) => {
                self.create_admin_key(key)?;
                (Some(key.id.clone()), Some(key.name.clone()))
            },
            (None, None) => (None, None),
        };
        self.conn
            .execute(
                "UPDATE llm_token_requests
                 SET status = 'issued',
                     admin_note = ?2,
                     failure_reason = NULL,
                     issued_key_id = ?3,
                     issued_key_name = ?4,
                     updated_at_ms = ?5,
                     processed_at_ms = ?5
                 WHERE request_id = ?1",
                params![
                    request_id,
                    &action.admin_note,
                    &issued_key_id,
                    &issued_key_name,
                    action.updated_at_ms,
                ],
            )
            .context("issue admin token request")?;
        self.get_admin_token_request(request_id)
    }

    /// Reject a token request and disable any partially issued key.
    pub fn reject_admin_token_request(
        &self,
        request_id: &str,
        action: &AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminTokenRequest>> {
        let Some(current) = self.get_admin_token_request(request_id)? else {
            return Ok(None);
        };
        if let Some(key_id) = current.issued_key_id.as_deref() {
            self.disable_admin_key_if_present(key_id, action.updated_at_ms)?;
        }
        self.conn
            .execute(
                "UPDATE llm_token_requests
                 SET status = 'rejected',
                     admin_note = ?2,
                     failure_reason = NULL,
                     updated_at_ms = ?3,
                     processed_at_ms = ?3
                 WHERE request_id = ?1",
                params![request_id, &action.admin_note, action.updated_at_ms],
            )
            .context("reject admin token request")?;
        self.get_admin_token_request(request_id)
    }

    /// Issue an account contribution request and create account, group, and key
    /// rows when supplied.
    pub fn issue_admin_account_contribution_request(
        &self,
        request_id: &str,
        account: Option<&NewAdminCodexAccount>,
        account_group: Option<&NewAdminAccountGroup>,
        key: Option<&NewAdminKey>,
        action: &AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>> {
        let Some(current) = self.get_admin_account_contribution_request(request_id)? else {
            return Ok(None);
        };
        let imported_account_name = match (current.imported_account_name, account) {
            (Some(name), _) => Some(name),
            (None, Some(account)) => {
                self.create_admin_codex_account(account)?;
                Some(account.name.clone())
            },
            (None, None) => None,
        };
        if let Some(group) = account_group {
            self.create_admin_account_group(group)?;
        }
        let (issued_key_id, issued_key_name) = match (current.issued_key_id, key) {
            (Some(id), _) => (Some(id), current.issued_key_name),
            (None, Some(key)) => {
                self.create_admin_key(key)?;
                if let Some(group) = account_group {
                    self.patch_admin_key(&key.id, &AdminKeyPatch {
                        name: None,
                        status: None,
                        public_visible: None,
                        quota_billable_limit: None,
                        route_strategy: Some(Some("fixed".to_string())),
                        account_group_id: Some(Some(group.id.clone())),
                        fixed_account_name: None,
                        auto_account_names: None,
                        model_name_map: None,
                        request_max_concurrency: None,
                        request_min_start_interval_ms: None,
                        updated_at_ms: action.updated_at_ms,
                        ..AdminKeyPatch::default()
                    })?;
                }
                (Some(key.id.clone()), Some(key.name.clone()))
            },
            (None, None) => (None, None),
        };
        self.conn
            .execute(
                "UPDATE llm_account_contribution_requests
                 SET status = 'issued',
                     admin_note = ?2,
                     failure_reason = NULL,
                     imported_account_name = ?3,
                     issued_key_id = ?4,
                     issued_key_name = ?5,
                     updated_at_ms = ?6,
                     processed_at_ms = ?6
                 WHERE request_id = ?1",
                params![
                    request_id,
                    &action.admin_note,
                    &imported_account_name,
                    &issued_key_id,
                    &issued_key_name,
                    action.updated_at_ms,
                ],
            )
            .context("issue admin account contribution request")?;
        self.get_admin_account_contribution_request(request_id)
    }

    /// Reject an account contribution request and clean up partial records.
    pub fn reject_admin_account_contribution_request(
        &self,
        request_id: &str,
        action: &AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>> {
        let Some(current) = self.get_admin_account_contribution_request(request_id)? else {
            return Ok(None);
        };
        if let Some(key_id) = current.issued_key_id.as_deref() {
            self.disable_admin_key_if_present(key_id, action.updated_at_ms)?;
        }
        if let Some(account_name) = current.imported_account_name.as_deref() {
            self.delete_admin_codex_account(account_name)?;
        }
        self.conn
            .execute(
                "UPDATE llm_account_contribution_requests
                 SET status = 'rejected',
                     admin_note = ?2,
                     failure_reason = NULL,
                     updated_at_ms = ?3,
                     processed_at_ms = ?3
                 WHERE request_id = ?1",
                params![request_id, &action.admin_note, action.updated_at_ms],
            )
            .context("reject admin account contribution request")?;
        self.get_admin_account_contribution_request(request_id)
    }

    /// Approve one sponsor request.
    pub fn approve_admin_sponsor_request(
        &self,
        request_id: &str,
        action: &AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminSponsorRequest>> {
        if self.get_admin_sponsor_request(request_id)?.is_none() {
            return Ok(None);
        }
        self.conn
            .execute(
                "UPDATE llm_sponsor_requests
                 SET status = 'approved',
                     admin_note = ?2,
                     failure_reason = NULL,
                     updated_at_ms = ?3,
                     processed_at_ms = ?3
                 WHERE request_id = ?1",
                params![request_id, &action.admin_note, action.updated_at_ms],
            )
            .context("approve admin sponsor request")?;
        self.get_admin_sponsor_request(request_id)
    }

    /// Delete one sponsor request.
    pub fn delete_admin_sponsor_request(&self, request_id: &str) -> anyhow::Result<bool> {
        let changed = self
            .conn
            .execute("DELETE FROM llm_sponsor_requests WHERE request_id = ?1", [request_id])
            .context("delete admin sponsor request")?;
        Ok(changed > 0)
    }

    fn disable_admin_key_if_present(&self, key_id: &str, updated_at_ms: i64) -> anyhow::Result<()> {
        if self.get_key(key_id)?.is_some() {
            self.patch_admin_key(key_id, &AdminKeyPatch {
                name: None,
                status: Some(core_store::KEY_STATUS_DISABLED.to_string()),
                public_visible: None,
                quota_billable_limit: None,
                route_strategy: None,
                account_group_id: None,
                fixed_account_name: None,
                auto_account_names: None,
                model_name_map: None,
                request_max_concurrency: None,
                request_min_start_interval_ms: None,
                updated_at_ms,
                ..AdminKeyPatch::default()
            })?;
        }
        Ok(())
    }

    /// Load one token request by id.
    pub fn get_admin_token_request(
        &self,
        request_id: &str,
    ) -> anyhow::Result<Option<AdminTokenRequest>> {
        self.conn
            .query_row(
                "SELECT
                    request_id, requester_email, requested_quota_billable_limit,
                    request_reason, frontend_page_url, status, client_ip, ip_region,
                    admin_note, failure_reason, issued_key_id, issued_key_name,
                    created_at_ms, updated_at_ms, processed_at_ms
                 FROM llm_token_requests
                 WHERE request_id = ?1",
                [request_id],
                |row| {
                    Ok(AdminTokenRequest {
                        request_id: row.get(0)?,
                        requester_email: row.get(1)?,
                        requested_quota_billable_limit: row.get::<_, i64>(2)? as u64,
                        request_reason: row.get(3)?,
                        frontend_page_url: row.get(4)?,
                        status: row.get(5)?,
                        client_ip: row.get(6)?,
                        ip_region: row.get(7)?,
                        admin_note: row.get(8)?,
                        failure_reason: row.get(9)?,
                        issued_key_id: row.get(10)?,
                        issued_key_name: row.get(11)?,
                        created_at: row.get(12)?,
                        updated_at: row.get(13)?,
                        processed_at: row.get(14)?,
                    })
                },
            )
            .optional()
            .context("load admin token request")
    }

    /// Load one account contribution request by id.
    pub fn get_admin_account_contribution_request(
        &self,
        request_id: &str,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>> {
        self.conn
            .query_row(
                "SELECT
                    request_id, account_name, account_id, id_token, access_token,
                    refresh_token, requester_email, contributor_message, github_id,
                    frontend_page_url, status, client_ip, ip_region, admin_note,
                    failure_reason, imported_account_name, issued_key_id, issued_key_name,
                    created_at_ms, updated_at_ms, processed_at_ms
                 FROM llm_account_contribution_requests
                 WHERE request_id = ?1",
                [request_id],
                |row| {
                    Ok(AdminAccountContributionRequest {
                        request_id: row.get(0)?,
                        account_name: row.get(1)?,
                        account_id: row.get(2)?,
                        id_token: row.get(3)?,
                        access_token: row.get(4)?,
                        refresh_token: row.get(5)?,
                        requester_email: row.get(6)?,
                        contributor_message: row.get(7)?,
                        github_id: row.get(8)?,
                        frontend_page_url: row.get(9)?,
                        status: row.get(10)?,
                        client_ip: row.get(11)?,
                        ip_region: row.get(12)?,
                        admin_note: row.get(13)?,
                        failure_reason: row.get(14)?,
                        imported_account_name: row.get(15)?,
                        issued_key_id: row.get(16)?,
                        issued_key_name: row.get(17)?,
                        created_at: row.get(18)?,
                        updated_at: row.get(19)?,
                        processed_at: row.get(20)?,
                    })
                },
            )
            .optional()
            .context("load admin account contribution request")
    }

    /// Load one sponsor request by id.
    pub fn get_admin_sponsor_request(
        &self,
        request_id: &str,
    ) -> anyhow::Result<Option<AdminSponsorRequest>> {
        self.conn
            .query_row(
                "SELECT
                    request_id, requester_email, sponsor_message, display_name, github_id,
                    frontend_page_url, status, client_ip, ip_region, admin_note,
                    failure_reason, payment_email_sent_at_ms, created_at_ms, updated_at_ms,
                    processed_at_ms
                 FROM llm_sponsor_requests
                 WHERE request_id = ?1",
                [request_id],
                |row| {
                    Ok(AdminSponsorRequest {
                        request_id: row.get(0)?,
                        requester_email: row.get(1)?,
                        sponsor_message: row.get(2)?,
                        display_name: row.get(3)?,
                        github_id: row.get(4)?,
                        frontend_page_url: row.get(5)?,
                        status: row.get(6)?,
                        client_ip: row.get(7)?,
                        ip_region: row.get(8)?,
                        admin_note: row.get(9)?,
                        failure_reason: row.get(10)?,
                        payment_email_sent_at: row.get(11)?,
                        created_at: row.get(12)?,
                        updated_at: row.get(13)?,
                        processed_at: row.get(14)?,
                    })
                },
            )
            .optional()
            .context("load admin sponsor request")
    }

    /// Insert one public token request.
    pub fn create_public_token_request(
        &self,
        request: &NewPublicTokenRequest,
    ) -> anyhow::Result<()> {
        self.conn
            .execute(
                "INSERT INTO llm_token_requests (
                    request_id, requester_email, requested_quota_billable_limit, request_reason,
                    frontend_page_url, status, fingerprint, client_ip, ip_region, admin_note,
                    failure_reason, issued_key_id, issued_key_name, created_at_ms,
                    updated_at_ms, processed_at_ms
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, NULL, NULL, NULL, ?10, ?10, NULL
                )",
                params![
                    &request.request_id,
                    &request.requester_email,
                    request.requested_quota_billable_limit as i64,
                    &request.request_reason,
                    &request.frontend_page_url,
                    PUBLIC_TOKEN_REQUEST_STATUS_PENDING,
                    &request.fingerprint,
                    &request.client_ip,
                    &request.ip_region,
                    request.created_at_ms,
                ],
            )
            .context("create public token request")?;
        Ok(())
    }

    /// Insert one public account contribution request.
    pub fn create_public_account_contribution_request(
        &self,
        request: &NewPublicAccountContributionRequest,
    ) -> anyhow::Result<()> {
        self.conn
            .execute(
                "INSERT INTO llm_account_contribution_requests (
                    request_id, account_name, account_id, id_token, access_token, refresh_token,
                    requester_email, contributor_message, github_id, frontend_page_url, status,
                    fingerprint, client_ip, ip_region, admin_note, failure_reason,
                    imported_account_name, issued_key_id, issued_key_name, created_at_ms,
                    updated_at_ms, processed_at_ms
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                    NULL, NULL, NULL, NULL, NULL, ?15, ?15, NULL
                )",
                params![
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
                    PUBLIC_TOKEN_REQUEST_STATUS_PENDING,
                    &request.fingerprint,
                    &request.client_ip,
                    &request.ip_region,
                    request.created_at_ms,
                ],
            )
            .context("create public account contribution request")?;
        Ok(())
    }

    /// Insert one public sponsor request.
    pub fn create_public_sponsor_request(
        &self,
        request: &NewPublicSponsorRequest,
    ) -> anyhow::Result<()> {
        self.conn
            .execute(
                "INSERT INTO llm_sponsor_requests (
                    request_id, requester_email, sponsor_message, display_name, github_id,
                    frontend_page_url, status, fingerprint, client_ip, ip_region, admin_note,
                    failure_reason, payment_email_sent_at_ms, created_at_ms, updated_at_ms,
                    processed_at_ms
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, ?11, NULL, ?12, ?12, NULL
                )",
                params![
                    &request.request_id,
                    &request.requester_email,
                    &request.sponsor_message,
                    &request.display_name,
                    &request.github_id,
                    &request.frontend_page_url,
                    PUBLIC_SPONSOR_REQUEST_STATUS_SUBMITTED,
                    &request.fingerprint,
                    &request.client_ip,
                    &request.ip_region,
                    "email notifier is not configured",
                    request.created_at_ms,
                ],
            )
            .context("create public sponsor request")?;
        Ok(())
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

    /// Persist refreshed Kiro credential fields without changing scheduler or
    /// proxy settings.
    pub fn save_kiro_auth_update(&self, update: &ProviderKiroAuthUpdate) -> anyhow::Result<()> {
        let Some(mut record) = self.get_kiro_account(&update.account_name)? else {
            anyhow::bail!("kiro account `{}` is not configured", update.account_name);
        };
        record.auth_json = update.auth_json.clone();
        record.auth_method = update.auth_method.clone();
        record.account_id = update.account_id.clone();
        record.profile_arn = update.profile_arn.clone();
        record.user_id = update.user_id.clone();
        record.status = update.status.clone();
        record.last_refresh_at_ms = Some(update.refreshed_at_ms);
        record.last_error = update.last_error.clone();
        record.updated_at_ms = update.refreshed_at_ms;
        self.upsert_kiro_account(&record)
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

    fn get_kiro_account(&self, account_name: &str) -> anyhow::Result<Option<KiroAccountRecord>> {
        self.conn
            .query_row(
                "SELECT
                    account_name, auth_method, account_id, profile_arn, user_id,
                    status, auth_json, max_concurrency, min_start_interval_ms,
                    proxy_config_id, last_refresh_at_ms, last_error, created_at_ms,
                    updated_at_ms
                 FROM llm_kiro_accounts
                 WHERE account_name = ?1",
                [account_name],
                decode_kiro_account,
            )
            .optional()
            .context("load kiro account")
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
            .map(|value| value as u64),
        request_min_start_interval_ms: bundle
            .route
            .request_min_start_interval_ms
            .map(|value| value as u64),
        kiro_request_validation_enabled: bundle.route.kiro_request_validation_enabled,
        kiro_cache_estimation_enabled: bundle.route.kiro_cache_estimation_enabled,
        kiro_zero_cache_debug_enabled: bundle.route.kiro_zero_cache_debug_enabled,
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
    }
}

fn decode_admin_account_group(row: &rusqlite::Row<'_>) -> rusqlite::Result<AdminAccountGroup> {
    let account_names_json: String = row.get(3)?;
    let account_names = serde_json::from_str::<Vec<String>>(&account_names_json)
        .map_err(|err| rusqlite::Error::FromSqlConversionFailure(3, Type::Text, Box::new(err)))?;
    Ok(AdminAccountGroup {
        id: row.get(0)?,
        provider_type: row.get(1)?,
        name: row.get(2)?,
        account_names,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

fn decode_admin_proxy_config(row: &rusqlite::Row<'_>) -> rusqlite::Result<AdminProxyConfig> {
    Ok(AdminProxyConfig {
        id: row.get(0)?,
        name: row.get(1)?,
        proxy_url: row.get(2)?,
        proxy_username: row.get(3)?,
        proxy_password: row.get(4)?,
        status: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
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

fn route_strategy_from_config(route: &KeyRouteConfig) -> anyhow::Result<RouteStrategy> {
    match route.route_strategy.as_deref().unwrap_or("auto") {
        "auto" => Ok(RouteStrategy::Auto),
        "fixed" => Ok(RouteStrategy::Fixed),
        other => anyhow::bail!("unsupported route strategy `{other}`"),
    }
}

fn non_negative_i64_to_u64(value: i64) -> Option<u64> {
    u64::try_from(value.max(0)).ok()
}

fn provider_proxy_from_admin_proxy(proxy: AdminProxyConfig) -> ProviderProxyConfig {
    ProviderProxyConfig {
        proxy_url: proxy.proxy_url,
        proxy_username: proxy.proxy_username,
        proxy_password: proxy.proxy_password,
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
        .context("parse kiro auth json for legacy proxy cleanup")?;
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
    serde_json::to_string(&value).context("serialize kiro auth json after proxy cleanup")
}

fn decode_codex_account_settings(value: &str) -> anyhow::Result<CodexAccountSettings> {
    serde_json::from_str(value).context("decode codex account settings")
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

impl Default for RuntimeConfigRecord {
    fn default() -> Self {
        Self {
            id: "default".to_string(),
            auth_cache_ttl_seconds: core_store::DEFAULT_AUTH_CACHE_TTL_SECONDS as i64,
            max_request_body_bytes: core_store::DEFAULT_MAX_REQUEST_BODY_BYTES as i64,
            account_failure_retry_limit: core_store::DEFAULT_ACCOUNT_FAILURE_RETRY_LIMIT as i64,
            codex_client_version: core_store::DEFAULT_CODEX_CLIENT_VERSION.to_string(),
            kiro_channel_max_concurrency: core_store::DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY as i64,
            kiro_channel_min_start_interval_ms:
                core_store::DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS as i64,
            codex_status_refresh_min_interval_seconds:
                core_store::DEFAULT_CODEX_STATUS_REFRESH_MIN_INTERVAL_SECONDS as i64,
            codex_status_refresh_max_interval_seconds:
                core_store::DEFAULT_CODEX_STATUS_REFRESH_MAX_INTERVAL_SECONDS as i64,
            codex_status_account_jitter_max_seconds:
                core_store::DEFAULT_CODEX_STATUS_ACCOUNT_JITTER_MAX_SECONDS as i64,
            kiro_status_refresh_min_interval_seconds:
                core_store::DEFAULT_KIRO_STATUS_REFRESH_MIN_INTERVAL_SECONDS as i64,
            kiro_status_refresh_max_interval_seconds:
                core_store::DEFAULT_KIRO_STATUS_REFRESH_MAX_INTERVAL_SECONDS as i64,
            kiro_status_account_jitter_max_seconds:
                core_store::DEFAULT_KIRO_STATUS_ACCOUNT_JITTER_MAX_SECONDS as i64,
            usage_event_flush_batch_size: core_store::DEFAULT_USAGE_EVENT_FLUSH_BATCH_SIZE as i64,
            usage_event_flush_interval_seconds:
                core_store::DEFAULT_USAGE_EVENT_FLUSH_INTERVAL_SECONDS as i64,
            usage_event_flush_max_buffer_bytes:
                core_store::DEFAULT_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES as i64,
            usage_event_maintenance_enabled: core_store::DEFAULT_USAGE_EVENT_MAINTENANCE_ENABLED,
            usage_event_maintenance_interval_seconds:
                core_store::DEFAULT_USAGE_EVENT_MAINTENANCE_INTERVAL_SECONDS as i64,
            usage_event_detail_retention_days:
                core_store::DEFAULT_USAGE_EVENT_DETAIL_RETENTION_DAYS,
            kiro_cache_kmodels_json: core_store::default_kiro_cache_kmodels_json(),
            kiro_billable_model_multipliers_json:
                core_store::default_kiro_billable_model_multipliers_json(),
            kiro_cache_policy_json: core_store::default_kiro_cache_policy_json(),
            kiro_prefix_cache_mode: core_store::DEFAULT_KIRO_PREFIX_CACHE_MODE.to_string(),
            kiro_prefix_cache_max_tokens: core_store::DEFAULT_KIRO_PREFIX_CACHE_MAX_TOKENS as i64,
            kiro_prefix_cache_entry_ttl_seconds:
                core_store::DEFAULT_KIRO_PREFIX_CACHE_ENTRY_TTL_SECONDS as i64,
            kiro_conversation_anchor_max_entries:
                core_store::DEFAULT_KIRO_CONVERSATION_ANCHOR_MAX_ENTRIES as i64,
            kiro_conversation_anchor_ttl_seconds:
                core_store::DEFAULT_KIRO_CONVERSATION_ANCHOR_TTL_SECONDS as i64,
            updated_at_ms: now_ms(),
        }
    }
}

impl RuntimeConfigRecord {
    /// Convert the storage row into the admin response view.
    pub fn to_admin_runtime_config(&self) -> AdminRuntimeConfig {
        AdminRuntimeConfig {
            auth_cache_ttl_seconds: self.auth_cache_ttl_seconds as u64,
            max_request_body_bytes: self.max_request_body_bytes as u64,
            account_failure_retry_limit: self.account_failure_retry_limit as u64,
            codex_client_version: self.codex_client_version.clone(),
            codex_status_refresh_min_interval_seconds: self
                .codex_status_refresh_min_interval_seconds
                as u64,
            codex_status_refresh_max_interval_seconds: self
                .codex_status_refresh_max_interval_seconds
                as u64,
            codex_status_account_jitter_max_seconds: self.codex_status_account_jitter_max_seconds
                as u64,
            kiro_status_refresh_min_interval_seconds: self.kiro_status_refresh_min_interval_seconds
                as u64,
            kiro_status_refresh_max_interval_seconds: self.kiro_status_refresh_max_interval_seconds
                as u64,
            kiro_status_account_jitter_max_seconds: self.kiro_status_account_jitter_max_seconds
                as u64,
            usage_event_flush_batch_size: self.usage_event_flush_batch_size as u64,
            usage_event_flush_interval_seconds: self.usage_event_flush_interval_seconds as u64,
            usage_event_flush_max_buffer_bytes: self.usage_event_flush_max_buffer_bytes as u64,
            kiro_cache_kmodels_json: self.kiro_cache_kmodels_json.clone(),
            kiro_billable_model_multipliers_json: self.kiro_billable_model_multipliers_json.clone(),
            kiro_cache_policy_json: self.kiro_cache_policy_json.clone(),
            kiro_prefix_cache_mode: self.kiro_prefix_cache_mode.clone(),
            kiro_prefix_cache_max_tokens: self.kiro_prefix_cache_max_tokens as u64,
            kiro_prefix_cache_entry_ttl_seconds: self.kiro_prefix_cache_entry_ttl_seconds as u64,
            kiro_conversation_anchor_max_entries: self.kiro_conversation_anchor_max_entries as u64,
            kiro_conversation_anchor_ttl_seconds: self.kiro_conversation_anchor_ttl_seconds as u64,
        }
    }

    /// Apply the admin-visible config fields and preserve internal-only fields.
    pub fn apply_admin_runtime_config(&mut self, config: &AdminRuntimeConfig) {
        self.id = "default".to_string();
        self.auth_cache_ttl_seconds = config.auth_cache_ttl_seconds as i64;
        self.max_request_body_bytes = config.max_request_body_bytes as i64;
        self.account_failure_retry_limit = config.account_failure_retry_limit as i64;
        self.codex_client_version = config.codex_client_version.clone();
        self.codex_status_refresh_min_interval_seconds =
            config.codex_status_refresh_min_interval_seconds as i64;
        self.codex_status_refresh_max_interval_seconds =
            config.codex_status_refresh_max_interval_seconds as i64;
        self.codex_status_account_jitter_max_seconds =
            config.codex_status_account_jitter_max_seconds as i64;
        self.kiro_status_refresh_min_interval_seconds =
            config.kiro_status_refresh_min_interval_seconds as i64;
        self.kiro_status_refresh_max_interval_seconds =
            config.kiro_status_refresh_max_interval_seconds as i64;
        self.kiro_status_account_jitter_max_seconds =
            config.kiro_status_account_jitter_max_seconds as i64;
        self.usage_event_flush_batch_size = config.usage_event_flush_batch_size as i64;
        self.usage_event_flush_interval_seconds = config.usage_event_flush_interval_seconds as i64;
        self.usage_event_flush_max_buffer_bytes = config.usage_event_flush_max_buffer_bytes as i64;
        self.kiro_cache_kmodels_json = config.kiro_cache_kmodels_json.clone();
        self.kiro_billable_model_multipliers_json =
            config.kiro_billable_model_multipliers_json.clone();
        self.kiro_cache_policy_json = config.kiro_cache_policy_json.clone();
        self.kiro_prefix_cache_mode = config.kiro_prefix_cache_mode.clone();
        self.kiro_prefix_cache_max_tokens = config.kiro_prefix_cache_max_tokens as i64;
        self.kiro_prefix_cache_entry_ttl_seconds =
            config.kiro_prefix_cache_entry_ttl_seconds as i64;
        self.kiro_conversation_anchor_max_entries =
            config.kiro_conversation_anchor_max_entries as i64;
        self.kiro_conversation_anchor_ttl_seconds =
            config.kiro_conversation_anchor_ttl_seconds as i64;
        self.updated_at_ms = now_ms();
    }
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
            account_group_id_at_event: None,
            route_strategy_at_event: Some(llm_access_core::provider::RouteStrategy::Auto),
            request_method: "POST".to_string(),
            request_url: "/api/kiro-gateway/v1/messages".to_string(),
            endpoint: "/v1/messages".to_string(),
            model: Some("claude-sonnet-4-5".to_string()),
            mapped_model: None,
            status_code: 200,
            request_body_bytes: Some(1024),
            quota_failover_count: 0,
            routing_diagnostics_json: None,
            input_uncached_tokens: 7,
            input_cached_tokens: 8,
            output_tokens: 9,
            billable_tokens: 10,
            credit_usage: Some("0.25".to_string()),
            usage_missing: false,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: None,
            client_request_body_json: None,
            upstream_request_body_json: None,
            full_request_json: None,
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
    fn public_submission_repository_persists_request_rows() {
        let conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("init schema");
        let repo = super::SqliteControlStore::new(conn);

        repo.create_public_token_request(&llm_access_core::store::NewPublicTokenRequest {
            request_id: "llmwish-test".to_string(),
            requester_email: "user@example.com".to_string(),
            requested_quota_billable_limit: 1000,
            request_reason: "please issue a key".to_string(),
            frontend_page_url: Some("https://example.test/llm-access".to_string()),
            fingerprint: "fingerprint".to_string(),
            client_ip: "198.51.100.10".to_string(),
            ip_region: "unknown".to_string(),
            created_at_ms: 100,
        })
        .expect("create token request");
        repo.create_public_account_contribution_request(
            &llm_access_core::store::NewPublicAccountContributionRequest {
                request_id: "llmacct-test".to_string(),
                account_name: "account_a".to_string(),
                account_id: Some("acct-1".to_string()),
                id_token: "id".to_string(),
                access_token: "access".to_string(),
                refresh_token: "refresh".to_string(),
                requester_email: "user@example.com".to_string(),
                contributor_message: "shared for tests".to_string(),
                github_id: Some("acking-you".to_string()),
                frontend_page_url: None,
                fingerprint: "fingerprint".to_string(),
                client_ip: "198.51.100.11".to_string(),
                ip_region: "unknown".to_string(),
                created_at_ms: 200,
            },
        )
        .expect("create account contribution request");
        repo.create_public_sponsor_request(&llm_access_core::store::NewPublicSponsorRequest {
            request_id: "llmsponsor-test".to_string(),
            requester_email: "user@example.com".to_string(),
            sponsor_message: "thanks".to_string(),
            display_name: Some("Sponsor".to_string()),
            github_id: Some("acking-you".to_string()),
            frontend_page_url: None,
            fingerprint: "fingerprint".to_string(),
            client_ip: "198.51.100.12".to_string(),
            ip_region: "unknown".to_string(),
            created_at_ms: 300,
        })
        .expect("create sponsor request");

        let token_status: String = repo
            .conn
            .query_row(
                "SELECT status FROM llm_token_requests WHERE request_id = 'llmwish-test'",
                [],
                |row| row.get(0),
            )
            .expect("load token request status");
        let account_status: String = repo
            .conn
            .query_row(
                "SELECT status FROM llm_account_contribution_requests
                 WHERE request_id = 'llmacct-test'",
                [],
                |row| row.get(0),
            )
            .expect("load account contribution status");
        let (sponsor_status, sponsor_failure): (String, Option<String>) = repo
            .conn
            .query_row(
                "SELECT status, failure_reason FROM llm_sponsor_requests
                 WHERE request_id = 'llmsponsor-test'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("load sponsor request status");

        assert_eq!(token_status, "pending");
        assert_eq!(account_status, "pending");
        assert_eq!(sponsor_status, "submitted");
        assert_eq!(sponsor_failure.as_deref(), Some("email notifier is not configured"));
    }

    #[test]
    fn admin_review_queue_repository_lists_request_rows() {
        let conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("init schema");
        let repo = super::SqliteControlStore::new(conn);

        repo.create_public_token_request(&llm_access_core::store::NewPublicTokenRequest {
            request_id: "llmwish-old".to_string(),
            requester_email: "old@example.com".to_string(),
            requested_quota_billable_limit: 1000,
            request_reason: "old request".to_string(),
            frontend_page_url: None,
            fingerprint: "fingerprint-old".to_string(),
            client_ip: "198.51.100.20".to_string(),
            ip_region: "unknown".to_string(),
            created_at_ms: 100,
        })
        .expect("create old token request");
        repo.create_public_token_request(&llm_access_core::store::NewPublicTokenRequest {
            request_id: "llmwish-new".to_string(),
            requester_email: "new@example.com".to_string(),
            requested_quota_billable_limit: 2000,
            request_reason: "new request".to_string(),
            frontend_page_url: Some("https://example.test/llm-access".to_string()),
            fingerprint: "fingerprint-new".to_string(),
            client_ip: "198.51.100.21".to_string(),
            ip_region: "unknown".to_string(),
            created_at_ms: 200,
        })
        .expect("create new token request");
        repo.conn
            .execute(
                "UPDATE llm_token_requests
                 SET status = 'rejected', updated_at_ms = 300, processed_at_ms = 300
                 WHERE request_id = 'llmwish-old'",
                [],
            )
            .expect("mark old token request rejected");
        repo.create_public_account_contribution_request(
            &llm_access_core::store::NewPublicAccountContributionRequest {
                request_id: "llmacct-review".to_string(),
                account_name: "account_review".to_string(),
                account_id: Some("acct-review".to_string()),
                id_token: "id-token".to_string(),
                access_token: "access-token".to_string(),
                refresh_token: "refresh-token".to_string(),
                requester_email: "account@example.com".to_string(),
                contributor_message: "please import this account".to_string(),
                github_id: Some("acking-you".to_string()),
                frontend_page_url: None,
                fingerprint: "fingerprint-account".to_string(),
                client_ip: "198.51.100.22".to_string(),
                ip_region: "unknown".to_string(),
                created_at_ms: 300,
            },
        )
        .expect("create account contribution request");
        repo.create_public_sponsor_request(&llm_access_core::store::NewPublicSponsorRequest {
            request_id: "llmsponsor-review".to_string(),
            requester_email: "sponsor@example.com".to_string(),
            sponsor_message: "thanks".to_string(),
            display_name: Some("Sponsor".to_string()),
            github_id: Some("acking-you".to_string()),
            frontend_page_url: None,
            fingerprint: "fingerprint-sponsor".to_string(),
            client_ip: "198.51.100.23".to_string(),
            ip_region: "unknown".to_string(),
            created_at_ms: 400,
        })
        .expect("create sponsor request");

        let token_page = repo
            .list_admin_token_requests(&llm_access_core::store::AdminReviewQueueQuery {
                status: None,
                limit: 1,
                offset: 0,
            })
            .expect("list token requests");
        assert_eq!(token_page.total, 2);
        assert_eq!(token_page.requests.len(), 1);
        assert!(token_page.has_more);
        assert_eq!(token_page.requests[0].request_id, "llmwish-new");
        assert_eq!(token_page.requests[0].requested_quota_billable_limit, 2000);

        let rejected_tokens = repo
            .list_admin_token_requests(&llm_access_core::store::AdminReviewQueueQuery {
                status: Some("rejected".to_string()),
                limit: 50,
                offset: 0,
            })
            .expect("list rejected token requests");
        assert_eq!(rejected_tokens.total, 1);
        assert_eq!(rejected_tokens.requests[0].request_id, "llmwish-old");
        assert_eq!(rejected_tokens.requests[0].processed_at, Some(300));

        let account_page = repo
            .list_admin_account_contribution_requests(
                &llm_access_core::store::AdminReviewQueueQuery {
                    status: Some("pending".to_string()),
                    limit: 50,
                    offset: 0,
                },
            )
            .expect("list account contribution requests");
        assert_eq!(account_page.total, 1);
        assert_eq!(account_page.requests[0].request_id, "llmacct-review");
        assert_eq!(account_page.requests[0].access_token, "access-token");

        let sponsor_page = repo
            .list_admin_sponsor_requests(&llm_access_core::store::AdminReviewQueueQuery {
                status: Some("submitted".to_string()),
                limit: 50,
                offset: 0,
            })
            .expect("list sponsor requests");
        assert_eq!(sponsor_page.total, 1);
        assert_eq!(sponsor_page.requests[0].request_id, "llmsponsor-review");
        assert_eq!(
            sponsor_page.requests[0].failure_reason.as_deref(),
            Some("email notifier is not configured")
        );
    }

    #[test]
    fn admin_review_queue_repository_applies_request_actions() {
        let conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("init schema");
        let repo = super::SqliteControlStore::new(conn);

        repo.create_public_token_request(&llm_access_core::store::NewPublicTokenRequest {
            request_id: "llmwish-action".to_string(),
            requester_email: "token@example.com".to_string(),
            requested_quota_billable_limit: 1234,
            request_reason: "issue a token".to_string(),
            frontend_page_url: None,
            fingerprint: "fingerprint-token".to_string(),
            client_ip: "198.51.100.30".to_string(),
            ip_region: "unknown".to_string(),
            created_at_ms: 100,
        })
        .expect("create token request");
        let issued_token = repo
            .issue_admin_token_request(
                "llmwish-action",
                Some(&llm_access_core::store::NewAdminKey {
                    id: "key-token-action".to_string(),
                    name: "wish-llmwish-action".to_string(),
                    secret: "sfk_token_action".to_string(),
                    key_hash: "hash-token-action".to_string(),
                    provider_type: "codex".to_string(),
                    protocol_family: "openai".to_string(),
                    public_visible: false,
                    quota_billable_limit: 1234,
                    request_max_concurrency: None,
                    request_min_start_interval_ms: None,
                    created_at_ms: 200,
                }),
                &llm_access_core::store::AdminReviewQueueAction {
                    admin_note: Some("issued".to_string()),
                    updated_at_ms: 200,
                },
            )
            .expect("issue token request")
            .expect("token request exists");
        assert_eq!(issued_token.status, "issued");
        assert_eq!(issued_token.issued_key_id.as_deref(), Some("key-token-action"));
        assert_eq!(issued_token.processed_at, Some(200));
        assert!(repo
            .get_key("key-token-action")
            .expect("load issued key")
            .is_some());

        repo.create_public_account_contribution_request(
            &llm_access_core::store::NewPublicAccountContributionRequest {
                request_id: "llmacct-action".to_string(),
                account_name: "contrib_account".to_string(),
                account_id: Some("acct-action".to_string()),
                id_token: "id-token".to_string(),
                access_token: "access-token".to_string(),
                refresh_token: "refresh-token".to_string(),
                requester_email: "account@example.com".to_string(),
                contributor_message: "shared account".to_string(),
                github_id: None,
                frontend_page_url: None,
                fingerprint: "fingerprint-account".to_string(),
                client_ip: "198.51.100.31".to_string(),
                ip_region: "unknown".to_string(),
                created_at_ms: 300,
            },
        )
        .expect("create account contribution request");
        let issued_account = repo
            .issue_admin_account_contribution_request(
                "llmacct-action",
                Some(&llm_access_core::store::NewAdminCodexAccount {
                    name: "contrib_account".to_string(),
                    account_id: Some("acct-action".to_string()),
                    auth_json: r#"{"tokens":{"access_token":"access-token"}}"#.to_string(),
                    map_gpt53_codex_to_spark: false,
                    created_at_ms: 400,
                }),
                Some(&llm_access_core::store::NewAdminAccountGroup {
                    id: "group-contrib-action".to_string(),
                    provider_type: "codex".to_string(),
                    name: "contrib-llmacct-action".to_string(),
                    account_names: vec!["contrib_account".to_string()],
                    created_at_ms: 400,
                }),
                Some(&llm_access_core::store::NewAdminKey {
                    id: "key-contrib-action".to_string(),
                    name: "contrib-llmacct-action".to_string(),
                    secret: "sfk_contrib_action".to_string(),
                    key_hash: "hash-contrib-action".to_string(),
                    provider_type: "codex".to_string(),
                    protocol_family: "openai".to_string(),
                    public_visible: false,
                    quota_billable_limit: 100_000_000_000,
                    request_max_concurrency: None,
                    request_min_start_interval_ms: None,
                    created_at_ms: 400,
                }),
                &llm_access_core::store::AdminReviewQueueAction {
                    admin_note: None,
                    updated_at_ms: 400,
                },
            )
            .expect("issue account contribution request")
            .expect("account contribution request exists");
        assert_eq!(issued_account.status, "issued");
        assert_eq!(issued_account.imported_account_name.as_deref(), Some("contrib_account"));
        let issued_key = repo
            .get_key("key-contrib-action")
            .expect("load contribution key")
            .expect("contribution key exists");
        assert_eq!(issued_key.route.account_group_id.as_deref(), Some("group-contrib-action"));
        assert_eq!(issued_key.route.route_strategy.as_deref(), Some("fixed"));

        repo.create_public_sponsor_request(&llm_access_core::store::NewPublicSponsorRequest {
            request_id: "llmsponsor-action".to_string(),
            requester_email: "sponsor@example.com".to_string(),
            sponsor_message: "thanks".to_string(),
            display_name: Some("Sponsor".to_string()),
            github_id: None,
            frontend_page_url: None,
            fingerprint: "fingerprint-sponsor".to_string(),
            client_ip: "198.51.100.32".to_string(),
            ip_region: "unknown".to_string(),
            created_at_ms: 500,
        })
        .expect("create sponsor request");
        let approved_sponsor = repo
            .approve_admin_sponsor_request(
                "llmsponsor-action",
                &llm_access_core::store::AdminReviewQueueAction {
                    admin_note: Some("approved".to_string()),
                    updated_at_ms: 600,
                },
            )
            .expect("approve sponsor request")
            .expect("sponsor request exists");
        assert_eq!(approved_sponsor.status, "approved");
        assert_eq!(approved_sponsor.processed_at, Some(600));
        assert!(repo
            .delete_admin_sponsor_request("llmsponsor-action")
            .expect("delete sponsor request"));
    }

    #[test]
    fn provider_route_repository_resolves_codex_account_from_key_route() {
        let conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("init schema");
        let repo = super::SqliteControlStore::new(conn);

        repo.create_admin_codex_account(&llm_access_core::store::NewAdminCodexAccount {
            name: "codex-route".to_string(),
            account_id: Some("acct-route".to_string()),
            auth_json: r#"{"access_token":"access-route"}"#.to_string(),
            map_gpt53_codex_to_spark: true,
            created_at_ms: 100,
        })
        .expect("create codex account");
        repo.create_admin_proxy_config(&llm_access_core::store::NewAdminProxyConfig {
            id: "proxy-codex-route".to_string(),
            name: "codex proxy".to_string(),
            proxy_url: "http://127.0.0.1:9010".to_string(),
            proxy_username: Some("codex-user".to_string()),
            proxy_password: Some("codex-pass".to_string()),
            created_at_ms: 105,
        })
        .expect("create codex proxy");
        repo.patch_admin_codex_account(
            "codex-route",
            &llm_access_core::store::AdminCodexAccountPatch {
                map_gpt53_codex_to_spark: None,
                proxy_mode: Some("fixed".to_string()),
                proxy_config_id: Some(Some("proxy-codex-route".to_string())),
                request_max_concurrency: Some(Some(3)),
                request_min_start_interval_ms: Some(Some(75)),
                updated_at_ms: 110,
            },
        )
        .expect("patch codex account limits");
        repo.create_admin_account_group(&llm_access_core::store::NewAdminAccountGroup {
            id: "group-route".to_string(),
            provider_type: "codex".to_string(),
            name: "route group".to_string(),
            account_names: vec!["codex-route".to_string()],
            created_at_ms: 100,
        })
        .expect("create group");
        repo.create_admin_key(&llm_access_core::store::NewAdminKey {
            id: "key-route".to_string(),
            name: "route key".to_string(),
            secret: "sfk_route".to_string(),
            key_hash: "hash-route".to_string(),
            provider_type: "codex".to_string(),
            protocol_family: "openai".to_string(),
            public_visible: false,
            quota_billable_limit: 1000,
            request_max_concurrency: Some(2),
            request_min_start_interval_ms: Some(50),
            created_at_ms: 100,
        })
        .expect("create key");
        repo.patch_admin_key("key-route", &llm_access_core::store::AdminKeyPatch {
            name: None,
            status: None,
            public_visible: None,
            quota_billable_limit: None,
            route_strategy: Some(Some("fixed".to_string())),
            account_group_id: Some(Some("group-route".to_string())),
            fixed_account_name: None,
            auto_account_names: None,
            model_name_map: None,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            updated_at_ms: 200,
            ..llm_access_core::store::AdminKeyPatch::default()
        })
        .expect("patch key route");

        let route = repo
            .resolve_provider_codex_route(&llm_access_core::store::AuthenticatedKey {
                key_id: "key-route".to_string(),
                key_name: "route key".to_string(),
                provider_type: "codex".to_string(),
                protocol_family: "openai".to_string(),
                status: "active".to_string(),
                quota_billable_limit: 1000,
                billable_tokens_used: 0,
            })
            .expect("resolve route")
            .expect("route exists");
        assert_eq!(route.account_name, "codex-route");
        assert_eq!(route.account_group_id_at_event.as_deref(), Some("group-route"));
        assert_eq!(route.route_strategy_at_event, llm_access_core::provider::RouteStrategy::Fixed);
        assert_eq!(route.auth_json, r#"{"access_token":"access-route"}"#);
        assert!(route.map_gpt53_codex_to_spark);
        assert_eq!(route.request_max_concurrency, Some(2));
        assert_eq!(route.request_min_start_interval_ms, Some(50));
        assert_eq!(route.account_request_max_concurrency, Some(3));
        assert_eq!(route.account_request_min_start_interval_ms, Some(75));
        let proxy = route.proxy.expect("codex proxy");
        assert_eq!(proxy.proxy_url, "http://127.0.0.1:9010");
        assert_eq!(proxy.proxy_username.as_deref(), Some("codex-user"));
    }

    #[test]
    fn provider_route_repository_resolves_kiro_account_from_key_route() {
        let conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("init schema");
        let repo = super::SqliteControlStore::new(conn);

        repo.create_admin_proxy_config(&llm_access_core::store::NewAdminProxyConfig {
            id: "proxy-kiro-route".to_string(),
            name: "kiro proxy".to_string(),
            proxy_url: "socks5h://127.0.0.1:9011".to_string(),
            proxy_username: None,
            proxy_password: None,
            created_at_ms: 20,
        })
        .expect("create kiro proxy");
        repo.upsert_kiro_account(&super::KiroAccountRecord {
            account_name: "kiro-route".to_string(),
            auth_method: "idc".to_string(),
            account_id: Some("kiro-account".to_string()),
            profile_arn: Some("arn:aws:kiro:test".to_string()),
            user_id: Some("user-1".to_string()),
            status: "active".to_string(),
            auth_json: r#"{"accessToken":"access-route","apiRegion":"us-west-2"}"#.to_string(),
            max_concurrency: Some(1),
            min_start_interval_ms: Some(100),
            proxy_config_id: Some("proxy-kiro-route".to_string()),
            last_refresh_at_ms: Some(200),
            last_error: None,
            created_at_ms: 30,
            updated_at_ms: 40,
        })
        .expect("upsert kiro account");
        repo.create_admin_account_group(&llm_access_core::store::NewAdminAccountGroup {
            id: "kiro-group-route".to_string(),
            provider_type: "kiro".to_string(),
            name: "kiro route group".to_string(),
            account_names: vec!["kiro-route".to_string()],
            created_at_ms: 100,
        })
        .expect("create group");
        repo.upsert_key_bundle(
            &super::KeyRecord {
                key_id: "kiro-key-route".to_string(),
                name: "kiro route key".to_string(),
                secret: "sfk_kiro_route".to_string(),
                key_hash: "hash-kiro-route".to_string(),
                status: "active".to_string(),
                provider_type: "kiro".to_string(),
                protocol_family: "anthropic".to_string(),
                public_visible: false,
                quota_billable_limit: 1000,
                created_at_ms: 100,
                updated_at_ms: 200,
            },
            &super::KeyRouteConfig {
                key_id: "kiro-key-route".to_string(),
                route_strategy: Some("fixed".to_string()),
                fixed_account_name: None,
                auto_account_names_json: None,
                account_group_id: Some("kiro-group-route".to_string()),
                model_name_map_json: Some(
                    r#"{"claude-haiku-4-5-20251001":"claude-sonnet-4-6"}"#.to_string(),
                ),
                request_max_concurrency: Some(2),
                request_min_start_interval_ms: Some(50),
                kiro_request_validation_enabled: true,
                kiro_cache_estimation_enabled: true,
                kiro_zero_cache_debug_enabled: false,
                kiro_cache_policy_override_json: None,
                kiro_billable_model_multipliers_override_json: Some(r#"{"sonnet":2}"#.to_string()),
            },
            &super::KeyUsageRollup {
                key_id: "kiro-key-route".to_string(),
                input_uncached_tokens: 0,
                input_cached_tokens: 0,
                output_tokens: 0,
                billable_tokens: 0,
                credit_total: 0.0,
                credit_missing_events: 0,
                last_used_at_ms: None,
                updated_at_ms: 100,
            },
        )
        .expect("upsert kiro key bundle");

        let route = repo
            .resolve_provider_kiro_route(&llm_access_core::store::AuthenticatedKey {
                key_id: "kiro-key-route".to_string(),
                key_name: "kiro route key".to_string(),
                provider_type: "kiro".to_string(),
                protocol_family: "anthropic".to_string(),
                status: "active".to_string(),
                quota_billable_limit: 1000,
                billable_tokens_used: 0,
            })
            .expect("resolve route")
            .expect("route exists");
        assert_eq!(route.account_name, "kiro-route");
        assert_eq!(route.account_group_id_at_event.as_deref(), Some("kiro-group-route"));
        assert_eq!(route.route_strategy_at_event, llm_access_core::provider::RouteStrategy::Fixed);
        assert_eq!(route.auth_json, r#"{"accessToken":"access-route","apiRegion":"us-west-2"}"#);
        assert_eq!(route.profile_arn.as_deref(), Some("arn:aws:kiro:test"));
        assert_eq!(route.api_region, "us-west-2");
        assert!(route.request_validation_enabled);
        assert!(route.cache_estimation_enabled);
        assert_eq!(
            route.cache_kmodels_json,
            llm_access_core::store::default_kiro_cache_kmodels_json()
        );
        assert_eq!(
            route.model_name_map_json,
            r#"{"claude-haiku-4-5-20251001":"claude-sonnet-4-6"}"#
        );
        assert_eq!(route.prefix_cache_mode, llm_access_core::store::DEFAULT_KIRO_PREFIX_CACHE_MODE);
        assert_eq!(route.billable_model_multipliers_json, r#"{"sonnet":2}"#);
        assert_eq!(route.request_max_concurrency, Some(2));
        assert_eq!(route.request_min_start_interval_ms, Some(50));
        assert_eq!(route.account_request_max_concurrency, Some(1));
        assert_eq!(route.account_request_min_start_interval_ms, Some(100));
        let proxy = route.proxy.expect("kiro proxy");
        assert_eq!(proxy.proxy_url, "socks5h://127.0.0.1:9011");

        let mut direct_record = repo
            .get_kiro_account("kiro-route")
            .expect("load kiro route account")
            .expect("kiro route account exists");
        direct_record.auth_json =
            r#"{"accessToken":"access-route","apiRegion":"us-west-2","proxyMode":"none"}"#
                .to_string();
        repo.upsert_kiro_account(&direct_record)
            .expect("upsert direct-proxy kiro account");
        let direct_route = repo
            .resolve_provider_kiro_route(&llm_access_core::store::AuthenticatedKey {
                key_id: "kiro-key-route".to_string(),
                key_name: "kiro route key".to_string(),
                provider_type: "kiro".to_string(),
                protocol_family: "anthropic".to_string(),
                status: "active".to_string(),
                quota_billable_limit: 1000,
                billable_tokens_used: 0,
            })
            .expect("resolve direct route")
            .expect("direct route exists");
        assert!(direct_route.proxy.is_none());
    }

    #[test]
    fn legacy_kiro_proxy_import_creates_shared_proxy_and_updates_accounts() {
        let conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("init schema");
        let repo = super::SqliteControlStore::new(conn);

        repo.upsert_kiro_account(&super::KiroAccountRecord {
            account_name: "kiro-legacy".to_string(),
            auth_method: "idc".to_string(),
            account_id: Some("kiro-account".to_string()),
            profile_arn: Some("arn:aws:kiro:test".to_string()),
            user_id: Some("user-1".to_string()),
            status: "active".to_string(),
            auth_json: r#"{
                "accessToken":"access-route",
                "apiRegion":"us-west-2",
                "proxyUrl":"http://127.0.0.1:9020",
                "proxyUsername":"legacy-user",
                "proxyPassword":"legacy-pass"
            }"#
            .to_string(),
            max_concurrency: Some(1),
            min_start_interval_ms: Some(100),
            proxy_config_id: None,
            last_refresh_at_ms: Some(200),
            last_error: None,
            created_at_ms: 30,
            updated_at_ms: 40,
        })
        .expect("upsert legacy kiro account");

        let result = repo
            .import_legacy_kiro_proxy_configs()
            .expect("import legacy proxies");
        assert_eq!(result.created_configs.len(), 1);
        assert_eq!(result.reused_configs.len(), 0);
        assert_eq!(result.migrated_account_names, vec!["kiro-legacy"]);
        let proxy = &result.created_configs[0];
        assert_eq!(proxy.proxy_url, "http://127.0.0.1:9020");
        assert_eq!(proxy.proxy_username.as_deref(), Some("legacy-user"));
        let account = repo
            .get_kiro_account("kiro-legacy")
            .expect("load migrated account")
            .expect("account exists");
        assert_eq!(account.proxy_config_id.as_deref(), Some(proxy.id.as_str()));
        let auth_json =
            serde_json::from_str::<serde_json::Value>(&account.auth_json).expect("auth json");
        assert!(auth_json.get("proxyUrl").is_none());
        assert_eq!(auth_json["proxyMode"], "fixed");
        assert_eq!(auth_json["proxyConfigId"], proxy.id);
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
