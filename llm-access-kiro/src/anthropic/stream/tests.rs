use super::*;
use crate::{
    parser::{
        frame::Frame,
        header::{HeaderValue, Headers},
    },
    wire::{ContextUsageEvent, Event, MeteringEvent, ToolUseEvent},
};

#[test]
fn resolve_input_tokens_prefers_request_estimate_for_small_requests() {
    let (input_tokens, source) = resolve_input_tokens(18, Some(4_118));

    assert_eq!(input_tokens, 18);
    assert_eq!(source, KiroInputTokenSource::LocalRequestEstimateFallback);
}

#[test]
fn resolve_input_tokens_prefers_request_estimate_for_inflated_small_context_usage() {
    let (input_tokens, source) = resolve_input_tokens(148, Some(8_008));

    assert_eq!(input_tokens, 148);
    assert_eq!(source, KiroInputTokenSource::LocalRequestEstimateFallback);
}

#[test]
fn resolve_input_tokens_prefers_request_estimate_for_small_request_when_context_exceeds_local() {
    let (input_tokens, source) = resolve_input_tokens(1_000, Some(6_000));

    assert_eq!(input_tokens, 1_000);
    assert_eq!(source, KiroInputTokenSource::LocalRequestEstimateFallback);
}

#[test]
fn resolve_input_tokens_uses_context_usage_above_default_threshold() {
    let (input_tokens, source) = resolve_input_tokens(16_000, Some(20_000));

    assert_eq!(input_tokens, 20_000);
    assert_eq!(source, KiroInputTokenSource::UpstreamContextUsage);
}

#[test]
fn resolve_input_tokens_respects_configured_threshold() {
    let (input_tokens, source) = resolve_input_tokens_with_threshold(16_000, Some(20_000), 50_000);

    assert_eq!(input_tokens, 16_000);
    assert_eq!(source, KiroInputTokenSource::LocalRequestEstimateFallback);
}

#[test]
fn resolve_input_tokens_keeps_upstream_context_for_large_requests() {
    let (input_tokens, source) = resolve_input_tokens(60_000, Some(90_000));

    assert_eq!(input_tokens, 90_000);
    assert_eq!(source, KiroInputTokenSource::UpstreamContextUsage);
}

#[test]
fn resolve_input_tokens_falls_back_to_local_request_without_context_usage() {
    let (input_tokens, source) = resolve_input_tokens(123, None);

    assert_eq!(input_tokens, 123);
    assert_eq!(source, KiroInputTokenSource::LocalRequestEstimateFallback);
}

fn collect_delta_text(events: &[SseEvent], delta_type: &str, field: &str) -> String {
    events
        .iter()
        .filter(|event| {
            event.event == "content_block_delta" && event.data["delta"]["type"] == delta_type
        })
        .map(|event| event.data["delta"][field].as_str().unwrap_or(""))
        .filter(|text| !text.is_empty())
        .collect()
}

fn parse_kiro_event(event_type: &str, payload: serde_json::Value) -> Event {
    let mut headers = Headers::new();
    headers.insert(":message-type".to_string(), HeaderValue::String("event".to_string()));
    headers.insert(":event-type".to_string(), HeaderValue::String(event_type.to_string()));
    Event::from_frame(Frame {
        headers,
        payload: serde_json::to_vec(&payload).expect("payload json"),
    })
    .expect("event should parse")
}

fn read_proto_varint(buf: &[u8], offset: &mut usize) -> u64 {
    let mut shift = 0;
    let mut value = 0u64;
    loop {
        let byte = *buf
            .get(*offset)
            .expect("protobuf varint should be in bounds");
        *offset += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
        shift += 7;
    }
}

type ProtoVarintFields = HashMap<u32, Vec<u64>>;
type ProtoBytesFields = HashMap<u32, Vec<Vec<u8>>>;

fn parse_proto_fields(buf: &[u8]) -> (ProtoVarintFields, ProtoBytesFields) {
    let mut varints = ProtoVarintFields::new();
    let mut bytes = ProtoBytesFields::new();
    let mut offset = 0usize;
    while offset < buf.len() {
        let key = read_proto_varint(buf, &mut offset);
        let field_number = (key >> 3) as u32;
        let wire_type = (key & 0x07) as u8;
        match wire_type {
            0 => {
                let value = read_proto_varint(buf, &mut offset);
                varints.entry(field_number).or_default().push(value);
            },
            2 => {
                let len = read_proto_varint(buf, &mut offset) as usize;
                let end = offset + len;
                let value = buf
                    .get(offset..end)
                    .expect("protobuf length-delimited field should be in bounds")
                    .to_vec();
                offset = end;
                bytes.entry(field_number).or_default().push(value);
            },
            other => panic!("unexpected protobuf wire type {other}"),
        }
    }
    (varints, bytes)
}

fn assert_claude_shaped_signature(signature: &str, expected_model: &str) {
    assert!(signature.len() >= 900);

    let decoded = STANDARD
        .decode(signature.as_bytes())
        .expect("signature should be valid base64");
    let (outer_varints, outer_bytes) = parse_proto_fields(&decoded);
    assert_eq!(outer_varints.get(&3).map(Vec::as_slice), Some(&[1][..]));

    let outer_payloads = outer_bytes
        .get(&2)
        .expect("signature envelope should contain a field-2 payload");
    assert_eq!(outer_payloads.len(), 1);

    let payload = &outer_payloads[0];
    let (inner_varints, inner_bytes) = parse_proto_fields(payload);
    assert!(inner_varints.is_empty());
    assert!(payload.len() >= 791);

    let header = inner_bytes
        .get(&1)
        .and_then(|values| values.first())
        .expect("signature payload should contain the header block");
    let (header_varints, header_bytes) = parse_proto_fields(header);
    assert_eq!(
        header_varints.get(&1).map(Vec::as_slice),
        Some(&[THINKING_SIGNATURE_HEADER_KIND][..])
    );
    assert_eq!(
        header_bytes.get(&6).map(|values| values[0].as_slice()),
        Some(expected_model.as_bytes())
    );
    assert_eq!(
        header_bytes.get(&5).map(|values| values[0].len()),
        Some(THINKING_SIGNATURE_HEADER_BODY_LEN)
    );
    assert_eq!(
        header_varints.get(&3).map(Vec::as_slice),
        Some(&[THINKING_SIGNATURE_HEADER_MODE][..])
    );
    assert_eq!(
        inner_bytes.get(&2).map(|values| values[0].len()),
        Some(THINKING_SIGNATURE_HEADER_NONCE_LEN)
    );
    assert_eq!(
        inner_bytes.get(&3).map(|values| values[0].len()),
        Some(THINKING_SIGNATURE_HEADER_NONCE_LEN)
    );
    assert_eq!(
        inner_bytes.get(&4).map(|values| values[0].len()),
        Some(THINKING_SIGNATURE_HEADER_PROOF_LEN)
    );
    assert_eq!(header_varints.get(&7).map(Vec::as_slice), Some(&[0][..]));
    assert!(
        inner_bytes
            .get(&5)
            .map(|values| values[0].len())
            .unwrap_or_default()
            >= THINKING_SIGNATURE_BODY_MIN_LEN
    );
}

#[test]
fn sse_event_format_is_valid() {
    let event = SseEvent::new("message_start", json!({"type": "message_start"}));
    let sse = event.to_sse_string();
    assert!(sse.starts_with("event: message_start\n"));
    assert!(sse.contains("data: "));
    assert!(sse.ends_with("\n\n"));
}

#[test]
fn split_inline_thinking_content_extracts_non_stream_blocks() {
    let blocks =
        split_inline_thinking_content("<thinking>\nCount carefully.\n</thinking>\n\nbeta", true);

    assert_eq!(blocks, vec![
        InlineThinkingBlock::Thinking("Count carefully.\n".to_string()),
        InlineThinkingBlock::Text("beta".to_string()),
    ]);
}

#[test]
fn build_inline_thinking_content_blocks_attach_signature() {
    let blocks = build_inline_thinking_content_blocks(
        "<thinking>\nCount carefully.\n</thinking>\n\nbeta",
        "claude-opus-4-6",
        true,
    );

    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0]["type"], "thinking");
    assert_eq!(blocks[0]["thinking"], "Count carefully.\n");
    let signature = blocks[0]["signature"]
        .as_str()
        .expect("thinking block should include signature");
    assert_claude_shaped_signature(signature, "claude-opus-4-6");
    assert_eq!(blocks[1], json!({"type": "text", "text": "beta"}));
}

#[test]
fn text_delta_after_tool_use_restarts_text_block() {
    let mut ctx = StreamContext::new_with_thinking("test-model", 1, false, HashMap::new(), None);
    let initial_events = ctx.generate_initial_events();
    assert!(initial_events.iter().any(|event| {
        event.event == "content_block_start" && event.data["content_block"]["type"] == "text"
    }));

    let initial_text_index = ctx
        .text_block_index
        .expect("initial text block index should exist");

    let tool_events = ctx.process_tool_use(&ToolUseEvent {
        name: "test_tool".to_string(),
        tool_use_id: "tool_1".to_string(),
        input: "{}".to_string(),
        stop: false,
    });
    assert!(tool_events.iter().any(|event| {
        event.event == "content_block_stop"
            && event.data["index"].as_i64() == Some(initial_text_index as i64)
    }));

    let text_events = ctx.process_assistant_response("hello");
    let new_text_index = text_events.iter().find_map(|event| {
        if event.event == "content_block_start" && event.data["content_block"]["type"] == "text" {
            event.data["index"].as_i64()
        } else {
            None
        }
    });
    assert!(new_text_index.is_some());
    assert_ne!(new_text_index, Some(initial_text_index as i64));
    assert!(text_events.iter().any(|event| {
        event.event == "content_block_delta"
            && event.data["delta"]["type"] == "text_delta"
            && event.data["delta"]["text"] == "hello"
    }));
}

#[test]
fn tool_use_flushes_buffered_text_before_tool_block() {
    let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new(), None);
    let _ = ctx.generate_initial_events();

    let first = ctx.process_assistant_response("有修");
    assert!(first
        .iter()
        .all(|event| event.event != "content_block_delta"));
    let second = ctx.process_assistant_response("改：");
    assert!(second
        .iter()
        .all(|event| event.event != "content_block_delta"));

    let events = ctx.process_tool_use(&ToolUseEvent {
        name: "Write".to_string(),
        tool_use_id: "tool_1".to_string(),
        input: "{}".to_string(),
        stop: false,
    });

    let text_start_index = events.iter().find_map(|event| {
        if event.event == "content_block_start" && event.data["content_block"]["type"] == "text" {
            event.data["index"].as_i64()
        } else {
            None
        }
    });
    let pos_text_delta = events.iter().position(|event| {
        event.event == "content_block_delta" && event.data["delta"]["type"] == "text_delta"
    });
    let pos_text_stop = text_start_index.and_then(|index| {
        events.iter().position(|event| {
            event.event == "content_block_stop" && event.data["index"].as_i64() == Some(index)
        })
    });
    let pos_tool_start = events.iter().position(|event| {
        event.event == "content_block_start" && event.data["content_block"]["type"] == "tool_use"
    });

    assert!(text_start_index.is_some());
    let pos_text_delta = pos_text_delta.expect("text delta should be emitted before tool start");
    let pos_text_stop = pos_text_stop.expect("text block stop should be emitted");
    let pos_tool_start = pos_tool_start.expect("tool block start should be emitted");
    assert!(pos_text_delta < pos_text_stop);
    assert!(pos_text_stop < pos_tool_start);
    assert!(events.iter().any(|event| {
        event.event == "content_block_delta"
            && event.data["delta"]["type"] == "text_delta"
            && event.data["delta"]["text"] == "有修改："
    }));
}

#[test]
fn tool_use_after_thinking_closes_block_and_filters_end_tag() {
    let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new(), None);
    let _ = ctx.generate_initial_events();

    let mut events = ctx.process_assistant_response("<thinking>abc</thinking>");
    events.extend(ctx.process_tool_use(&ToolUseEvent {
        name: "Write".to_string(),
        tool_use_id: "tool_1".to_string(),
        input: "{}".to_string(),
        stop: false,
    }));
    events.extend(ctx.generate_final_events());

    assert!(events.iter().all(|event| {
        !(event.event == "content_block_delta"
            && event.data["delta"]["type"] == "thinking_delta"
            && event.data["delta"]["thinking"] == "</thinking>")
    }));

    let thinking_index = ctx
        .thinking_block_index
        .expect("thinking block index should exist");
    let pos_thinking_stop = events.iter().position(|event| {
        event.event == "content_block_stop"
            && event.data["index"].as_i64() == Some(thinking_index as i64)
    });
    let pos_tool_start = events.iter().position(|event| {
        event.event == "content_block_start" && event.data["content_block"]["type"] == "tool_use"
    });
    let pos_thinking_stop =
        pos_thinking_stop.expect("thinking block stop should be emitted before tool start");
    let pos_tool_start = pos_tool_start.expect("tool block start should be emitted");
    assert!(pos_thinking_stop < pos_tool_start);
}

#[test]
fn thinking_strips_leading_newline_across_chunks() {
    let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new(), None);
    let _ = ctx.generate_initial_events();

    let mut events = ctx.process_assistant_response("<thinking>");
    events.extend(ctx.process_assistant_response("\nHello world"));
    events.extend(ctx.generate_final_events());

    let thinking = collect_delta_text(&events, "thinking_delta", "thinking");
    assert!(!thinking.starts_with('\n'));
    assert_eq!(thinking, "Hello world");
}

#[test]
fn thinking_only_sets_max_tokens_stop_reason_and_pads_text() {
    let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new(), None);
    let _ = ctx.generate_initial_events();

    let mut events = ctx.process_assistant_response("<thinking>\nabc</thinking>");
    events.extend(ctx.generate_final_events());

    let message_delta = events
        .iter()
        .find(|event| event.event == "message_delta")
        .expect("should have message_delta");
    assert_eq!(message_delta.data["delta"]["stop_reason"], "max_tokens");
    assert!(events.iter().any(|event| {
        event.event == "content_block_delta"
            && event.data["delta"]["type"] == "text_delta"
            && event.data["delta"]["text"] == " "
    }));
}

#[test]
fn identity_probe_buffers_and_rewrites_kiro_self_identification() {
    let mut ctx = StreamContext::new_with_identity(
        "claude-opus-4-7",
        1,
        false,
        HashMap::new(),
        None,
        ResponseModelIdentity {
            model_name: "Claude Opus 4.7".to_string(),
            model_id: "claude-opus-4-7".to_string(),
        },
    );

    let initial = ctx.generate_initial_events();
    assert!(initial
        .iter()
        .all(|event| event.event != "content_block_start"));

    let deltas = ctx.process_assistant_response("我是 Kiro。关于具体的模型信息，我无法讨论。");
    assert!(deltas.is_empty());

    let final_events = ctx.generate_final_events();
    let text = collect_delta_text(&final_events, "text_delta", "text");
    assert!(text.contains("Claude Opus 4.7"));
    assert!(text.contains("claude-opus-4-7"));
    assert!(!text.contains("Kiro"));
}

#[test]
fn identity_probe_with_thinking_still_emits_signature() {
    let mut ctx = StreamContext::new_with_identity(
        "claude-opus-4-8",
        1,
        true,
        HashMap::new(),
        None,
        ResponseModelIdentity {
            model_name: "Claude Opus 4.8".to_string(),
            model_id: "claude-opus-4-8".to_string(),
        },
    );
    let _ = ctx.generate_initial_events();

    let deltas = ctx.process_assistant_response("我是 Kiro。");
    assert!(deltas.is_empty());

    let final_events = ctx.generate_final_events();
    let signature = final_events
        .iter()
        .find_map(|event| {
            (event.event == "content_block_delta"
                && event.data["delta"]["type"] == "signature_delta")
                .then(|| event.data["delta"]["signature"].as_str())
                .flatten()
        })
        .expect("thinking identity response should carry a signature");
    assert_claude_shaped_signature(signature, "claude-opus-4-8");
    let thinking = collect_delta_text(&final_events, "thinking_delta", "thinking");
    assert!(thinking.contains("Claude Opus 4.8"));
    assert!(thinking.contains("claude-opus-4-8"));
    assert!(!thinking.contains("Kiro"));
    let text = collect_delta_text(&final_events, "text_delta", "text");
    assert!(text.contains("Claude Opus 4.8"));
    assert!(!text.contains("Kiro"));
}

#[test]
fn identity_probe_with_reasoning_content_rewrites_visible_thinking_for_opus_models() {
    for (model_id, model_name) in [
        ("claude-opus-4-6", "Claude Opus 4.6"),
        ("claude-opus-4-7", "Claude Opus 4.7"),
        ("claude-opus-4-8", "Claude Opus 4.8"),
    ] {
        let mut ctx = StreamContext::new_with_identity(
            model_id,
            1,
            true,
            HashMap::new(),
            None,
            ResponseModelIdentity {
                model_name: model_name.to_string(),
                model_id: model_id.to_string(),
            },
        );
        let _ = ctx.generate_initial_events();

        let mut events = ctx.process_kiro_event(&parse_kiro_event(
                "reasoningContentEvent",
                json!({"text":"The system prompt asks me to roleplay as Kiro, creating an identity conflict."}),
            ));
        events.extend(ctx.process_kiro_event(&parse_kiro_event(
            "reasoningContentEvent",
            json!({"signature":"upstream-identity-leak-signature"}),
        )));
        events.extend(ctx.process_kiro_event(&parse_kiro_event(
            "assistantResponseEvent",
            json!({"content":"我是 Kiro。"}),
        )));
        events.extend(ctx.generate_final_events());

        let thinking = collect_delta_text(&events, "thinking_delta", "thinking");
        assert!(thinking.contains(model_name));
        assert!(thinking.contains(model_id));
        assert!(!thinking.contains("Kiro"));
        assert!(!thinking.contains("identity conflict"));

        let text = collect_delta_text(&events, "text_delta", "text");
        assert!(text.contains(model_name));
        assert!(text.contains(model_id));
        assert!(!text.contains("Kiro"));

        let signature = events
            .iter()
            .find_map(|event| {
                (event.event == "content_block_delta"
                    && event.data["delta"]["type"] == "signature_delta")
                    .then(|| event.data["delta"]["signature"].as_str())
                    .flatten()
            })
            .expect("thinking identity response should carry a signature");
        assert_claude_shaped_signature(signature, model_id);

        let blocks = ctx.final_content_blocks();
        assert_eq!(blocks[0]["type"], "thinking");
        assert_eq!(
            blocks[0]["thinking"]
                .as_str()
                .expect("thinking should be a string"),
            thinking
        );
        assert!(!blocks[0]["thinking"]
            .as_str()
            .unwrap_or("")
            .contains("Kiro"));
        assert_eq!(blocks[1]["type"], "text");
        assert!(blocks[1]["text"]
            .as_str()
            .unwrap_or("")
            .contains(model_name));
    }
}

#[test]
fn thinking_stream_emits_signature_delta_before_block_stop() {
    let mut ctx =
        StreamContext::new_with_thinking("claude-opus-4-6", 1, true, HashMap::new(), None);
    let _ = ctx.generate_initial_events();

    let mut events = ctx.process_assistant_response("<thinking>\nabc</thinking>\n\nbeta");
    events.extend(ctx.generate_final_events());

    let thinking_index = ctx
        .thinking_block_index
        .expect("thinking block index should exist");
    let signature_pos = events
        .iter()
        .position(|event| {
            event.event == "content_block_delta"
                && event.data["index"].as_i64() == Some(thinking_index as i64)
                && event.data["delta"]["type"] == "signature_delta"
        })
        .expect("should emit signature delta");
    let stop_pos = events
        .iter()
        .position(|event| {
            event.event == "content_block_stop"
                && event.data["index"].as_i64() == Some(thinking_index as i64)
        })
        .expect("should emit thinking block stop");
    assert!(signature_pos < stop_pos);

    let signature = events[signature_pos].data["delta"]["signature"]
        .as_str()
        .expect("signature should be a string");
    assert_claude_shaped_signature(signature, "claude-opus-4-6");
}

#[test]
fn reasoning_content_event_normalizes_signature_for_opus_47() {
    let mut ctx =
        StreamContext::new_with_thinking("claude-opus-4-7", 1, true, HashMap::new(), None);
    let _ = ctx.generate_initial_events();

    let mut events = ctx
        .process_kiro_event(&parse_kiro_event("reasoningContentEvent", json!({"text":"先想一步"})));
    events.extend(ctx.process_kiro_event(&parse_kiro_event(
        "reasoningContentEvent",
        json!({"signature":"upstream-signature-47"}),
    )));
    events.extend(ctx.process_kiro_event(&parse_kiro_event(
        "assistantResponseEvent",
        json!({"content":"最终答案"}),
    )));
    events.extend(ctx.generate_final_events());

    assert!(events.iter().any(|event| {
        event.event == "content_block_start" && event.data["content_block"]["type"] == "thinking"
    }));
    assert!(events.iter().any(|event| {
        event.event == "content_block_delta"
            && event.data["delta"]["type"] == "thinking_delta"
            && event.data["delta"]["thinking"] == "先想一步"
    }));
    assert!(events.iter().any(|event| {
        event.event == "content_block_delta" && event.data["delta"]["type"] == "signature_delta"
    }));
    let signature = events
        .iter()
        .find_map(|event| {
            (event.event == "content_block_delta"
                && event.data["delta"]["type"] == "signature_delta")
                .then(|| event.data["delta"]["signature"].as_str())
                .flatten()
        })
        .expect("signature delta should exist");
    assert_ne!(signature, "upstream-signature-47");
    assert_claude_shaped_signature(signature, "claude-opus-4-7");
    assert!(events.iter().any(|event| {
        event.event == "content_block_delta"
            && event.data["delta"]["type"] == "text_delta"
            && event.data["delta"]["text"] == "最终答案"
    }));

    let blocks = ctx.final_content_blocks();
    assert_eq!(blocks[0]["type"], "thinking");
    assert_eq!(blocks[0]["thinking"], "先想一步");
    assert_claude_shaped_signature(
        blocks[0]["signature"]
            .as_str()
            .expect("signature should be string"),
        "claude-opus-4-7",
    );
    assert_eq!(blocks[1]["type"], "text");
    assert_eq!(blocks[1]["text"], "最终答案");
}

#[test]
fn thinking_stream_synthesizes_signature_before_plain_text() {
    let mut ctx =
        StreamContext::new_with_thinking("claude-opus-4-8", 1, true, HashMap::new(), None);
    let _ = ctx.generate_initial_events();

    let mut events = ctx.process_assistant_response("plain answer without thinking markers");
    events.extend(ctx.generate_final_events());

    let signature_pos = events
        .iter()
        .position(|event| {
            event.event == "content_block_delta" && event.data["delta"]["type"] == "signature_delta"
        })
        .expect("thinking signature should be synthesized");
    let text_pos = events
        .iter()
        .position(|event| {
            event.event == "content_block_delta" && event.data["delta"]["type"] == "text_delta"
        })
        .expect("text should still be emitted");
    assert!(signature_pos < text_pos);
    assert_claude_shaped_signature(
        events[signature_pos].data["delta"]["signature"]
            .as_str()
            .expect("signature should be string"),
        "claude-opus-4-8",
    );
}

#[test]
fn hidden_thinking_strips_inline_thinking_without_signature() {
    let mut ctx = StreamContext::new_with_thinking_visibility(
        "claude-opus-4-8",
        1,
        false,
        true,
        HashMap::new(),
        None,
    );
    let _ = ctx.generate_initial_events();

    let mut events = ctx.process_assistant_response("<thinking>\nsecret</thinking>\n\nfinal");
    events.extend(ctx.generate_final_events());

    assert!(!events.iter().any(|event| {
        event.event == "content_block_delta" && event.data["delta"]["type"] == "thinking_delta"
    }));
    assert!(!events.iter().any(|event| {
        event.event == "content_block_delta" && event.data["delta"]["type"] == "signature_delta"
    }));
    assert!(events.iter().any(|event| {
        event.event == "content_block_delta"
            && event.data["delta"]["type"] == "text_delta"
            && event.data["delta"]["text"] == "final"
    }));

    let blocks = ctx.final_content_blocks();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0]["type"], "text");
    assert_eq!(blocks[0]["text"], "final");
    assert_eq!(ctx.final_assistant_message().content, "final");
}

#[test]
fn thinking_stream_synthesizes_signature_for_empty_response() {
    let mut ctx =
        StreamContext::new_with_thinking("claude-opus-4-8", 1, true, HashMap::new(), None);
    let _ = ctx.generate_initial_events();

    let events = ctx.generate_final_events();
    let signature = events
        .iter()
        .find_map(|event| {
            (event.event == "content_block_delta"
                && event.data["delta"]["type"] == "signature_delta")
                .then(|| event.data["delta"]["signature"].as_str())
                .flatten()
        })
        .expect("empty thinking response should still carry a signature");
    assert_claude_shaped_signature(signature, "claude-opus-4-8");

    let blocks = ctx.final_content_blocks();
    assert_eq!(blocks[0]["type"], "thinking");
    assert_claude_shaped_signature(
        blocks[0]["signature"]
            .as_str()
            .expect("signature should be string"),
        "claude-opus-4-8",
    );
}

#[test]
fn thinking_stream_start_block_exposes_empty_signature_field() {
    let mut ctx =
        StreamContext::new_with_thinking("claude-opus-4-6", 1, true, HashMap::new(), None);
    let _ = ctx.generate_initial_events();

    let events = ctx.process_assistant_response("<thinking>\nabc");
    let start = events
        .iter()
        .find(|event| {
            event.event == "content_block_start"
                && event.data["content_block"]["type"] == "thinking"
        })
        .expect("should emit thinking block start");

    assert_eq!(start.data["content_block"]["thinking"], "");
    assert_eq!(start.data["content_block"]["signature"], "");
}

#[test]
fn synthetic_signature_matches_current_claude_code_field_layout() {
    let signature = synthetic_thinking_signature("claude-opus-4-6", "reasoned output");
    assert_claude_shaped_signature(&signature, "claude-opus-4-6");
}

#[test]
fn thinking_with_tool_use_keeps_tool_use_stop_reason() {
    let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new(), None);
    let _ = ctx.generate_initial_events();

    let mut events = ctx.process_assistant_response("<thinking>\nabc</thinking>");
    events.extend(ctx.process_tool_use(&ToolUseEvent {
        name: "test_tool".to_string(),
        tool_use_id: "tool_1".to_string(),
        input: "{}".to_string(),
        stop: true,
    }));
    events.extend(ctx.generate_final_events());

    let message_delta = events
        .iter()
        .find(|event| event.event == "message_delta")
        .expect("should have message_delta");
    assert_eq!(message_delta.data["delta"]["stop_reason"], "tool_use");
}

#[test]
fn buffered_stream_context_rewrites_large_message_start_input_tokens_from_upstream_context_usage() {
    let mut ctx =
        BufferedStreamContext::new("claude-sonnet-4-6", 60_000, false, HashMap::new(), None);
    ctx.process_and_buffer(&Event::ContextUsage(ContextUsageEvent {
        context_usage_percentage: 12.5,
    }));
    let events = ctx.finish_and_get_all_events();

    let message_start = events
        .iter()
        .find(|event| event.event == "message_start")
        .expect("should have message_start");
    assert_eq!(message_start.data["message"]["usage"]["input_tokens"], serde_json::json!(125000));
}

#[test]
fn message_start_marks_half_input_as_cache_creation_when_cache_read_is_zero() {
    let ctx =
        StreamContext::new_with_thinking("claude-sonnet-4-6", 123, false, HashMap::new(), None);
    let event = ctx.create_message_start_event();
    assert_eq!(event["message"]["usage"]["input_tokens"], serde_json::json!(62));
    assert_eq!(event["message"]["usage"]["cache_creation_input_tokens"], serde_json::json!(61));
    assert_eq!(event["message"]["usage"]["cache_read_input_tokens"], serde_json::json!(0));
}

#[test]
fn metering_event_accumulates_credit_usage() {
    let mut ctx =
        StreamContext::new_with_thinking("claude-sonnet-4-6", 123, false, HashMap::new(), None);
    let _ = ctx.process_kiro_event(&Event::Metering(MeteringEvent {
        unit: Some("credit".to_string()),
        _unit_plural: Some("credits".to_string()),
        usage: Some(0.125),
    }));
    let _ = ctx.process_kiro_event(&Event::Metering(MeteringEvent {
        unit: Some("credit".to_string()),
        _unit_plural: Some("credits".to_string()),
        usage: Some(0.25),
    }));
    assert_eq!(ctx.final_credit_usage(), (Some(0.375), false));
}

#[test]
fn tool_use_restores_original_name_from_mapping() {
    let mut tool_name_map = HashMap::new();
    tool_name_map.insert(
        "short_tool_name".to_string(),
        "tool_name_that_is_much_longer_than_the_kiro_limit_and_should_be_restored".to_string(),
    );
    let mut ctx = StreamContext::new_with_thinking("test-model", 1, false, tool_name_map, None);
    let _ = ctx.generate_initial_events();

    let events = ctx.process_tool_use(&ToolUseEvent {
        name: "short_tool_name".to_string(),
        tool_use_id: "tool_1".to_string(),
        input: "{}".to_string(),
        stop: false,
    });

    let tool_start = events
        .iter()
        .find(|event| {
            event.event == "content_block_start"
                && event.data["content_block"]["type"] == "tool_use"
        })
        .expect("tool_use content block should exist");
    assert_eq!(
        tool_start.data["content_block"]["name"],
        "tool_name_that_is_much_longer_than_the_kiro_limit_and_should_be_restored"
    );
}

#[test]
fn structured_output_tool_is_emitted_as_json_text() {
    let mut ctx = StreamContext::new_with_thinking(
        "claude-opus-4-6",
        1,
        false,
        HashMap::new(),
        Some("sf_emit_structured_output".to_string()),
    );
    let initial_events = ctx.generate_initial_events();
    assert_eq!(initial_events.len(), 1);
    assert_eq!(initial_events[0].event, "message_start");

    let mut events = ctx.process_assistant_response("Here is the answer:");
    events.extend(ctx.process_tool_use(&ToolUseEvent {
        name: "sf_emit_structured_output".to_string(),
        tool_use_id: "tool_1".to_string(),
        input: "{\"result\":16,\"expression\":\"4 * 4\"}".to_string(),
        stop: true,
    }));
    events.extend(ctx.generate_final_events());

    assert!(events.iter().all(|event| {
        !(event.event == "content_block_start" && event.data["content_block"]["type"] == "tool_use")
    }));
    let json_text = collect_delta_text(&events, "text_delta", "text");
    assert_eq!(json_text, "{\"expression\":\"4 * 4\",\"result\":16}");
    let assistant = ctx.final_assistant_message();
    assert_eq!(assistant.content, "{\"expression\":\"4 * 4\",\"result\":16}");
    assert!(assistant.tool_uses.is_none());
}
