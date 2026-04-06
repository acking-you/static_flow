//! Arrow batch encoders/decoders for the LLM gateway store.
//!
//! These helpers keep LanceDB serialization in one place so schema changes are
//! reflected consistently across table writes, reads, and migrations.

use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Context, Result};
use arrow_array::{
    builder::{
        BooleanBuilder, Float64Builder, Int32Builder, StringBuilder, TimestampMillisecondBuilder,
        UInt64Builder,
    },
    Array, ArrayRef, BooleanArray, Float64Array, Int32Array, RecordBatch, StringArray,
    TimestampMillisecondArray, UInt64Array,
};

use super::{
    schema::{
        llm_gateway_account_contribution_requests_schema, llm_gateway_keys_schema,
        llm_gateway_proxy_bindings_schema, llm_gateway_proxy_configs_schema,
        llm_gateway_runtime_config_schema, llm_gateway_sponsor_requests_schema,
        llm_gateway_token_requests_schema, llm_gateway_usage_events_schema,
    },
    types::{
        default_kiro_cache_kmodels_json, LlmGatewayAccountContributionRequestRecord,
        LlmGatewayKeyRecord, LlmGatewayProxyBindingRecord, LlmGatewayProxyConfigRecord,
        LlmGatewayRuntimeConfigRecord, LlmGatewaySponsorRequestRecord,
        LlmGatewayTokenRequestRecord, LlmGatewayUsageEventRecord,
        DEFAULT_CODEX_STATUS_ACCOUNT_JITTER_MAX_SECONDS,
        DEFAULT_CODEX_STATUS_REFRESH_MAX_INTERVAL_SECONDS,
        DEFAULT_CODEX_STATUS_REFRESH_MIN_INTERVAL_SECONDS, DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY,
        DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS, DEFAULT_KIRO_CONVERSATION_ANCHOR_MAX_ENTRIES,
        DEFAULT_KIRO_CONVERSATION_ANCHOR_TTL_SECONDS, DEFAULT_KIRO_PREFIX_CACHE_ENTRY_TTL_SECONDS,
        DEFAULT_KIRO_PREFIX_CACHE_MAX_TOKENS, DEFAULT_KIRO_PREFIX_CACHE_MODE,
        DEFAULT_KIRO_STATUS_ACCOUNT_JITTER_MAX_SECONDS,
        DEFAULT_KIRO_STATUS_REFRESH_MAX_INTERVAL_SECONDS,
        DEFAULT_KIRO_STATUS_REFRESH_MIN_INTERVAL_SECONDS,
        DEFAULT_LLM_GATEWAY_ACCOUNT_FAILURE_RETRY_LIMIT,
        DEFAULT_LLM_GATEWAY_MAX_REQUEST_BODY_BYTES,
        DEFAULT_LLM_GATEWAY_USAGE_EVENT_FLUSH_BATCH_SIZE,
        DEFAULT_LLM_GATEWAY_USAGE_EVENT_FLUSH_INTERVAL_SECONDS,
        DEFAULT_LLM_GATEWAY_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES, LLM_GATEWAY_PROTOCOL_OPENAI,
        LLM_GATEWAY_PROVIDER_CODEX, LLM_GATEWAY_PROVIDER_KIRO,
    },
};

/// Serialize a slice of [`LlmGatewayKeyRecord`] into an Arrow [`RecordBatch`].
///
/// Includes the `provider_type` and `protocol_family` columns added for
/// multi-provider gateway support.
pub fn build_keys_batch(records: &[LlmGatewayKeyRecord]) -> Result<RecordBatch> {
    let schema = llm_gateway_keys_schema();
    let mut id = StringBuilder::new();
    let mut name = StringBuilder::new();
    let mut secret = StringBuilder::new();
    let mut key_hash = StringBuilder::new();
    let mut status = StringBuilder::new();
    let mut provider_type = StringBuilder::new(); // upstream LLM provider (e.g. "codex", "anthropic")
    let mut protocol_family = StringBuilder::new(); // wire protocol dialect (e.g. "openai")
    let mut public_visible = BooleanBuilder::new();
    let mut quota_billable_limit = UInt64Builder::new();
    let mut usage_input_uncached_tokens = UInt64Builder::new();
    let mut usage_input_cached_tokens = UInt64Builder::new();
    let mut usage_output_tokens = UInt64Builder::new();
    let mut usage_billable_tokens = UInt64Builder::new();
    let mut usage_credit_total = Float64Builder::new();
    let mut usage_credit_missing_events = UInt64Builder::new();
    let mut last_used_at = TimestampMillisecondBuilder::new();
    let mut created_at = TimestampMillisecondBuilder::new();
    let mut updated_at = TimestampMillisecondBuilder::new();
    let mut route_strategy = StringBuilder::new();
    let mut fixed_account_name = StringBuilder::new();
    let mut auto_account_names_json = StringBuilder::new();
    let mut model_name_map_json = StringBuilder::new();
    let mut request_max_concurrency = UInt64Builder::new();
    let mut request_min_start_interval_ms = UInt64Builder::new();
    let mut kiro_request_validation_enabled = BooleanBuilder::new();
    let mut kiro_cache_estimation_enabled = BooleanBuilder::new();

    for record in records {
        id.append_value(&record.id);
        name.append_value(&record.name);
        secret.append_value(&record.secret);
        key_hash.append_value(&record.key_hash);
        status.append_value(&record.status);
        provider_type.append_value(&record.provider_type);
        protocol_family.append_value(&record.protocol_family);
        public_visible.append_value(record.public_visible);
        quota_billable_limit.append_value(record.quota_billable_limit);
        usage_input_uncached_tokens.append_value(record.usage_input_uncached_tokens);
        usage_input_cached_tokens.append_value(record.usage_input_cached_tokens);
        usage_output_tokens.append_value(record.usage_output_tokens);
        usage_billable_tokens.append_value(record.usage_billable_tokens);
        usage_credit_total.append_value(record.usage_credit_total);
        usage_credit_missing_events.append_value(record.usage_credit_missing_events);
        append_optional_ts(&mut last_used_at, record.last_used_at);
        created_at.append_value(record.created_at);
        updated_at.append_value(record.updated_at);
        append_optional_str(&mut route_strategy, record.route_strategy.as_deref());
        append_optional_str(&mut fixed_account_name, record.fixed_account_name.as_deref());
        append_optional_str(
            &mut auto_account_names_json,
            serialize_string_vec_json(record.auto_account_names.as_deref())?.as_deref(),
        );
        append_optional_str(
            &mut model_name_map_json,
            serialize_string_map_json(record.model_name_map.as_ref())?.as_deref(),
        );
        append_optional_u64(&mut request_max_concurrency, record.request_max_concurrency);
        append_optional_u64(
            &mut request_min_start_interval_ms,
            record.request_min_start_interval_ms,
        );
        kiro_request_validation_enabled.append_value(record.kiro_request_validation_enabled);
        kiro_cache_estimation_enabled.append_value(record.kiro_cache_estimation_enabled);
    }

    RecordBatch::try_new(schema, vec![
        Arc::new(id.finish()) as ArrayRef,
        Arc::new(name.finish()),
        Arc::new(secret.finish()),
        Arc::new(key_hash.finish()),
        Arc::new(status.finish()),
        Arc::new(provider_type.finish()),
        Arc::new(protocol_family.finish()),
        Arc::new(public_visible.finish()),
        Arc::new(quota_billable_limit.finish()),
        Arc::new(usage_input_uncached_tokens.finish()),
        Arc::new(usage_input_cached_tokens.finish()),
        Arc::new(usage_output_tokens.finish()),
        Arc::new(usage_billable_tokens.finish()),
        Arc::new(usage_credit_total.finish()),
        Arc::new(usage_credit_missing_events.finish()),
        Arc::new(last_used_at.finish()),
        Arc::new(created_at.finish()),
        Arc::new(updated_at.finish()),
        Arc::new(route_strategy.finish()),
        Arc::new(fixed_account_name.finish()),
        Arc::new(auto_account_names_json.finish()),
        Arc::new(model_name_map_json.finish()),
        Arc::new(request_max_concurrency.finish()),
        Arc::new(request_min_start_interval_ms.finish()),
        Arc::new(kiro_request_validation_enabled.finish()),
        Arc::new(kiro_cache_estimation_enabled.finish()),
    ])
    .context("failed to build llm gateway keys batch")
}

/// Serialize a slice of [`LlmGatewayUsageEventRecord`] into an Arrow
/// [`RecordBatch`].
///
/// The `provider_type` column records which upstream provider served the
/// request.
pub fn build_usage_events_batch(records: &[LlmGatewayUsageEventRecord]) -> Result<RecordBatch> {
    let schema = llm_gateway_usage_events_schema();
    let mut id = StringBuilder::new();
    let mut key_id = StringBuilder::new();
    let mut key_name = StringBuilder::new();
    let mut provider_type = StringBuilder::new(); // upstream LLM provider that handled this event
    let mut account_name = StringBuilder::new();
    let mut request_method = StringBuilder::new();
    let mut request_url = StringBuilder::new();
    let mut latency_ms = Int32Builder::new();
    let mut endpoint = StringBuilder::new();
    let mut model = StringBuilder::new();
    let mut status_code = Int32Builder::new();
    let mut input_uncached_tokens = UInt64Builder::new();
    let mut input_cached_tokens = UInt64Builder::new();
    let mut output_tokens = UInt64Builder::new();
    let mut billable_tokens = UInt64Builder::new();
    let mut usage_missing = BooleanBuilder::new();
    let mut credit_usage = Float64Builder::new();
    let mut credit_usage_missing = BooleanBuilder::new();
    let mut client_ip = StringBuilder::new();
    let mut ip_region = StringBuilder::new();
    let mut request_headers_json = StringBuilder::new();
    let mut last_message_content = StringBuilder::new();
    let mut created_at = TimestampMillisecondBuilder::new();

    for record in records {
        id.append_value(&record.id);
        key_id.append_value(&record.key_id);
        key_name.append_value(&record.key_name);
        provider_type.append_value(&record.provider_type);
        append_optional_str(&mut account_name, record.account_name.as_deref());
        request_method.append_value(&record.request_method);
        request_url.append_value(&record.request_url);
        latency_ms.append_value(record.latency_ms);
        endpoint.append_value(&record.endpoint);
        append_optional_str(&mut model, record.model.as_deref());
        status_code.append_value(record.status_code);
        input_uncached_tokens.append_value(record.input_uncached_tokens);
        input_cached_tokens.append_value(record.input_cached_tokens);
        output_tokens.append_value(record.output_tokens);
        billable_tokens.append_value(record.billable_tokens);
        usage_missing.append_value(record.usage_missing);
        append_optional_f64(&mut credit_usage, record.credit_usage);
        credit_usage_missing.append_value(record.credit_usage_missing);
        client_ip.append_value(&record.client_ip);
        ip_region.append_value(&record.ip_region);
        request_headers_json.append_value(&record.request_headers_json);
        append_optional_str(&mut last_message_content, record.last_message_content.as_deref());
        created_at.append_value(record.created_at);
    }

    RecordBatch::try_new(schema, vec![
        Arc::new(id.finish()) as ArrayRef,
        Arc::new(key_id.finish()),
        Arc::new(key_name.finish()),
        Arc::new(provider_type.finish()),
        Arc::new(account_name.finish()),
        Arc::new(request_method.finish()),
        Arc::new(request_url.finish()),
        Arc::new(latency_ms.finish()),
        Arc::new(endpoint.finish()),
        Arc::new(model.finish()),
        Arc::new(status_code.finish()),
        Arc::new(input_uncached_tokens.finish()),
        Arc::new(input_cached_tokens.finish()),
        Arc::new(output_tokens.finish()),
        Arc::new(billable_tokens.finish()),
        Arc::new(usage_missing.finish()),
        Arc::new(credit_usage.finish()),
        Arc::new(credit_usage_missing.finish()),
        Arc::new(client_ip.finish()),
        Arc::new(ip_region.finish()),
        Arc::new(request_headers_json.finish()),
        Arc::new(last_message_content.finish()),
        Arc::new(created_at.finish()),
    ])
    .context("failed to build llm gateway usage events batch")
}

/// Serialize a slice of [`LlmGatewayRuntimeConfigRecord`] into an Arrow
/// [`RecordBatch`].
///
/// Includes the `max_request_body_bytes` column for per-instance request size
/// limits.
pub fn build_runtime_config_batch(
    records: &[LlmGatewayRuntimeConfigRecord],
) -> Result<RecordBatch> {
    let schema = llm_gateway_runtime_config_schema();
    let mut id = StringBuilder::new();
    let mut auth_cache_ttl_seconds = UInt64Builder::new();
    let mut max_request_body_bytes = UInt64Builder::new(); // max allowed request body size in bytes
    let mut account_failure_retry_limit = UInt64Builder::new();
    let mut kiro_channel_max_concurrency = UInt64Builder::new();
    let mut kiro_channel_min_start_interval_ms = UInt64Builder::new();
    let mut codex_status_refresh_min_interval_seconds = UInt64Builder::new();
    let mut codex_status_refresh_max_interval_seconds = UInt64Builder::new();
    let mut codex_status_account_jitter_max_seconds = UInt64Builder::new();
    let mut kiro_status_refresh_min_interval_seconds = UInt64Builder::new();
    let mut kiro_status_refresh_max_interval_seconds = UInt64Builder::new();
    let mut kiro_status_account_jitter_max_seconds = UInt64Builder::new();
    let mut usage_event_flush_batch_size = UInt64Builder::new();
    let mut usage_event_flush_interval_seconds = UInt64Builder::new();
    let mut usage_event_flush_max_buffer_bytes = UInt64Builder::new();
    let mut kiro_cache_kmodels_json = StringBuilder::new();
    let mut kiro_prefix_cache_mode = StringBuilder::new();
    let mut kiro_prefix_cache_max_tokens = UInt64Builder::new();
    let mut kiro_prefix_cache_entry_ttl_seconds = UInt64Builder::new();
    let mut kiro_conversation_anchor_max_entries = UInt64Builder::new();
    let mut kiro_conversation_anchor_ttl_seconds = UInt64Builder::new();
    let mut updated_at = TimestampMillisecondBuilder::new();

    for record in records {
        id.append_value(&record.id);
        auth_cache_ttl_seconds.append_value(record.auth_cache_ttl_seconds);
        max_request_body_bytes.append_value(record.max_request_body_bytes);
        account_failure_retry_limit.append_value(record.account_failure_retry_limit);
        kiro_channel_max_concurrency.append_value(record.kiro_channel_max_concurrency);
        kiro_channel_min_start_interval_ms.append_value(record.kiro_channel_min_start_interval_ms);
        codex_status_refresh_min_interval_seconds
            .append_value(record.codex_status_refresh_min_interval_seconds);
        codex_status_refresh_max_interval_seconds
            .append_value(record.codex_status_refresh_max_interval_seconds);
        codex_status_account_jitter_max_seconds
            .append_value(record.codex_status_account_jitter_max_seconds);
        kiro_status_refresh_min_interval_seconds
            .append_value(record.kiro_status_refresh_min_interval_seconds);
        kiro_status_refresh_max_interval_seconds
            .append_value(record.kiro_status_refresh_max_interval_seconds);
        kiro_status_account_jitter_max_seconds
            .append_value(record.kiro_status_account_jitter_max_seconds);
        usage_event_flush_batch_size.append_value(record.usage_event_flush_batch_size);
        usage_event_flush_interval_seconds.append_value(record.usage_event_flush_interval_seconds);
        usage_event_flush_max_buffer_bytes.append_value(record.usage_event_flush_max_buffer_bytes);
        kiro_cache_kmodels_json.append_value(&record.kiro_cache_kmodels_json);
        kiro_prefix_cache_mode.append_value(&record.kiro_prefix_cache_mode);
        kiro_prefix_cache_max_tokens.append_value(record.kiro_prefix_cache_max_tokens);
        kiro_prefix_cache_entry_ttl_seconds
            .append_value(record.kiro_prefix_cache_entry_ttl_seconds);
        kiro_conversation_anchor_max_entries
            .append_value(record.kiro_conversation_anchor_max_entries);
        kiro_conversation_anchor_ttl_seconds
            .append_value(record.kiro_conversation_anchor_ttl_seconds);
        updated_at.append_value(record.updated_at);
    }

    RecordBatch::try_new(schema, vec![
        Arc::new(id.finish()) as ArrayRef,
        Arc::new(auth_cache_ttl_seconds.finish()),
        Arc::new(max_request_body_bytes.finish()),
        Arc::new(account_failure_retry_limit.finish()),
        Arc::new(kiro_channel_max_concurrency.finish()),
        Arc::new(kiro_channel_min_start_interval_ms.finish()),
        Arc::new(codex_status_refresh_min_interval_seconds.finish()),
        Arc::new(codex_status_refresh_max_interval_seconds.finish()),
        Arc::new(codex_status_account_jitter_max_seconds.finish()),
        Arc::new(kiro_status_refresh_min_interval_seconds.finish()),
        Arc::new(kiro_status_refresh_max_interval_seconds.finish()),
        Arc::new(kiro_status_account_jitter_max_seconds.finish()),
        Arc::new(usage_event_flush_batch_size.finish()),
        Arc::new(usage_event_flush_interval_seconds.finish()),
        Arc::new(usage_event_flush_max_buffer_bytes.finish()),
        Arc::new(kiro_cache_kmodels_json.finish()),
        Arc::new(kiro_prefix_cache_mode.finish()),
        Arc::new(kiro_prefix_cache_max_tokens.finish()),
        Arc::new(kiro_prefix_cache_entry_ttl_seconds.finish()),
        Arc::new(kiro_conversation_anchor_max_entries.finish()),
        Arc::new(kiro_conversation_anchor_ttl_seconds.finish()),
        Arc::new(updated_at.finish()),
    ])
    .context("failed to build llm gateway runtime config batch")
}

pub fn build_proxy_configs_batch(records: &[LlmGatewayProxyConfigRecord]) -> Result<RecordBatch> {
    let schema = llm_gateway_proxy_configs_schema();
    let mut id = StringBuilder::new();
    let mut name = StringBuilder::new();
    let mut proxy_url = StringBuilder::new();
    let mut proxy_username = StringBuilder::new();
    let mut proxy_password = StringBuilder::new();
    let mut status = StringBuilder::new();
    let mut created_at = TimestampMillisecondBuilder::new();
    let mut updated_at = TimestampMillisecondBuilder::new();

    for record in records {
        id.append_value(&record.id);
        name.append_value(&record.name);
        proxy_url.append_value(&record.proxy_url);
        append_optional_str(&mut proxy_username, record.proxy_username.as_deref());
        append_optional_str(&mut proxy_password, record.proxy_password.as_deref());
        status.append_value(&record.status);
        created_at.append_value(record.created_at);
        updated_at.append_value(record.updated_at);
    }

    RecordBatch::try_new(schema, vec![
        Arc::new(id.finish()) as ArrayRef,
        Arc::new(name.finish()),
        Arc::new(proxy_url.finish()),
        Arc::new(proxy_username.finish()),
        Arc::new(proxy_password.finish()),
        Arc::new(status.finish()),
        Arc::new(created_at.finish()),
        Arc::new(updated_at.finish()),
    ])
    .context("failed to build llm gateway proxy configs batch")
}

pub fn build_proxy_bindings_batch(records: &[LlmGatewayProxyBindingRecord]) -> Result<RecordBatch> {
    let schema = llm_gateway_proxy_bindings_schema();
    let mut provider_type = StringBuilder::new();
    let mut proxy_config_id = StringBuilder::new();
    let mut updated_at = TimestampMillisecondBuilder::new();

    for record in records {
        provider_type.append_value(&record.provider_type);
        proxy_config_id.append_value(&record.proxy_config_id);
        updated_at.append_value(record.updated_at);
    }

    RecordBatch::try_new(schema, vec![
        Arc::new(provider_type.finish()) as ArrayRef,
        Arc::new(proxy_config_id.finish()),
        Arc::new(updated_at.finish()),
    ])
    .context("failed to build llm gateway proxy bindings batch")
}

pub fn build_token_requests_batch(records: &[LlmGatewayTokenRequestRecord]) -> Result<RecordBatch> {
    let schema = llm_gateway_token_requests_schema();
    let mut request_id = StringBuilder::new();
    let mut requester_email = StringBuilder::new();
    let mut requested_quota_billable_limit = UInt64Builder::new();
    let mut request_reason = StringBuilder::new();
    let mut frontend_page_url = StringBuilder::new();
    let mut status = StringBuilder::new();
    let mut fingerprint = StringBuilder::new();
    let mut client_ip = StringBuilder::new();
    let mut ip_region = StringBuilder::new();
    let mut admin_note = StringBuilder::new();
    let mut failure_reason = StringBuilder::new();
    let mut issued_key_id = StringBuilder::new();
    let mut issued_key_name = StringBuilder::new();
    let mut created_at = TimestampMillisecondBuilder::new();
    let mut updated_at = TimestampMillisecondBuilder::new();
    let mut processed_at = TimestampMillisecondBuilder::new();

    for record in records {
        request_id.append_value(&record.request_id);
        requester_email.append_value(&record.requester_email);
        requested_quota_billable_limit.append_value(record.requested_quota_billable_limit);
        request_reason.append_value(&record.request_reason);
        append_optional_str(&mut frontend_page_url, record.frontend_page_url.as_deref());
        status.append_value(&record.status);
        fingerprint.append_value(&record.fingerprint);
        client_ip.append_value(&record.client_ip);
        ip_region.append_value(&record.ip_region);
        append_optional_str(&mut admin_note, record.admin_note.as_deref());
        append_optional_str(&mut failure_reason, record.failure_reason.as_deref());
        append_optional_str(&mut issued_key_id, record.issued_key_id.as_deref());
        append_optional_str(&mut issued_key_name, record.issued_key_name.as_deref());
        created_at.append_value(record.created_at);
        updated_at.append_value(record.updated_at);
        append_optional_ts(&mut processed_at, record.processed_at);
    }

    RecordBatch::try_new(schema, vec![
        Arc::new(request_id.finish()) as ArrayRef,
        Arc::new(requester_email.finish()),
        Arc::new(requested_quota_billable_limit.finish()),
        Arc::new(request_reason.finish()),
        Arc::new(frontend_page_url.finish()),
        Arc::new(status.finish()),
        Arc::new(fingerprint.finish()),
        Arc::new(client_ip.finish()),
        Arc::new(ip_region.finish()),
        Arc::new(admin_note.finish()),
        Arc::new(failure_reason.finish()),
        Arc::new(issued_key_id.finish()),
        Arc::new(issued_key_name.finish()),
        Arc::new(created_at.finish()),
        Arc::new(updated_at.finish()),
        Arc::new(processed_at.finish()),
    ])
    .context("failed to build llm gateway token requests batch")
}

pub fn build_account_contribution_requests_batch(
    records: &[LlmGatewayAccountContributionRequestRecord],
) -> Result<RecordBatch> {
    let schema = llm_gateway_account_contribution_requests_schema();
    let mut request_id = StringBuilder::new();
    let mut account_name = StringBuilder::new();
    let mut account_id = StringBuilder::new();
    let mut id_token = StringBuilder::new();
    let mut access_token = StringBuilder::new();
    let mut refresh_token = StringBuilder::new();
    let mut requester_email = StringBuilder::new();
    let mut contributor_message = StringBuilder::new();
    let mut github_id = StringBuilder::new();
    let mut frontend_page_url = StringBuilder::new();
    let mut status = StringBuilder::new();
    let mut fingerprint = StringBuilder::new();
    let mut client_ip = StringBuilder::new();
    let mut ip_region = StringBuilder::new();
    let mut admin_note = StringBuilder::new();
    let mut failure_reason = StringBuilder::new();
    let mut imported_account_name = StringBuilder::new();
    let mut issued_key_id = StringBuilder::new();
    let mut issued_key_name = StringBuilder::new();
    let mut created_at = TimestampMillisecondBuilder::new();
    let mut updated_at = TimestampMillisecondBuilder::new();
    let mut processed_at = TimestampMillisecondBuilder::new();

    for record in records {
        request_id.append_value(&record.request_id);
        account_name.append_value(&record.account_name);
        append_optional_str(&mut account_id, record.account_id.as_deref());
        id_token.append_value(&record.id_token);
        access_token.append_value(&record.access_token);
        refresh_token.append_value(&record.refresh_token);
        requester_email.append_value(&record.requester_email);
        contributor_message.append_value(&record.contributor_message);
        append_optional_str(&mut github_id, record.github_id.as_deref());
        append_optional_str(&mut frontend_page_url, record.frontend_page_url.as_deref());
        status.append_value(&record.status);
        fingerprint.append_value(&record.fingerprint);
        client_ip.append_value(&record.client_ip);
        ip_region.append_value(&record.ip_region);
        append_optional_str(&mut admin_note, record.admin_note.as_deref());
        append_optional_str(&mut failure_reason, record.failure_reason.as_deref());
        append_optional_str(&mut imported_account_name, record.imported_account_name.as_deref());
        append_optional_str(&mut issued_key_id, record.issued_key_id.as_deref());
        append_optional_str(&mut issued_key_name, record.issued_key_name.as_deref());
        created_at.append_value(record.created_at);
        updated_at.append_value(record.updated_at);
        append_optional_ts(&mut processed_at, record.processed_at);
    }

    RecordBatch::try_new(schema, vec![
        Arc::new(request_id.finish()) as ArrayRef,
        Arc::new(account_name.finish()),
        Arc::new(account_id.finish()),
        Arc::new(id_token.finish()),
        Arc::new(access_token.finish()),
        Arc::new(refresh_token.finish()),
        Arc::new(requester_email.finish()),
        Arc::new(contributor_message.finish()),
        Arc::new(github_id.finish()),
        Arc::new(frontend_page_url.finish()),
        Arc::new(status.finish()),
        Arc::new(fingerprint.finish()),
        Arc::new(client_ip.finish()),
        Arc::new(ip_region.finish()),
        Arc::new(admin_note.finish()),
        Arc::new(failure_reason.finish()),
        Arc::new(imported_account_name.finish()),
        Arc::new(issued_key_id.finish()),
        Arc::new(issued_key_name.finish()),
        Arc::new(created_at.finish()),
        Arc::new(updated_at.finish()),
        Arc::new(processed_at.finish()),
    ])
    .context("failed to build llm gateway account contribution requests batch")
}

pub fn build_sponsor_requests_batch(
    records: &[LlmGatewaySponsorRequestRecord],
) -> Result<RecordBatch> {
    let schema = llm_gateway_sponsor_requests_schema();
    let mut request_id = StringBuilder::new();
    let mut requester_email = StringBuilder::new();
    let mut sponsor_message = StringBuilder::new();
    let mut display_name = StringBuilder::new();
    let mut github_id = StringBuilder::new();
    let mut frontend_page_url = StringBuilder::new();
    let mut status = StringBuilder::new();
    let mut fingerprint = StringBuilder::new();
    let mut client_ip = StringBuilder::new();
    let mut ip_region = StringBuilder::new();
    let mut admin_note = StringBuilder::new();
    let mut failure_reason = StringBuilder::new();
    let mut payment_email_sent_at = TimestampMillisecondBuilder::new();
    let mut created_at = TimestampMillisecondBuilder::new();
    let mut updated_at = TimestampMillisecondBuilder::new();
    let mut processed_at = TimestampMillisecondBuilder::new();

    for record in records {
        request_id.append_value(&record.request_id);
        requester_email.append_value(&record.requester_email);
        sponsor_message.append_value(&record.sponsor_message);
        append_optional_str(&mut display_name, record.display_name.as_deref());
        append_optional_str(&mut github_id, record.github_id.as_deref());
        append_optional_str(&mut frontend_page_url, record.frontend_page_url.as_deref());
        status.append_value(&record.status);
        fingerprint.append_value(&record.fingerprint);
        client_ip.append_value(&record.client_ip);
        ip_region.append_value(&record.ip_region);
        append_optional_str(&mut admin_note, record.admin_note.as_deref());
        append_optional_str(&mut failure_reason, record.failure_reason.as_deref());
        append_optional_ts(&mut payment_email_sent_at, record.payment_email_sent_at);
        created_at.append_value(record.created_at);
        updated_at.append_value(record.updated_at);
        append_optional_ts(&mut processed_at, record.processed_at);
    }

    RecordBatch::try_new(schema, vec![
        Arc::new(request_id.finish()) as ArrayRef,
        Arc::new(requester_email.finish()),
        Arc::new(sponsor_message.finish()),
        Arc::new(display_name.finish()),
        Arc::new(github_id.finish()),
        Arc::new(frontend_page_url.finish()),
        Arc::new(status.finish()),
        Arc::new(fingerprint.finish()),
        Arc::new(client_ip.finish()),
        Arc::new(ip_region.finish()),
        Arc::new(admin_note.finish()),
        Arc::new(failure_reason.finish()),
        Arc::new(payment_email_sent_at.finish()),
        Arc::new(created_at.finish()),
        Arc::new(updated_at.finish()),
        Arc::new(processed_at.finish()),
    ])
    .context("failed to build llm gateway sponsor requests batch")
}

/// Decode Arrow [`RecordBatch`]es back into [`LlmGatewayKeyRecord`] rows.
///
/// Columns added after the initial schema (`provider_type`, `protocol_family`)
/// are read optionally so that rows written before the migration still decode
/// with sensible defaults.
pub fn batches_to_keys(batches: &[RecordBatch]) -> Result<Vec<LlmGatewayKeyRecord>> {
    let mut rows = Vec::with_capacity(total_rows(batches));
    for batch in batches {
        let id = required_str_col(batch, "id")?;
        let name = required_str_col(batch, "name")?;
        let secret = required_str_col(batch, "secret")?;
        let key_hash = required_str_col(batch, "key_hash")?;
        let status = required_str_col(batch, "status")?;
        // Optional: column may be absent in rows written before multi-provider support
        let provider_type = batch
            .column_by_name("provider_type")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        // Optional: column may be absent in rows written before protocol-family support
        let protocol_family = batch
            .column_by_name("protocol_family")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let public_visible = required_bool_col(batch, "public_visible")?;
        let quota_billable_limit = required_u64_col(batch, "quota_billable_limit")?;
        let usage_input_uncached_tokens = required_u64_col(batch, "usage_input_uncached_tokens")?;
        let usage_input_cached_tokens = required_u64_col(batch, "usage_input_cached_tokens")?;
        let usage_output_tokens = required_u64_col(batch, "usage_output_tokens")?;
        let usage_billable_tokens = batch
            .column_by_name("usage_billable_tokens")
            .and_then(|column| column.as_any().downcast_ref::<UInt64Array>());
        let usage_credit_total = batch
            .column_by_name("usage_credit_total")
            .and_then(|column| column.as_any().downcast_ref::<Float64Array>());
        let usage_credit_missing_events = batch
            .column_by_name("usage_credit_missing_events")
            .and_then(|column| column.as_any().downcast_ref::<UInt64Array>());
        let last_used_at = optional_ts_col(batch, "last_used_at")?;
        let created_at = required_ts_col(batch, "created_at")?;
        let updated_at = required_ts_col(batch, "updated_at")?;
        let route_strategy = batch
            .column_by_name("route_strategy")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let fixed_account_name = batch
            .column_by_name("fixed_account_name")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let auto_account_names_json = batch
            .column_by_name("auto_account_names_json")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let model_name_map_json = batch
            .column_by_name("model_name_map_json")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let request_max_concurrency = batch
            .column_by_name("request_max_concurrency")
            .and_then(|column| column.as_any().downcast_ref::<UInt64Array>());
        let request_min_start_interval_ms = batch
            .column_by_name("request_min_start_interval_ms")
            .and_then(|column| column.as_any().downcast_ref::<UInt64Array>());
        let kiro_request_validation_enabled = batch
            .column_by_name("kiro_request_validation_enabled")
            .and_then(|column| column.as_any().downcast_ref::<BooleanArray>());
        let kiro_cache_estimation_enabled = batch
            .column_by_name("kiro_cache_estimation_enabled")
            .and_then(|column| column.as_any().downcast_ref::<BooleanArray>());

        for idx in 0..batch.num_rows() {
            let raw_billable_tokens = usage_input_uncached_tokens
                .value(idx)
                .saturating_add(usage_output_tokens.value(idx));
            rows.push(LlmGatewayKeyRecord {
                id: id.value(idx).to_string(),
                name: name.value(idx).to_string(),
                secret: secret.value(idx).to_string(),
                key_hash: key_hash.value(idx).to_string(),
                status: status.value(idx).to_string(),
                provider_type: provider_type
                    .and_then(|column| value_string_opt(column, idx))
                    .unwrap_or_else(|| LLM_GATEWAY_PROVIDER_CODEX.to_string()),
                protocol_family: protocol_family
                    .and_then(|column| value_string_opt(column, idx))
                    .unwrap_or_else(|| LLM_GATEWAY_PROTOCOL_OPENAI.to_string()),
                public_visible: public_visible.value(idx),
                quota_billable_limit: quota_billable_limit.value(idx),
                usage_input_uncached_tokens: usage_input_uncached_tokens.value(idx),
                usage_input_cached_tokens: usage_input_cached_tokens.value(idx),
                usage_output_tokens: usage_output_tokens.value(idx),
                usage_billable_tokens: usage_billable_tokens
                    .and_then(|column| value_u64_opt(column, idx))
                    .unwrap_or(raw_billable_tokens),
                usage_credit_total: usage_credit_total
                    .and_then(|column| value_f64_opt(column, idx))
                    .unwrap_or(0.0),
                usage_credit_missing_events: usage_credit_missing_events
                    .and_then(|column| value_u64_opt(column, idx))
                    .unwrap_or(0),
                last_used_at: value_ts_opt(last_used_at, idx),
                created_at: created_at.value(idx),
                updated_at: updated_at.value(idx),
                route_strategy: route_strategy.and_then(|col| value_string_opt(col, idx)),
                fixed_account_name: fixed_account_name.and_then(|col| value_string_opt(col, idx)),
                auto_account_names: parse_string_vec_json_opt(
                    auto_account_names_json
                        .and_then(|col| value_string_opt(col, idx))
                        .as_deref(),
                )?,
                model_name_map: parse_string_map_json_opt(
                    model_name_map_json
                        .and_then(|col| value_string_opt(col, idx))
                        .as_deref(),
                )?,
                request_max_concurrency: request_max_concurrency
                    .and_then(|column| value_u64_opt(column, idx)),
                request_min_start_interval_ms: request_min_start_interval_ms
                    .and_then(|column| value_u64_opt(column, idx)),
                kiro_request_validation_enabled: kiro_request_validation_enabled
                    .and_then(|column| value_bool_opt(column, idx))
                    .unwrap_or(true),
                kiro_cache_estimation_enabled: kiro_cache_estimation_enabled
                    .and_then(|column| value_bool_opt(column, idx))
                    .unwrap_or(true),
            });
        }
    }
    Ok(rows)
}

fn serialize_string_vec_json(value: Option<&[String]>) -> Result<Option<String>> {
    match value {
        Some(items) if !items.is_empty() => serde_json::to_string(items)
            .map(Some)
            .context("failed to serialize llm gateway auto account names"),
        _ => Ok(None),
    }
}

fn serialize_string_map_json(value: Option<&BTreeMap<String, String>>) -> Result<Option<String>> {
    match value {
        Some(items) if !items.is_empty() => serde_json::to_string(items)
            .map(Some)
            .context("failed to serialize llm gateway model name map"),
        _ => Ok(None),
    }
}

fn parse_string_vec_json_opt(value: Option<&str>) -> Result<Option<Vec<String>>> {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(raw) => serde_json::from_str::<Vec<String>>(raw)
            .map(Some)
            .with_context(|| format!("failed to parse llm gateway auto account names JSON: {raw}")),
        None => Ok(None),
    }
}

fn parse_string_map_json_opt(value: Option<&str>) -> Result<Option<BTreeMap<String, String>>> {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(raw) => serde_json::from_str::<BTreeMap<String, String>>(raw)
            .map(Some)
            .with_context(|| format!("failed to parse llm gateway model name map JSON: {raw}")),
        None => Ok(None),
    }
}

pub fn batches_to_usage_events(batches: &[RecordBatch]) -> Result<Vec<LlmGatewayUsageEventRecord>> {
    let mut rows = Vec::with_capacity(total_rows(batches));
    for batch in batches {
        let id = required_str_col(batch, "id")?;
        let key_id = required_str_col(batch, "key_id")?;
        let key_name = batch
            .column_by_name("key_name")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let provider_type = batch
            .column_by_name("provider_type")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let account_name = batch
            .column_by_name("account_name")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let request_method = batch
            .column_by_name("request_method")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let request_url = batch
            .column_by_name("request_url")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let latency_ms = batch
            .column_by_name("latency_ms")
            .and_then(|column| column.as_any().downcast_ref::<Int32Array>());
        let endpoint = required_str_col(batch, "endpoint")?;
        let model = optional_str_col(batch, "model")?;
        let status_code = required_i32_col(batch, "status_code")?;
        let input_uncached_tokens = required_u64_col(batch, "input_uncached_tokens")?;
        let input_cached_tokens = required_u64_col(batch, "input_cached_tokens")?;
        let output_tokens = required_u64_col(batch, "output_tokens")?;
        let billable_tokens = required_u64_col(batch, "billable_tokens")?;
        let usage_missing = required_bool_col(batch, "usage_missing")?;
        let credit_usage = batch
            .column_by_name("credit_usage")
            .and_then(|column| column.as_any().downcast_ref::<Float64Array>());
        let credit_usage_missing = batch
            .column_by_name("credit_usage_missing")
            .and_then(|column| column.as_any().downcast_ref::<BooleanArray>());
        let client_ip = batch
            .column_by_name("client_ip")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let ip_region = batch
            .column_by_name("ip_region")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let request_headers_json = batch
            .column_by_name("request_headers_json")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let last_message_content = batch
            .column_by_name("last_message_content")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let created_at = required_ts_col(batch, "created_at")?;

        for idx in 0..batch.num_rows() {
            let provider_type_value = provider_type
                .and_then(|column| value_string_opt(column, idx))
                .unwrap_or_else(|| LLM_GATEWAY_PROVIDER_CODEX.to_string());
            let credit_usage_value = credit_usage.and_then(|column| value_f64_opt(column, idx));
            rows.push(LlmGatewayUsageEventRecord {
                id: id.value(idx).to_string(),
                key_id: key_id.value(idx).to_string(),
                key_name: key_name
                    .and_then(|column| value_string_opt(column, idx))
                    .unwrap_or_else(|| key_id.value(idx).to_string()),
                provider_type: provider_type_value.clone(),
                account_name: account_name.and_then(|column| value_string_opt(column, idx)),
                request_method: request_method
                    .and_then(|column| value_string_opt(column, idx))
                    .unwrap_or_else(|| "POST".to_string()),
                request_url: request_url
                    .and_then(|column| value_string_opt(column, idx))
                    .unwrap_or_else(|| endpoint.value(idx).to_string()),
                latency_ms: latency_ms
                    .and_then(|column| value_i32_opt(column, idx))
                    .unwrap_or_default(),
                endpoint: endpoint.value(idx).to_string(),
                model: value_string_opt(model, idx),
                status_code: status_code.value(idx),
                input_uncached_tokens: input_uncached_tokens.value(idx),
                input_cached_tokens: input_cached_tokens.value(idx),
                output_tokens: output_tokens.value(idx),
                billable_tokens: billable_tokens.value(idx),
                usage_missing: usage_missing.value(idx),
                credit_usage: credit_usage_value,
                credit_usage_missing: credit_usage_missing
                    .and_then(|column| value_bool_opt(column, idx))
                    .unwrap_or(
                        provider_type_value == LLM_GATEWAY_PROVIDER_KIRO
                            && credit_usage_value.is_none(),
                    ),
                client_ip: client_ip
                    .and_then(|column| value_string_opt(column, idx))
                    .unwrap_or_else(|| "unknown".to_string()),
                ip_region: ip_region
                    .and_then(|column| value_string_opt(column, idx))
                    .unwrap_or_else(|| "Unknown".to_string()),
                request_headers_json: request_headers_json
                    .and_then(|column| value_string_opt(column, idx))
                    .unwrap_or_else(|| "{}".to_string()),
                last_message_content: last_message_content
                    .and_then(|column| value_string_opt(column, idx)),
                created_at: created_at.value(idx),
            });
        }
    }
    Ok(rows)
}

/// Decode Arrow [`RecordBatch`]es back into
/// [`LlmGatewayRuntimeConfigRecord`] rows.
///
/// Columns added after the initial schema (`max_request_body_bytes`,
/// `account_failure_retry_limit`, `kiro_channel_max_concurrency`,
/// `kiro_channel_min_start_interval_ms`) are read optionally so that rows
/// written before the migration still decode with sensible defaults.
pub fn batches_to_runtime_config(
    batches: &[RecordBatch],
) -> Result<Vec<LlmGatewayRuntimeConfigRecord>> {
    let mut rows = Vec::with_capacity(total_rows(batches));
    for batch in batches {
        let id = required_str_col(batch, "id")?;
        let auth_cache_ttl_seconds = required_u64_col(batch, "auth_cache_ttl_seconds")?;
        let max_request_body_bytes = batch
            .column_by_name("max_request_body_bytes")
            .and_then(|column| column.as_any().downcast_ref::<UInt64Array>());
        let account_failure_retry_limit = batch
            .column_by_name("account_failure_retry_limit")
            .and_then(|column| column.as_any().downcast_ref::<UInt64Array>());
        let kiro_channel_max_concurrency = batch
            .column_by_name("kiro_channel_max_concurrency")
            .and_then(|column| column.as_any().downcast_ref::<UInt64Array>());
        let kiro_channel_min_start_interval_ms = batch
            .column_by_name("kiro_channel_min_start_interval_ms")
            .and_then(|column| column.as_any().downcast_ref::<UInt64Array>());
        let codex_status_refresh_min_interval_seconds = batch
            .column_by_name("codex_status_refresh_min_interval_seconds")
            .and_then(|column| column.as_any().downcast_ref::<UInt64Array>());
        let codex_status_refresh_max_interval_seconds = batch
            .column_by_name("codex_status_refresh_max_interval_seconds")
            .and_then(|column| column.as_any().downcast_ref::<UInt64Array>());
        let codex_status_account_jitter_max_seconds = batch
            .column_by_name("codex_status_account_jitter_max_seconds")
            .and_then(|column| column.as_any().downcast_ref::<UInt64Array>());
        let kiro_status_refresh_min_interval_seconds = batch
            .column_by_name("kiro_status_refresh_min_interval_seconds")
            .and_then(|column| column.as_any().downcast_ref::<UInt64Array>());
        let kiro_status_refresh_max_interval_seconds = batch
            .column_by_name("kiro_status_refresh_max_interval_seconds")
            .and_then(|column| column.as_any().downcast_ref::<UInt64Array>());
        let kiro_status_account_jitter_max_seconds = batch
            .column_by_name("kiro_status_account_jitter_max_seconds")
            .and_then(|column| column.as_any().downcast_ref::<UInt64Array>());
        let usage_event_flush_batch_size = batch
            .column_by_name("usage_event_flush_batch_size")
            .and_then(|column| column.as_any().downcast_ref::<UInt64Array>());
        let usage_event_flush_interval_seconds = batch
            .column_by_name("usage_event_flush_interval_seconds")
            .and_then(|column| column.as_any().downcast_ref::<UInt64Array>());
        let usage_event_flush_max_buffer_bytes = batch
            .column_by_name("usage_event_flush_max_buffer_bytes")
            .and_then(|column| column.as_any().downcast_ref::<UInt64Array>());
        let kiro_cache_kmodels_json = batch
            .column_by_name("kiro_cache_kmodels_json")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let kiro_prefix_cache_mode = batch
            .column_by_name("kiro_prefix_cache_mode")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let kiro_prefix_cache_max_tokens = batch
            .column_by_name("kiro_prefix_cache_max_tokens")
            .and_then(|column| column.as_any().downcast_ref::<UInt64Array>());
        let kiro_prefix_cache_entry_ttl_seconds = batch
            .column_by_name("kiro_prefix_cache_entry_ttl_seconds")
            .and_then(|column| column.as_any().downcast_ref::<UInt64Array>());
        let kiro_conversation_anchor_max_entries = batch
            .column_by_name("kiro_conversation_anchor_max_entries")
            .and_then(|column| column.as_any().downcast_ref::<UInt64Array>());
        let kiro_conversation_anchor_ttl_seconds = batch
            .column_by_name("kiro_conversation_anchor_ttl_seconds")
            .and_then(|column| column.as_any().downcast_ref::<UInt64Array>());
        let updated_at = required_ts_col(batch, "updated_at")?;
        for idx in 0..batch.num_rows() {
            rows.push(LlmGatewayRuntimeConfigRecord {
                id: id.value(idx).to_string(),
                auth_cache_ttl_seconds: auth_cache_ttl_seconds.value(idx),
                max_request_body_bytes: max_request_body_bytes
                    .and_then(|column| value_u64_opt(column, idx))
                    .unwrap_or(DEFAULT_LLM_GATEWAY_MAX_REQUEST_BODY_BYTES),
                account_failure_retry_limit: account_failure_retry_limit
                    .and_then(|column| value_u64_opt(column, idx))
                    .unwrap_or(DEFAULT_LLM_GATEWAY_ACCOUNT_FAILURE_RETRY_LIMIT),
                kiro_channel_max_concurrency: kiro_channel_max_concurrency
                    .and_then(|column| value_u64_opt(column, idx))
                    .unwrap_or(DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY),
                kiro_channel_min_start_interval_ms: kiro_channel_min_start_interval_ms
                    .and_then(|column| value_u64_opt(column, idx))
                    .unwrap_or(DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS),
                codex_status_refresh_min_interval_seconds:
                    codex_status_refresh_min_interval_seconds
                        .and_then(|column| value_u64_opt(column, idx))
                        .unwrap_or(DEFAULT_CODEX_STATUS_REFRESH_MIN_INTERVAL_SECONDS),
                codex_status_refresh_max_interval_seconds:
                    codex_status_refresh_max_interval_seconds
                        .and_then(|column| value_u64_opt(column, idx))
                        .unwrap_or(DEFAULT_CODEX_STATUS_REFRESH_MAX_INTERVAL_SECONDS),
                codex_status_account_jitter_max_seconds: codex_status_account_jitter_max_seconds
                    .and_then(|column| value_u64_opt(column, idx))
                    .unwrap_or(DEFAULT_CODEX_STATUS_ACCOUNT_JITTER_MAX_SECONDS),
                kiro_status_refresh_min_interval_seconds: kiro_status_refresh_min_interval_seconds
                    .and_then(|column| value_u64_opt(column, idx))
                    .unwrap_or(DEFAULT_KIRO_STATUS_REFRESH_MIN_INTERVAL_SECONDS),
                kiro_status_refresh_max_interval_seconds: kiro_status_refresh_max_interval_seconds
                    .and_then(|column| value_u64_opt(column, idx))
                    .unwrap_or(DEFAULT_KIRO_STATUS_REFRESH_MAX_INTERVAL_SECONDS),
                kiro_status_account_jitter_max_seconds: kiro_status_account_jitter_max_seconds
                    .and_then(|column| value_u64_opt(column, idx))
                    .unwrap_or(DEFAULT_KIRO_STATUS_ACCOUNT_JITTER_MAX_SECONDS),
                usage_event_flush_batch_size: usage_event_flush_batch_size
                    .and_then(|column| value_u64_opt(column, idx))
                    .unwrap_or(DEFAULT_LLM_GATEWAY_USAGE_EVENT_FLUSH_BATCH_SIZE),
                usage_event_flush_interval_seconds: usage_event_flush_interval_seconds
                    .and_then(|column| value_u64_opt(column, idx))
                    .unwrap_or(DEFAULT_LLM_GATEWAY_USAGE_EVENT_FLUSH_INTERVAL_SECONDS),
                usage_event_flush_max_buffer_bytes: usage_event_flush_max_buffer_bytes
                    .and_then(|column| value_u64_opt(column, idx))
                    .unwrap_or(DEFAULT_LLM_GATEWAY_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES),
                kiro_cache_kmodels_json: kiro_cache_kmodels_json
                    .and_then(|column| value_string_opt(column, idx))
                    .unwrap_or_else(default_kiro_cache_kmodels_json),
                kiro_prefix_cache_mode: kiro_prefix_cache_mode
                    .and_then(|column| value_string_opt(column, idx))
                    .unwrap_or_else(|| DEFAULT_KIRO_PREFIX_CACHE_MODE.to_string()),
                kiro_prefix_cache_max_tokens: kiro_prefix_cache_max_tokens
                    .and_then(|column| value_u64_opt(column, idx))
                    .unwrap_or(DEFAULT_KIRO_PREFIX_CACHE_MAX_TOKENS),
                kiro_prefix_cache_entry_ttl_seconds: kiro_prefix_cache_entry_ttl_seconds
                    .and_then(|column| value_u64_opt(column, idx))
                    .unwrap_or(DEFAULT_KIRO_PREFIX_CACHE_ENTRY_TTL_SECONDS),
                kiro_conversation_anchor_max_entries: kiro_conversation_anchor_max_entries
                    .and_then(|column| value_u64_opt(column, idx))
                    .unwrap_or(DEFAULT_KIRO_CONVERSATION_ANCHOR_MAX_ENTRIES),
                kiro_conversation_anchor_ttl_seconds: kiro_conversation_anchor_ttl_seconds
                    .and_then(|column| value_u64_opt(column, idx))
                    .unwrap_or(DEFAULT_KIRO_CONVERSATION_ANCHOR_TTL_SECONDS),
                updated_at: updated_at.value(idx),
            });
        }
    }
    Ok(rows)
}

pub fn batches_to_proxy_configs(
    batches: &[RecordBatch],
) -> Result<Vec<LlmGatewayProxyConfigRecord>> {
    let mut rows = Vec::with_capacity(total_rows(batches));
    for batch in batches {
        let id = required_str_col(batch, "id")?;
        let name = required_str_col(batch, "name")?;
        let proxy_url = required_str_col(batch, "proxy_url")?;
        let proxy_username = optional_str_col(batch, "proxy_username")?;
        let proxy_password = optional_str_col(batch, "proxy_password")?;
        let status = required_str_col(batch, "status")?;
        let created_at = required_ts_col(batch, "created_at")?;
        let updated_at = required_ts_col(batch, "updated_at")?;
        for idx in 0..batch.num_rows() {
            rows.push(LlmGatewayProxyConfigRecord {
                id: id.value(idx).to_string(),
                name: name.value(idx).to_string(),
                proxy_url: proxy_url.value(idx).to_string(),
                proxy_username: value_string_opt(proxy_username, idx),
                proxy_password: value_string_opt(proxy_password, idx),
                status: status.value(idx).to_string(),
                created_at: created_at.value(idx),
                updated_at: updated_at.value(idx),
            });
        }
    }
    Ok(rows)
}

pub fn batches_to_proxy_bindings(
    batches: &[RecordBatch],
) -> Result<Vec<LlmGatewayProxyBindingRecord>> {
    let mut rows = Vec::with_capacity(total_rows(batches));
    for batch in batches {
        let provider_type = required_str_col(batch, "provider_type")?;
        let proxy_config_id = required_str_col(batch, "proxy_config_id")?;
        let updated_at = required_ts_col(batch, "updated_at")?;
        for idx in 0..batch.num_rows() {
            rows.push(LlmGatewayProxyBindingRecord {
                provider_type: provider_type.value(idx).to_string(),
                proxy_config_id: proxy_config_id.value(idx).to_string(),
                updated_at: updated_at.value(idx),
            });
        }
    }
    Ok(rows)
}

pub fn batches_to_token_requests(
    batches: &[RecordBatch],
) -> Result<Vec<LlmGatewayTokenRequestRecord>> {
    let mut rows = Vec::with_capacity(total_rows(batches));
    for batch in batches {
        let request_id = required_str_col(batch, "request_id")?;
        let requester_email = required_str_col(batch, "requester_email")?;
        let requested_quota_billable_limit =
            required_u64_col(batch, "requested_quota_billable_limit")?;
        let request_reason = required_str_col(batch, "request_reason")?;
        let frontend_page_url = batch
            .column_by_name("frontend_page_url")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let status = required_str_col(batch, "status")?;
        let fingerprint = required_str_col(batch, "fingerprint")?;
        let client_ip = required_str_col(batch, "client_ip")?;
        let ip_region = required_str_col(batch, "ip_region")?;
        let admin_note = batch
            .column_by_name("admin_note")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let failure_reason = batch
            .column_by_name("failure_reason")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let issued_key_id = batch
            .column_by_name("issued_key_id")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let issued_key_name = batch
            .column_by_name("issued_key_name")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let created_at = required_ts_col(batch, "created_at")?;
        let updated_at = required_ts_col(batch, "updated_at")?;
        let processed_at = optional_ts_col(batch, "processed_at")?;

        for idx in 0..batch.num_rows() {
            rows.push(LlmGatewayTokenRequestRecord {
                request_id: request_id.value(idx).to_string(),
                requester_email: requester_email.value(idx).to_string(),
                requested_quota_billable_limit: requested_quota_billable_limit.value(idx),
                request_reason: request_reason.value(idx).to_string(),
                frontend_page_url: frontend_page_url.and_then(|col| value_string_opt(col, idx)),
                status: status.value(idx).to_string(),
                fingerprint: fingerprint.value(idx).to_string(),
                client_ip: client_ip.value(idx).to_string(),
                ip_region: ip_region.value(idx).to_string(),
                admin_note: admin_note.and_then(|col| value_string_opt(col, idx)),
                failure_reason: failure_reason.and_then(|col| value_string_opt(col, idx)),
                issued_key_id: issued_key_id.and_then(|col| value_string_opt(col, idx)),
                issued_key_name: issued_key_name.and_then(|col| value_string_opt(col, idx)),
                created_at: created_at.value(idx),
                updated_at: updated_at.value(idx),
                processed_at: value_ts_opt(processed_at, idx),
            });
        }
    }
    Ok(rows)
}

pub fn batches_to_account_contribution_requests(
    batches: &[RecordBatch],
) -> Result<Vec<LlmGatewayAccountContributionRequestRecord>> {
    let mut rows = Vec::with_capacity(total_rows(batches));
    for batch in batches {
        let request_id = required_str_col(batch, "request_id")?;
        let account_name = required_str_col(batch, "account_name")?;
        let account_id = batch
            .column_by_name("account_id")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let id_token = required_str_col(batch, "id_token")?;
        let access_token = required_str_col(batch, "access_token")?;
        let refresh_token = required_str_col(batch, "refresh_token")?;
        let requester_email = required_str_col(batch, "requester_email")?;
        let contributor_message = required_str_col(batch, "contributor_message")?;
        let github_id = batch
            .column_by_name("github_id")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let frontend_page_url = batch
            .column_by_name("frontend_page_url")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let status = required_str_col(batch, "status")?;
        let fingerprint = required_str_col(batch, "fingerprint")?;
        let client_ip = required_str_col(batch, "client_ip")?;
        let ip_region = required_str_col(batch, "ip_region")?;
        let admin_note = batch
            .column_by_name("admin_note")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let failure_reason = batch
            .column_by_name("failure_reason")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let imported_account_name = batch
            .column_by_name("imported_account_name")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let issued_key_id = batch
            .column_by_name("issued_key_id")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let issued_key_name = batch
            .column_by_name("issued_key_name")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let created_at = required_ts_col(batch, "created_at")?;
        let updated_at = required_ts_col(batch, "updated_at")?;
        let processed_at = optional_ts_col(batch, "processed_at")?;

        for idx in 0..batch.num_rows() {
            rows.push(LlmGatewayAccountContributionRequestRecord {
                request_id: request_id.value(idx).to_string(),
                account_name: account_name.value(idx).to_string(),
                account_id: account_id.and_then(|col| value_string_opt(col, idx)),
                id_token: id_token.value(idx).to_string(),
                access_token: access_token.value(idx).to_string(),
                refresh_token: refresh_token.value(idx).to_string(),
                requester_email: requester_email.value(idx).to_string(),
                contributor_message: contributor_message.value(idx).to_string(),
                github_id: github_id.and_then(|col| value_string_opt(col, idx)),
                frontend_page_url: frontend_page_url.and_then(|col| value_string_opt(col, idx)),
                status: status.value(idx).to_string(),
                fingerprint: fingerprint.value(idx).to_string(),
                client_ip: client_ip.value(idx).to_string(),
                ip_region: ip_region.value(idx).to_string(),
                admin_note: admin_note.and_then(|col| value_string_opt(col, idx)),
                failure_reason: failure_reason.and_then(|col| value_string_opt(col, idx)),
                imported_account_name: imported_account_name
                    .and_then(|col| value_string_opt(col, idx)),
                issued_key_id: issued_key_id.and_then(|col| value_string_opt(col, idx)),
                issued_key_name: issued_key_name.and_then(|col| value_string_opt(col, idx)),
                created_at: created_at.value(idx),
                updated_at: updated_at.value(idx),
                processed_at: value_ts_opt(processed_at, idx),
            });
        }
    }
    Ok(rows)
}

pub fn batches_to_sponsor_requests(
    batches: &[RecordBatch],
) -> Result<Vec<LlmGatewaySponsorRequestRecord>> {
    let mut rows = Vec::with_capacity(total_rows(batches));
    for batch in batches {
        let request_id = required_str_col(batch, "request_id")?;
        let requester_email = required_str_col(batch, "requester_email")?;
        let sponsor_message = required_str_col(batch, "sponsor_message")?;
        let display_name = batch
            .column_by_name("display_name")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let github_id = batch
            .column_by_name("github_id")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let frontend_page_url = batch
            .column_by_name("frontend_page_url")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let status = required_str_col(batch, "status")?;
        let fingerprint = required_str_col(batch, "fingerprint")?;
        let client_ip = required_str_col(batch, "client_ip")?;
        let ip_region = required_str_col(batch, "ip_region")?;
        let admin_note = batch
            .column_by_name("admin_note")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let failure_reason = batch
            .column_by_name("failure_reason")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>());
        let payment_email_sent_at = optional_ts_col(batch, "payment_email_sent_at")?;
        let created_at = required_ts_col(batch, "created_at")?;
        let updated_at = required_ts_col(batch, "updated_at")?;
        let processed_at = optional_ts_col(batch, "processed_at")?;

        for idx in 0..batch.num_rows() {
            rows.push(LlmGatewaySponsorRequestRecord {
                request_id: request_id.value(idx).to_string(),
                requester_email: requester_email.value(idx).to_string(),
                sponsor_message: sponsor_message.value(idx).to_string(),
                display_name: display_name.and_then(|col| value_string_opt(col, idx)),
                github_id: github_id.and_then(|col| value_string_opt(col, idx)),
                frontend_page_url: frontend_page_url.and_then(|col| value_string_opt(col, idx)),
                status: status.value(idx).to_string(),
                fingerprint: fingerprint.value(idx).to_string(),
                client_ip: client_ip.value(idx).to_string(),
                ip_region: ip_region.value(idx).to_string(),
                admin_note: admin_note.and_then(|col| value_string_opt(col, idx)),
                failure_reason: failure_reason.and_then(|col| value_string_opt(col, idx)),
                payment_email_sent_at: value_ts_opt(payment_email_sent_at, idx),
                created_at: created_at.value(idx),
                updated_at: updated_at.value(idx),
                processed_at: value_ts_opt(processed_at, idx),
            });
        }
    }
    Ok(rows)
}

fn required_str_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .with_context(|| format!("column `{name}` is not StringArray"))
}

fn optional_str_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    required_str_col(batch, name)
}

fn required_bool_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a BooleanArray> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<BooleanArray>())
        .with_context(|| format!("column `{name}` is not BooleanArray"))
}

fn required_u64_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a UInt64Array> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<UInt64Array>())
        .with_context(|| format!("column `{name}` is not UInt64Array"))
}

fn required_i32_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Int32Array> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Int32Array>())
        .with_context(|| format!("column `{name}` is not Int32Array"))
}

fn required_ts_col<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Result<&'a TimestampMillisecondArray> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<TimestampMillisecondArray>())
        .with_context(|| format!("column `{name}` is not TimestampMillisecondArray"))
}

fn optional_ts_col<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Result<&'a TimestampMillisecondArray> {
    required_ts_col(batch, name)
}

fn value_string_opt(array: &StringArray, idx: usize) -> Option<String> {
    if array.is_null(idx) {
        None
    } else {
        let value = array.value(idx).trim();
        if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        }
    }
}

fn value_ts_opt(array: &TimestampMillisecondArray, idx: usize) -> Option<i64> {
    if array.is_null(idx) {
        None
    } else {
        Some(array.value(idx))
    }
}

fn value_u64_opt(array: &UInt64Array, idx: usize) -> Option<u64> {
    if array.is_null(idx) {
        None
    } else {
        Some(array.value(idx))
    }
}

fn value_i32_opt(array: &Int32Array, idx: usize) -> Option<i32> {
    if array.is_null(idx) {
        None
    } else {
        Some(array.value(idx))
    }
}

fn value_f64_opt(array: &Float64Array, idx: usize) -> Option<f64> {
    if array.is_null(idx) {
        None
    } else {
        Some(array.value(idx))
    }
}

fn value_bool_opt(array: &BooleanArray, idx: usize) -> Option<bool> {
    if array.is_null(idx) {
        None
    } else {
        Some(array.value(idx))
    }
}

fn append_optional_str(builder: &mut StringBuilder, value: Option<&str>) {
    match value {
        Some(value) if !value.trim().is_empty() => builder.append_value(value),
        _ => builder.append_null(),
    }
}

fn append_optional_ts(builder: &mut TimestampMillisecondBuilder, value: Option<i64>) {
    match value {
        Some(value) => builder.append_value(value),
        None => builder.append_null(),
    }
}

fn append_optional_f64(builder: &mut Float64Builder, value: Option<f64>) {
    match value {
        Some(value) => builder.append_value(value),
        None => builder.append_null(),
    }
}

fn append_optional_u64(builder: &mut UInt64Builder, value: Option<u64>) {
    match value {
        Some(value) => builder.append_value(value),
        None => builder.append_null(),
    }
}

fn total_rows(batches: &[RecordBatch]) -> usize {
    batches.iter().map(RecordBatch::num_rows).sum()
}
