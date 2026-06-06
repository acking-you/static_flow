//! In-process Codex account affinity keyed by explicit session ids or a
//! request-body prefix hash when upstream clients provide no session id.

use std::{
    num::NonZeroUsize,
    sync::Mutex,
    time::{Duration, Instant},
};

use axum::http::HeaderMap;
use llm_access_core::store::{
    AdminRuntimeConfig, DEFAULT_CODEX_FALLBACK_AFFINITY_ENABLED,
    DEFAULT_CODEX_FALLBACK_AFFINITY_MIN_BODY_BYTES, DEFAULT_CODEX_FALLBACK_AFFINITY_PREFIX_BYTES,
    DEFAULT_CODEX_FALLBACK_AFFINITY_TTL_SECONDS, DEFAULT_CODEX_SESSION_AFFINITY_ENABLED,
    DEFAULT_CODEX_SESSION_AFFINITY_MAX_ENTRIES, DEFAULT_CODEX_SESSION_AFFINITY_TTL_SECONDS,
};
use lru::LruCache;
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodexAffinityRuntimeConfig {
    pub session_enabled: bool,
    pub max_entries: usize,
    pub session_ttl: Duration,
    pub fallback_enabled: bool,
    pub fallback_ttl: Duration,
    pub fallback_prefix_bytes: usize,
    pub fallback_min_body_bytes: usize,
}

impl Default for CodexAffinityRuntimeConfig {
    fn default() -> Self {
        Self {
            session_enabled: DEFAULT_CODEX_SESSION_AFFINITY_ENABLED,
            max_entries: DEFAULT_CODEX_SESSION_AFFINITY_MAX_ENTRIES as usize,
            session_ttl: Duration::from_secs(DEFAULT_CODEX_SESSION_AFFINITY_TTL_SECONDS),
            fallback_enabled: DEFAULT_CODEX_FALLBACK_AFFINITY_ENABLED,
            fallback_ttl: Duration::from_secs(DEFAULT_CODEX_FALLBACK_AFFINITY_TTL_SECONDS),
            fallback_prefix_bytes: DEFAULT_CODEX_FALLBACK_AFFINITY_PREFIX_BYTES as usize,
            fallback_min_body_bytes: DEFAULT_CODEX_FALLBACK_AFFINITY_MIN_BODY_BYTES as usize,
        }
    }
}

impl CodexAffinityRuntimeConfig {
    pub(crate) fn from_admin_config(config: &AdminRuntimeConfig) -> Self {
        Self {
            session_enabled: config.codex_session_affinity_enabled,
            max_entries: usize::try_from(config.codex_session_affinity_max_entries)
                .unwrap_or(usize::MAX),
            session_ttl: Duration::from_secs(config.codex_session_affinity_ttl_seconds),
            fallback_enabled: config.codex_fallback_affinity_enabled,
            fallback_ttl: Duration::from_secs(config.codex_fallback_affinity_ttl_seconds),
            fallback_prefix_bytes: usize::try_from(config.codex_fallback_affinity_prefix_bytes)
                .unwrap_or(usize::MAX),
            fallback_min_body_bytes: usize::try_from(config.codex_fallback_affinity_min_body_bytes)
                .unwrap_or(usize::MAX),
        }
    }

    fn ttl_for_source(&self, source: CodexAffinitySource) -> Duration {
        match source {
            CodexAffinitySource::Explicit => self.session_ttl,
            CodexAffinitySource::FallbackBodyPrefix => self.fallback_ttl,
        }
    }

    fn can_store_source(&self, source: CodexAffinitySource) -> bool {
        self.session_enabled && self.max_entries > 0 && !self.ttl_for_source(source).is_zero()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodexAffinitySource {
    Explicit,
    FallbackBodyPrefix,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodexAffinityId {
    pub key: String,
    pub source: CodexAffinitySource,
}

#[derive(Debug, Clone)]
struct CodexAffinityEntry {
    account_name: String,
    source: CodexAffinitySource,
    last_seen: Instant,
}

struct CodexSessionAffinityInner {
    capacity: usize,
    entries: LruCache<String, CodexAffinityEntry>,
}

pub(crate) struct CodexSessionAffinity {
    inner: Mutex<CodexSessionAffinityInner>,
}

impl Default for CodexSessionAffinity {
    fn default() -> Self {
        Self::new(DEFAULT_CODEX_SESSION_AFFINITY_MAX_ENTRIES as usize)
    }
}

impl CodexSessionAffinity {
    fn new(max_entries: usize) -> Self {
        let capacity = max_entries.max(1);
        Self {
            inner: Mutex::new(CodexSessionAffinityInner {
                capacity,
                entries: LruCache::new(NonZeroUsize::new(capacity).expect("capacity is non-zero")),
            }),
        }
    }

    pub(crate) fn lookup(
        &self,
        affinity_id: &CodexAffinityId,
        config: &CodexAffinityRuntimeConfig,
    ) -> Option<String> {
        if !config.can_store_source(affinity_id.source) {
            return None;
        }
        let mut inner = self.reconfigure(config);
        let entry = inner.entries.get(&affinity_id.key)?;
        if entry.source != affinity_id.source
            || entry.last_seen.elapsed() > config.ttl_for_source(entry.source)
        {
            inner.entries.pop(&affinity_id.key);
            return None;
        }
        Some(entry.account_name.clone())
    }

    pub(crate) fn remember(
        &self,
        affinity_id: &CodexAffinityId,
        account_name: &str,
        config: &CodexAffinityRuntimeConfig,
    ) {
        if !config.can_store_source(affinity_id.source) {
            return;
        }
        let mut inner = self.reconfigure(config);
        inner
            .entries
            .put(affinity_id.key.clone(), CodexAffinityEntry {
                account_name: account_name.to_string(),
                source: affinity_id.source,
                last_seen: Instant::now(),
            });
    }

    fn reconfigure(
        &self,
        config: &CodexAffinityRuntimeConfig,
    ) -> std::sync::MutexGuard<'_, CodexSessionAffinityInner> {
        let capacity = config.max_entries.max(1);
        let mut inner = self.inner.lock().expect("codex session affinity mutex");
        if inner.capacity != capacity {
            inner.capacity = capacity;
            inner.entries =
                LruCache::new(NonZeroUsize::new(capacity).expect("capacity is non-zero"));
        }
        inner
    }
}

pub(crate) fn build_codex_affinity_id(
    key_id: &str,
    gateway_path: &str,
    request_headers: &HeaderMap,
    thread_anchor: Option<&str>,
    body: &[u8],
    config: &CodexAffinityRuntimeConfig,
) -> Option<CodexAffinityId> {
    if !config.session_enabled || config.max_entries == 0 {
        return None;
    }
    if let Some(value) = explicit_affinity_value(request_headers, thread_anchor) {
        return Some(CodexAffinityId {
            key: format!("{key_id}:explicit:{value}"),
            source: CodexAffinitySource::Explicit,
        });
    }
    if !config.fallback_enabled
        || config.fallback_prefix_bytes == 0
        || body.len() < config.fallback_min_body_bytes
    {
        return None;
    }
    let prefix_len = body.len().min(config.fallback_prefix_bytes);
    if prefix_len == 0 {
        return None;
    }
    let mut hasher = Sha256::new();
    hasher.update(b"codex-fallback-affinity-v1\0");
    hasher.update(gateway_path.as_bytes());
    hasher.update(b"\0");
    hasher.update(&body[..prefix_len]);
    let digest = hasher.finalize();
    Some(CodexAffinityId {
        key: format!("{key_id}:fallback-body-prefix:{}", hex_prefix(&digest, 32)),
        source: CodexAffinitySource::FallbackBodyPrefix,
    })
}

fn explicit_affinity_value(
    request_headers: &HeaderMap,
    thread_anchor: Option<&str>,
) -> Option<String> {
    let metadata = parse_codex_turn_metadata_header(request_headers);
    first_header_value(request_headers, &["session_id", "session-id"])
        .or(metadata.session_id)
        .or_else(|| first_header_value(request_headers, &["thread_id", "thread-id"]))
        .or(metadata.thread_id)
        .or_else(|| header_value(request_headers, "conversation_id"))
        .or_else(|| {
            thread_anchor
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
        })
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

#[derive(Default)]
struct CodexTurnMetadataHeader {
    session_id: Option<String>,
    thread_id: Option<String>,
}

fn parse_codex_turn_metadata_header(headers: &HeaderMap) -> CodexTurnMetadataHeader {
    let Some(raw) = header_value(headers, "x-codex-turn-metadata") else {
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

fn json_string_field(value: &Value, name: &str) -> Option<String> {
    value
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn hex_prefix(bytes: &[u8], max_chars: usize) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len().saturating_mul(2).min(max_chars));
    for byte in bytes {
        if output.len() >= max_chars {
            break;
        }
        output.push(HEX[(byte >> 4) as usize] as char);
        if output.len() >= max_chars {
            break;
        }
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}
