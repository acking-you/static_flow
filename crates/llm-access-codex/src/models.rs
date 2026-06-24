//! Pure Codex model-list and model-catalog normalization helpers.

use std::collections::{BTreeSet, HashMap};

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::{
    instructions::codex_default_instructions, types::GatewayModelDescriptor, GPT53_CODEX_MODEL_ID,
    GPT53_CODEX_SPARK_MODEL_ID,
};

const GATEWAY_MODELS_OWNER: &str = "static-flow";
const BUNDLED_CODEX_MODELS_JSON: &str = include_str!("../codex_models.json");

/// Owner label used on the OpenAI-compatible `/v1/models` response.
pub fn gateway_models_owner() -> &'static str {
    GATEWAY_MODELS_OWNER
}

/// Extract model ids from either ChatGPT-style or OpenAI-style model payloads.
pub fn extract_gateway_model_descriptors(
    value: &Value,
    owned_by: &'static str,
) -> Vec<GatewayModelDescriptor> {
    let mut items = BTreeSet::<GatewayModelDescriptor>::new();
    if let Some(models) = value.get("models").and_then(Value::as_array) {
        for item in models {
            if !catalog_model_is_api_visible(item) {
                continue;
            }
            let id = item
                .get("slug")
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if let Some(id) = id {
                items.insert(GatewayModelDescriptor {
                    id: id.to_string(),
                    owned_by,
                });
            }
        }
    }
    if let Some(data) = value.get("data").and_then(Value::as_array) {
        for item in data {
            let id = item
                .get("id")
                .or_else(|| item.get("slug"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if let Some(id) = id {
                items.insert(GatewayModelDescriptor {
                    id: id.to_string(),
                    owned_by,
                });
            }
        }
    }
    items.into_iter().collect()
}

fn catalog_model_is_api_visible(item: &Value) -> bool {
    let supported_in_api = item
        .get("supported_in_api")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let visible = item
        .get("visibility")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_none_or(|value| !value.eq_ignore_ascii_case("hide"));
    supported_in_api && visible
}

fn map_catalog_slug(slug: &str, map_gpt53_codex_to_spark: bool) -> (&str, bool) {
    if map_gpt53_codex_to_spark && slug == GPT53_CODEX_SPARK_MODEL_ID {
        (GPT53_CODEX_MODEL_ID, true)
    } else {
        (slug, false)
    }
}

/// Normalize the raw Codex model catalog exposed to clients.
pub fn normalize_public_model_catalog_value(
    mut value: Value,
    map_gpt53_codex_to_spark: bool,
) -> Result<Value> {
    let models = value
        .get_mut("models")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow!("upstream models payload is missing a models array"))?;
    let mut seen = HashMap::<String, usize>::new();
    let mut chosen = Vec::<(bool, Value)>::new();

    for mut item in std::mem::take(models) {
        let raw_slug = item
            .get("slug")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        let Some(raw_slug) = raw_slug else {
            continue;
        };
        let (final_slug, was_alias) = map_catalog_slug(&raw_slug, map_gpt53_codex_to_spark);
        if let Some(object) = item.as_object_mut() {
            object.insert("slug".to_string(), Value::String(final_slug.to_string()));
            let needs_fallback_instructions = object
                .get("base_instructions")
                .and_then(Value::as_str)
                .map(|value| value.trim().is_empty())
                .unwrap_or(true);
            if needs_fallback_instructions {
                object.insert(
                    "base_instructions".to_string(),
                    Value::String(codex_default_instructions().to_string()),
                );
            }
            if was_alias
                && object.get("display_name").and_then(Value::as_str) == Some(raw_slug.as_str())
            {
                object.insert("display_name".to_string(), Value::String(final_slug.to_string()));
            }
        }
        if let Some(index) = seen.get(final_slug).copied() {
            if chosen[index].0 && !was_alias {
                chosen[index] = (false, item);
            }
            continue;
        }
        seen.insert(final_slug.to_string(), chosen.len());
        chosen.push((was_alias, item));
    }

    if chosen.is_empty() {
        return Err(anyhow!("upstream model catalog contains no usable models"));
    }
    *models = chosen.into_iter().map(|(_, item)| item).collect();
    Ok(value)
}

/// Build the standalone service's default public model catalog.
pub fn default_public_model_catalog_value() -> Result<Value> {
    normalize_public_model_catalog_value(serde_json::from_str(BUNDLED_CODEX_MODELS_JSON)?, false)
}

/// Encode the standalone service's default public model catalog as JSON.
pub fn default_public_model_catalog_json() -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&default_public_model_catalog_value()?)?)
}

/// Build an OpenAI-compatible `/v1/models` payload from a Codex model catalog.
pub fn openai_models_response_value_from_catalog(
    catalog: &Value,
    map_gpt53_codex_to_spark: bool,
    created: i64,
) -> Value {
    let data = apply_model_aliases(
        extract_gateway_model_descriptors(catalog, gateway_models_owner()),
        map_gpt53_codex_to_spark,
    )
    .into_iter()
    .map(|item| {
        json!({
            "id": item.id,
            "object": "model",
            "created": created,
            "owned_by": item.owned_by,
        })
    })
    .collect::<Vec<_>>();
    json!({
        "object": "list",
        "data": data,
    })
}

/// Encode the standalone service's default OpenAI-compatible `/v1/models`
/// response as JSON.
pub fn default_openai_models_response_json(created: i64) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&openai_models_response_value_from_catalog(
        &default_public_model_catalog_value()?,
        false,
        created,
    ))?)
}

#[cfg(test)]
fn parse_public_model_catalog_json(body: &[u8], map_gpt53_codex_to_spark: bool) -> Result<Value> {
    use anyhow::Context;

    let value = serde_json::from_slice::<Value>(body).context("failed to parse catalog json")?;
    normalize_public_model_catalog_value(value, map_gpt53_codex_to_spark)
}

/// Apply StaticFlow's public model aliasing to a model descriptor list.
pub fn apply_model_aliases(
    models: Vec<GatewayModelDescriptor>,
    map_gpt53_codex_to_spark: bool,
) -> Vec<GatewayModelDescriptor> {
    if !map_gpt53_codex_to_spark {
        return models;
    }

    models
        .into_iter()
        .map(|mut item| {
            if item.id == GPT53_CODEX_SPARK_MODEL_ID {
                item.id = GPT53_CODEX_MODEL_ID.to_string();
            }
            item
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Ensure model requests carry a Codex client_version query parameter.
pub fn append_client_version_query(url: &str, client_version: &str) -> String {
    if url.contains("client_version=") {
        return url.to_string();
    }
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}client_version={}", urlencoding::encode(client_version))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        append_client_version_query, gateway_models_owner, parse_public_model_catalog_json,
    };
    use crate::instructions::codex_default_instructions;

    #[test]
    fn codex_models_are_tagged_with_static_flow_owner() {
        assert_eq!(gateway_models_owner(), "static-flow");
    }

    #[test]
    fn append_client_version_query_url_encodes_value() {
        let url = append_client_version_query("https://example.com/v1/models", "0.124.0 beta");
        assert_eq!(url, "https://example.com/v1/models?client_version=0.124.0%20beta");
    }

    #[test]
    fn public_model_catalog_rewrites_alias_slug_and_dedupes() {
        let body = serde_json::to_vec(&json!({
            "models": [
                {
                    "slug": "gpt-5.3-codex-spark",
                    "display_name": "gpt-5.3-codex-spark",
                    "supported_in_api": false
                },
                {
                    "slug": "gpt-5.3-codex",
                    "display_name": "gpt-5.3-codex",
                    "supported_in_api": true
                },
                {
                    "slug": "gpt-5.5",
                    "display_name": "gpt-5.5",
                    "supported_in_api": true,
                    "base_instructions": "upstream instructions",
                    "model_messages": {
                        "instructions_template": "upstream template",
                        "instructions_variables": null
                    }
                }
            ]
        }))
        .expect("serialize sample models payload");

        let value =
            parse_public_model_catalog_json(&body, true).expect("catalog json should parse");
        let models = value["models"]
            .as_array()
            .expect("models should stay an array");

        assert_eq!(models.len(), 2);
        assert_eq!(models[0]["slug"], "gpt-5.3-codex");
        assert_eq!(models[0]["display_name"], "gpt-5.3-codex");
        assert_eq!(models[0]["supported_in_api"], true);
        assert_eq!(models[1]["slug"], "gpt-5.5");
        assert_eq!(models[1]["base_instructions"], "upstream instructions");
        assert_eq!(models[1]["model_messages"]["instructions_template"], "upstream template");
    }

    #[test]
    fn public_model_catalog_preserves_upstream_model_prompts() {
        let body = serde_json::to_vec(&json!({
            "models": [
                {
                    "slug": "gpt-5.5",
                    "display_name": "GPT-5.5",
                    "supported_in_api": true,
                    "base_instructions": "upstream model instructions",
                    "model_messages": {
                        "instructions_template": "upstream template",
                        "instructions_variables": {
                            "personality_pragmatic": "pragmatic personality"
                        }
                    }
                }
            ]
        }))
        .expect("serialize sample models payload");

        let value =
            parse_public_model_catalog_json(&body, false).expect("catalog json should parse");
        let model = &value["models"][0];

        assert_eq!(model["base_instructions"], "upstream model instructions");
        assert_eq!(model["model_messages"]["instructions_template"], "upstream template");
        assert_eq!(
            model["model_messages"]["instructions_variables"]["personality_pragmatic"],
            "pragmatic personality"
        );
    }

    #[test]
    fn public_model_catalog_default_instructions_json_round_trips() {
        let body = serde_json::to_vec(&json!({
            "models": [
                {
                    "slug": "gpt-5.5",
                    "display_name": "gpt-5.5",
                    "supported_in_api": true
                }
            ]
        }))
        .expect("serialize sample models payload");

        let value =
            parse_public_model_catalog_json(&body, false).expect("catalog json should parse");
        let encoded = serde_json::to_vec(&value).expect("catalog json should encode");
        let raw_json = String::from_utf8(encoded.clone()).expect("catalog json is utf8");
        assert!(raw_json.contains("You are a coding agent running in the Codex CLI"));

        let decoded: serde_json::Value =
            serde_json::from_slice(&encoded).expect("encoded catalog should decode");
        assert_eq!(
            decoded["models"][0]["base_instructions"].as_str(),
            Some(codex_default_instructions())
        );
    }

    #[test]
    fn public_model_catalog_requires_models_array() {
        let body = serde_json::to_vec(&json!({
            "data": [
                { "id": "gpt-5.5" }
            ]
        }))
        .expect("serialize fallback list payload");

        let err = parse_public_model_catalog_json(&body, false)
            .expect_err("payload without models array should fail");

        assert!(err.to_string().contains("models array"));
    }

    #[test]
    fn default_public_model_catalog_carries_model_specific_instructions() {
        let value = super::default_public_model_catalog_value()
            .expect("default model catalog should build");
        let models = value["models"]
            .as_array()
            .expect("models should be an array");

        let gpt55 = models
            .iter()
            .find(|item| item["slug"].as_str() == Some("gpt-5.5"))
            .expect("gpt-5.5 should be in bundled catalog");
        assert!(gpt55["base_instructions"]
            .as_str()
            .is_some_and(|instructions| instructions.starts_with("You are Codex")));
        assert!(gpt55.get("model_messages").is_some());
        assert!(models.iter().all(|item| item
            .get("base_instructions")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|instructions| !instructions.trim().is_empty())));
    }

    #[test]
    fn default_public_model_catalog_tracks_bundled_codex_models() {
        let value = super::default_public_model_catalog_value()
            .expect("default model catalog should build");
        let slugs = value["models"]
            .as_array()
            .expect("models should be an array")
            .iter()
            .filter_map(|item| item["slug"].as_str())
            .collect::<Vec<_>>();

        assert!(slugs.contains(&"gpt-5.5"));
        assert!(slugs.contains(&"gpt-5.2"));
        assert!(!slugs.contains(&"gpt-5.5-mini"));
    }

    #[test]
    fn openai_models_response_filters_hidden_and_unsupported_catalog_models() {
        let catalog = json!({
            "models": [
                { "slug": "gpt-visible", "supported_in_api": true },
                { "slug": "gpt-hidden", "visibility": "hide", "supported_in_api": true },
                { "slug": "gpt-unsupported", "supported_in_api": false }
            ]
        });

        let value = super::openai_models_response_value_from_catalog(&catalog, false, 123);
        let ids = value["data"]
            .as_array()
            .expect("data should be an array")
            .iter()
            .filter_map(|item| item["id"].as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["gpt-visible"]);
    }

    #[test]
    fn default_openai_models_response_uses_staticflow_owner() {
        let body = super::default_openai_models_response_json(123)
            .expect("default models response should build");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        let data = value["data"].as_array().expect("data should be an array");

        assert_eq!(value["object"], "list");
        assert!(data.iter().any(|item| item["id"] == "gpt-5.5"
            && item["object"] == "model"
            && item["created"] == 123
            && item["owned_by"] == "static-flow"));
    }
}
