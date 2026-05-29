//! Codex usage recording and preflight-failure recording.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) async fn record_codex_preflight_failure(record: CodexPreflightFailureRecord<'_>) {
    record.meta.mark_stream_finish();
    let event = UsageEvent {
        event_id: format!("llm-usage-{}", uuid::Uuid::new_v4()),
        created_at_ms: now_millis(),
        provider_type: ProviderType::Codex,
        protocol_family: codex_protocol_family_for_endpoint(record.endpoint),
        key_id: record.key.key_id.clone(),
        key_name: record.key.key_name.clone(),
        account_name: None,
        account_group_id_at_event: None,
        route_strategy_at_event: None,
        request_method: record.meta.request_method.clone(),
        request_url: record.meta.request_url.clone(),
        endpoint: record.endpoint.to_string(),
        model: record.model,
        mapped_model: None,
        status_code: i64::from(record.status.as_u16()),
        request_body_bytes: record.meta.request_body_bytes,
        quota_failover_count: record.meta.quota_failover_count,
        routing_diagnostics_json: record.meta.routing_diagnostics_json.clone(),
        input_uncached_tokens: 0,
        input_cached_tokens: 0,
        output_tokens: 0,
        billable_tokens: 0,
        credit_usage: None,
        usage_missing: true,
        credit_usage_missing: false,
        client_ip: record.meta.client_ip.clone(),
        ip_region: record.meta.ip_region.clone(),
        request_headers_json: record.meta.request_headers_json.clone(),
        last_message_content: record.meta.last_message_content.clone(),
        client_request_body_json: captured_body_json(&record.meta.client_request_body_json),
        upstream_request_body_json: captured_body_json(&record.meta.upstream_request_body_json),
        full_request_json: captured_body_json(&record.meta.full_request_json),
        error_message: record.meta.error_message.clone(),
        error_body: record.meta.error_body.clone(),
        timing: record.meta.to_timing(),
        stream: record.meta.to_stream_details(),
    };
    if let Err(err) = record.control_store.apply_usage_rollup_owned(event).await {
        tracing::warn!(
            key_id = %record.key.key_id,
            endpoint = record.endpoint,
            status = %record.status,
            error = %err,
            "failed to record codex preflight failure usage"
        );
    }
}
pub(crate) async fn record_codex_usage(
    control_store: &dyn ControlStore,
    key: &AuthenticatedKey,
    prepared: &PreparedGatewayRequest,
    status: StatusCode,
    route: &ProviderCodexRoute,
    usage: UsageBreakdown,
    meta: &ProviderUsageMetadata,
) -> anyhow::Result<()> {
    let capture_request_details = !status.is_success();
    let event = UsageEvent {
        event_id: format!("llm-usage-{}", uuid::Uuid::new_v4()),
        created_at_ms: now_millis(),
        provider_type: ProviderType::Codex,
        protocol_family: codex_protocol_family_for_endpoint(&prepared.original_path),
        key_id: key.key_id.clone(),
        key_name: key.key_name.clone(),
        account_name: Some(route.account_name.clone()),
        account_group_id_at_event: route.account_group_id_at_event.clone(),
        route_strategy_at_event: Some(route.route_strategy_at_event),
        request_method: meta.request_method.clone(),
        request_url: meta.request_url.clone(),
        endpoint: prepared.original_path.clone(),
        model: prepared
            .client_visible_model
            .clone()
            .or_else(|| prepared.model.clone()),
        mapped_model: prepared.model.clone(),
        status_code: i64::from(status.as_u16()),
        request_body_bytes: meta
            .request_body_bytes
            .or(Some(clamp_usize_to_i64(prepared.request_body.len()))),
        quota_failover_count: meta.quota_failover_count,
        routing_diagnostics_json: meta.routing_diagnostics_json.clone(),
        input_uncached_tokens: clamp_u64_to_i64(usage.input_uncached_tokens),
        input_cached_tokens: clamp_u64_to_i64(usage.input_cached_tokens),
        output_tokens: clamp_u64_to_i64(usage.output_tokens),
        billable_tokens: clamp_u64_to_i64(
            usage.billable_tokens_with_multiplier(prepared.billable_multiplier),
        ),
        credit_usage: None,
        usage_missing: usage.usage_missing,
        credit_usage_missing: false,
        client_ip: meta.client_ip.clone(),
        ip_region: meta.ip_region.clone(),
        request_headers_json: meta.request_headers_json.clone(),
        last_message_content: meta.last_message_content.clone(),
        client_request_body_json: capture_request_details
            .then(|| captured_body_json(&meta.client_request_body_json))
            .flatten(),
        upstream_request_body_json: capture_request_details
            .then(|| captured_body_json(&meta.upstream_request_body_json))
            .flatten(),
        full_request_json: capture_request_details
            .then(|| {
                captured_body_json(&meta.full_request_json)
                    .or_else(|| captured_body_json(&meta.client_request_body_json))
            })
            .flatten(),
        error_message: meta.error_message.clone(),
        error_body: meta.error_body.clone(),
        timing: meta.to_timing(),
        stream: meta.to_stream_details(),
    };
    control_store.apply_usage_rollup_owned(event).await
}
pub(crate) fn missing_codex_usage() -> UsageBreakdown {
    UsageBreakdown {
        usage_missing: true,
        ..UsageBreakdown::default()
    }
}
