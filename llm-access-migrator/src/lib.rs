//! Migration helpers for moving StaticFlow LLM gateway data into llm-access.

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};
use serde_json::Value;

/// Replay configuration for one source outbox consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayOptions<'a> {
    /// Consumer name used for target `cdc_consumer_offsets`.
    pub consumer_name: &'a str,
    /// Maximum number of source events to process in this call.
    pub max_events: usize,
}

/// Replay result counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayStats {
    /// Number of source outbox rows read.
    pub read_events: usize,
    /// Number of source outbox rows applied.
    pub applied_events: usize,
    /// Last source outbox sequence applied by this call.
    pub last_applied_seq: i64,
}

#[derive(Debug)]
struct SourceOutboxEvent {
    seq: i64,
    event_id: String,
    entity: String,
    op: String,
    primary_key: String,
    payload_json: String,
}

/// Replay source `cdc_outbox` rows into the target SQLite control plane.
pub fn replay_source_outbox_to_sqlite_target(
    source: &Connection,
    target: &Connection,
    options: &ReplayOptions<'_>,
) -> Result<ReplayStats> {
    if options.consumer_name.trim().is_empty() {
        bail!("consumer_name must not be empty");
    }
    let max_events = options.max_events.max(1);
    let last_offset = read_target_offset(target, options.consumer_name)?;
    let events = read_source_events(source, last_offset, max_events)?;
    if events.is_empty() {
        return Ok(ReplayStats {
            read_events: 0,
            applied_events: 0,
            last_applied_seq: last_offset,
        });
    }

    let tx = target
        .unchecked_transaction()
        .context("failed to begin target replay transaction")?;
    let mut applied = 0usize;
    let mut last_applied_seq = last_offset;
    for event in &events {
        apply_event(&tx, event)?;
        tx.execute(
            "INSERT OR REPLACE INTO cdc_applied_events_recent (
                event_id, source_seq, applied_at_ms
             ) VALUES (?1, ?2, unixepoch('subsec') * 1000)",
            params![event.event_id, event.seq],
        )
        .context("failed to record applied event")?;
        applied += 1;
        last_applied_seq = event.seq;
    }
    tx.execute(
        "INSERT INTO cdc_consumer_offsets (consumer_name, last_applied_seq, updated_at_ms)
         VALUES (?1, ?2, unixepoch('subsec') * 1000)
         ON CONFLICT(consumer_name) DO UPDATE SET
            last_applied_seq = excluded.last_applied_seq,
            updated_at_ms = excluded.updated_at_ms",
        params![options.consumer_name, last_applied_seq],
    )
    .context("failed to update consumer offset")?;
    tx.commit()
        .context("failed to commit target replay transaction")?;

    Ok(ReplayStats {
        read_events: events.len(),
        applied_events: applied,
        last_applied_seq,
    })
}

fn read_target_offset(target: &Connection, consumer_name: &str) -> Result<i64> {
    let offset = target
        .query_row(
            "SELECT last_applied_seq FROM cdc_consumer_offsets WHERE consumer_name = ?1",
            [consumer_name],
            |row| row.get(0),
        )
        .or_else(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => Ok(0),
            err => Err(err),
        })
        .context("failed to read target consumer offset")?;
    Ok(offset)
}

fn read_source_events(
    source: &Connection,
    after_seq: i64,
    limit: usize,
) -> Result<Vec<SourceOutboxEvent>> {
    let mut stmt = source
        .prepare(
            "SELECT seq, event_id, entity, op, primary_key, payload_json
             FROM cdc_outbox
             WHERE seq > ?1
             ORDER BY seq ASC
             LIMIT ?2",
        )
        .context("failed to prepare source outbox query")?;
    let rows = stmt
        .query_map(params![after_seq, limit as i64], |row| {
            Ok(SourceOutboxEvent {
                seq: row.get(0)?,
                event_id: row.get(1)?,
                entity: row.get(2)?,
                op: row.get(3)?,
                primary_key: row.get(4)?,
                payload_json: row.get(5)?,
            })
        })
        .context("failed to query source outbox")?;

    let mut events = Vec::new();
    for row in rows {
        events.push(row.context("failed to decode source outbox row")?);
    }
    Ok(events)
}

fn apply_event(conn: &Connection, event: &SourceOutboxEvent) -> Result<()> {
    match (event.entity.as_str(), event.op.as_str()) {
        ("key", "upsert") => apply_key_upsert(conn, event),
        ("key", "delete") => conn
            .execute("DELETE FROM llm_keys WHERE key_id = ?1", [&event.primary_key])
            .map(|_| ())
            .context("failed to delete replayed key"),
        ("usage_event", "append") => Ok(()),
        (entity, op) => bail!("unsupported replay event entity={entity} op={op}"),
    }
}

fn apply_key_upsert(conn: &Connection, event: &SourceOutboxEvent) -> Result<()> {
    let payload: Value =
        serde_json::from_str(&event.payload_json).context("invalid key payload")?;
    let key_id = string_field(&payload, "id")?;
    let auto_account_names_json = optional_json_field(&payload, "auto_account_names")?;
    let model_name_map_json = optional_json_field(&payload, "model_name_map")?;

    conn.execute(
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
            key_id,
            string_field(&payload, "name")?,
            string_field(&payload, "secret")?,
            string_field(&payload, "key_hash")?,
            string_field(&payload, "status")?,
            string_field(&payload, "provider_type")?,
            string_field(&payload, "protocol_family")?,
            bool_int_field(&payload, "public_visible")?,
            u64_field(&payload, "quota_billable_limit")?,
            i64_field(&payload, "created_at")?,
            i64_field(&payload, "updated_at")?,
        ],
    )
    .context("failed to upsert replayed key")?;

    conn.execute(
        "INSERT INTO llm_key_route_config (
            key_id, route_strategy, fixed_account_name, auto_account_names_json,
            account_group_id, model_name_map_json, request_max_concurrency,
            request_min_start_interval_ms, kiro_request_validation_enabled,
            kiro_cache_estimation_enabled, kiro_zero_cache_debug_enabled,
            kiro_cache_policy_override_json, kiro_billable_model_multipliers_override_json
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
            key_id,
            optional_string_field(&payload, "route_strategy")?,
            optional_string_field(&payload, "fixed_account_name")?,
            auto_account_names_json,
            optional_string_field(&payload, "account_group_id")?,
            model_name_map_json,
            optional_u64_field(&payload, "request_max_concurrency")?,
            optional_u64_field(&payload, "request_min_start_interval_ms")?,
            bool_int_field(&payload, "kiro_request_validation_enabled")?,
            bool_int_field(&payload, "kiro_cache_estimation_enabled")?,
            bool_int_field(&payload, "kiro_zero_cache_debug_enabled")?,
            optional_string_field(&payload, "kiro_cache_policy_override_json")?,
            optional_string_field(&payload, "kiro_billable_model_multipliers_override_json")?,
        ],
    )
    .context("failed to upsert replayed key route config")?;

    conn.execute(
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
            key_id,
            u64_field(&payload, "usage_input_uncached_tokens")?,
            u64_field(&payload, "usage_input_cached_tokens")?,
            u64_field(&payload, "usage_output_tokens")?,
            u64_field(&payload, "usage_billable_tokens")?,
            f64_field(&payload, "usage_credit_total")?.to_string(),
            u64_field(&payload, "usage_credit_missing_events")?,
            optional_i64_field(&payload, "last_used_at")?,
            i64_field(&payload, "updated_at")?,
        ],
    )
    .context("failed to upsert replayed key usage rollup")?;

    Ok(())
}

fn value_field<'a>(payload: &'a Value, field: &str) -> Result<&'a Value> {
    payload
        .get(field)
        .with_context(|| format!("missing payload field `{field}`"))
}

fn string_field(payload: &Value, field: &str) -> Result<String> {
    value_field(payload, field)?
        .as_str()
        .map(ToOwned::to_owned)
        .with_context(|| format!("payload field `{field}` must be a string"))
}

fn optional_string_field(payload: &Value, field: &str) -> Result<Option<String>> {
    match value_field(payload, field)? {
        Value::Null => Ok(None),
        Value::String(value) => Ok(Some(value.clone())),
        _ => bail!("payload field `{field}` must be null or a string"),
    }
}

fn i64_field(payload: &Value, field: &str) -> Result<i64> {
    value_field(payload, field)?
        .as_i64()
        .with_context(|| format!("payload field `{field}` must be an i64"))
}

fn optional_i64_field(payload: &Value, field: &str) -> Result<Option<i64>> {
    match value_field(payload, field)? {
        Value::Null => Ok(None),
        value => value
            .as_i64()
            .map(Some)
            .with_context(|| format!("payload field `{field}` must be null or an i64")),
    }
}

fn u64_field(payload: &Value, field: &str) -> Result<u64> {
    value_field(payload, field)?
        .as_u64()
        .with_context(|| format!("payload field `{field}` must be a u64"))
}

fn optional_u64_field(payload: &Value, field: &str) -> Result<Option<u64>> {
    match value_field(payload, field)? {
        Value::Null => Ok(None),
        value => value
            .as_u64()
            .map(Some)
            .with_context(|| format!("payload field `{field}` must be null or a u64")),
    }
}

fn f64_field(payload: &Value, field: &str) -> Result<f64> {
    value_field(payload, field)?
        .as_f64()
        .with_context(|| format!("payload field `{field}` must be an f64"))
}

fn bool_int_field(payload: &Value, field: &str) -> Result<i64> {
    Ok(
        if value_field(payload, field)?
            .as_bool()
            .with_context(|| format!("payload field `{field}` must be a bool"))?
        {
            1
        } else {
            0
        },
    )
}

fn optional_json_field(payload: &Value, field: &str) -> Result<Option<String>> {
    match value_field(payload, field)? {
        Value::Null => Ok(None),
        value => serde_json::to_string(value)
            .map(Some)
            .with_context(|| format!("failed to encode payload field `{field}` as JSON")),
    }
}

#[cfg(test)]
mod tests {
    fn source_outbox() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("open source");
        conn.execute_batch(
            "CREATE TABLE cdc_outbox (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                event_id TEXT NOT NULL UNIQUE,
                source_instance TEXT NOT NULL,
                entity TEXT NOT NULL,
                op TEXT NOT NULL,
                primary_key TEXT NOT NULL,
                schema_version INTEGER NOT NULL,
                payload_json TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL,
                committed_at_ms INTEGER NOT NULL
            ) STRICT;",
        )
        .expect("create source outbox");
        conn
    }

    fn insert_source_event(
        conn: &rusqlite::Connection,
        event_id: &str,
        entity: &str,
        op: &str,
        primary_key: &str,
        payload_json: &str,
    ) {
        conn.execute(
            "INSERT INTO cdc_outbox (
                event_id, source_instance, entity, op, primary_key, schema_version,
                payload_json, created_at_ms, committed_at_ms
            ) VALUES (?1, 'source-test', ?2, ?3, ?4, 1, ?5, 10, 10)",
            rusqlite::params![event_id, entity, op, primary_key, payload_json],
        )
        .expect("insert source event");
    }

    #[test]
    fn replays_key_upsert_delete_and_records_offset() {
        let source = source_outbox();
        let target = rusqlite::Connection::open_in_memory().expect("open target");
        llm_access_store::initialize_sqlite_target(&target).expect("initialize target");
        insert_source_event(
            &source,
            "event-key-upsert",
            "key",
            "upsert",
            "key-1",
            r#"{
                "id":"key-1",
                "name":"Kiro public",
                "secret":"secret",
                "key_hash":"hash-1",
                "status":"active",
                "provider_type":"kiro",
                "protocol_family":"anthropic",
                "public_visible":true,
                "quota_billable_limit":1000,
                "usage_input_uncached_tokens":10,
                "usage_input_cached_tokens":20,
                "usage_output_tokens":3,
                "usage_billable_tokens":45,
                "usage_credit_total":1.25,
                "usage_credit_missing_events":2,
                "last_used_at":99,
                "created_at":10,
                "updated_at":20,
                "route_strategy":"auto",
                "fixed_account_name":null,
                "auto_account_names":["a","b"],
                "account_group_id":"group-1",
                "model_name_map":{"x":"y"},
                "request_max_concurrency":2,
                "request_min_start_interval_ms":5,
                "kiro_request_validation_enabled":true,
                "kiro_cache_estimation_enabled":true,
                "kiro_zero_cache_debug_enabled":false,
                "kiro_cache_policy_override_json":null,
                "kiro_billable_model_multipliers_override_json":null
            }"#,
        );
        insert_source_event(
            &source,
            "event-key-delete",
            "key",
            "delete",
            "key-1",
            r#"{"primary_key":"key-1"}"#,
        );

        let stats =
            super::replay_source_outbox_to_sqlite_target(&source, &target, &super::ReplayOptions {
                consumer_name: "test-consumer",
                max_events: 100,
            })
            .expect("replay source outbox");

        assert_eq!(stats.read_events, 2);
        assert_eq!(stats.applied_events, 2);
        assert_eq!(stats.last_applied_seq, 2);

        let key_count: i64 = target
            .query_row("SELECT count(*) FROM llm_keys", [], |row| row.get(0))
            .expect("count keys");
        assert_eq!(key_count, 0);

        let last_offset: i64 = target
            .query_row(
                "SELECT last_applied_seq FROM cdc_consumer_offsets WHERE consumer_name = \
                 'test-consumer'",
                [],
                |row| row.get(0),
            )
            .expect("read offset");
        assert_eq!(last_offset, 2);
    }
}
