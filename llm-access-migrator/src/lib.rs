//! Migration helpers for moving StaticFlow LLM gateway data into llm-access.

pub mod snapshot;

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
        ("key", "delete") => delete_by_id(conn, "llm_keys", "key_id", &event.primary_key),
        ("runtime_config", "upsert") => apply_runtime_config_upsert(conn, event),
        ("account_group", "upsert") => apply_account_group_upsert(conn, event),
        ("account_group", "delete") => {
            delete_by_id(conn, "llm_account_groups", "group_id", &event.primary_key)
        },
        ("proxy_config", "upsert") => apply_proxy_config_upsert(conn, event),
        ("proxy_config", "delete") => {
            delete_by_id(conn, "llm_proxy_configs", "proxy_config_id", &event.primary_key)
        },
        ("proxy_binding", "upsert") => apply_proxy_binding_upsert(conn, event),
        ("proxy_binding", "delete") => {
            delete_by_id(conn, "llm_proxy_bindings", "provider_type", &event.primary_key)
        },
        ("token_request", "upsert") => apply_token_request_upsert(conn, event),
        ("account_contribution_request", "upsert") => {
            apply_account_contribution_request_upsert(conn, event)
        },
        ("gpt2api_account_contribution_request", "upsert") => {
            apply_gpt2api_account_contribution_request_upsert(conn, event)
        },
        ("sponsor_request", "upsert") => apply_sponsor_request_upsert(conn, event),
        ("sponsor_request", "delete") => {
            delete_by_id(conn, "llm_sponsor_requests", "request_id", &event.primary_key)
        },
        ("usage_event", "append") => Ok(()),
        (entity, op) => bail!("unsupported replay event entity={entity} op={op}"),
    }
}

fn delete_by_id(conn: &Connection, table: &str, column: &str, value: &str) -> Result<()> {
    let sql = match (table, column) {
        ("llm_keys", "key_id") => "DELETE FROM llm_keys WHERE key_id = ?1",
        ("llm_account_groups", "group_id") => "DELETE FROM llm_account_groups WHERE group_id = ?1",
        ("llm_proxy_configs", "proxy_config_id") => {
            "DELETE FROM llm_proxy_configs WHERE proxy_config_id = ?1"
        },
        ("llm_proxy_bindings", "provider_type") => {
            "DELETE FROM llm_proxy_bindings WHERE provider_type = ?1"
        },
        ("llm_sponsor_requests", "request_id") => {
            "DELETE FROM llm_sponsor_requests WHERE request_id = ?1"
        },
        _ => bail!("unsupported delete target table={table} column={column}"),
    };
    conn.execute(sql, [value])
        .map(|_| ())
        .context("failed to delete replayed row")
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

fn apply_runtime_config_upsert(conn: &Connection, event: &SourceOutboxEvent) -> Result<()> {
    let payload: Value =
        serde_json::from_str(&event.payload_json).context("invalid runtime_config payload")?;
    conn.execute(
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
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
            ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26,
            ?27, ?28
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
            string_field(&payload, "id")?,
            u64_field(&payload, "auth_cache_ttl_seconds")?,
            u64_field(&payload, "max_request_body_bytes")?,
            u64_field(&payload, "account_failure_retry_limit")?,
            string_field(&payload, "codex_client_version")?,
            u64_field(&payload, "kiro_channel_max_concurrency")?,
            u64_field(&payload, "kiro_channel_min_start_interval_ms")?,
            u64_field(&payload, "codex_status_refresh_min_interval_seconds")?,
            u64_field(&payload, "codex_status_refresh_max_interval_seconds")?,
            u64_field(&payload, "codex_status_account_jitter_max_seconds")?,
            u64_field(&payload, "kiro_status_refresh_min_interval_seconds")?,
            u64_field(&payload, "kiro_status_refresh_max_interval_seconds")?,
            u64_field(&payload, "kiro_status_account_jitter_max_seconds")?,
            u64_field(&payload, "usage_event_flush_batch_size")?,
            u64_field(&payload, "usage_event_flush_interval_seconds")?,
            u64_field(&payload, "usage_event_flush_max_buffer_bytes")?,
            bool_int_field(&payload, "usage_event_maintenance_enabled")?,
            u64_field(&payload, "usage_event_maintenance_interval_seconds")?,
            i64_field(&payload, "usage_event_detail_retention_days")?,
            string_field(&payload, "kiro_cache_kmodels_json")?,
            string_field(&payload, "kiro_billable_model_multipliers_json")?,
            string_field(&payload, "kiro_cache_policy_json")?,
            string_field(&payload, "kiro_prefix_cache_mode")?,
            u64_field(&payload, "kiro_prefix_cache_max_tokens")?,
            u64_field(&payload, "kiro_prefix_cache_entry_ttl_seconds")?,
            u64_field(&payload, "kiro_conversation_anchor_max_entries")?,
            u64_field(&payload, "kiro_conversation_anchor_ttl_seconds")?,
            i64_field(&payload, "updated_at")?,
        ],
    )
    .context("failed to upsert replayed runtime config")?;
    Ok(())
}

fn apply_account_group_upsert(conn: &Connection, event: &SourceOutboxEvent) -> Result<()> {
    let payload: Value =
        serde_json::from_str(&event.payload_json).context("invalid account_group payload")?;
    conn.execute(
        "INSERT INTO llm_account_groups (
            group_id, provider_type, name, account_names_json, created_at_ms, updated_at_ms
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        ON CONFLICT(group_id) DO UPDATE SET
            provider_type = excluded.provider_type,
            name = excluded.name,
            account_names_json = excluded.account_names_json,
            created_at_ms = excluded.created_at_ms,
            updated_at_ms = excluded.updated_at_ms",
        params![
            string_field(&payload, "id")?,
            string_field(&payload, "provider_type")?,
            string_field(&payload, "name")?,
            json_text_field(&payload, "account_names")?,
            i64_field(&payload, "created_at")?,
            i64_field(&payload, "updated_at")?,
        ],
    )
    .context("failed to upsert replayed account group")?;
    Ok(())
}

fn apply_proxy_config_upsert(conn: &Connection, event: &SourceOutboxEvent) -> Result<()> {
    let payload: Value =
        serde_json::from_str(&event.payload_json).context("invalid proxy_config payload")?;
    conn.execute(
        "INSERT INTO llm_proxy_configs (
            proxy_config_id, name, proxy_url, proxy_username, proxy_password,
            status, created_at_ms, updated_at_ms
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
        ON CONFLICT(proxy_config_id) DO UPDATE SET
            name = excluded.name,
            proxy_url = excluded.proxy_url,
            proxy_username = excluded.proxy_username,
            proxy_password = excluded.proxy_password,
            status = excluded.status,
            created_at_ms = excluded.created_at_ms,
            updated_at_ms = excluded.updated_at_ms",
        params![
            string_field(&payload, "id")?,
            string_field(&payload, "name")?,
            string_field(&payload, "proxy_url")?,
            optional_string_field(&payload, "proxy_username")?,
            optional_string_field(&payload, "proxy_password")?,
            string_field(&payload, "status")?,
            i64_field(&payload, "created_at")?,
            i64_field(&payload, "updated_at")?,
        ],
    )
    .context("failed to upsert replayed proxy config")?;
    Ok(())
}

fn apply_proxy_binding_upsert(conn: &Connection, event: &SourceOutboxEvent) -> Result<()> {
    let payload: Value =
        serde_json::from_str(&event.payload_json).context("invalid proxy_binding payload")?;
    conn.execute(
        "INSERT INTO llm_proxy_bindings (
            provider_type, proxy_config_id, updated_at_ms
        ) VALUES (?1, ?2, ?3)
        ON CONFLICT(provider_type) DO UPDATE SET
            proxy_config_id = excluded.proxy_config_id,
            updated_at_ms = excluded.updated_at_ms",
        params![
            string_field(&payload, "provider_type")?,
            string_field(&payload, "proxy_config_id")?,
            i64_field(&payload, "updated_at")?,
        ],
    )
    .context("failed to upsert replayed proxy binding")?;
    Ok(())
}

fn apply_token_request_upsert(conn: &Connection, event: &SourceOutboxEvent) -> Result<()> {
    let payload: Value =
        serde_json::from_str(&event.payload_json).context("invalid token_request payload")?;
    conn.execute(
        "INSERT INTO llm_token_requests (
            request_id, requester_email, requested_quota_billable_limit, request_reason,
            frontend_page_url, status, fingerprint, client_ip, ip_region, admin_note,
            failure_reason, issued_key_id, issued_key_name, created_at_ms, updated_at_ms,
            processed_at_ms
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
        ON CONFLICT(request_id) DO UPDATE SET
            requester_email = excluded.requester_email,
            requested_quota_billable_limit = excluded.requested_quota_billable_limit,
            request_reason = excluded.request_reason,
            frontend_page_url = excluded.frontend_page_url,
            status = excluded.status,
            fingerprint = excluded.fingerprint,
            client_ip = excluded.client_ip,
            ip_region = excluded.ip_region,
            admin_note = excluded.admin_note,
            failure_reason = excluded.failure_reason,
            issued_key_id = excluded.issued_key_id,
            issued_key_name = excluded.issued_key_name,
            created_at_ms = excluded.created_at_ms,
            updated_at_ms = excluded.updated_at_ms,
            processed_at_ms = excluded.processed_at_ms",
        params![
            string_field(&payload, "request_id")?,
            string_field(&payload, "requester_email")?,
            u64_field(&payload, "requested_quota_billable_limit")?,
            string_field(&payload, "request_reason")?,
            optional_string_field(&payload, "frontend_page_url")?,
            string_field(&payload, "status")?,
            string_field(&payload, "fingerprint")?,
            string_field(&payload, "client_ip")?,
            string_field(&payload, "ip_region")?,
            optional_string_field(&payload, "admin_note")?,
            optional_string_field(&payload, "failure_reason")?,
            optional_string_field(&payload, "issued_key_id")?,
            optional_string_field(&payload, "issued_key_name")?,
            i64_field(&payload, "created_at")?,
            i64_field(&payload, "updated_at")?,
            optional_i64_field(&payload, "processed_at")?,
        ],
    )
    .context("failed to upsert replayed token request")?;
    Ok(())
}

fn apply_account_contribution_request_upsert(
    conn: &Connection,
    event: &SourceOutboxEvent,
) -> Result<()> {
    let payload: Value = serde_json::from_str(&event.payload_json)
        .context("invalid account_contribution_request payload")?;
    conn.execute(
        "INSERT INTO llm_account_contribution_requests (
            request_id, account_name, account_id, id_token, access_token, refresh_token,
            requester_email, contributor_message, github_id, frontend_page_url, status,
            fingerprint, client_ip, ip_region, admin_note, failure_reason,
            imported_account_name, issued_key_id, issued_key_name, created_at_ms,
            updated_at_ms, processed_at_ms
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, \
         ?19, ?20, ?21, ?22)
        ON CONFLICT(request_id) DO UPDATE SET
            account_name = excluded.account_name,
            account_id = excluded.account_id,
            id_token = excluded.id_token,
            access_token = excluded.access_token,
            refresh_token = excluded.refresh_token,
            requester_email = excluded.requester_email,
            contributor_message = excluded.contributor_message,
            github_id = excluded.github_id,
            frontend_page_url = excluded.frontend_page_url,
            status = excluded.status,
            fingerprint = excluded.fingerprint,
            client_ip = excluded.client_ip,
            ip_region = excluded.ip_region,
            admin_note = excluded.admin_note,
            failure_reason = excluded.failure_reason,
            imported_account_name = excluded.imported_account_name,
            issued_key_id = excluded.issued_key_id,
            issued_key_name = excluded.issued_key_name,
            created_at_ms = excluded.created_at_ms,
            updated_at_ms = excluded.updated_at_ms,
            processed_at_ms = excluded.processed_at_ms",
        params![
            string_field(&payload, "request_id")?,
            string_field(&payload, "account_name")?,
            optional_string_field(&payload, "account_id")?,
            string_field(&payload, "id_token")?,
            string_field(&payload, "access_token")?,
            string_field(&payload, "refresh_token")?,
            string_field(&payload, "requester_email")?,
            string_field(&payload, "contributor_message")?,
            optional_string_field(&payload, "github_id")?,
            optional_string_field(&payload, "frontend_page_url")?,
            string_field(&payload, "status")?,
            string_field(&payload, "fingerprint")?,
            string_field(&payload, "client_ip")?,
            string_field(&payload, "ip_region")?,
            optional_string_field(&payload, "admin_note")?,
            optional_string_field(&payload, "failure_reason")?,
            optional_string_field(&payload, "imported_account_name")?,
            optional_string_field(&payload, "issued_key_id")?,
            optional_string_field(&payload, "issued_key_name")?,
            i64_field(&payload, "created_at")?,
            i64_field(&payload, "updated_at")?,
            optional_i64_field(&payload, "processed_at")?,
        ],
    )
    .context("failed to upsert replayed account contribution request")?;
    Ok(())
}

fn apply_gpt2api_account_contribution_request_upsert(
    conn: &Connection,
    event: &SourceOutboxEvent,
) -> Result<()> {
    let payload: Value = serde_json::from_str(&event.payload_json)
        .context("invalid gpt2api_account_contribution_request payload")?;
    conn.execute(
        "INSERT INTO gpt2api_account_contribution_requests (
            request_id, account_name, access_token, session_json, requester_email,
            contributor_message, github_id, frontend_page_url, status, fingerprint,
            client_ip, ip_region, admin_note, failure_reason, imported_account_name,
            issued_key_id, issued_key_name, created_at_ms, updated_at_ms, processed_at_ms
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, \
         ?19, ?20)
        ON CONFLICT(request_id) DO UPDATE SET
            account_name = excluded.account_name,
            access_token = excluded.access_token,
            session_json = excluded.session_json,
            requester_email = excluded.requester_email,
            contributor_message = excluded.contributor_message,
            github_id = excluded.github_id,
            frontend_page_url = excluded.frontend_page_url,
            status = excluded.status,
            fingerprint = excluded.fingerprint,
            client_ip = excluded.client_ip,
            ip_region = excluded.ip_region,
            admin_note = excluded.admin_note,
            failure_reason = excluded.failure_reason,
            imported_account_name = excluded.imported_account_name,
            issued_key_id = excluded.issued_key_id,
            issued_key_name = excluded.issued_key_name,
            created_at_ms = excluded.created_at_ms,
            updated_at_ms = excluded.updated_at_ms,
            processed_at_ms = excluded.processed_at_ms",
        params![
            string_field(&payload, "request_id")?,
            string_field(&payload, "account_name")?,
            optional_string_field(&payload, "access_token")?,
            optional_string_field(&payload, "session_json")?,
            string_field(&payload, "requester_email")?,
            string_field(&payload, "contributor_message")?,
            optional_string_field(&payload, "github_id")?,
            optional_string_field(&payload, "frontend_page_url")?,
            string_field(&payload, "status")?,
            string_field(&payload, "fingerprint")?,
            string_field(&payload, "client_ip")?,
            string_field(&payload, "ip_region")?,
            optional_string_field(&payload, "admin_note")?,
            optional_string_field(&payload, "failure_reason")?,
            optional_string_field(&payload, "imported_account_name")?,
            optional_string_field(&payload, "issued_key_id")?,
            optional_string_field(&payload, "issued_key_name")?,
            i64_field(&payload, "created_at")?,
            i64_field(&payload, "updated_at")?,
            optional_i64_field(&payload, "processed_at")?,
        ],
    )
    .context("failed to upsert replayed gpt2api account contribution request")?;
    Ok(())
}

fn apply_sponsor_request_upsert(conn: &Connection, event: &SourceOutboxEvent) -> Result<()> {
    let payload: Value =
        serde_json::from_str(&event.payload_json).context("invalid sponsor_request payload")?;
    conn.execute(
        "INSERT INTO llm_sponsor_requests (
            request_id, requester_email, sponsor_message, display_name, github_id,
            frontend_page_url, status, fingerprint, client_ip, ip_region, admin_note,
            failure_reason, payment_email_sent_at_ms, created_at_ms, updated_at_ms,
            processed_at_ms
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
        ON CONFLICT(request_id) DO UPDATE SET
            requester_email = excluded.requester_email,
            sponsor_message = excluded.sponsor_message,
            display_name = excluded.display_name,
            github_id = excluded.github_id,
            frontend_page_url = excluded.frontend_page_url,
            status = excluded.status,
            fingerprint = excluded.fingerprint,
            client_ip = excluded.client_ip,
            ip_region = excluded.ip_region,
            admin_note = excluded.admin_note,
            failure_reason = excluded.failure_reason,
            payment_email_sent_at_ms = excluded.payment_email_sent_at_ms,
            created_at_ms = excluded.created_at_ms,
            updated_at_ms = excluded.updated_at_ms,
            processed_at_ms = excluded.processed_at_ms",
        params![
            string_field(&payload, "request_id")?,
            string_field(&payload, "requester_email")?,
            string_field(&payload, "sponsor_message")?,
            optional_string_field(&payload, "display_name")?,
            optional_string_field(&payload, "github_id")?,
            optional_string_field(&payload, "frontend_page_url")?,
            string_field(&payload, "status")?,
            string_field(&payload, "fingerprint")?,
            string_field(&payload, "client_ip")?,
            string_field(&payload, "ip_region")?,
            optional_string_field(&payload, "admin_note")?,
            optional_string_field(&payload, "failure_reason")?,
            optional_i64_field(&payload, "payment_email_sent_at")?,
            i64_field(&payload, "created_at")?,
            i64_field(&payload, "updated_at")?,
            optional_i64_field(&payload, "processed_at")?,
        ],
    )
    .context("failed to upsert replayed sponsor request")?;
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

fn json_text_field(payload: &Value, field: &str) -> Result<String> {
    serde_json::to_string(value_field(payload, field)?)
        .with_context(|| format!("failed to encode payload field `{field}` as JSON"))
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
    fn replays_runtime_config_and_account_group_events() {
        let source = source_outbox();
        let target = rusqlite::Connection::open_in_memory().expect("open target");
        llm_access_store::initialize_sqlite_target(&target).expect("initialize target");
        insert_source_event(
            &source,
            "event-runtime-config-upsert",
            "runtime_config",
            "upsert",
            "default",
            r#"{
                "id":"default",
                "auth_cache_ttl_seconds":60,
                "max_request_body_bytes":1048576,
                "account_failure_retry_limit":3,
                "codex_client_version":"0.124.0",
                "kiro_channel_max_concurrency":4,
                "kiro_channel_min_start_interval_ms":100,
                "codex_status_refresh_min_interval_seconds":240,
                "codex_status_refresh_max_interval_seconds":300,
                "codex_status_account_jitter_max_seconds":10,
                "kiro_status_refresh_min_interval_seconds":240,
                "kiro_status_refresh_max_interval_seconds":300,
                "kiro_status_account_jitter_max_seconds":10,
                "usage_event_flush_batch_size":32,
                "usage_event_flush_interval_seconds":5,
                "usage_event_flush_max_buffer_bytes":1048576,
                "usage_event_maintenance_enabled":true,
                "usage_event_maintenance_interval_seconds":3600,
                "usage_event_detail_retention_days":30,
                "kiro_cache_kmodels_json":"[]",
                "kiro_billable_model_multipliers_json":"{}",
                "kiro_cache_policy_json":"{}",
                "kiro_prefix_cache_mode":"formula",
                "kiro_prefix_cache_max_tokens":100000,
                "kiro_prefix_cache_entry_ttl_seconds":3600,
                "kiro_conversation_anchor_max_entries":1024,
                "kiro_conversation_anchor_ttl_seconds":3600,
                "updated_at":100
            }"#,
        );
        insert_source_event(
            &source,
            "event-account-group-upsert",
            "account_group",
            "upsert",
            "group-a",
            r#"{
                "id":"group-a",
                "provider_type":"kiro",
                "name":"Group A",
                "account_names":["a","b"],
                "created_at":10,
                "updated_at":200
            }"#,
        );

        let stats =
            super::replay_source_outbox_to_sqlite_target(&source, &target, &super::ReplayOptions {
                consumer_name: "test-consumer",
                max_events: 10,
            })
            .expect("replay source outbox");

        assert_eq!(stats.applied_events, 2);
        let codex_client_version: String = target
            .query_row(
                "SELECT codex_client_version FROM llm_runtime_config WHERE id = 'default'",
                [],
                |row| row.get(0),
            )
            .expect("read runtime config");
        let account_names_json: String = target
            .query_row(
                "SELECT account_names_json FROM llm_account_groups WHERE group_id = 'group-a'",
                [],
                |row| row.get(0),
            )
            .expect("read account group");
        assert_eq!(codex_client_version, "0.124.0");
        assert_eq!(account_names_json, r#"["a","b"]"#);
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
