//! Codex model-routing and billing policy: spark remap, fast tier, store
//! alignment, billable multiplier.


use axum::body::Bytes;
use serde_json::Value;

use super::{
    native_responses::strip_input_item_ids, normalization::is_azure_responses_upstream_base,
};
use crate::{
    error::{internal_error, CodexGatewayResult},
    types::PreparedGatewayRequest,
    FAST_BILLABLE_MULTIPLIER, GPT53_CODEX_MODEL_ID, GPT53_CODEX_SPARK_MODEL_ID,
};
/// Map the public `gpt-5.3-codex` id onto the current upstream Spark id.
pub fn apply_gpt53_codex_spark_mapping(
    prepared: &PreparedGatewayRequest,
    enabled: bool,
) -> CodexGatewayResult<PreparedGatewayRequest> {
    if !enabled || prepared.model.as_deref() != Some(GPT53_CODEX_MODEL_ID) {
        return Ok(prepared.clone());
    }

    let mut value = serde_json::from_slice::<Value>(&prepared.request_body)
        .map_err(|err| internal_error("Failed to parse mapped llm gateway request body", err))?;
    let Some(root) = value.as_object_mut() else {
        return Err(internal_error(
            "Failed to map llm gateway request model",
            "request body is not a JSON object",
        ));
    };
    root.insert("model".to_string(), Value::String(GPT53_CODEX_SPARK_MODEL_ID.to_string()));
    let request_body =
        Bytes::from(serde_json::to_vec(&value).map_err(|err| {
            internal_error("Failed to encode mapped llm gateway request body", err)
        })?);

    let mut mapped = prepared.clone();
    mapped.request_body = request_body;
    mapped.model = Some(GPT53_CODEX_SPARK_MODEL_ID.to_string());
    mapped.client_visible_model = Some(GPT53_CODEX_MODEL_ID.to_string());
    Ok(mapped)
}
/// Apply the per-key Codex fast policy to the upstream request body.
pub fn apply_codex_fast_policy(
    prepared: &PreparedGatewayRequest,
    enabled: bool,
) -> CodexGatewayResult<PreparedGatewayRequest> {
    let mut value = serde_json::from_slice::<Value>(&prepared.request_body)
        .map_err(|err| internal_error("Failed to parse llm gateway request body", err))?;
    let Some(root) = value.as_object_mut() else {
        return Err(internal_error(
            "Failed to apply llm gateway fast policy",
            "request body is not a JSON object",
        ));
    };

    let mut changed = false;
    if let Some(service_tier) = root.get_mut("service_tier") {
        let normalized_fast = service_tier
            .as_str()
            .is_some_and(|raw| raw.eq_ignore_ascii_case("fast"));
        let is_priority = service_tier
            .as_str()
            .is_some_and(|raw| raw.eq_ignore_ascii_case("priority"));
        if normalized_fast {
            *service_tier = Value::String("priority".to_string());
            changed = true;
        }
        if !enabled && (normalized_fast || is_priority) {
            root.remove("service_tier");
            changed = true;
        }
    }

    let billable_multiplier = resolve_billable_multiplier(Some(&value));
    if !changed && prepared.billable_multiplier == billable_multiplier {
        return Ok(prepared.clone());
    }

    let request_body = Bytes::from(serde_json::to_vec(&value).map_err(|err| {
        internal_error("Failed to encode llm gateway fast-policy request body", err)
    })?);

    let mut adjusted = prepared.clone();
    adjusted.request_body = request_body;
    adjusted.billable_multiplier = billable_multiplier;
    Ok(adjusted)
}
/// Align the outgoing native responses `store` field with the selected
/// upstream provider semantics.
pub fn align_responses_store_with_upstream(
    prepared: &PreparedGatewayRequest,
    upstream_base: &str,
) -> CodexGatewayResult<PreparedGatewayRequest> {
    if !prepared.upstream_path.starts_with("/v1/responses") {
        return Ok(prepared.clone());
    }

    let mut value = serde_json::from_slice::<Value>(&prepared.request_body).map_err(|err| {
        internal_error("Failed to parse llm gateway request body for store alignment", err)
    })?;
    let Some(root) = value.as_object_mut() else {
        return Err(internal_error(
            "Failed to align llm gateway request store field",
            "request body is not a JSON object",
        ));
    };

    let changed = if prepared.upstream_path.starts_with("/v1/responses/compact") {
        let removed_store = root.remove("store").is_some();
        let removed_previous_response_id = root.remove("previous_response_id").is_some();
        let removed_item_ids = strip_input_item_ids(root);
        removed_store || removed_previous_response_id || removed_item_ids
    } else {
        let is_azure = is_azure_responses_upstream_base(upstream_base);
        let store = is_azure;
        let mut changed = false;
        if root.get("store") != Some(&Value::Bool(store)) {
            root.insert("store".to_string(), Value::Bool(store));
            changed = true;
        }
        if !store && root.remove("previous_response_id").is_some() {
            changed = true;
        }
        if !(store && is_azure) && strip_input_item_ids(root) {
            changed = true;
        }
        changed
    };

    if !changed {
        return Ok(prepared.clone());
    }

    let request_body = Bytes::from(serde_json::to_vec(&value).map_err(|err| {
        internal_error("Failed to encode llm gateway request body after store alignment", err)
    })?);

    let mut aligned = prepared.clone();
    aligned.request_body = request_body;
    Ok(aligned)
}
/// Convert request-level service tier hints into a billing multiplier.
/// Resolve the billing multiplier implied by a request JSON object.
pub fn resolve_billable_multiplier(json_value: Option<&Value>) -> u64 {
    if request_uses_fast_service_tier(json_value) {
        FAST_BILLABLE_MULTIPLIER
    } else {
        1
    }
}
/// Detect whether the request explicitly opted into the fast/priority tier.
fn request_uses_fast_service_tier(json_value: Option<&Value>) -> bool {
    json_value
        .and_then(Value::as_object)
        .and_then(|root| root.get("service_tier"))
        .and_then(Value::as_str)
        .is_some_and(|tier| {
            tier.eq_ignore_ascii_case("fast") || tier.eq_ignore_ascii_case("priority")
        })
}
