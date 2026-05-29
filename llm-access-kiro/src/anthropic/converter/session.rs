//! Conversation/session-id resolution from request metadata, with UUID
//! validation and generated fallbacks.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub fn preview_session_value(value: &str) -> String {
    let mut preview = value
        .chars()
        .take(SESSION_SOURCE_PREVIEW_MAX_CHARS)
        .collect::<String>();
    if value.chars().count() > SESSION_SOURCE_PREVIEW_MAX_CHARS {
        preview.push_str("...[truncated]");
    }
    preview
}
// Extracts a UUID session ID from the Anthropic `user_id` metadata field.
// Supports either a JSON payload containing `session_id` or the legacy
// `..._session_<uuid>...` string format.
#[cfg(test)]
pub(crate) fn extract_session_id(user_id: &str) -> Option<String> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(user_id) {
        if let Some(session_id) = value.get("session_id").and_then(|value| value.as_str()) {
            if is_valid_uuid(session_id) {
                return Some(session_id.to_string());
            }
        }
    }

    let pos = user_id.find("session_")?;
    let session_part = &user_id[pos + 8..];
    if session_part.len() < 36 {
        return None;
    }

    let uuid = &session_part[..36];
    is_valid_uuid(uuid).then(|| uuid.to_string())
}
pub(crate) fn is_valid_uuid(value: &str) -> bool {
    value.len() == 36 && value.chars().filter(|ch| *ch == '-').count() == 4
}
pub(crate) fn generated_fallback(
    reason: SessionFallbackReason,
    source_name: Option<&'static str>,
    source_value_preview: Option<String>,
) -> ResolvedConversationId {
    ResolvedConversationId {
        conversation_id: Uuid::new_v4().to_string(),
        session_tracking: SessionTracking {
            source: SessionIdSource::GeneratedFallback(reason),
            source_name,
            source_value_preview,
        },
    }
}
pub fn resolve_conversation_id_from_metadata(
    metadata: Option<&Metadata>,
) -> ResolvedConversationId {
    let Some(metadata) = metadata else {
        return generated_fallback(SessionFallbackReason::MissingMetadata, None, None);
    };

    let Some(user_id) = metadata.user_id.as_deref() else {
        return generated_fallback(SessionFallbackReason::MissingUserId, None, None);
    };

    let user_id_preview = Some(preview_session_value(user_id));
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(user_id) {
        if let Some(session_id) = value.get("session_id").and_then(|value| value.as_str()) {
            if is_valid_uuid(session_id) {
                return ResolvedConversationId {
                    conversation_id: session_id.to_string(),
                    session_tracking: SessionTracking {
                        source: SessionIdSource::MetadataJson,
                        source_name: None,
                        source_value_preview: user_id_preview,
                    },
                };
            }
            return generated_fallback(
                SessionFallbackReason::InvalidJsonSessionId,
                None,
                user_id_preview,
            );
        }
        return generated_fallback(
            SessionFallbackReason::MissingJsonSessionId,
            None,
            user_id_preview,
        );
    }

    let Some(pos) = user_id.find("session_") else {
        return generated_fallback(
            SessionFallbackReason::MissingLegacySessionId,
            None,
            user_id_preview,
        );
    };
    let session_part = &user_id[pos + 8..];
    if session_part.len() < 36 {
        return generated_fallback(
            SessionFallbackReason::InvalidLegacySessionId,
            None,
            user_id_preview,
        );
    }

    let uuid = &session_part[..36];
    if is_valid_uuid(uuid) {
        ResolvedConversationId {
            conversation_id: uuid.to_string(),
            session_tracking: SessionTracking {
                source: SessionIdSource::MetadataLegacy,
                source_name: None,
                source_value_preview: user_id_preview,
            },
        }
    } else {
        generated_fallback(SessionFallbackReason::InvalidLegacySessionId, None, user_id_preview)
    }
}
