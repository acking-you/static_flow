//! Resolve a stable Codex session id from normalized request shape.
//!
//! Explicit client-provided anchors win. Derived ids are only used when the
//! request lacks a usable session/thread/conversation/prompt-cache anchor.

use std::fmt::Write as _;

use axum::http::HeaderMap;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::extract_non_empty_string;
use crate::types::CodexResolvedSessionSource;

const SESSION_ID_PREFIX: &str = "codex-session-v1-";
const STABLE_PREFIX_HASH_SALT: &[u8] = b"codex-stable-prefix-v1\0";
const BOOTSTRAP_HASH_SALT: &[u8] = b"codex-bootstrap-request-v1\0";
const HASH_ID_HEX_LEN: usize = 32;
const HASH_PREVIEW_HEX_LEN: usize = 12;
const SESSION_METADATA_HEADER: &str = "x-codex-turn-metadata";
const PROMPT_CACHE_KEY: &str = "prompt_cache_key";

const HASH_IGNORED_OBJECT_KEYS: &[&str] = &[
    "id",
    "model",
    "stream",
    "store",
    "include",
    "metadata",
    "client_metadata",
    "previous_response_id",
    "service_tier",
    "prompt_cache_key",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCodexSession {
    pub id: String,
    pub source: CodexResolvedSessionSource,
    pub hash_preview: Option<String>,
}

impl ResolvedCodexSession {
    fn explicit(id: String, source: CodexResolvedSessionSource) -> Self {
        Self {
            id,
            source,
            hash_preview: None,
        }
    }

    fn derived(hash: String, source: CodexResolvedSessionSource) -> Self {
        let id_hex = hex_prefix(&hash, HASH_ID_HEX_LEN);
        let preview = hex_prefix(&hash, HASH_PREVIEW_HEX_LEN);
        Self {
            id: format!("{SESSION_ID_PREFIX}{id_hex}"),
            source,
            hash_preview: Some(preview),
        }
    }
}

#[derive(Default)]
struct CodexTurnMetadataHeader {
    session_id: Option<String>,
    thread_id: Option<String>,
}

pub fn resolve_codex_session(
    headers: &HeaderMap,
    body: &mut Value,
) -> Option<ResolvedCodexSession> {
    let root = body.as_object_mut()?;
    if let Some(explicit) = explicit_session(headers, root) {
        return Some(explicit);
    }

    let (hash_source, source) = if let Some(projection) = stable_prefix_projection(root) {
        (projection, CodexResolvedSessionSource::StablePrefix)
    } else {
        (bootstrap_projection(root), CodexResolvedSessionSource::BootstrapRequest)
    };
    let hash = hash_canonical_value(&hash_source, match source {
        CodexResolvedSessionSource::StablePrefix => STABLE_PREFIX_HASH_SALT,
        CodexResolvedSessionSource::BootstrapRequest => BOOTSTRAP_HASH_SALT,
        _ => unreachable!("only derived sources reach hash generation"),
    });
    let session = ResolvedCodexSession::derived(hash, source);
    root.insert(PROMPT_CACHE_KEY.to_string(), Value::String(session.id.clone()));
    Some(session)
}

fn explicit_session(
    headers: &HeaderMap,
    root: &Map<String, Value>,
) -> Option<ResolvedCodexSession> {
    if let Some(value) = first_header_value(headers, &["session_id", "session-id"]) {
        return Some(ResolvedCodexSession::explicit(
            value,
            CodexResolvedSessionSource::HeaderSessionId,
        ));
    }
    let metadata = parse_codex_turn_metadata_header(headers);
    if let Some(value) = metadata.session_id {
        return Some(ResolvedCodexSession::explicit(
            value,
            CodexResolvedSessionSource::MetadataSessionId,
        ));
    }
    if let Some(value) = first_header_value(headers, &["thread_id", "thread-id"]) {
        return Some(ResolvedCodexSession::explicit(
            value,
            CodexResolvedSessionSource::HeaderThreadId,
        ));
    }
    if let Some(value) = metadata.thread_id {
        return Some(ResolvedCodexSession::explicit(
            value,
            CodexResolvedSessionSource::MetadataThreadId,
        ));
    }
    if let Some(value) = header_value(headers, "conversation_id") {
        return Some(ResolvedCodexSession::explicit(
            value,
            CodexResolvedSessionSource::ConversationId,
        ));
    }
    extract_non_empty_string(root.get(PROMPT_CACHE_KEY)).map(|value| {
        ResolvedCodexSession::explicit(
            value.to_string(),
            CodexResolvedSessionSource::PromptCacheKey,
        )
    })
}

fn stable_prefix_projection(root: &Map<String, Value>) -> Option<Value> {
    let stable_input = stable_input_prefix(root.get("input")?)?;
    let mut projection = Map::new();
    copy_if_present(root, &mut projection, "instructions");
    copy_if_present(root, &mut projection, "tools");
    copy_if_present(root, &mut projection, "tool_choice");
    copy_if_present(root, &mut projection, "parallel_tool_calls");
    copy_if_present(root, &mut projection, "reasoning");
    copy_if_present(root, &mut projection, "text");
    projection.insert("input".to_string(), Value::Array(stable_input));
    Some(sanitize_for_hash(&Value::Object(projection)))
}

fn stable_input_prefix(input: &Value) -> Option<Vec<Value>> {
    let Value::Array(items) = input else {
        return None;
    };
    if items.len() <= 1 {
        return None;
    }
    let prefix = items[..items.len() - 1].to_vec();
    has_conversation_history(&prefix).then_some(prefix)
}

fn has_conversation_history(items: &[Value]) -> bool {
    items.iter().any(|item| {
        let Some(obj) = item.as_object() else {
            return false;
        };
        match obj.get("type").and_then(Value::as_str) {
            Some("function_call" | "function_call_output" | "custom_tool_call") => true,
            Some("message") | None => obj
                .get("role")
                .and_then(Value::as_str)
                .is_some_and(|role| !matches!(role, "system" | "developer")),
            _ => false,
        }
    })
}

fn bootstrap_projection(root: &Map<String, Value>) -> Value {
    sanitize_for_hash(&Value::Object(root.clone()))
}

fn copy_if_present(source: &Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    if let Some(value) = source.get(key) {
        target.insert(key.to_string(), value.clone());
    }
}

fn sanitize_for_hash(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(sanitize_for_hash).collect()),
        Value::Object(obj) => {
            let mut sanitized = Map::new();
            for (key, value) in obj {
                if HASH_IGNORED_OBJECT_KEYS.contains(&key.as_str()) {
                    continue;
                }
                sanitized.insert(key.clone(), sanitize_for_hash(value));
            }
            Value::Object(sanitized)
        },
        other => other.clone(),
    }
}

fn hash_canonical_value(value: &Value, salt: &[u8]) -> String {
    let mut canonical = String::new();
    write_canonical_value(value, &mut canonical);
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(canonical.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn write_canonical_value(value: &Value, out: &mut String) {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            out.push_str(&serde_json::to_string(value).expect("primitive JSON serializes"));
        },
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical_value(item, out);
            }
            out.push(']');
        },
        Value::Object(obj) => {
            out.push('{');
            let mut keys = obj.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                let _ = write!(out, "{}", serde_json::to_string(key).expect("key serializes"));
                out.push(':');
                write_canonical_value(
                    obj.get(*key).expect("sorted key came from this object"),
                    out,
                );
            }
            out.push('}');
        },
    }
}

fn parse_codex_turn_metadata_header(headers: &HeaderMap) -> CodexTurnMetadataHeader {
    let Some(raw) = header_value(headers, SESSION_METADATA_HEADER) else {
        return CodexTurnMetadataHeader::default();
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return CodexTurnMetadataHeader::default();
    };
    CodexTurnMetadataHeader {
        session_id: json_string_field(&value, "session_id"),
        thread_id: json_string_field(&value, "thread_id"),
    }
}

fn first_header_value(headers: &HeaderMap, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| header_value(headers, name))
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn json_string_field(value: &Value, name: &str) -> Option<String> {
    value
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn hex_prefix(hex: &str, max_chars: usize) -> String {
    hex.chars().take(max_chars).collect()
}
