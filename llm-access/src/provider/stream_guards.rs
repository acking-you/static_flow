//! Codex/Kiro stream record guards and Kiro upstream stream relay.

use std::time::Instant;

use async_stream::stream;
use axum::{
    body::{Body, Bytes},
    http::{header, StatusCode},
    response::Response,
};
use futures_util::StreamExt;
use llm_access_kiro::{
    anthropic::stream::StreamContext, parser::decoder::EventStreamDecoder, wire::Event,
};

use super::{
    codex_sse::{missing_codex_usage, record_codex_usage},
    errors::{
        anthropic_json_error, anthropic_json_error_body,
        kiro_events_contain_content_length_exceeded, kiro_prompt_too_long_message,
    },
    kiro_error::kiro_json_error,
    kiro_model::decode_kiro_events_from_bytes,
    kiro_usage::{
        anthropic_usage_json_from_summary_with_policy, build_kiro_usage_summary, record_kiro_usage,
    },
    usage_meta::{capture_error_body, capture_error_message},
    CodexStreamRecordGuard, KiroPeekedStream, KiroResponseContext, KiroStreamRecordGuard,
    KiroUsageInputs, KiroUsageRecord, KiroUsageSummary, StreamRecordState,
};

impl CodexStreamRecordGuard {
    pub(super) fn observe_chunk(&mut self, bytes: &Bytes, event_type: Option<&str>) {
        self.usage_meta
            .observe_stream_write(bytes.len(), event_type);
    }

    pub(super) fn mark_internal_failure(&mut self) {
        self.state = StreamRecordState::InternalFailure;
    }

    pub(super) async fn finish_success(mut self) {
        self.usage_meta.mark_post_headers_body();
        self.usage_meta.mark_stream_completed_cleanly();
        let usage = self
            .usage_collector
            .usage
            .clone()
            .unwrap_or_else(missing_codex_usage);
        if let Err(err) = record_codex_usage(
            self.control_store.as_ref(),
            &self.key,
            &self.prepared,
            self.status,
            &self.route,
            usage,
            &self.usage_meta,
        )
        .await
        {
            tracing::warn!(
                key_id = %self.key.key_id,
                account = %self.route.account_name,
                error = %err,
                "failed to record codex stream usage"
            );
        }
        self.record_committed = true;
    }
}
impl Drop for CodexStreamRecordGuard {
    fn drop(&mut self) {
        if self.record_committed {
            return;
        }
        match self.state {
            StreamRecordState::Pending => self.usage_meta.mark_downstream_disconnect(),
            StreamRecordState::InternalFailure => self.usage_meta.mark_stream_internal_incomplete(),
        }
        let control_store = self.control_store.clone();
        let key = self.key.clone();
        let prepared = self.prepared.clone();
        let route = self.route.clone();
        let status = self.status;
        let usage = self
            .usage_collector
            .usage
            .clone()
            .unwrap_or_else(missing_codex_usage);
        let meta = self.usage_meta.clone();
        tokio::spawn(async move {
            if let Err(err) = record_codex_usage(
                control_store.as_ref(),
                &key,
                &prepared,
                status,
                &route,
                usage,
                &meta,
            )
            .await
            {
                tracing::warn!(
                    key_id = %key.key_id,
                    account = %route.account_name,
                    error = %err,
                    "failed to record incomplete codex stream usage"
                );
            }
        });
        self.record_committed = true;
    }
}
impl KiroStreamRecordGuard {
    pub(super) fn observe_chunk(&mut self, bytes: &Bytes, event_type: Option<&str>) {
        self.usage_meta
            .observe_stream_write(bytes.len(), event_type);
    }

    pub(super) fn mark_internal_failure(&mut self) {
        self.state = StreamRecordState::InternalFailure;
    }

    fn current_usage_summary(&self) -> KiroUsageSummary {
        let (_resolved_input_tokens, output_tokens) = self.stream_ctx.final_usage();
        let (credit_usage, credit_usage_missing) = self.stream_ctx.final_credit_usage();
        build_kiro_usage_summary(
            &self.model,
            KiroUsageInputs {
                request_input_tokens: self.stream_ctx.request_input_tokens(),
                context_input_tokens: self.stream_ctx.context_input_tokens(),
                context_usage_min_request_tokens: self.route.context_usage_min_request_tokens,
                output_tokens,
                credit_usage,
                credit_usage_missing,
                cache_estimation_enabled: self.route.cache_estimation_enabled,
            },
            &self.cache_ctx,
        )
    }

    pub(super) async fn finish_success(mut self, usage: KiroUsageSummary) {
        self.usage_meta.mark_stream_completed_cleanly();
        if let Err(err) = record_kiro_usage(KiroUsageRecord {
            control_store: self.control_store.as_ref(),
            key: &self.key,
            route: &self.route,
            endpoint: &self.endpoint,
            model: &self.model,
            status: self.status,
            usage,
            cache_ctx: &self.cache_ctx,
            meta: &self.usage_meta,
        })
        .await
        {
            tracing::warn!(
                key_id = %self.key.key_id,
                account = %self.route.account_name,
                error = %err,
                "failed to record kiro stream usage"
            );
        }
        self.record_committed = true;
    }
}
impl Drop for KiroStreamRecordGuard {
    fn drop(&mut self) {
        if self.record_committed {
            return;
        }
        match self.state {
            StreamRecordState::Pending => self.usage_meta.mark_downstream_disconnect(),
            StreamRecordState::InternalFailure => self.usage_meta.mark_stream_internal_incomplete(),
        }
        let control_store = self.control_store.clone();
        let key = self.key.clone();
        let route = self.route.clone();
        let endpoint = self.endpoint.clone();
        let model = self.model.clone();
        let status = self.status;
        let cache_ctx = self.cache_ctx.clone();
        let usage = self.current_usage_summary();
        let meta = self.usage_meta.clone();
        tokio::spawn(async move {
            if let Err(err) = record_kiro_usage(KiroUsageRecord {
                control_store: control_store.as_ref(),
                key: &key,
                route: &route,
                endpoint: &endpoint,
                model: &model,
                status,
                usage,
                cache_ctx: &cache_ctx,
                meta: &meta,
            })
            .await
            {
                tracing::warn!(
                    key_id = %key.key_id,
                    account = %route.account_name,
                    error = %err,
                    "failed to record incomplete kiro stream usage"
                );
            }
        });
        self.record_committed = true;
    }
}
pub fn stream_kiro_upstream_response(
    response: KiroPeekedStream,
    ctx: KiroResponseContext,
) -> Response {
    let status = response.status;
    let body_stream = stream! {
        let KiroResponseContext {
            key,
            route,
            public_path,
            model,
            request_input_tokens,
            thinking_enabled,
            hidden_thinking_enabled,
            tool_name_map,
            structured_output_tool_name,
            response_identity,
            cache_ctx,
            control_store,
            kiro_cache_simulator,
            usage_meta,
            affinity_update: _affinity_update,
            _key_permit,
            _account_permit,
        } = ctx;
        let stream_model = model.clone();
        let context_usage_min_request_tokens = route.context_usage_min_request_tokens;
        let mut guard = KiroStreamRecordGuard {
            control_store,
            key,
            route,
            endpoint: public_path,
            model,
            status,
            cache_ctx,
            usage_meta,
            stream_ctx: StreamContext::new_with_thinking_visibility(
                &stream_model,
                request_input_tokens,
                thinking_enabled,
                hidden_thinking_enabled,
                tool_name_map,
                structured_output_tool_name,
            )
            .with_context_usage_min_request_tokens(context_usage_min_request_tokens)
            .with_response_identity(response_identity),
            state: StreamRecordState::Pending,
            record_committed: false,
        };
        for event in guard.stream_ctx.generate_initial_events() {
            let bytes = Bytes::from(event.to_sse_string());
            guard.observe_chunk(&bytes, Some(event.event.as_str()));
            yield Ok::<Bytes, std::io::Error>(bytes);
        }
        let mut body_stream = futures_util::stream::once(async move { Ok(response.buffered_prefix) })
            .chain(response.remaining)
            .boxed();
        let mut decoder = EventStreamDecoder::new();
        while let Some(chunk_result) = body_stream.next().await {
            let chunk = match chunk_result {
                Ok(chunk) => chunk,
                Err(err) => {
                    guard.mark_internal_failure();
                    yield Err(std::io::Error::other(format!("failed to read kiro upstream stream: {err}")));
                    return;
                },
            };
            let _ = decoder.feed(&chunk);
            for frame in decoder.decode_iter() {
                let frame = match frame {
                    Ok(frame) => frame,
                    Err(err) => {
                        guard.mark_internal_failure();
                        yield Err(std::io::Error::other(format!("failed to decode kiro event frame: {err}")));
                        return;
                    },
                };
                let event = match Event::from_frame(frame) {
                    Ok(event) => event,
                    Err(err) => {
                        guard.mark_internal_failure();
                        yield Err(std::io::Error::other(format!("failed to parse kiro event: {err}")));
                        return;
                    },
                };
                for sse_event in guard.stream_ctx.process_kiro_event(&event) {
                    let bytes = Bytes::from(sse_event.to_sse_string());
                    guard.observe_chunk(&bytes, Some(sse_event.event.as_str()));
                    yield Ok::<Bytes, std::io::Error>(bytes);
                }
            }
        }
        guard.usage_meta.mark_post_headers_body();
        let (_resolved_input_tokens, output_tokens) = guard.stream_ctx.final_usage();
        let (credit_usage, credit_usage_missing) = guard.stream_ctx.final_credit_usage();
        let usage = build_kiro_usage_summary(
            &guard.model,
            KiroUsageInputs {
                request_input_tokens,
                context_input_tokens: guard.stream_ctx.context_input_tokens(),
                context_usage_min_request_tokens: guard.route.context_usage_min_request_tokens,
                output_tokens,
                credit_usage,
                credit_usage_missing,
                cache_estimation_enabled: guard.route.cache_estimation_enabled,
            },
            &guard.cache_ctx,
        );
        let mut final_events = guard.stream_ctx.generate_final_events();
        let anthropic_usage = anthropic_usage_json_from_summary_with_policy(usage, &guard.cache_ctx);
        for event in &mut final_events {
            if event.event == "message_delta" {
                if let Some(value) = event.data.get_mut("usage") {
                    *value = anthropic_usage.clone();
                }
            }
        }
        let assistant_message = guard.stream_ctx.final_assistant_message();
        kiro_cache_simulator.record_success_from_runtime_projection(
            &guard.cache_ctx.projection,
            &assistant_message,
            &guard.cache_ctx.conversation_id,
            guard.route.cache_estimation_enabled,
            guard.cache_ctx.simulation_config,
            Instant::now(),
        );
        for event in final_events {
            let bytes = Bytes::from(event.to_sse_string());
            guard.observe_chunk(&bytes, Some(event.event.as_str()));
            yield Ok::<Bytes, std::io::Error>(bytes);
        }
        guard.finish_success(usage).await;
    };
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(Body::from_stream(body_stream))
        .unwrap_or_else(|_| {
            kiro_json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                "failed to build stream response",
            )
        })
}
pub async fn non_stream_kiro_response(
    response: reqwest::Response,
    ctx: KiroResponseContext,
) -> Response {
    let status = response.status();
    let mut usage_meta = ctx.usage_meta.clone();
    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(_) => {
            return kiro_json_error(
                StatusCode::BAD_GATEWAY,
                "api_error",
                "failed to read kiro upstream response",
            )
        },
    };
    usage_meta.mark_post_headers_body();
    usage_meta.mark_stream_finish();
    let events = match decode_kiro_events_from_bytes(&bytes) {
        Ok(events) => events,
        Err(err) => return kiro_json_error(StatusCode::BAD_GATEWAY, "api_error", &err),
    };
    if kiro_events_contain_content_length_exceeded(&events) {
        let status = StatusCode::PAYLOAD_TOO_LARGE;
        let message = kiro_prompt_too_long_message(&ctx.model, ctx.request_input_tokens);
        let response = anthropic_json_error(status, "invalid_request_error", &message);
        capture_error_message(&mut usage_meta, &message);
        capture_error_body(
            &mut usage_meta,
            &anthropic_json_error_body("invalid_request_error", &message),
        );
        let usage = build_kiro_usage_summary(
            &ctx.model,
            KiroUsageInputs {
                request_input_tokens: ctx.request_input_tokens,
                context_input_tokens: None,
                context_usage_min_request_tokens: ctx.route.context_usage_min_request_tokens,
                output_tokens: 0,
                credit_usage: None,
                credit_usage_missing: true,
                cache_estimation_enabled: false,
            },
            &ctx.cache_ctx,
        );
        if let Err(err) = record_kiro_usage(KiroUsageRecord {
            control_store: ctx.control_store.as_ref(),
            key: &ctx.key,
            route: &ctx.route,
            endpoint: &ctx.public_path,
            model: &ctx.model,
            status,
            usage,
            cache_ctx: &ctx.cache_ctx,
            meta: &usage_meta,
        })
        .await
        {
            tracing::error!(
                error = %err,
                "Failed to record gateway usage for non-stream content length exception"
            );
            return kiro_json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                "failed to record usage",
            );
        }
        return response;
    }
    let mut stream_ctx = StreamContext::new_with_thinking_visibility(
        &ctx.model,
        ctx.request_input_tokens,
        ctx.thinking_enabled,
        ctx.hidden_thinking_enabled,
        ctx.tool_name_map,
        ctx.structured_output_tool_name.clone(),
    )
    .with_context_usage_min_request_tokens(ctx.route.context_usage_min_request_tokens)
    .with_response_identity(ctx.response_identity.clone());
    for event in &events {
        let _ = stream_ctx.process_kiro_event(event);
    }
    let _ = stream_ctx.generate_final_events();
    let (_resolved_input_tokens, output_tokens) = stream_ctx.final_usage();
    let (credit_usage, credit_usage_missing) = stream_ctx.final_credit_usage();
    let usage = build_kiro_usage_summary(
        &ctx.model,
        KiroUsageInputs {
            request_input_tokens: ctx.request_input_tokens,
            context_input_tokens: stream_ctx.context_input_tokens(),
            context_usage_min_request_tokens: ctx.route.context_usage_min_request_tokens,
            output_tokens,
            credit_usage,
            credit_usage_missing,
            cache_estimation_enabled: ctx.route.cache_estimation_enabled,
        },
        &ctx.cache_ctx,
    );
    let assistant_message = stream_ctx.final_assistant_message();
    let mut content = stream_ctx.final_content_blocks();
    if let Some(tool_uses) = assistant_message.tool_uses.clone() {
        content.extend(tool_uses.into_iter().map(|tool_use| {
            serde_json::json!({
                "type": "tool_use",
                "id": tool_use.tool_use_id,
                "name": tool_use.name,
                "input": tool_use.input,
            })
        }));
    }
    let stop_reason = stream_ctx.state_manager.get_stop_reason();
    ctx.kiro_cache_simulator
        .record_success_from_runtime_projection(
            &ctx.cache_ctx.projection,
            &assistant_message,
            &ctx.cache_ctx.conversation_id,
            ctx.route.cache_estimation_enabled,
            ctx.cache_ctx.simulation_config,
            Instant::now(),
        );
    if let Err(err) = record_kiro_usage(KiroUsageRecord {
        control_store: ctx.control_store.as_ref(),
        key: &ctx.key,
        route: &ctx.route,
        endpoint: &ctx.public_path,
        model: &ctx.model,
        status,
        usage,
        cache_ctx: &ctx.cache_ctx,
        meta: &usage_meta,
    })
    .await
    {
        tracing::error!(error = %err, "Failed to record gateway usage for non-stream response");
        return kiro_json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "api_error",
            "failed to record usage",
        );
    }
    if let Some(affinity_update) = &ctx.affinity_update {
        affinity_update.affinity.remember(
            &ctx.key.key_id,
            &affinity_update.session_id,
            &ctx.route.account_name,
        );
    }
    let body = serde_json::json!({
        "id": format!("msg_{}", uuid::Uuid::new_v4().simple()),
        "type": "message",
        "role": "assistant",
        "content": content,
        "model": ctx.model,
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": anthropic_usage_json_from_summary_with_policy(usage, &ctx.cache_ctx),
    });
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(body.to_string()))
        .unwrap_or_else(|_| {
            kiro_json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                "failed to build response",
            )
        })
}
