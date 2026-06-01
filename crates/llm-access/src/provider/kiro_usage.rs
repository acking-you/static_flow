//! Kiro usage summary/billing + usage/preflight/websearch recording.

use std::collections::BTreeMap;

use llm_access_core::{
    provider::{ProtocolFamily, ProviderType},
    store::compute_kiro_billable_tokens,
    usage::UsageEvent,
};
use llm_access_kiro::{
    anthropic::stream::resolve_input_tokens_with_threshold,
    cache_policy::{
        adjust_input_tokens_for_cache_creation_cost_with_policy,
        prefix_tree_credit_ratio_cap_basis_points_with_policy, KiroCachePolicy,
    },
    cache_sim::KiroCacheSimulationMode,
    wire::ConversationState,
};

use super::{
    kiro_model::{build_kiro_cache_context, parse_kiro_billable_model_multipliers_json},
    usage_meta::captured_body_json,
    util::{clamp_u64_to_i64, now_millis},
    KiroCacheContext, KiroPreflightFailureRecord, KiroUsageInputs, KiroUsageRecord,
    KiroUsageSummary, KiroWebsearchUsageRecord,
};

pub fn build_kiro_usage_summary(
    model: &str,
    usage: KiroUsageInputs,
    cache_ctx: &KiroCacheContext,
) -> KiroUsageSummary {
    let (resolved_input_tokens, _) = resolve_input_tokens_with_threshold(
        usage.request_input_tokens,
        usage.context_input_tokens,
        usage.context_usage_min_request_tokens,
    );
    if !usage.cache_estimation_enabled {
        return KiroUsageSummary {
            input_uncached_tokens: resolved_input_tokens,
            input_cached_tokens: 0,
            output_tokens: usage.output_tokens,
            credit_usage: usage.credit_usage,
            credit_usage_missing: usage.credit_usage_missing,
        };
    }
    let authoritative_input_tokens = adjust_input_tokens_for_cache_creation_cost_with_policy(
        &cache_ctx.policy,
        resolved_input_tokens,
        usage.credit_usage,
        usage.cache_estimation_enabled,
    );
    let cached = match cache_ctx.simulation_config.mode {
        KiroCacheSimulationMode::Formula => estimate_formula_cached_tokens(
            model,
            authoritative_input_tokens,
            usage.output_tokens,
            usage.credit_usage,
            &cache_ctx.cache_kmodels,
        ),
        KiroCacheSimulationMode::PrefixTree => {
            estimate_prefix_cached_tokens(authoritative_input_tokens, usage.credit_usage, cache_ctx)
        },
    };
    KiroUsageSummary {
        input_uncached_tokens: authoritative_input_tokens.saturating_sub(cached),
        input_cached_tokens: cached,
        output_tokens: usage.output_tokens,
        credit_usage: usage.credit_usage,
        credit_usage_missing: usage.credit_usage_missing,
    }
}
pub fn anthropic_usage_json_with_policy(
    policy: &KiroCachePolicy,
    input_tokens_total: i32,
    output_tokens: i32,
    cache_read_input_tokens: i32,
) -> serde_json::Value {
    let input_tokens_total = input_tokens_total.max(0);
    let cache_read_input_tokens = cache_read_input_tokens.max(0).min(input_tokens_total);
    let non_cached_input_tokens_total = input_tokens_total.saturating_sub(cache_read_input_tokens);
    let cache_creation_input_tokens = if cache_read_input_tokens == 0 {
        non_cached_input_tokens_total / 2
    } else {
        let ratio = policy.anthropic_cache_creation_input_ratio;
        if !ratio.is_finite() || ratio <= 0.0 {
            0
        } else {
            (((non_cached_input_tokens_total as f64) * ratio).floor() as i32)
                .max(0)
                .min(non_cached_input_tokens_total)
        }
    };
    let input_tokens = non_cached_input_tokens_total.saturating_sub(cache_creation_input_tokens);
    serde_json::json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens.max(0),
        "cache_creation_input_tokens": cache_creation_input_tokens,
        "cache_read_input_tokens": cache_read_input_tokens,
    })
}
pub fn anthropic_usage_json_from_summary_with_policy(
    usage: KiroUsageSummary,
    cache_ctx: &KiroCacheContext,
) -> serde_json::Value {
    anthropic_usage_json_with_policy(
        &cache_ctx.policy,
        usage.input_uncached_tokens + usage.input_cached_tokens,
        usage.output_tokens,
        usage.input_cached_tokens,
    )
}
fn estimate_formula_cached_tokens(
    model: &str,
    input_tokens_total: i32,
    output_tokens: i32,
    credit_usage: Option<f64>,
    kmodels: &BTreeMap<String, f64>,
) -> i32 {
    let safe_input = input_tokens_total.max(0);
    let Some(observed_credit) = credit_usage.filter(|value| value.is_finite() && *value >= 0.0)
    else {
        return 0;
    };
    let Some(kmodel) = kmodels
        .get(normalize_kiro_kmodel_name(model))
        .copied()
        .filter(|value| value.is_finite() && *value > 0.0)
    else {
        return 0;
    };
    let safe_full_cost = kmodel * (safe_input as f64 + 5.0 * output_tokens.max(0) as f64);
    if !safe_full_cost.is_finite() || safe_full_cost <= observed_credit || safe_input <= 0 {
        return 0;
    }
    ((safe_full_cost - observed_credit) / (0.9 * kmodel))
        .floor()
        .max(0.0)
        .min(safe_input as f64) as i32
}
fn estimate_prefix_cached_tokens(
    authoritative_input_tokens: i32,
    credit_usage: Option<f64>,
    cache_ctx: &KiroCacheContext,
) -> i32 {
    let authoritative_input_u64 = authoritative_input_tokens.max(0) as u64;
    let projected_total = cache_ctx.projection.projected_input_token_count().max(1);
    let matched = cache_ctx
        .prefix_cache_match
        .matched_tokens
        .min(projected_total);
    let prefix_cached = ((u128::from(authoritative_input_u64) * u128::from(matched))
        / u128::from(projected_total))
    .min(u128::from(authoritative_input_u64)) as i32;
    let Some(cap_basis_points) =
        prefix_tree_credit_ratio_cap_basis_points_with_policy(&cache_ctx.policy, credit_usage)
    else {
        return prefix_cached;
    };
    let ratio_cap = ((u128::from(authoritative_input_u64) * u128::from(cap_basis_points))
        / 10_000_u128)
        .min(u128::from(authoritative_input_u64)) as i32;
    prefix_cached.min(ratio_cap)
}
pub fn normalize_kiro_kmodel_name(model: &str) -> &str {
    match model {
        "claude-opus-4.6" => "claude-opus-4-6",
        "claude-opus-4.7" => "claude-opus-4-7",
        "claude-opus-4.8" => "claude-opus-4-8",
        _ => model,
    }
}
fn zero_kiro_usage_summary() -> KiroUsageSummary {
    KiroUsageSummary {
        input_uncached_tokens: 0,
        input_cached_tokens: 0,
        output_tokens: 0,
        credit_usage: None,
        credit_usage_missing: false,
    }
}
pub async fn record_kiro_preflight_failure(record: KiroPreflightFailureRecord<'_>) {
    record.meta.mark_stream_finish();
    let conversation_state = ConversationState::new(uuid::Uuid::new_v4().to_string());
    let cache_ctx =
        match build_kiro_cache_context(record.route, &conversation_state, record.cache_simulator) {
            Ok(cache_ctx) => cache_ctx,
            Err(err) => {
                tracing::warn!(
                    key_id = %record.key.key_id,
                    account = %record.route.account_name,
                    error = %err,
                    "failed to build kiro cache context for preflight failure usage"
                );
                return;
            },
        };
    if let Err(err) = record_kiro_usage(KiroUsageRecord {
        control_store: record.control_store,
        key: record.key,
        route: record.route,
        endpoint: record.endpoint,
        model: record.model,
        status: record.status,
        usage: zero_kiro_usage_summary(),
        cache_ctx: &cache_ctx,
        meta: record.meta,
    })
    .await
    {
        tracing::warn!(
            key_id = %record.key.key_id,
            account = %record.route.account_name,
            error = %err,
            "failed to record kiro preflight failure usage"
        );
    }
}
pub async fn record_kiro_usage(record: KiroUsageRecord<'_>) -> anyhow::Result<()> {
    let billable_tokens = kiro_billable_tokens(record.model, record.usage, record.cache_ctx);
    let capture_request_details = record.status.as_u16() >= 400
        || record.route.full_request_logging_enabled
        || (record.route.zero_cache_debug_enabled
            && record.status.is_success()
            && record.usage.input_cached_tokens <= 0);
    let client_request_body_json = capture_request_details
        .then(|| captured_body_json(&record.meta.client_request_body_json))
        .flatten();
    let upstream_request_body_json = capture_request_details
        .then(|| captured_body_json(&record.meta.upstream_request_body_json))
        .flatten();
    let full_request_json = capture_request_details
        .then(|| {
            captured_body_json(&record.meta.full_request_json)
                .or_else(|| captured_body_json(&record.meta.client_request_body_json))
        })
        .flatten();
    let event = UsageEvent {
        event_id: format!("llm-usage-{}", uuid::Uuid::new_v4()),
        created_at_ms: now_millis(),
        provider_type: ProviderType::Kiro,
        protocol_family: ProtocolFamily::Anthropic,
        key_id: record.key.key_id.clone(),
        key_name: record.key.key_name.clone(),
        account_name: Some(record.route.account_name.clone()),
        account_group_id_at_event: record.route.account_group_id_at_event.clone(),
        route_strategy_at_event: Some(record.route.route_strategy_at_event),
        request_method: record.meta.request_method.clone(),
        request_url: record.meta.request_url.clone(),
        endpoint: record.endpoint.to_string(),
        model: Some(record.model.to_string()),
        mapped_model: None,
        status_code: record.status.as_u16() as i64,
        request_body_bytes: record.meta.request_body_bytes,
        quota_failover_count: record.meta.quota_failover_count,
        routing_diagnostics_json: record.meta.routing_diagnostics_json.clone(),
        input_uncached_tokens: i64::from(record.usage.input_uncached_tokens.max(0)),
        input_cached_tokens: i64::from(record.usage.input_cached_tokens.max(0)),
        output_tokens: i64::from(record.usage.output_tokens.max(0)),
        billable_tokens: clamp_u64_to_i64(billable_tokens),
        credit_usage: record
            .usage
            .credit_usage
            .map(|value| value.max(0.0).to_string()),
        usage_missing: false,
        credit_usage_missing: record.usage.credit_usage_missing,
        client_ip: record.meta.client_ip.clone(),
        ip_region: record.meta.ip_region.clone(),
        request_headers_json: record.meta.request_headers_json.clone(),
        last_message_content: record.meta.last_message_content.clone(),
        client_request_body_json,
        upstream_request_body_json,
        full_request_json,
        error_message: record.meta.error_message.clone(),
        error_body: record.meta.error_body.clone(),
        timing: record.meta.to_timing(),
        stream: record.meta.to_stream_details(),
    };
    record.control_store.apply_usage_rollup_owned(event).await
}
pub async fn record_kiro_websearch_usage(
    record: KiroWebsearchUsageRecord<'_>,
) -> anyhow::Result<()> {
    let multipliers =
        parse_kiro_billable_model_multipliers_json(&record.route.billable_model_multipliers_json)?;
    let capture_request_details = record.capture_request_details || !record.status.is_success();
    let event = UsageEvent {
        event_id: format!("llm-usage-{}", uuid::Uuid::new_v4()),
        created_at_ms: now_millis(),
        provider_type: ProviderType::Kiro,
        protocol_family: ProtocolFamily::Anthropic,
        key_id: record.key.key_id.clone(),
        key_name: record.key.key_name.clone(),
        account_name: Some(record.route.account_name.clone()),
        account_group_id_at_event: record.route.account_group_id_at_event.clone(),
        route_strategy_at_event: Some(record.route.route_strategy_at_event),
        request_method: record.meta.request_method.clone(),
        request_url: record.meta.request_url.clone(),
        endpoint: "/mcp".to_string(),
        model: Some(record.model.to_string()),
        mapped_model: None,
        status_code: record.status.as_u16() as i64,
        request_body_bytes: record.meta.request_body_bytes,
        quota_failover_count: record.meta.quota_failover_count,
        routing_diagnostics_json: record.meta.routing_diagnostics_json.clone(),
        input_uncached_tokens: i64::from(record.usage.input_uncached_tokens.max(0)),
        input_cached_tokens: i64::from(record.usage.input_cached_tokens.max(0)),
        output_tokens: i64::from(record.usage.output_tokens.max(0)),
        billable_tokens: clamp_u64_to_i64(kiro_billable_tokens_with_multipliers(
            record.model,
            record.usage,
            &multipliers,
        )),
        credit_usage: None,
        usage_missing: false,
        credit_usage_missing: true,
        client_ip: record.meta.client_ip.clone(),
        ip_region: record.meta.ip_region.clone(),
        request_headers_json: record.meta.request_headers_json.clone(),
        last_message_content: record.meta.last_message_content.clone(),
        client_request_body_json: capture_request_details
            .then(|| captured_body_json(&record.meta.client_request_body_json))
            .flatten(),
        upstream_request_body_json: capture_request_details
            .then(|| captured_body_json(&record.meta.upstream_request_body_json))
            .flatten(),
        full_request_json: capture_request_details
            .then(|| {
                captured_body_json(&record.meta.full_request_json)
                    .or_else(|| captured_body_json(&record.meta.client_request_body_json))
            })
            .flatten(),
        error_message: record.meta.error_message.clone(),
        error_body: record.meta.error_body.clone(),
        timing: record.meta.to_timing(),
        stream: record.meta.to_stream_details(),
    };
    record.control_store.apply_usage_rollup_owned(event).await
}
fn kiro_billable_tokens(model: &str, usage: KiroUsageSummary, cache_ctx: &KiroCacheContext) -> u64 {
    kiro_billable_tokens_with_multipliers(model, usage, &cache_ctx.billable_model_multipliers)
}
pub fn kiro_billable_tokens_with_multipliers(
    model: &str,
    usage: KiroUsageSummary,
    multipliers: &BTreeMap<String, f64>,
) -> u64 {
    compute_kiro_billable_tokens(
        Some(model),
        usage.input_uncached_tokens.max(0) as u64,
        usage.input_cached_tokens.max(0) as u64,
        usage.output_tokens.max(0) as u64,
        multipliers,
    )
}
