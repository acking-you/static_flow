//! Codex upstream request header assembly: header extraction, turn-metadata
//! parsing, and upstream session header resolution.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}
pub(crate) fn first_header_value(headers: &HeaderMap, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| header_value(headers, name))
}
pub(crate) fn parse_codex_turn_metadata_header(headers: &HeaderMap) -> CodexTurnMetadataHeader {
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
pub(crate) fn json_string_field(value: &Value, name: &str) -> Option<String> {
    value
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}
pub(crate) fn is_standard_codex_responses_path(prepared: &PreparedGatewayRequest) -> bool {
    prepared
        .upstream_path
        .split('?')
        .next()
        .is_some_and(|path| path == "/v1/responses")
}
pub(crate) fn resolve_codex_upstream_session_headers(
    request_headers: &HeaderMap,
    prepared: &PreparedGatewayRequest,
) -> CodexUpstreamSessionHeaders {
    let metadata = parse_codex_turn_metadata_header(request_headers);
    let thread_anchor = prepared
        .thread_anchor
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let should_reconstruct = is_standard_codex_responses_path(prepared);
    let session_id =
        first_header_value(request_headers, &["session_id", "session-id"]).or_else(|| {
            if should_reconstruct {
                metadata
                    .session_id
                    .clone()
                    .or_else(|| thread_anchor.map(ToString::to_string))
            } else {
                None
            }
        });
    let thread_id =
        first_header_value(request_headers, &["thread_id", "thread-id"]).or_else(|| {
            if should_reconstruct {
                metadata
                    .thread_id
                    .clone()
                    .or_else(|| thread_anchor.map(ToString::to_string))
                    .or_else(|| session_id.clone())
            } else {
                None
            }
        });
    let conversation_id = header_value(request_headers, "conversation_id").or_else(|| {
        if should_reconstruct {
            thread_anchor
                .map(ToString::to_string)
                .or_else(|| metadata.thread_id.clone())
        } else {
            None
        }
    });
    let client_request_id = header_value(request_headers, "x-client-request-id").or_else(|| {
        if should_reconstruct {
            thread_id
                .clone()
                .or_else(|| thread_anchor.map(ToString::to_string))
        } else {
            None
        }
    });

    CodexUpstreamSessionHeaders {
        conversation_id,
        session_id,
        thread_id,
        client_request_id,
    }
}
pub(crate) fn add_codex_upstream_headers(
    mut upstream: reqwest::RequestBuilder,
    request_headers: &HeaderMap,
    prepared: &PreparedGatewayRequest,
    auth: &CodexAuthSnapshot,
    codex_client_version: &str,
) -> reqwest::RequestBuilder {
    let session_headers = resolve_codex_upstream_session_headers(request_headers, prepared);
    let incoming_turn_state = header_value(request_headers, "x-codex-turn-state");

    upstream = upstream
        .bearer_auth(&auth.access_token)
        .header(
            reqwest::header::ACCEPT,
            if prepared.wants_stream || prepared.force_upstream_stream {
                "text/event-stream"
            } else {
                "application/json"
            },
        )
        .header(
            reqwest::header::USER_AGENT,
            header_value(request_headers, header::USER_AGENT.as_str())
                .unwrap_or_else(|| codex_user_agent(codex_client_version)),
        )
        .header(
            reqwest::header::HeaderName::from_static("originator"),
            header_value(request_headers, "originator")
                .unwrap_or_else(|| DEFAULT_WIRE_ORIGINATOR.to_string()),
        );
    if !prepared.request_body.is_empty() {
        upstream = upstream
            .header(reqwest::header::CONTENT_TYPE, prepared.content_type.as_str())
            .body(prepared.request_body.clone());
    }
    if let Some(conversation_id) = session_headers.conversation_id.as_deref() {
        upstream = upstream.header("conversation_id", conversation_id);
    }
    if let Some(client_request_id) = session_headers.client_request_id.as_deref() {
        upstream = upstream.header("x-client-request-id", client_request_id);
    }
    if let Some(turn_state) = incoming_turn_state.as_deref() {
        upstream = upstream.header("x-codex-turn-state", turn_state);
    }
    for header_name in [
        "openai-beta",
        "x-openai-subagent",
        "x-codex-beta-features",
        "x-codex-turn-metadata",
        "x-codex-installation-id",
        "x-codex-parent-thread-id",
        "x-codex-window-id",
        "x-openai-memgen-request",
        "x-responsesapi-include-timing-metrics",
        "traceparent",
        "tracestate",
        "baggage",
    ] {
        if let Some(value) = header_value(request_headers, header_name) {
            upstream = upstream.header(header_name, value);
        }
    }
    if let Some(session_id) = session_headers.session_id.as_deref() {
        upstream = upstream
            .header("session_id", session_id)
            .header("session-id", session_id);
    }
    if let Some(thread_id) = session_headers.thread_id.as_deref() {
        upstream = upstream
            .header("thread_id", thread_id)
            .header("thread-id", thread_id);
    }
    if let Some(account_id) = auth.account_id.as_deref() {
        upstream = upstream.header("chatgpt-account-id", account_id);
    }
    if auth.is_fedramp_account {
        upstream = upstream.header("x-openai-fedramp", "true");
    }
    upstream
}
