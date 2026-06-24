//! Resolve a stable Codex session id from normalized request shape.
//!
//! Explicit client-provided anchors win. Derived ids are only used when the
//! request lacks a usable session/thread/conversation/prompt-cache anchor.

use std::io::{self, Write as _};

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
const STABLE_ROOT_KEYS: &[&str] =
    &["instructions", "tools", "tool_choice", "parallel_tool_calls", "reasoning", "text"];

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

    let (hash_source, source, salt) = if let Some(input) = stable_input_prefix(root.get("input")) {
        (
            SessionHashProjection::Stable {
                root,
                input,
            },
            CodexResolvedSessionSource::StablePrefix,
            STABLE_PREFIX_HASH_SALT,
        )
    } else {
        (
            SessionHashProjection::Bootstrap {
                root,
            },
            CodexResolvedSessionSource::BootstrapRequest,
            BOOTSTRAP_HASH_SALT,
        )
    };
    let hash = hash_session_projection(hash_source, salt);
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
    if let Some(value) = first_header_value(headers, &["conversation_id", "conversation-id"]) {
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

fn stable_input_prefix(input: Option<&Value>) -> Option<&[Value]> {
    let Value::Array(items) = input? else {
        return None;
    };
    let end = items.iter().position(is_conversation_seed_item)?;
    Some(&items[..=end])
}

fn is_conversation_seed_item(item: &Value) -> bool {
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
}

enum SessionHashProjection<'a> {
    Stable { root: &'a Map<String, Value>, input: &'a [Value] },
    Bootstrap { root: &'a Map<String, Value> },
}

enum CanonicalFragment<'a> {
    Value(&'a Value),
    Array(&'a [Value]),
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

fn hash_session_projection(projection: SessionHashProjection<'_>, salt: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(salt);
    match projection {
        SessionHashProjection::Stable {
            root,
            input,
        } => {
            write_stable_projection(root, input, &mut hasher);
        },
        SessionHashProjection::Bootstrap {
            root,
        } => {
            write_canonical_map(root, &mut hasher);
        },
    }
    format!("{:x}", hasher.finalize())
}

fn write_stable_projection(root: &Map<String, Value>, input: &[Value], hasher: &mut Sha256) {
    let mut entries = STABLE_ROOT_KEYS
        .iter()
        .filter_map(|key| {
            root.get(*key)
                .map(|value| (*key, CanonicalFragment::Value(value)))
        })
        .collect::<Vec<_>>();
    entries.push(("input", CanonicalFragment::Array(input)));
    entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));

    hasher.update(b"{");
    for (index, (key, value)) in entries.iter().enumerate() {
        if index > 0 {
            hasher.update(b",");
        }
        write_json_string(key, hasher);
        hasher.update(b":");
        write_canonical_fragment(value, hasher);
    }
    hasher.update(b"}");
}

fn write_canonical_fragment(value: &CanonicalFragment<'_>, hasher: &mut Sha256) {
    match value {
        CanonicalFragment::Value(value) => write_canonical_value(value, hasher),
        CanonicalFragment::Array(items) => write_canonical_array(items, hasher),
    }
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
