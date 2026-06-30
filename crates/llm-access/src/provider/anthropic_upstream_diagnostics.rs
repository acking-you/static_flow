use llm_access_kiro::anthropic::preflight::PreprocessedMessagesRequest;
use serde_json::json;

const PREFLIGHT_DETAIL_LIMIT: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DirectAnthropicPreflightStats {
    pub(super) normalized: bool,
    pub(super) tool_use_id_rewrite_count: usize,
    pub(super) normalization_event_count: usize,
    pub(super) tool_normalization_event_count: usize,
    pub(super) tool_schema_keyword_count: usize,
}

pub(super) fn direct_anthropic_preflight_stats(
    preflight: &PreprocessedMessagesRequest,
) -> DirectAnthropicPreflightStats {
    let tool_schema_keyword_count = preflight
        .tool_validation_summary
        .schema_keyword_counts
        .values()
        .sum();
    let normalized = !preflight.tool_use_id_rewrites.is_empty()
        || !preflight.normalization_events.is_empty()
        || !preflight.tool_normalization_events.is_empty()
        || preflight
            .tool_validation_summary
            .normalized_tool_description_count
            > 0
        || preflight.tool_validation_summary.empty_tool_name_count > 0
        || tool_schema_keyword_count > 0;

    DirectAnthropicPreflightStats {
        normalized,
        tool_use_id_rewrite_count: preflight.tool_use_id_rewrites.len(),
        normalization_event_count: preflight.normalization_events.len(),
        tool_normalization_event_count: preflight.tool_normalization_events.len(),
        tool_schema_keyword_count,
    }
}

pub(super) fn build_direct_anthropic_routing_diagnostics(
    channel_name: &str,
    pool_mode: &str,
    preflight: &PreprocessedMessagesRequest,
) -> String {
    let stats = direct_anthropic_preflight_stats(preflight);
    json!({
        "upstream_pool": "direct_anthropic",
        "channel_name": channel_name,
        "pool_mode": pool_mode,
        "preflight": {
            "normalized": stats.normalized,
            "tool_use_id_rewrite_count": stats.tool_use_id_rewrite_count,
            "normalization_event_count": stats.normalization_event_count,
            "tool_normalization_event_count": stats.tool_normalization_event_count,
            "tool_schema_keyword_count": stats.tool_schema_keyword_count,
            "tool_validation_summary": {
                "normalized_tool_description_count": preflight.tool_validation_summary.normalized_tool_description_count,
                "empty_tool_name_count": preflight.tool_validation_summary.empty_tool_name_count,
                "schema_keyword_counts": &preflight.tool_validation_summary.schema_keyword_counts,
            },
            "tool_use_id_rewrites": preflight.tool_use_id_rewrites.iter()
                .take(PREFLIGHT_DETAIL_LIMIT)
                .map(|rewrite| json!({
                    "original_tool_use_id": &rewrite.original_tool_use_id,
                    "rewritten_tool_use_id": &rewrite.rewritten_tool_use_id,
                    "assistant_message_index": rewrite.assistant_message_index,
                    "content_block_index": rewrite.content_block_index,
                    "rewritten_tool_result_count": rewrite.rewritten_tool_result_count,
                }))
                .collect::<Vec<_>>(),
            "tool_use_id_rewrites_truncated": preflight.tool_use_id_rewrites.len() > PREFLIGHT_DETAIL_LIMIT,
            "normalization_events": preflight.normalization_events.iter()
                .take(PREFLIGHT_DETAIL_LIMIT)
                .map(|event| json!({
                    "message_index": event.message_index,
                    "role": &event.role,
                    "content_block_index": event.content_block_index,
                    "block_type": event.block_type.as_ref(),
                    "action": event.action,
                    "reason": event.reason,
                }))
                .collect::<Vec<_>>(),
            "normalization_events_truncated": preflight.normalization_events.len() > PREFLIGHT_DETAIL_LIMIT,
            "tool_normalization_events": preflight.tool_normalization_events.iter()
                .take(PREFLIGHT_DETAIL_LIMIT)
                .map(|event| json!({
                    "tool_index": event.tool_index,
                    "tool_name": &event.tool_name,
                    "action": event.action,
                    "reason": event.reason,
                }))
                .collect::<Vec<_>>(),
            "tool_normalization_events_truncated": preflight.tool_normalization_events.len() > PREFLIGHT_DETAIL_LIMIT,
        }
    })
    .to_string()
}
