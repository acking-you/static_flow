//! Kiro request payload shaping: model mapping, thinking override,
//! cache-context build, and kmodel/multiplier JSON parsing.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) fn apply_kiro_model_mapping(
    model_name_map_json: &str,
    payload: &mut MessagesRequest,
) -> anyhow::Result<Option<(String, String)>> {
    let trimmed = model_name_map_json.trim();
    if trimmed.is_empty() || trimmed == "{}" {
        return Ok(None);
    }
    let map = serde_json::from_str::<BTreeMap<String, String>>(trimmed)?;
    let Some(target_model) = map.get(&payload.model).cloned() else {
        return Ok(None);
    };
    if target_model == payload.model {
        return Ok(None);
    }
    let source_model = payload.model.clone();
    payload.model = target_model.clone();
    Ok(Some((source_model, target_model)))
}
pub(crate) fn override_kiro_thinking_from_model_name(payload: &mut MessagesRequest) {
    let model = payload.model.to_lowercase();
    if !model.contains("thinking") {
        return;
    }
    let is_high_reasoning_opus = model.contains("opus")
        && (model.contains("4-6")
            || model.contains("4.6")
            || model.contains("4-7")
            || model.contains("4.7")
            || model.contains("4-8")
            || model.contains("4.8"));
    payload.thinking = Some(Thinking {
        thinking_type: if is_high_reasoning_opus {
            "adaptive".to_string()
        } else {
            "enabled".to_string()
        },
        display: None,
        budget_tokens: 20_000,
    });
    if is_high_reasoning_opus {
        let output_config = payload.output_config.get_or_insert(OutputConfig {
            effort: None,
            format: None,
        });
        if output_config.effort.is_none() {
            output_config.effort = Some("xhigh".to_string());
        }
    }
}
pub(crate) fn build_kiro_cache_context(
    route: &ProviderKiroRoute,
    conversation_state: &llm_access_kiro::wire::ConversationState,
    cache_simulator: &KiroCacheSimulator,
) -> anyhow::Result<KiroCacheContext> {
    let policy = if route.cache_policy_json.trim().is_empty() {
        default_kiro_cache_policy()
    } else {
        serde_json::from_str::<KiroCachePolicy>(&route.cache_policy_json)?
    };
    validate_kiro_cache_policy(&policy)?;
    let simulation_config = KiroCacheSimulationConfig {
        mode: KiroCacheSimulationMode::from_runtime_value(&route.prefix_cache_mode),
        prefix_cache_max_tokens: route.prefix_cache_max_tokens,
        prefix_cache_entry_ttl: Duration::from_secs(route.prefix_cache_entry_ttl_seconds),
        conversation_anchor_max_entries: route.conversation_anchor_max_entries as usize,
        conversation_anchor_ttl: Duration::from_secs(route.conversation_anchor_ttl_seconds),
    };
    let projection = RuntimePromptProjection::from_conversation_state(conversation_state);
    let prefix_cache_match = if route.cache_estimation_enabled
        && simulation_config.mode == KiroCacheSimulationMode::PrefixTree
    {
        cache_simulator.match_prefix_from_runtime_projection(
            &projection,
            simulation_config,
            Instant::now(),
        )
    } else {
        llm_access_kiro::cache_sim::PrefixCacheMatch::default()
    };
    Ok(KiroCacheContext {
        policy,
        simulation_config,
        projection,
        prefix_cache_match,
        conversation_id: conversation_state.conversation_id.clone(),
        cache_kmodels: parse_kiro_cache_kmodels_json(&route.cache_kmodels_json)?,
        billable_model_multipliers: parse_kiro_billable_model_multipliers_json(
            &route.billable_model_multipliers_json,
        )?,
    })
}
pub(crate) fn parse_kiro_cache_kmodels_json(value: &str) -> anyhow::Result<BTreeMap<String, f64>> {
    let map = serde_json::from_str::<BTreeMap<String, f64>>(value)?;
    for (model, kmodel) in &map {
        if !kmodel.is_finite() || *kmodel <= 0.0 {
            anyhow::bail!("kiro cache kmodel `{model}` must be positive and finite");
        }
    }
    Ok(map)
}
pub(crate) fn parse_kiro_billable_model_multipliers_json(
    value: &str,
) -> anyhow::Result<BTreeMap<String, f64>> {
    let map = serde_json::from_str::<BTreeMap<String, f64>>(value)?;
    for (family, multiplier) in &map {
        if !matches!(family.as_str(), "opus" | "sonnet" | "haiku") {
            anyhow::bail!("kiro billable multiplier family `{family}` is invalid");
        }
        if !multiplier.is_finite() || *multiplier <= 0.0 {
            anyhow::bail!("kiro billable multiplier `{family}` must be positive and finite");
        }
    }
    Ok(map)
}
pub(crate) fn decode_kiro_events_from_bytes(bytes: &[u8]) -> Result<Vec<Event>, String> {
    let mut decoder = EventStreamDecoder::new();
    let _ = decoder.feed(bytes);
    let mut events = Vec::new();
    for result in decoder.decode_iter() {
        let frame = result.map_err(|err| format!("failed to decode kiro event frame: {err}"))?;
        let event =
            Event::from_frame(frame).map_err(|err| format!("failed to parse kiro event: {err}"))?;
        events.push(event);
    }
    Ok(events)
}
pub(crate) fn normalize_kiro_kmodel_name(model: &str) -> &str {
    match model {
        "claude-opus-4.6" => "claude-opus-4-6",
        "claude-opus-4.7" => "claude-opus-4-7",
        "claude-opus-4.8" => "claude-opus-4-8",
        _ => model,
    }
}
