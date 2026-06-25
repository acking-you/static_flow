//! Resolve a stable Codex session id from normalized request shape.
//!
//! Explicit client-provided anchors win. Derived ids are only used when the
//! request lacks a usable session/thread/conversation/prompt-cache anchor.

use std::io::{self, Write as _};

use axum::http::HeaderMap;
use bytes::Bytes;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::extract_non_empty_string;
use crate::{
    error::{internal_error, CodexGatewayResult},
    types::{CodexResolvedSessionSource, CodexSessionProjection, PreparedGatewayRequest},
};

const SESSION_ID_PREFIX: &str = "codex-session-v1-";
const LOOKUP_ANCHOR_HASH_SALT: &[u8] = b"codex-lookup-anchor-v1\0";
const REQUEST_ANCHOR_HASH_SALT: &[u8] = b"codex-request-anchor-v1\0";
const BOOTSTRAP_HASH_SALT: &[u8] = b"codex-bootstrap-anchor-v1\0";
const SEGMENT_HASH_SALT: &[u8] = b"codex-session-segment-v1\0";
const HASH_ID_HEX_LEN: usize = 32;
const HASH_PREVIEW_HEX_LEN: usize = 12;
const SESSION_METADATA_HEADER: &str = "x-codex-turn-metadata";
const PROMPT_CACHE_KEY: &str = "prompt_cache_key";
const STABLE_ROOT_KEYS: &[&str] =
    &["instructions", "tools", "tool_choice", "parallel_tool_calls", "reasoning", "text"];

const HASH_IGNORED_OBJECT_KEYS: &[&str] = &[
    "id",
    "annotations",
    "model",
    "stream",
    "store",
    "include",
    "logprobs",
    "metadata",
    "client_metadata",
    "previous_response_id",
    "service_tier",
    "prompt_cache_key",
    "status",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCodexSession {
    pub id: String,
    pub source: CodexResolvedSessionSource,
    pub hash_preview: Option<String>,
    pub projection: Option<CodexSessionProjection>,
}

impl ResolvedCodexSession {
    fn explicit(
        id: String,
        source: CodexResolvedSessionSource,
        projection: Option<CodexSessionProjection>,
    ) -> Self {
        Self {
            id,
            source,
            hash_preview: None,
            projection,
        }
    }

    fn derived(
        hash: String,
        source: CodexResolvedSessionSource,
        projection: CodexSessionProjection,
    ) -> Self {
        let id_hex = hex_prefix(&hash, HASH_ID_HEX_LEN);
        let preview = hex_prefix(&hash, HASH_PREVIEW_HEX_LEN);
        Self {
            id: format!("{SESSION_ID_PREFIX}{id_hex}"),
            source,
            hash_preview: Some(preview),
            projection: Some(projection),
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
    let projection = build_session_projection(root);
    if let Some(explicit) = explicit_session(headers, root, projection.clone()) {
        return Some(explicit);
    }

    let source = if has_input_history(root.get("input")) {
        CodexResolvedSessionSource::StablePrefix
    } else {
        CodexResolvedSessionSource::BootstrapRequest
    };
    let session =
        ResolvedCodexSession::derived(projection.bootstrap_anchor_hash.clone(), source, projection);
    Some(session)
}

fn explicit_session(
    headers: &HeaderMap,
    root: &Map<String, Value>,
    projection: CodexSessionProjection,
) -> Option<ResolvedCodexSession> {
    if let Some(value) = first_header_value(headers, &["session_id", "session-id"]) {
        return Some(ResolvedCodexSession::explicit(
            value,
            CodexResolvedSessionSource::HeaderSessionId,
            Some(projection),
        ));
    }
    let metadata = parse_codex_turn_metadata_header(headers);
    if let Some(value) = metadata.session_id {
        return Some(ResolvedCodexSession::explicit(
            value,
            CodexResolvedSessionSource::MetadataSessionId,
            Some(projection),
        ));
    }
    if let Some(value) = first_header_value(headers, &["thread_id", "thread-id"]) {
        return Some(ResolvedCodexSession::explicit(
            value,
            CodexResolvedSessionSource::HeaderThreadId,
            Some(projection),
        ));
    }
    if let Some(value) = metadata.thread_id {
        return Some(ResolvedCodexSession::explicit(
            value,
            CodexResolvedSessionSource::MetadataThreadId,
            Some(projection),
        ));
    }
    if let Some(value) = first_header_value(headers, &["conversation_id", "conversation-id"]) {
        return Some(ResolvedCodexSession::explicit(
            value,
            CodexResolvedSessionSource::ConversationId,
            Some(projection),
        ));
    }
    extract_non_empty_string(root.get(PROMPT_CACHE_KEY))
        .filter(|value| !value.starts_with(SESSION_ID_PREFIX))
        .map(|value| {
            ResolvedCodexSession::explicit(
                value.to_string(),
                CodexResolvedSessionSource::PromptCacheKey,
                Some(projection),
            )
        })
}

fn has_input_history(input: Option<&Value>) -> bool {
    input
        .and_then(Value::as_array)
        .is_some_and(|items| !items.is_empty())
}

fn is_current_turn_start_item(item: &Value) -> bool {
    let Some(obj) = item.as_object() else {
        return false;
    };
    match obj.get("type").and_then(Value::as_str) {
        Some("function_call_output" | "custom_tool_call_output") => true,
        Some("message") | None => obj
            .get("role")
            .and_then(Value::as_str)
            .is_some_and(|role| role == "user"),
        _ => false,
    }
}

struct HashWriter<'a>(&'a mut Sha256);

impl io::Write for HashWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.update(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Build the anchor hash that a follow-up request should use as its lookup hash
/// after this request succeeds with `completed_response`.
pub fn build_codex_session_resume_anchor_hash(
    projection: &CodexSessionProjection,
    completed_response: &Value,
) -> String {
    let mut segments = projection.request_anchor_segments.clone();
    segments.extend(canonical_response_output_segments(completed_response));
    hash_segment_hashes(LOOKUP_ANCHOR_HASH_SALT, segments.iter().map(String::as_str))
}

/// Replace the prepared upstream request's locally resolved session id.
///
/// Used after provider-side recovery finds an older synthetic session for this
/// request's lookup anchor.
pub fn apply_codex_resolved_session(
    prepared: &mut PreparedGatewayRequest,
    session_id: String,
    source: CodexResolvedSessionSource,
    hash_preview: Option<String>,
) -> CodexGatewayResult<()> {
    debug_assert!(
        prepared
            .thread_anchor
            .as_deref()
            .is_none_or(|anchor| anchor == session_id),
        "derived recovery should not overwrite an unrelated explicit thread anchor"
    );
    prepared.thread_anchor = Some(session_id.clone());
    prepared.resolved_session_id = Some(session_id);
    prepared.resolved_session_source = Some(source);
    prepared.resolved_session_hash_preview = hash_preview;
    Ok(())
}

/// Insert the final resolved Codex session id into the upstream JSON body.
pub fn inject_codex_resolved_session_into_request_body(
    prepared: &PreparedGatewayRequest,
) -> CodexGatewayResult<PreparedGatewayRequest> {
    if prepared.upstream_path.starts_with("/v1/responses/compact") {
        return Ok(prepared.clone());
    }
    let Some(session_id) = prepared.resolved_session_id.as_deref() else {
        return Ok(prepared.clone());
    };
    if prepared.request_body.is_empty() {
        return Ok(prepared.clone());
    }
    let mut body = serde_json::from_slice::<Value>(&prepared.request_body)
        .map_err(|err| internal_error("Failed to decode prepared Codex request body", err))?;
    if let Some(root) = body.as_object_mut() {
        if let Some(existing) = extract_non_empty_string(root.get(PROMPT_CACHE_KEY)) {
            if existing == session_id || !existing.starts_with(SESSION_ID_PREFIX) {
                return Ok(prepared.clone());
            }
        }
        root.insert(PROMPT_CACHE_KEY.to_string(), Value::String(session_id.to_string()));
    } else {
        return Ok(prepared.clone());
    }
    let mut injected = prepared.clone();
    injected.request_body = Bytes::from(
        serde_json::to_vec(&body)
            .map_err(|err| internal_error("Failed to encode prepared Codex request body", err))?,
    );
    Ok(injected)
}

fn build_session_projection(root: &Map<String, Value>) -> CodexSessionProjection {
    let mut stable_segments = stable_root_segments(root);
    let input_segments = root
        .get("input")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(canonical_session_item_segment_hash)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let current_start = root
        .get("input")
        .and_then(Value::as_array)
        .map(|items| current_turn_start(items))
        .unwrap_or(0);

    let mut lookup_segments = stable_segments.clone();
    lookup_segments.extend(input_segments.iter().take(current_start).cloned());

    stable_segments.extend(input_segments);
    let request_anchor_segments = stable_segments;
    let lookup_anchor_hash =
        hash_segment_hashes(LOOKUP_ANCHOR_HASH_SALT, lookup_segments.iter().map(String::as_str));
    let request_anchor_hash = hash_segment_hashes(
        REQUEST_ANCHOR_HASH_SALT,
        request_anchor_segments.iter().map(String::as_str),
    );
    let bootstrap_anchor_hash = hash_segment_hashes(
        BOOTSTRAP_HASH_SALT,
        request_anchor_segments.iter().map(String::as_str),
    );

    CodexSessionProjection {
        lookup_anchor_hash,
        bootstrap_anchor_hash,
        request_anchor_hash,
        request_anchor_segments,
    }
}

fn stable_root_segments(root: &Map<String, Value>) -> Vec<String> {
    let mut entries = STABLE_ROOT_KEYS
        .iter()
        .filter_map(|key| root.get(*key).map(|value| (*key, value)))
        .collect::<Vec<_>>();
    entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));

    entries
        .into_iter()
        .map(|(key, value)| named_fragment_hash(key, value))
        .collect()
}

fn current_turn_start(items: &[Value]) -> usize {
    items
        .iter()
        .rposition(is_current_turn_start_item)
        .unwrap_or(items.len())
}

fn canonical_response_output_segments(completed_response: &Value) -> Vec<String> {
    completed_response
        .get("output")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(canonical_response_output_segment_hash)
                .collect()
        })
        .unwrap_or_default()
}

fn canonical_response_output_segment_hash(value: &Value) -> Option<String> {
    let Some(obj) = value.as_object() else {
        return Some(canonical_segment_hash(value));
    };
    if obj.get("type").and_then(Value::as_str) == Some("reasoning") {
        return None;
    }
    if obj.get("type").and_then(Value::as_str) != Some("message") || obj.contains_key("role") {
        return Some(canonical_segment_hash(value));
    }
    // Upstream response output messages often omit the assistant role while
    // the next request history includes it.
    let mut normalized = obj.clone();
    normalized.insert("role".to_string(), Value::String("assistant".to_string()));
    canonical_session_item_segment_hash(&Value::Object(normalized))
}

fn canonical_session_item_segment_hash(value: &Value) -> Option<String> {
    let Some(obj) = value.as_object() else {
        return Some(canonical_segment_hash(value));
    };
    if obj.get("type").and_then(Value::as_str) == Some("reasoning") {
        return None;
    }
    let Some(normalized) = normalize_session_message_item(obj) else {
        return Some(canonical_segment_hash(value));
    };
    Some(canonical_segment_hash(&normalized))
}

fn normalize_session_message_item(obj: &Map<String, Value>) -> Option<Value> {
    if obj.get("type").and_then(Value::as_str) != Some("message") {
        return None;
    }
    let role = obj
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("assistant");
    if role != "assistant" {
        return None;
    }
    let mut normalized = obj.clone();
    normalized.insert("role".to_string(), Value::String("assistant".to_string()));
    if let Some(content) = normalized.get("content").cloned() {
        normalized.insert("content".to_string(), normalize_assistant_content(&content));
    }
    Some(Value::Object(normalized))
}

fn normalize_assistant_content(content: &Value) -> Value {
    let items = match content {
        Value::Array(items) => items.as_slice(),
        value => std::slice::from_ref(value),
    };
    let normalized = items
        .iter()
        .filter_map(normalize_assistant_content_item)
        .collect::<Vec<_>>();
    Value::Array(normalized)
}

fn normalize_assistant_content_item(item: &Value) -> Option<Value> {
    if let Some(text) = item.as_str() {
        return normalized_output_text(text);
    }
    let obj = item.as_object()?;
    match obj.get("type").and_then(Value::as_str).unwrap_or_default() {
        "text" | "input_text" | "output_text" => obj
            .get("text")
            .and_then(Value::as_str)
            .and_then(normalized_output_text),
        _ => Some(Value::Object(obj.clone())),
    }
}

fn normalized_output_text(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut normalized = Map::new();
    normalized.insert("type".to_string(), Value::String("output_text".to_string()));
    normalized.insert("text".to_string(), Value::String(trimmed.to_string()));
    Some(Value::Object(normalized))
}

fn named_fragment_hash(key: &str, value: &Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(SEGMENT_HASH_SALT);
    hasher.update(b"{");
    write_json_string(key, &mut hasher);
    hasher.update(b":");
    write_canonical_value(value, &mut hasher);
    hasher.update(b"}");
    format!("{:x}", hasher.finalize())
}

fn canonical_segment_hash(value: &Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(SEGMENT_HASH_SALT);
    write_canonical_value(value, &mut hasher);
    format!("{:x}", hasher.finalize())
}

fn hash_segment_hashes<'a>(salt: &[u8], segments: impl IntoIterator<Item = &'a str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(b"[");
    for (index, segment) in segments.into_iter().enumerate() {
        if index > 0 {
            hasher.update(b",");
        }
        hasher.update(segment.as_bytes());
    }
    hasher.update(b"]");
    format!("{:x}", hasher.finalize())
}

fn write_canonical_value(value: &Value, hasher: &mut Sha256) {
    match value {
        Value::Null => hasher.update(b"null"),
        Value::Bool(true) => hasher.update(b"true"),
        Value::Bool(false) => hasher.update(b"false"),
        Value::Number(number) => {
            let mut writer = HashWriter(hasher);
            write!(&mut writer, "{number}").expect("hash writer cannot fail");
        },
        Value::String(value) => write_json_string(value, hasher),
        Value::Array(items) => write_canonical_array(items, hasher),
        Value::Object(obj) => write_canonical_map(obj, hasher),
    }
}

fn write_canonical_array(items: &[Value], hasher: &mut Sha256) {
    hasher.update(b"[");
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            hasher.update(b",");
        }
        write_canonical_value(item, hasher);
    }
    hasher.update(b"]");
}

fn write_canonical_map(obj: &Map<String, Value>, hasher: &mut Sha256) {
    let mut keys = obj
        .keys()
        .filter(|key| !HASH_IGNORED_OBJECT_KEYS.contains(&key.as_str()))
        .collect::<Vec<_>>();
    keys.sort_unstable();

    hasher.update(b"{");
    for (index, key) in keys.iter().enumerate() {
        if index > 0 {
            hasher.update(b",");
        }
        write_json_string(key, hasher);
        hasher.update(b":");
        write_canonical_value(obj.get(*key).expect("sorted key came from this object"), hasher);
    }
    hasher.update(b"}");
}

fn write_json_string(value: &str, hasher: &mut Sha256) {
    serde_json::to_writer(HashWriter(hasher), value).expect("string serializes");
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
