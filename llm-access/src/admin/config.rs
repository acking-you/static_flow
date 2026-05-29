//! Runtime/gateway config GET/POST handlers, the runtime-config update
//! application, and the range/window validators that guard runtime config
//! fields.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) async fn get_llm_gateway_config(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match state.admin_config_store.get_admin_runtime_config().await {
        Ok(config) => Json(config).into_response(),
        Err(_) => internal_error("Failed to load llm gateway config").into_response(),
    }
}
pub(crate) async fn post_llm_gateway_config(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<UpdateAdminRuntimeConfig>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let current = match state.admin_config_store.get_admin_runtime_config().await {
        Ok(config) => config,
        Err(_) => return internal_error("Failed to load llm gateway config").into_response(),
    };
    let config = match apply_runtime_config_update(current, request) {
        Ok(config) => config,
        Err(response) => return response.into_response(),
    };
    match state
        .admin_config_store
        .update_admin_runtime_config(config)
        .await
    {
        Ok(config) => Json(config).into_response(),
        Err(_) => internal_error("Failed to update llm gateway config").into_response(),
    }
}
pub(crate) fn apply_runtime_config_update(
    current: AdminRuntimeConfig,
    request: UpdateAdminRuntimeConfig,
) -> Result<AdminRuntimeConfig, AdminHttpError> {
    let auth_cache_ttl_seconds = request
        .auth_cache_ttl_seconds
        .unwrap_or(current.auth_cache_ttl_seconds);
    validate_range(
        "auth_cache_ttl_seconds",
        auth_cache_ttl_seconds,
        MIN_RUNTIME_CACHE_TTL_SECONDS,
        MAX_RUNTIME_CACHE_TTL_SECONDS,
    )?;

    let max_request_body_bytes = request
        .max_request_body_bytes
        .unwrap_or(current.max_request_body_bytes);
    validate_range(
        "max_request_body_bytes",
        max_request_body_bytes,
        MIN_RUNTIME_REQUEST_BODY_BYTES,
        MAX_RUNTIME_REQUEST_BODY_BYTES,
    )?;

    let account_failure_retry_limit = request
        .account_failure_retry_limit
        .unwrap_or(current.account_failure_retry_limit);
    validate_range(
        "account_failure_retry_limit",
        account_failure_retry_limit,
        MIN_RUNTIME_ACCOUNT_FAILURE_RETRY_LIMIT,
        MAX_RUNTIME_ACCOUNT_FAILURE_RETRY_LIMIT,
    )?;

    let codex_client_version = match request.codex_client_version.as_deref() {
        Some(value) => normalize_codex_client_version(value)
            .ok_or_else(|| bad_request("codex_client_version is invalid"))?,
        None => current.codex_client_version,
    };

    let codex_status_refresh_min_interval_seconds = request
        .codex_status_refresh_min_interval_seconds
        .unwrap_or(current.codex_status_refresh_min_interval_seconds);
    let codex_status_refresh_max_interval_seconds = request
        .codex_status_refresh_max_interval_seconds
        .unwrap_or(current.codex_status_refresh_max_interval_seconds);
    validate_runtime_refresh_window(
        codex_status_refresh_min_interval_seconds,
        codex_status_refresh_max_interval_seconds,
    )?;
    let codex_status_account_jitter_max_seconds = request
        .codex_status_account_jitter_max_seconds
        .unwrap_or(current.codex_status_account_jitter_max_seconds);
    validate_max(
        "codex_status_account_jitter_max_seconds",
        codex_status_account_jitter_max_seconds,
        MAX_RUNTIME_STATUS_ACCOUNT_JITTER_SECONDS,
    )?;
    let codex_weight_free = request
        .codex_weight_free
        .unwrap_or(current.codex_weight_free);
    let codex_weight_plus = request
        .codex_weight_plus
        .unwrap_or(current.codex_weight_plus);
    let codex_weight_pro5x = request
        .codex_weight_pro5x
        .unwrap_or(current.codex_weight_pro5x);
    let codex_weight_pro20x = request
        .codex_weight_pro20x
        .unwrap_or(current.codex_weight_pro20x);
    validate_max("codex_weight_free", codex_weight_free, u64::MAX)?;
    validate_max("codex_weight_plus", codex_weight_plus, u64::MAX)?;
    validate_max("codex_weight_pro5x", codex_weight_pro5x, u64::MAX)?;
    validate_max("codex_weight_pro20x", codex_weight_pro20x, u64::MAX)?;

    let kiro_status_refresh_min_interval_seconds = request
        .kiro_status_refresh_min_interval_seconds
        .unwrap_or(current.kiro_status_refresh_min_interval_seconds);
    let kiro_status_refresh_max_interval_seconds = request
        .kiro_status_refresh_max_interval_seconds
        .unwrap_or(current.kiro_status_refresh_max_interval_seconds);
    validate_runtime_refresh_window(
        kiro_status_refresh_min_interval_seconds,
        kiro_status_refresh_max_interval_seconds,
    )?;
    let kiro_status_account_jitter_max_seconds = request
        .kiro_status_account_jitter_max_seconds
        .unwrap_or(current.kiro_status_account_jitter_max_seconds);
    validate_max(
        "kiro_status_account_jitter_max_seconds",
        kiro_status_account_jitter_max_seconds,
        MAX_RUNTIME_STATUS_ACCOUNT_JITTER_SECONDS,
    )?;

    let usage_event_flush_batch_size = request
        .usage_event_flush_batch_size
        .unwrap_or(current.usage_event_flush_batch_size);
    validate_range(
        "usage_event_flush_batch_size",
        usage_event_flush_batch_size,
        MIN_RUNTIME_USAGE_EVENT_FLUSH_BATCH_SIZE,
        MAX_RUNTIME_USAGE_EVENT_FLUSH_BATCH_SIZE,
    )?;
    let usage_event_flush_interval_seconds = request
        .usage_event_flush_interval_seconds
        .unwrap_or(current.usage_event_flush_interval_seconds);
    validate_range(
        "usage_event_flush_interval_seconds",
        usage_event_flush_interval_seconds,
        MIN_RUNTIME_USAGE_EVENT_FLUSH_INTERVAL_SECONDS,
        MAX_RUNTIME_USAGE_EVENT_FLUSH_INTERVAL_SECONDS,
    )?;
    let usage_event_flush_max_buffer_bytes = request
        .usage_event_flush_max_buffer_bytes
        .unwrap_or(current.usage_event_flush_max_buffer_bytes);
    validate_range(
        "usage_event_flush_max_buffer_bytes",
        usage_event_flush_max_buffer_bytes,
        MIN_RUNTIME_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES,
        MAX_RUNTIME_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES,
    )?;
    let duckdb_usage_memory_limit_mib = request
        .duckdb_usage_memory_limit_mib
        .unwrap_or(current.duckdb_usage_memory_limit_mib);
    validate_range(
        "duckdb_usage_memory_limit_mib",
        duckdb_usage_memory_limit_mib,
        MIN_RUNTIME_DUCKDB_USAGE_MEMORY_LIMIT_MIB,
        MAX_RUNTIME_DUCKDB_USAGE_MEMORY_LIMIT_MIB,
    )?;
    let duckdb_usage_checkpoint_threshold_mib = request
        .duckdb_usage_checkpoint_threshold_mib
        .unwrap_or(current.duckdb_usage_checkpoint_threshold_mib);
    validate_range(
        "duckdb_usage_checkpoint_threshold_mib",
        duckdb_usage_checkpoint_threshold_mib,
        MIN_RUNTIME_DUCKDB_USAGE_CHECKPOINT_THRESHOLD_MIB,
        MAX_RUNTIME_DUCKDB_USAGE_CHECKPOINT_THRESHOLD_MIB,
    )?;
    let usage_analytics_retention_days = request
        .usage_analytics_retention_days
        .unwrap_or(current.usage_analytics_retention_days);
    validate_range(
        "usage_analytics_retention_days",
        usage_analytics_retention_days,
        MIN_RUNTIME_USAGE_ANALYTICS_RETENTION_DAYS,
        MAX_RUNTIME_USAGE_ANALYTICS_RETENTION_DAYS,
    )?;

    let usage_journal_enabled = request
        .usage_journal_enabled
        .unwrap_or(current.usage_journal_enabled);
    let usage_journal_max_file_bytes = request
        .usage_journal_max_file_bytes
        .unwrap_or(current.usage_journal_max_file_bytes);
    validate_range(
        "usage_journal_max_file_bytes",
        usage_journal_max_file_bytes,
        MIN_RUNTIME_USAGE_JOURNAL_FILE_BYTES,
        MAX_RUNTIME_USAGE_JOURNAL_FILE_BYTES,
    )?;
    let usage_journal_max_file_age_ms = request
        .usage_journal_max_file_age_ms
        .unwrap_or(current.usage_journal_max_file_age_ms);
    validate_range(
        "usage_journal_max_file_age_ms",
        usage_journal_max_file_age_ms,
        MIN_RUNTIME_USAGE_JOURNAL_FILE_AGE_MS,
        MAX_RUNTIME_USAGE_JOURNAL_FILE_AGE_MS,
    )?;
    let usage_journal_max_files = request
        .usage_journal_max_files
        .unwrap_or(current.usage_journal_max_files);
    validate_range(
        "usage_journal_max_files",
        usage_journal_max_files,
        1,
        MAX_RUNTIME_USAGE_JOURNAL_FILES,
    )?;
    let usage_journal_block_target_uncompressed_bytes = request
        .usage_journal_block_target_uncompressed_bytes
        .unwrap_or(current.usage_journal_block_target_uncompressed_bytes);
    validate_range(
        "usage_journal_block_target_uncompressed_bytes",
        usage_journal_block_target_uncompressed_bytes,
        MIN_RUNTIME_USAGE_JOURNAL_BLOCK_BYTES,
        MAX_RUNTIME_USAGE_JOURNAL_BLOCK_BYTES,
    )?;
    let usage_journal_block_max_events = request
        .usage_journal_block_max_events
        .unwrap_or(current.usage_journal_block_max_events);
    validate_range(
        "usage_journal_block_max_events",
        usage_journal_block_max_events,
        1,
        MAX_RUNTIME_USAGE_JOURNAL_BLOCK_EVENTS,
    )?;
    let usage_journal_fsync_interval_ms = request
        .usage_journal_fsync_interval_ms
        .unwrap_or(current.usage_journal_fsync_interval_ms);
    validate_max(
        "usage_journal_fsync_interval_ms",
        usage_journal_fsync_interval_ms,
        MAX_RUNTIME_USAGE_JOURNAL_FSYNC_INTERVAL_MS,
    )?;
    let usage_journal_zstd_level = request
        .usage_journal_zstd_level
        .unwrap_or(current.usage_journal_zstd_level);
    validate_i64_range(
        "usage_journal_zstd_level",
        usage_journal_zstd_level,
        0,
        MAX_RUNTIME_USAGE_JOURNAL_ZSTD_LEVEL,
    )?;
    let usage_journal_consumer_lease_ms = request
        .usage_journal_consumer_lease_ms
        .unwrap_or(current.usage_journal_consumer_lease_ms);
    validate_range(
        "usage_journal_consumer_lease_ms",
        usage_journal_consumer_lease_ms,
        MIN_RUNTIME_USAGE_JOURNAL_CONSUMER_LEASE_MS,
        MAX_RUNTIME_USAGE_JOURNAL_CONSUMER_LEASE_MS,
    )?;
    let usage_journal_delete_bad_files = request
        .usage_journal_delete_bad_files
        .unwrap_or(current.usage_journal_delete_bad_files);
    let usage_query_bind_addr = match request.usage_query_bind_addr.as_deref() {
        Some(value) => normalize_usage_query_bind_addr(value)?,
        None => current.usage_query_bind_addr,
    };
    let usage_query_base_url = match request.usage_query_base_url.as_deref() {
        Some(value) => normalize_usage_query_base_url(value)?,
        None => current.usage_query_base_url,
    };

    let kiro_cache_kmodels_json = request
        .kiro_cache_kmodels_json
        .unwrap_or(current.kiro_cache_kmodels_json);
    parse_kiro_cache_kmodels_json(&kiro_cache_kmodels_json)
        .map_err(|_| bad_request("kiro_cache_kmodels_json is invalid"))?;

    let kiro_billable_model_multipliers_json = match request.kiro_billable_model_multipliers_json {
        Some(value) => {
            let multipliers = parse_kiro_billable_model_multipliers_json(&value)
                .map_err(|_| bad_request("kiro_billable_model_multipliers_json is invalid"))?;
            serde_json::to_string(&multipliers).map_err(|_| {
                internal_error("Failed to normalize kiro billable multiplier config")
            })?
        },
        None => current.kiro_billable_model_multipliers_json,
    };

    let kiro_cache_policy_json = request
        .kiro_cache_policy_json
        .unwrap_or(current.kiro_cache_policy_json);
    parse_kiro_cache_policy_json(&kiro_cache_policy_json)
        .map_err(|_| bad_request("kiro_cache_policy_json is invalid"))?;

    let kiro_context_usage_min_request_tokens = request
        .kiro_context_usage_min_request_tokens
        .unwrap_or(current.kiro_context_usage_min_request_tokens);
    validate_range(
        "kiro_context_usage_min_request_tokens",
        kiro_context_usage_min_request_tokens,
        1,
        MAX_RUNTIME_KIRO_CONTEXT_USAGE_MIN_REQUEST_TOKENS,
    )?;

    let kiro_prefix_cache_mode = request
        .kiro_prefix_cache_mode
        .unwrap_or(current.kiro_prefix_cache_mode);
    validate_kiro_prefix_cache_mode(&kiro_prefix_cache_mode)?;

    let kiro_prefix_cache_max_tokens = request
        .kiro_prefix_cache_max_tokens
        .unwrap_or(current.kiro_prefix_cache_max_tokens);
    validate_positive("kiro_prefix_cache_max_tokens", kiro_prefix_cache_max_tokens)?;
    let kiro_prefix_cache_entry_ttl_seconds = request
        .kiro_prefix_cache_entry_ttl_seconds
        .unwrap_or(current.kiro_prefix_cache_entry_ttl_seconds);
    validate_positive("kiro_prefix_cache_entry_ttl_seconds", kiro_prefix_cache_entry_ttl_seconds)?;
    let kiro_conversation_anchor_max_entries = request
        .kiro_conversation_anchor_max_entries
        .unwrap_or(current.kiro_conversation_anchor_max_entries);
    validate_positive(
        "kiro_conversation_anchor_max_entries",
        kiro_conversation_anchor_max_entries,
    )?;
    let kiro_conversation_anchor_ttl_seconds = request
        .kiro_conversation_anchor_ttl_seconds
        .unwrap_or(current.kiro_conversation_anchor_ttl_seconds);
    validate_positive(
        "kiro_conversation_anchor_ttl_seconds",
        kiro_conversation_anchor_ttl_seconds,
    )?;

    Ok(AdminRuntimeConfig {
        auth_cache_ttl_seconds,
        max_request_body_bytes,
        account_failure_retry_limit,
        codex_client_version,
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
        kiro_cache_kmodels_json,
        kiro_billable_model_multipliers_json,
        kiro_cache_policy_json,
        kiro_context_usage_min_request_tokens,
        kiro_prefix_cache_mode,
        kiro_prefix_cache_max_tokens,
        kiro_prefix_cache_entry_ttl_seconds,
        kiro_conversation_anchor_max_entries,
        kiro_conversation_anchor_ttl_seconds,
    })
}
pub(crate) fn validate_range(
    field: &str,
    value: u64,
    min: u64,
    max: u64,
) -> Result<(), AdminHttpError> {
    if (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(bad_request(&format!("{field} is out of range")))
    }
}
pub(crate) fn validate_i64_range(
    field: &str,
    value: i64,
    min: i64,
    max: i64,
) -> Result<(), AdminHttpError> {
    if (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(bad_request(&format!("{field} is out of range")))
    }
}
pub(crate) fn validate_max(field: &str, value: u64, max: u64) -> Result<(), AdminHttpError> {
    if value <= max {
        Ok(())
    } else {
        Err(bad_request(&format!("{field} is out of range")))
    }
}
pub(crate) fn validate_positive(field: &str, value: u64) -> Result<(), AdminHttpError> {
    if value > 0 {
        Ok(())
    } else {
        Err(bad_request(&format!("{field} must be positive")))
    }
}
pub(crate) fn normalize_usage_query_bind_addr(value: &str) -> Result<String, AdminHttpError> {
    let trimmed = value.trim();
    if trimmed.parse::<SocketAddr>().is_ok() {
        Ok(trimmed.to_string())
    } else {
        Err(bad_request("usage_query_bind_addr is invalid"))
    }
}
pub(crate) fn normalize_usage_query_base_url(value: &str) -> Result<String, AdminHttpError> {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        Ok(trimmed.to_string())
    } else {
        Err(bad_request("usage_query_base_url is invalid"))
    }
}
pub(crate) fn validate_runtime_refresh_window(
    min_seconds: u64,
    max_seconds: u64,
) -> Result<(), AdminHttpError> {
    if !(MIN_RUNTIME_STATUS_REFRESH_INTERVAL_SECONDS..=MAX_RUNTIME_STATUS_REFRESH_INTERVAL_SECONDS)
        .contains(&min_seconds)
        || !(MIN_RUNTIME_STATUS_REFRESH_INTERVAL_SECONDS
            ..=MAX_RUNTIME_STATUS_REFRESH_INTERVAL_SECONDS)
            .contains(&max_seconds)
    {
        return Err(bad_request("refresh window seconds must be between 240 and 3600"));
    }
    if min_seconds > max_seconds {
        return Err(bad_request("refresh min interval must be less than or equal to max interval"));
    }
    Ok(())
}
pub(crate) fn validate_kiro_prefix_cache_mode(mode: &str) -> Result<(), AdminHttpError> {
    if matches!(mode, KIRO_PREFIX_CACHE_MODE_FORMULA | core_store::DEFAULT_KIRO_PREFIX_CACHE_MODE) {
        Ok(())
    } else {
        Err(bad_request("kiro_prefix_cache_mode is invalid"))
    }
}
pub(crate) fn validate_i64_backed_u64(field: &str, value: u64) -> Result<(), AdminHttpError> {
    if value <= i64::MAX as u64 {
        Ok(())
    } else {
        Err(bad_request(&format!("{field} is out of range")))
    }
}
