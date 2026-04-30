//! SQLite control-plane repository for `llm-access`.

use anyhow::Context;
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
    key_id: String,
    name: String,
    secret: String,
    key_hash: String,
    status: String,
    provider_type: String,
    protocol_family: String,
    public_visible: bool,
    quota_billable_limit: i64,
    created_at_ms: i64,
    updated_at_ms: i64,
}

/// API key route configuration row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRouteConfig {
    key_id: String,
    route_strategy: Option<String>,
    fixed_account_name: Option<String>,
    auto_account_names_json: Option<String>,
    account_group_id: Option<String>,
    model_name_map_json: Option<String>,
}

/// API key accumulated usage rollup row.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyUsageRollup {
    key_id: String,
    input_uncached_tokens: i64,
    input_cached_tokens: i64,
    output_tokens: i64,
    billable_tokens: i64,
    credit_total: f64,
    credit_missing_events: i64,
    last_used_at_ms: Option<i64>,
    updated_at_ms: i64,
}

/// Runtime configuration row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfigRecord {
    id: String,
    auth_cache_ttl_seconds: i64,
    max_request_body_bytes: i64,
    account_failure_retry_limit: i64,
    codex_client_version: String,
    kiro_channel_max_concurrency: i64,
    kiro_channel_min_start_interval_ms: i64,
    codex_status_refresh_min_interval_seconds: i64,
    codex_status_refresh_max_interval_seconds: i64,
    codex_status_account_jitter_max_seconds: i64,
    kiro_status_refresh_min_interval_seconds: i64,
    kiro_status_refresh_max_interval_seconds: i64,
    kiro_status_account_jitter_max_seconds: i64,
    usage_event_flush_batch_size: i64,
    usage_event_flush_interval_seconds: i64,
    usage_event_flush_max_buffer_bytes: i64,
    usage_event_maintenance_enabled: bool,
    usage_event_maintenance_interval_seconds: i64,
    usage_event_detail_retention_days: i64,
    kiro_cache_kmodels_json: String,
    kiro_billable_model_multipliers_json: String,
    kiro_cache_policy_json: String,
    kiro_prefix_cache_mode: String,
    kiro_prefix_cache_max_tokens: i64,
    kiro_prefix_cache_entry_ttl_seconds: i64,
    kiro_conversation_anchor_max_entries: i64,
    kiro_conversation_anchor_ttl_seconds: i64,
    updated_at_ms: i64,
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
                account_group_id, model_name_map_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(key_id) DO UPDATE SET
                route_strategy = excluded.route_strategy,
                fixed_account_name = excluded.fixed_account_name,
                auto_account_names_json = excluded.auto_account_names_json,
                account_group_id = excluded.account_group_id,
                model_name_map_json = excluded.model_name_map_json",
            params![
                &route.key_id,
                &route.route_strategy,
                &route.fixed_account_name,
                &route.auto_account_names_json,
                &route.account_group_id,
                &route.model_name_map_json,
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
}

fn decode_key_bundle(row: &rusqlite::Row<'_>) -> rusqlite::Result<KeyBundle> {
    let key_id: String = row.get(0)?;
    let credit_total_raw: String = row.get(20)?;
    let credit_total = credit_total_raw
        .parse::<f64>()
        .map_err(|err| rusqlite::Error::FromSqlConversionFailure(20, Type::Text, Box::new(err)))?;
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
        },
        rollup: KeyUsageRollup {
            key_id,
            input_uncached_tokens: row.get(16)?,
            input_cached_tokens: row.get(17)?,
            output_tokens: row.get(18)?,
            billable_tokens: row.get(19)?,
            credit_total,
            credit_missing_events: row.get(21)?,
            last_used_at_ms: row.get(22)?,
            updated_at_ms: row.get(23)?,
        },
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
        assert_eq!(loaded.rollup.output_tokens, 33);
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
}
