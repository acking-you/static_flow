//! Conversation/session-id resolution from request metadata, with UUID
//! validation and generated fallbacks.

use uuid::Uuid;

use super::{
    ResolvedConversationId, SessionFallbackReason, SessionIdSource, SessionTracking,
    SESSION_SOURCE_PREVIEW_MAX_CHARS,
};
use crate::anthropic::types::Metadata;

pub fn preview_session_value(value: &str) -> String {
    let mut chars = value.chars();
    let mut preview = chars
        .by_ref()
        .take(SESSION_SOURCE_PREVIEW_MAX_CHARS)
        .collect::<String>();
    if chars.next().is_some() {
        preview.push_str("...[truncated]");
    }
    preview
}

// Extracts a UUID session ID from the Anthropic `user_id` metadata field.
// Supports either a JSON payload containing `session_id` or the legacy
// `..._session_<uuid>...` string format.
#[cfg(test)]
fn extract_session_id(user_id: &str) -> Option<String> {
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

pub fn is_valid_uuid(value: &str) -> bool {
    value.len() == 36 && value.chars().filter(|ch| *ch == '-').count() == 4
}

fn generated_fallback(
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


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_session_id_handles_valid_and_invalid_values() {
        assert_eq!(
            extract_session_id("user_x_account__session_8bb5523b-ec7c-4540-a9ca-beb6d79f1552"),
            Some("8bb5523b-ec7c-4540-a9ca-beb6d79f1552".to_string())
        );
        assert_eq!(
            extract_session_id(
                r#"{"device_id":"dev","account_uuid":"acct","session_id":"a0662283-7fd3-4399-a7eb-52b9a717ae88"}"#
            ),
            Some("a0662283-7fd3-4399-a7eb-52b9a717ae88".to_string())
        );
        assert_eq!(extract_session_id(r#"{"session_id":"invalid-uuid"}"#), None);
        assert_eq!(extract_session_id("user_without_session"), None);
        assert_eq!(extract_session_id("user_x__session_invalid-uuid"), None);
    }
}
