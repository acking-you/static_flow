//! Per-request streaming drivers.
//!
//! `StreamContext` converts Kiro upstream events into Anthropic-compatible SSE
//! events: it drives the block state machine, extracts inline thinking,
//! manages tool_use blocks, synthesizes thinking signatures, and counts
//! tokens. `BufferedStreamContext` collects everything for the Claude Code
//! endpoint and rewrites input_tokens from context-usage feedback before
//! flushing.

use std::collections::HashMap;

use serde_json::json;
use uuid::Uuid;

use super::{
    inline_thinking::{
        build_inline_thinking_content_blocks_with_signature_context, find_real_thinking_end_tag,
        find_real_thinking_end_tag_at_buffer_end, find_real_thinking_start_tag,
        strip_inline_thinking_content,
    },
    signature::{synthetic_thinking_signature, ThinkingSignatureContext},
    sse_event::SseEvent,
    state::SseStateManager,
    usage::{
        anthropic_usage_json, resolve_input_tokens_with_threshold,
        KIRO_CONTEXT_USAGE_MIN_REQUEST_TOKENS,
    },
};
use crate::{
    anthropic::converter::{get_context_window_size, ResponseModelIdentity},
    wire::{AssistantMessage, Event, ToolUseEntry},
};

/// Placeholder emitted when a thinking block would otherwise be empty.
const SYNTHETIC_THINKING_PLACEHOLDER: &str = " ";

#[derive(Debug, Clone)]
struct ToolUseAccumulator {
    start_order: usize,
    name: String,
    input_buffer: String,
}

fn canonicalize_structured_output_json(input: &str) -> String {
    let value = if input.is_empty() {
        json!({})
    } else {
        serde_json::from_str(input).unwrap_or_else(|_| json!({}))
    };
    serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string())
}

/// Per-request streaming context that converts Kiro events into SSE events.
///
/// Handles thinking block extraction from inline `<thinking>` tags,
/// text/tool_use block management, and token counting.
pub struct StreamContext {
    pub state_manager: SseStateManager,
    pub model: String,
    pub message_id: String,
    pub input_tokens: i32,
    pub context_input_tokens: Option<i32>,
    context_usage_min_request_tokens: u64,
    pub output_tokens: i32,
    pub credit_usage: f64,
    pub credit_usage_observed: bool,
    pub tool_block_indices: HashMap<String, i32>,
    pub tool_name_map: HashMap<String, String>,
    assistant_content: String,
    tool_use_accumulators: HashMap<String, ToolUseAccumulator>,
    completed_tool_uses: Vec<(usize, ToolUseEntry)>,
    next_tool_use_order: usize,
    structured_output_tool_name: Option<String>,
    structured_output_text_buffer: String,
    structured_output_json_buffers: HashMap<String, String>,
    structured_output_emitted: bool,
    pub thinking_enabled: bool,
    pub thinking_buffer: String,
    pub in_thinking_block: bool,
    pub thinking_extracted: bool,
    reasoning_content_events_observed: bool,
    pub thinking_block_index: Option<i32>,
    pub text_block_index: Option<i32>,
    strip_thinking_leading_newline: bool,
    open_thinking_content: String,
    completed_thinking_content: Option<String>,
    completed_thinking_signature: Option<String>,
    thinking_signature_context: Option<ThinkingSignatureContext>,
    hidden_thinking_enabled: bool,
    response_identity: Option<ResponseModelIdentity>,
    response_identity_applied: bool,
    response_identity_flushed: bool,
}

impl StreamContext {
    pub fn new_with_thinking(
        model: impl Into<String>,
        input_tokens: i32,
        thinking_enabled: bool,
        tool_name_map: HashMap<String, String>,
        structured_output_tool_name: Option<String>,
    ) -> Self {
        Self {
            state_manager: SseStateManager::new(),
            model: model.into(),
            message_id: format!("msg_{}", Uuid::new_v4().simple()),
            input_tokens,
            context_input_tokens: None,
            context_usage_min_request_tokens: KIRO_CONTEXT_USAGE_MIN_REQUEST_TOKENS,
            output_tokens: 0,
            credit_usage: 0.0,
            credit_usage_observed: false,
            tool_block_indices: HashMap::new(),
            tool_name_map,
            assistant_content: String::new(),
            tool_use_accumulators: HashMap::new(),
            completed_tool_uses: Vec::new(),
            next_tool_use_order: 0,
            structured_output_tool_name,
            structured_output_text_buffer: String::new(),
            structured_output_json_buffers: HashMap::new(),
            structured_output_emitted: false,
            thinking_enabled,
            thinking_buffer: String::new(),
            in_thinking_block: false,
            thinking_extracted: false,
            reasoning_content_events_observed: false,
            thinking_block_index: None,
            text_block_index: None,
            strip_thinking_leading_newline: false,
            open_thinking_content: String::new(),
            completed_thinking_content: None,
            completed_thinking_signature: None,
            thinking_signature_context: None,
            hidden_thinking_enabled: false,
            response_identity: None,
            response_identity_applied: false,
            response_identity_flushed: false,
        }
    }

    pub fn new_with_thinking_visibility(
        model: impl Into<String>,
        input_tokens: i32,
        thinking_enabled: bool,
        hidden_thinking_enabled: bool,
        tool_name_map: HashMap<String, String>,
        structured_output_tool_name: Option<String>,
    ) -> Self {
        let mut context = Self::new_with_thinking(
            model,
            input_tokens,
            thinking_enabled,
            tool_name_map,
            structured_output_tool_name,
        );
        context.hidden_thinking_enabled = hidden_thinking_enabled;
        context
    }

    pub fn new_with_identity(
        model: impl Into<String>,
        input_tokens: i32,
        thinking_enabled: bool,
        tool_name_map: HashMap<String, String>,
        structured_output_tool_name: Option<String>,
        response_identity: ResponseModelIdentity,
    ) -> Self {
        Self::new_with_thinking(
            model,
            input_tokens,
            thinking_enabled,
            tool_name_map,
            structured_output_tool_name,
        )
        .with_response_identity(Some(response_identity))
    }

    pub fn with_response_identity(
        mut self,
        response_identity: Option<ResponseModelIdentity>,
    ) -> Self {
        self.response_identity = response_identity;
        self
    }

    pub fn with_context_usage_min_request_tokens(mut self, threshold: u64) -> Self {
        self.context_usage_min_request_tokens = threshold;
        self
    }

    pub fn with_thinking_signature_context(
        mut self,
        context: Option<ThinkingSignatureContext>,
    ) -> Self {
        self.thinking_signature_context = context;
        self
    }

    fn structured_output_mode(&self) -> bool {
        self.structured_output_tool_name.is_some() && !self.thinking_enabled
    }

    fn thinking_parser_enabled(&self) -> bool {
        self.thinking_enabled || self.hidden_thinking_enabled
    }

    fn final_assistant_text(&self) -> String {
        if self.hidden_thinking_enabled {
            strip_inline_thinking_content(&self.assistant_content)
        } else {
            self.assistant_content.clone()
        }
    }

    pub fn final_usage(&self) -> (i32, i32) {
        let (input_tokens, _) = resolve_input_tokens_with_threshold(
            self.input_tokens,
            self.context_input_tokens,
            self.context_usage_min_request_tokens,
        );
        (input_tokens, self.output_tokens.max(1))
    }

    pub fn request_input_tokens(&self) -> i32 {
        self.input_tokens
    }

    pub fn context_input_tokens(&self) -> Option<i32> {
        self.context_input_tokens
    }

    pub fn final_credit_usage(&self) -> (Option<f64>, bool) {
        if self.credit_usage_observed {
            (Some(self.credit_usage.max(0.0)), false)
        } else {
            (None, true)
        }
    }

    fn identity_response_enabled(&self) -> bool {
        self.response_identity.is_some() && !self.structured_output_mode()
    }

    fn apply_identity_response(&mut self) {
        if self.response_identity_applied {
            return;
        }
        let Some(identity) = &self.response_identity else {
            return;
        };
        self.assistant_content = identity.canonical_response();
        self.output_tokens = estimate_tokens(&self.assistant_content);
        self.response_identity_applied = true;
    }

    fn canonical_identity_thinking(&self) -> Option<String> {
        self.response_identity
            .as_ref()
            .map(ResponseModelIdentity::canonical_thinking)
    }

    fn thinking_signature(&self, thinking: &str) -> String {
        self.thinking_signature_context
            .as_ref()
            .map(|context| context.signature(&self.model, thinking))
            .unwrap_or_else(|| synthetic_thinking_signature(&self.model, thinking))
    }

    fn should_synthesize_thinking_block(&self) -> bool {
        self.thinking_enabled
            && !self.structured_output_mode()
            && self.completed_thinking_content.is_none()
            && self.thinking_block_index.is_none()
            && !self.thinking_extracted
    }

    fn synthesize_thinking_block(&mut self) -> Vec<SseEvent> {
        if !self.should_synthesize_thinking_block() {
            return Vec::new();
        }

        let index = self.state_manager.next_block_index();
        self.thinking_block_index = Some(index);
        self.in_thinking_block = true;
        let mut events = self.state_manager.handle_content_block_start(
            index,
            "thinking",
            json!({
                "type":"content_block_start",
                "index":index,
                "content_block":{"type":"thinking","thinking":"","signature":""}
            }),
        );
        let thinking = self
            .canonical_identity_thinking()
            .unwrap_or_else(|| SYNTHETIC_THINKING_PLACEHOLDER.to_string());
        events.push(self.create_thinking_delta_event(index, &thinking));
        self.in_thinking_block = false;
        self.thinking_extracted = true;
        events.extend(self.finalize_open_thinking_block());
        events
    }

    pub fn final_assistant_message(&self) -> AssistantMessage {
        let mut completed_tool_uses = self.completed_tool_uses.clone();
        completed_tool_uses.sort_by_key(|(start_order, _)| *start_order);
        let mut assistant = AssistantMessage::new(self.final_assistant_text());
        let tool_uses = completed_tool_uses
            .into_iter()
            .map(|(_, tool_use)| tool_use)
            .collect::<Vec<_>>();
        if !tool_uses.is_empty() {
            assistant = assistant.with_tool_uses(tool_uses);
        }
        assistant
    }

    pub fn final_content_blocks(&self) -> Vec<serde_json::Value> {
        let assistant_content = self.final_assistant_text();
        if self.thinking_enabled {
            if let Some(thinking) = self.completed_thinking_content.as_ref() {
                let signature = self.thinking_signature(thinking);
                let mut blocks = vec![json!({
                    "type": "thinking",
                    "thinking": thinking,
                    "signature": signature,
                })];
                if !assistant_content.is_empty() {
                    blocks.push(json!({
                        "type": "text",
                        "text": assistant_content,
                    }));
                }
                return blocks;
            }
        }

        build_inline_thinking_content_blocks_with_signature_context(
            &assistant_content,
            &self.model,
            self.thinking_enabled,
            self.thinking_signature_context.as_ref(),
        )
    }

    pub fn create_message_start_event(&self) -> serde_json::Value {
        json!({
            "type":"message_start",
            "message":{
                "id":self.message_id,
                "type":"message",
                "role":"assistant",
                "content":[],
                "model":self.model,
                "stop_reason":null,
                "stop_sequence":null,
                "usage": anthropic_usage_json(self.input_tokens, 1, 0)
            }
        })
    }

    /// Emits `message_start` and (if thinking is disabled) the initial text
    /// block start event.
    pub fn generate_initial_events(&mut self) -> Vec<SseEvent> {
        let mut events = Vec::new();
        if let Some(event) = self
            .state_manager
            .handle_message_start(self.create_message_start_event())
        {
            events.push(event);
        }
        if self.thinking_enabled
            || self.structured_output_mode()
            || self.identity_response_enabled()
        {
            return events;
        }
        let index = self.state_manager.next_block_index();
        self.text_block_index = Some(index);
        events.extend(self.state_manager.handle_content_block_start(
            index,
            "text",
            json!({"type":"content_block_start","index":index,"content_block":{"type":"text","text":""}}),
        ));
        events
    }

    /// Dispatches a single Kiro upstream event into zero or more SSE events.
    pub fn process_kiro_event(&mut self, event: &Event) -> Vec<SseEvent> {
        match event {
            Event::AssistantResponse(response) => {
                self.process_assistant_response(&response.content)
            },
            Event::ReasoningContent(reasoning) => self.process_reasoning_content(
                reasoning.text.as_deref(),
                reasoning.signature.as_deref(),
            ),
            Event::ToolUse(tool_use) => self.process_tool_use(tool_use),
            Event::ContextUsage(usage) => {
                let input_tokens = (usage.context_usage_percentage
                    * get_context_window_size(&self.model) as f64
                    / 100.0) as i32;
                self.context_input_tokens = Some(input_tokens);
                if usage.context_usage_percentage >= 100.0 {
                    self.state_manager
                        .set_stop_reason("model_context_window_exceeded");
                }
                Vec::new()
            },
            Event::Metering(metering) => {
                if let Some(usage) = metering.credit_usage() {
                    self.credit_usage += usage;
                    self.credit_usage_observed = true;
                }
                Vec::new()
            },
            Event::Error {
                error_code: _,
                error_message: _,
            } => Vec::new(),
            Event::Exception {
                exception_type,
                message,
            } => {
                if exception_type == "ContentLengthExceededException" {
                    self.state_manager
                        .set_stop_reason("model_context_window_exceeded");
                }
                let _ = message;
                Vec::new()
            },
            _ => Vec::new(),
        }
    }

    fn process_assistant_response(&mut self, content: &str) -> Vec<SseEvent> {
        if content.is_empty() {
            return Vec::new();
        }
        if self.structured_output_mode() {
            self.structured_output_text_buffer.push_str(content);
            return Vec::new();
        }
        if self.identity_response_enabled() {
            self.apply_identity_response();
            return Vec::new();
        }
        self.assistant_content.push_str(content);
        self.output_tokens += estimate_tokens(content);
        if self.thinking_parser_enabled() {
            if self.reasoning_content_events_observed {
                let mut events = Vec::new();
                if self.thinking_enabled && self.in_thinking_block {
                    self.in_thinking_block = false;
                    self.thinking_extracted = true;
                    events.extend(self.finalize_open_thinking_block());
                }
                events.extend(self.create_text_delta_events(content));
                return events;
            }
            return self.process_content_with_thinking(content);
        }
        self.create_text_delta_events(content)
    }

    fn process_reasoning_content(
        &mut self,
        text: Option<&str>,
        signature: Option<&str>,
    ) -> Vec<SseEvent> {
        if !self.thinking_parser_enabled() {
            return Vec::new();
        }

        self.reasoning_content_events_observed = true;
        if !self.thinking_enabled {
            return Vec::new();
        }
        if self.identity_response_enabled() {
            return self.process_identity_reasoning_content(signature);
        }
        let mut events = Vec::new();

        if !self.in_thinking_block && !self.thinking_extracted {
            let index = self.state_manager.next_block_index();
            self.thinking_block_index = Some(index);
            self.in_thinking_block = true;
            events.extend(self.state_manager.handle_content_block_start(
                index,
                "thinking",
                json!({
                    "type":"content_block_start",
                    "index":index,
                    "content_block":{"type":"thinking","thinking":"","signature":""}
                }),
            ));
        }

        if let Some(text) = text.filter(|value| !value.is_empty()) {
            self.output_tokens += estimate_tokens(text);
            if let Some(index) = self.thinking_block_index {
                events.push(self.create_thinking_delta_event(index, text));
            }
        }

        if let Some(signature) = signature.filter(|value| !value.is_empty()) {
            self.in_thinking_block = false;
            self.thinking_extracted = true;
            events.extend(self.finalize_open_thinking_block_with_signature(Some(signature)));
        }

        events
    }

    fn process_identity_reasoning_content(&mut self, signature: Option<&str>) -> Vec<SseEvent> {
        self.apply_identity_response();

        let mut events = Vec::new();
        if self.completed_thinking_content.is_none() && !self.thinking_extracted {
            if !self.in_thinking_block {
                let index = self.state_manager.next_block_index();
                self.thinking_block_index = Some(index);
                self.in_thinking_block = true;
                events.extend(self.state_manager.handle_content_block_start(
                    index,
                    "thinking",
                    json!({
                        "type":"content_block_start",
                        "index":index,
                        "content_block":{"type":"thinking","thinking":"","signature":""}
                    }),
                ));
            }

            if self.open_thinking_content.is_empty() {
                if let (Some(index), Some(thinking)) =
                    (self.thinking_block_index, self.canonical_identity_thinking())
                {
                    self.output_tokens += estimate_tokens(&thinking);
                    events.push(self.create_thinking_delta_event(index, &thinking));
                }
            }
        }

        if signature.filter(|value| !value.is_empty()).is_some() && self.in_thinking_block {
            self.in_thinking_block = false;
            self.thinking_extracted = true;
            events.extend(self.finalize_open_thinking_block());
        }

        events
    }

    // Parses `<thinking>...</thinking>` tags from the content buffer,
    // emitting thinking_delta and text_delta events as boundaries are found.
    // Buffers partial content when a tag boundary might span chunks.
    fn process_content_with_thinking(&mut self, content: &str) -> Vec<SseEvent> {
        self.thinking_buffer.push_str(content);
        let mut events = Vec::new();
        loop {
            if !self.in_thinking_block && !self.thinking_extracted {
                if let Some(start_pos) = find_real_thinking_start_tag(&self.thinking_buffer) {
                    let before = self.thinking_buffer[..start_pos].to_string();
                    if !before.trim().is_empty() {
                        events.extend(self.create_text_delta_events(&before));
                    }
                    self.in_thinking_block = true;
                    self.strip_thinking_leading_newline = true;
                    self.thinking_buffer =
                        self.thinking_buffer[start_pos + "<thinking>".len()..].to_string();
                    if self.thinking_enabled {
                        let index = self.state_manager.next_block_index();
                        self.thinking_block_index = Some(index);
                        events.extend(self.state_manager.handle_content_block_start(
                            index,
                            "thinking",
                            json!({
                                "type":"content_block_start",
                                "index":index,
                                "content_block":{"type":"thinking","thinking":"","signature":""}
                            }),
                        ));
                    }
                } else {
                    let target_len = self
                        .thinking_buffer
                        .len()
                        .saturating_sub("<thinking>".len());
                    let safe_len = find_char_boundary(&self.thinking_buffer, target_len);
                    if safe_len > 0 {
                        let safe = self.thinking_buffer[..safe_len].to_string();
                        if !safe.trim().is_empty() {
                            if self.thinking_enabled {
                                events.extend(self.synthesize_thinking_block());
                            }
                            events.extend(self.create_text_delta_events(&safe));
                            self.thinking_buffer = self.thinking_buffer[safe_len..].to_string();
                        }
                    }
                    break;
                }
            } else if self.in_thinking_block {
                if self.strip_thinking_leading_newline {
                    if self.thinking_buffer.starts_with('\n') {
                        self.thinking_buffer = self.thinking_buffer[1..].to_string();
                        self.strip_thinking_leading_newline = false;
                    } else if !self.thinking_buffer.is_empty() {
                        self.strip_thinking_leading_newline = false;
                    }
                }
                if let Some(end_pos) = find_real_thinking_end_tag(&self.thinking_buffer) {
                    let thinking = self.thinking_buffer[..end_pos].to_string();
                    if self.thinking_enabled && !thinking.is_empty() {
                        if let Some(index) = self.thinking_block_index {
                            events.push(self.create_thinking_delta_event(index, &thinking));
                        }
                    }
                    self.in_thinking_block = false;
                    self.thinking_extracted = true;
                    if self.thinking_enabled {
                        events.extend(self.finalize_open_thinking_block());
                    }
                    self.thinking_buffer =
                        self.thinking_buffer[end_pos + "</thinking>\n\n".len()..].to_string();
                } else {
                    let target_len = self
                        .thinking_buffer
                        .len()
                        .saturating_sub("</thinking>\n\n".len());
                    let safe_len = find_char_boundary(&self.thinking_buffer, target_len);
                    if safe_len > 0 {
                        let safe = self.thinking_buffer[..safe_len].to_string();
                        if self.thinking_enabled && !safe.is_empty() {
                            if let Some(index) = self.thinking_block_index {
                                events.push(self.create_thinking_delta_event(index, &safe));
                            }
                        }
                        self.thinking_buffer = self.thinking_buffer[safe_len..].to_string();
                    }
                    break;
                }
            } else {
                if !self.thinking_buffer.is_empty() {
                    let remaining = self.thinking_buffer.clone();
                    self.thinking_buffer.clear();
                    events.extend(self.create_text_delta_events(&remaining));
                }
                break;
            }
        }
        events
    }

    fn create_text_delta_events(&mut self, text: &str) -> Vec<SseEvent> {
        let mut events = Vec::new();
        if let Some(index) = self.text_block_index {
            if !self.state_manager.is_block_open_of_type(index, "text") {
                self.text_block_index = None;
            }
        }
        let index = if let Some(index) = self.text_block_index {
            index
        } else {
            let index = self.state_manager.next_block_index();
            self.text_block_index = Some(index);
            events.extend(self.state_manager.handle_content_block_start(
                index,
                "text",
                json!({"type":"content_block_start","index":index,"content_block":{"type":"text","text":""}}),
            ));
            index
        };
        if let Some(event) = self.state_manager.handle_content_block_delta(
            index,
            json!({"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":text}}),
        ) {
            events.push(event);
        }
        events
    }

    fn create_thinking_delta_event(&mut self, index: i32, thinking: &str) -> SseEvent {
        if !thinking.is_empty() {
            self.open_thinking_content.push_str(thinking);
        }
        SseEvent::new(
            "content_block_delta",
            json!({"type":"content_block_delta","index":index,"delta":{"type":"thinking_delta","thinking":thinking}}),
        )
    }

    fn finalize_open_thinking_block(&mut self) -> Vec<SseEvent> {
        self.finalize_open_thinking_block_with_signature(None)
    }

    fn finalize_open_thinking_block_with_signature(
        &mut self,
        _signature_override: Option<&str>,
    ) -> Vec<SseEvent> {
        let mut events = Vec::new();
        let Some(index) = self.thinking_block_index else {
            return events;
        };

        let thinking = self.open_thinking_content.clone();
        let signature = self.thinking_signature(&thinking);
        self.completed_thinking_content = Some(thinking);
        self.completed_thinking_signature = Some(signature.clone());
        if let Some(event) = self.state_manager.handle_content_block_delta(
            index,
            json!({
                "type": "content_block_delta",
                "index": index,
                "delta": {"type": "signature_delta", "signature": signature}
            }),
        ) {
            events.push(event);
        }
        if let Some(stop) = self.state_manager.handle_content_block_stop(index) {
            events.push(stop);
        }
        self.open_thinking_content.clear();
        events
    }

    // Handles a tool_use event: closes any open thinking block, flushes
    // buffered text, then emits tool_use block start/delta/stop events.
    fn process_tool_use(&mut self, tool_use: &crate::wire::ToolUseEvent) -> Vec<SseEvent> {
        let mut events = Vec::new();
        if self.structured_output_mode()
            && self.structured_output_tool_name.as_deref() == Some(&tool_use.name)
        {
            let buffer = self
                .structured_output_json_buffers
                .entry(tool_use.tool_use_id.clone())
                .or_default();
            buffer.push_str(&tool_use.input);
            if tool_use.stop {
                let json_text = canonicalize_structured_output_json(buffer);
                self.structured_output_text_buffer = json_text.clone();
                self.assistant_content = json_text.clone();
                self.output_tokens = estimate_tokens(&json_text);
                self.structured_output_emitted = true;
                self.structured_output_json_buffers
                    .remove(&tool_use.tool_use_id);
                events.extend(self.create_text_delta_events(&json_text));
            }
            return events;
        }
        self.state_manager.set_has_tool_use(true);

        if self.thinking_parser_enabled()
            && self.reasoning_content_events_observed
            && self.in_thinking_block
        {
            self.in_thinking_block = false;
            self.thinking_extracted = true;
            if self.thinking_enabled {
                events.extend(self.finalize_open_thinking_block());
            }
        }

        if self.thinking_parser_enabled() && self.in_thinking_block {
            if let Some(end_pos) = find_real_thinking_end_tag_at_buffer_end(&self.thinking_buffer) {
                let thinking = self.thinking_buffer[..end_pos].to_string();
                if self.thinking_enabled && !thinking.is_empty() {
                    if let Some(index) = self.thinking_block_index {
                        events.push(self.create_thinking_delta_event(index, &thinking));
                    }
                }

                self.in_thinking_block = false;
                self.thinking_extracted = true;

                if self.thinking_enabled {
                    events.extend(self.finalize_open_thinking_block());
                }

                let after_pos = end_pos + "</thinking>".len();
                let remaining = self.thinking_buffer[after_pos..].trim_start().to_string();
                self.thinking_buffer.clear();
                if !remaining.is_empty() {
                    events.extend(self.create_text_delta_events(&remaining));
                }
            }
        }

        if self.thinking_parser_enabled()
            && !self.in_thinking_block
            && !self.thinking_extracted
            && !self.thinking_buffer.is_empty()
        {
            let buffered = std::mem::take(&mut self.thinking_buffer);
            events.extend(self.create_text_delta_events(&buffered));
        }

        let block_index = if let Some(index) = self.tool_block_indices.get(&tool_use.tool_use_id) {
            *index
        } else {
            let index = self.state_manager.next_block_index();
            self.tool_block_indices
                .insert(tool_use.tool_use_id.clone(), index);
            index
        };
        let original_name = self
            .tool_name_map
            .get(&tool_use.name)
            .cloned()
            .unwrap_or_else(|| tool_use.name.clone());
        let accumulator = if let Some(accumulator) =
            self.tool_use_accumulators.get_mut(&tool_use.tool_use_id)
        {
            accumulator
        } else {
            let start_order = self.next_tool_use_order;
            self.next_tool_use_order += 1;
            self.tool_use_accumulators
                .insert(tool_use.tool_use_id.clone(), ToolUseAccumulator {
                    start_order,
                    name: original_name.clone(),
                    input_buffer: String::new(),
                });
            self.tool_use_accumulators
                .get_mut(&tool_use.tool_use_id)
                .expect("tool use accumulator inserted")
        };
        accumulator.name = original_name.clone();
        accumulator.input_buffer.push_str(&tool_use.input);

        events.extend(self.state_manager.handle_content_block_start(
            block_index,
            "tool_use",
            json!({"type":"content_block_start","index":block_index,"content_block":{"type":"tool_use","id":tool_use.tool_use_id,"name":original_name,"input":{}}}),
        ));
        if !tool_use.input.is_empty() {
            self.output_tokens += (tool_use.input.len() as i32 + 3) / 4;
            if let Some(event) = self.state_manager.handle_content_block_delta(
                block_index,
                json!({"type":"content_block_delta","index":block_index,"delta":{"type":"input_json_delta","partial_json":tool_use.input}}),
            ) {
                events.push(event);
            }
        }
        if tool_use.stop {
            if let Some(accumulator) = self.tool_use_accumulators.remove(&tool_use.tool_use_id) {
                let input = if accumulator.input_buffer.is_empty() {
                    json!({})
                } else {
                    serde_json::from_str(&accumulator.input_buffer).unwrap_or_else(|_| json!({}))
                };
                self.completed_tool_uses.push((
                    accumulator.start_order,
                    ToolUseEntry::new(tool_use.tool_use_id.clone(), accumulator.name)
                        .with_input(input),
                ));
            }
            if let Some(event) = self.state_manager.handle_content_block_stop(block_index) {
                events.push(event);
            }
        }
        events
    }

    /// Flushes remaining thinking/text buffers and emits final SSE events.
    ///
    /// If only a thinking block was produced (no text or tool_use), sets
    /// stop_reason to `max_tokens` and emits a single-space text block so
    /// clients always receive at least one non-thinking content block.
    pub fn generate_final_events(&mut self) -> Vec<SseEvent> {
        let mut events = Vec::new();
        if self.structured_output_mode() && !self.structured_output_emitted {
            let buffered = std::mem::take(&mut self.structured_output_text_buffer);
            if !buffered.is_empty() {
                self.assistant_content = buffered.clone();
                self.output_tokens = estimate_tokens(&buffered);
                events.extend(self.create_text_delta_events(&buffered));
            }
        }
        if self.thinking_parser_enabled()
            && self.reasoning_content_events_observed
            && self.in_thinking_block
        {
            self.in_thinking_block = false;
            self.thinking_extracted = true;
            if self.thinking_enabled {
                events.extend(self.finalize_open_thinking_block());
            }
        }
        if self.thinking_parser_enabled() && !self.thinking_buffer.is_empty() {
            if self.in_thinking_block {
                if let Some(end_pos) =
                    find_real_thinking_end_tag_at_buffer_end(&self.thinking_buffer)
                {
                    let thinking = self.thinking_buffer[..end_pos].to_string();
                    if self.thinking_enabled && !thinking.is_empty() {
                        if let Some(index) = self.thinking_block_index {
                            events.push(self.create_thinking_delta_event(index, &thinking));
                        }
                    }

                    if self.thinking_enabled {
                        events.extend(self.finalize_open_thinking_block());
                    }

                    let after_pos = end_pos + "</thinking>".len();
                    let remaining = self.thinking_buffer[after_pos..].trim_start().to_string();
                    self.thinking_buffer.clear();
                    self.in_thinking_block = false;
                    self.thinking_extracted = true;
                    if !remaining.is_empty() {
                        events.extend(self.create_text_delta_events(&remaining));
                    }
                } else if self.thinking_enabled {
                    if let Some(index) = self.thinking_block_index {
                        let buffered_thinking = self.thinking_buffer.clone();
                        events.push(self.create_thinking_delta_event(index, &buffered_thinking));
                    }
                    events.extend(self.finalize_open_thinking_block());
                }
            } else {
                let buffer_content = self.thinking_buffer.clone();
                if self.thinking_enabled {
                    events.extend(self.synthesize_thinking_block());
                }
                events.extend(self.create_text_delta_events(&buffer_content));
            }
            self.thinking_buffer.clear();
        }

        events.extend(self.synthesize_thinking_block());

        if self.identity_response_enabled() {
            self.apply_identity_response();
            if !self.response_identity_flushed && !self.assistant_content.is_empty() {
                let content = self.assistant_content.clone();
                events.extend(self.create_text_delta_events(&content));
                self.response_identity_flushed = true;
            }
        }

        if self.thinking_enabled
            && self.thinking_block_index.is_some()
            && !self.state_manager.has_non_thinking_blocks()
        {
            self.state_manager.set_stop_reason("max_tokens");
            events.extend(self.create_text_delta_events(" "));
        }
        let (input_tokens, output_tokens) = self.final_usage();
        events.extend(
            self.state_manager
                .generate_final_events(input_tokens, output_tokens),
        );
        events
    }
}

/// Buffered variant of [`StreamContext`] for the `/cc/v1/messages` endpoint.
///
/// Collects all SSE events in memory, then on finish rewrites the
/// `message_start` input_tokens with the actual value derived from
/// Kiro's context-usage feedback before flushing everything at once.
pub struct BufferedStreamContext {
    inner: StreamContext,
    event_buffer: Vec<SseEvent>,
    estimated_input_tokens: i32,
    initial_events_generated: bool,
}

impl BufferedStreamContext {
    pub fn new(
        model: impl Into<String>,
        estimated_input_tokens: i32,
        thinking_enabled: bool,
        tool_name_map: HashMap<String, String>,
        structured_output_tool_name: Option<String>,
    ) -> Self {
        Self {
            inner: StreamContext::new_with_thinking(
                model,
                estimated_input_tokens,
                thinking_enabled,
                tool_name_map,
                structured_output_tool_name,
            ),
            event_buffer: Vec::new(),
            estimated_input_tokens,
            initial_events_generated: false,
        }
    }

    pub fn with_context_usage_min_request_tokens(mut self, threshold: u64) -> Self {
        self.inner = self.inner.with_context_usage_min_request_tokens(threshold);
        self
    }

    /// Buffers a single Kiro event (lazily generates initial events on first
    /// call).
    pub fn process_and_buffer(&mut self, event: &Event) {
        if !self.initial_events_generated {
            self.event_buffer
                .extend(self.inner.generate_initial_events());
            self.initial_events_generated = true;
        }
        self.event_buffer
            .extend(self.inner.process_kiro_event(event));
    }

    pub fn model(&self) -> &str {
        &self.inner.model
    }

    pub fn thinking_enabled(&self) -> bool {
        self.inner.thinking_enabled
    }

    pub fn estimated_input_tokens(&self) -> i32 {
        self.estimated_input_tokens
    }

    pub fn context_input_tokens(&self) -> Option<i32> {
        self.inner.context_input_tokens()
    }

    /// Finalizes the stream: appends final events, patches input_tokens in
    /// `message_start`, and returns all buffered events.
    pub fn finish_and_get_all_events(&mut self) -> Vec<SseEvent> {
        if !self.initial_events_generated {
            self.event_buffer
                .extend(self.inner.generate_initial_events());
            self.initial_events_generated = true;
        }
        self.event_buffer.extend(self.inner.generate_final_events());
        let (input_tokens, _) = self.inner.final_usage();
        for event in &mut self.event_buffer {
            if event.event == "message_start" {
                if let Some(usage) = event
                    .data
                    .get_mut("message")
                    .and_then(|message| message.get_mut("usage"))
                {
                    usage["input_tokens"] = serde_json::json!(input_tokens);
                }
            }
        }
        std::mem::take(&mut self.event_buffer)
    }

    pub fn final_usage(&self) -> (i32, i32) {
        self.inner.final_usage()
    }

    pub fn final_credit_usage(&self) -> (Option<f64>, bool) {
        self.inner.final_credit_usage()
    }

    pub fn final_assistant_message(&self) -> AssistantMessage {
        self.inner.final_assistant_message()
    }
}

// Rough token estimate: CJK chars ~0.67 tokens each, others ~0.25 each.
fn estimate_tokens(text: &str) -> i32 {
    let mut chinese_count = 0;
    let mut other_count = 0;
    for ch in text.chars() {
        if ('\u{4E00}'..='\u{9FFF}').contains(&ch) {
            chinese_count += 1;
        } else {
            other_count += 1;
        }
    }
    (((chinese_count * 2 + 2) / 3) + ((other_count + 3) / 4)).max(1)
}

// Finds the nearest valid UTF-8 char boundary at or before `target`.
fn find_char_boundary(s: &str, target: usize) -> usize {
    if target >= s.len() {
        return s.len();
    }
    if target == 0 {
        return 0;
    }
    let mut pos = target;
    while pos > 0 && !s.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use serde_json::json;

    use super::{
        super::inline_thinking::build_inline_thinking_content_blocks, synthetic_thinking_signature,
        BufferedStreamContext, ResponseModelIdentity, SseEvent, StreamContext,
    };
    use crate::{
        anthropic::{
            converter::{ResponseIdentityKind, ResponseIdentityLanguage, ResponseIdentityPlatform},
            stream::signature::{
                THINKING_SIGNATURE_HEADER_BODY_LEN, THINKING_SIGNATURE_HEADER_MODE,
                THINKING_SIGNATURE_HEADER_NONCE_LEN, THINKING_SIGNATURE_HEADER_PROOF_LEN,
            },
        },
        parser::{
            frame::Frame,
            header::{HeaderValue, Headers},
        },
        wire::{ContextUsageEvent, Event, MeteringEvent, ToolUseEvent},
    };

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

    fn assert_bytecat_shaped_signature(signature: &str, expected_model: &str) -> usize {
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

        let header = inner_bytes
            .get(&1)
            .and_then(|values| values.first())
            .expect("signature payload should contain the header block");
        let (header_varints, header_bytes) = parse_proto_fields(header);
        assert_eq!(header_varints.get(&1).map(Vec::as_slice), Some(&[14][..]));
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
        assert_eq!(header_bytes.get(&8).map(|values| values[0].len()), Some(8));
        let body_len = inner_bytes
            .get(&5)
            .map(|values| values[0].len())
            .expect("signature payload should contain field 5");
        assert!(matches!(body_len, 140 | 425), "unexpected signature body length: {body_len}");
        body_len
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
        assert_bytecat_shaped_signature(signature, "claude-opus-4-6");
        assert_eq!(blocks[1], json!({"type": "text", "text": "beta"}));
    }

    #[test]
    fn text_delta_after_tool_use_restarts_text_block() {
        let mut ctx =
            StreamContext::new_with_thinking("test-model", 1, false, HashMap::new(), None);
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
            if event.event == "content_block_start" && event.data["content_block"]["type"] == "text"
            {
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
            if event.event == "content_block_start" && event.data["content_block"]["type"] == "text"
            {
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
            event.event == "content_block_start"
                && event.data["content_block"]["type"] == "tool_use"
        });

        assert!(text_start_index.is_some());
        let pos_text_delta =
            pos_text_delta.expect("text delta should be emitted before tool start");
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
            event.event == "content_block_start"
                && event.data["content_block"]["type"] == "tool_use"
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
    fn content_length_exception_sets_context_window_stop_reason() {
        let mut ctx =
            StreamContext::new_with_thinking("claude-sonnet-4-6", 1, false, HashMap::new(), None);
        let _ = ctx.generate_initial_events();

        let _ = ctx.process_kiro_event(&Event::Exception {
            exception_type: "ContentLengthExceededException".to_string(),
            message: "Input content length exceeds threshold.".to_string(),
        });
        let events = ctx.generate_final_events();

        let message_delta = events
            .iter()
            .find(|event| event.event == "message_delta")
            .expect("should have message_delta");
        assert_eq!(message_delta.data["delta"]["stop_reason"], "model_context_window_exceeded");
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
                model_short_name: "Opus 4.7".to_string(),
                model_id: "claude-opus-4-7".to_string(),
                kind: ResponseIdentityKind::ModelOnly,
                platform: ResponseIdentityPlatform::ClaudeCode,
                thinking_language: ResponseIdentityLanguage::Chinese,
                repo_name_hint: None,
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
                model_short_name: "Opus 4.8".to_string(),
                model_id: "claude-opus-4-8".to_string(),
                kind: ResponseIdentityKind::ModelOnly,
                platform: ResponseIdentityPlatform::ClaudeCode,
                thinking_language: ResponseIdentityLanguage::Chinese,
                repo_name_hint: None,
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
        assert_bytecat_shaped_signature(signature, "claude-opus-4-8");
        let thinking = collect_delta_text(&final_events, "thinking_delta", "thinking");
        assert_eq!(
            thinking,
            " The user is asking me to identify myself in Chinese, and they want an honest \
             answer. I should respond directly and truthfully about who I am."
        );
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
                    model_short_name: model_name
                        .strip_prefix("Claude ")
                        .unwrap_or(model_name)
                        .to_string(),
                    model_id: model_id.to_string(),
                    kind: ResponseIdentityKind::ModelOnly,
                    platform: ResponseIdentityPlatform::ClaudeCode,
                    thinking_language: ResponseIdentityLanguage::Chinese,
                    repo_name_hint: None,
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
            assert_eq!(
                thinking,
                " The user is asking me to identify myself in Chinese, and they want an honest \
                 answer. I should respond directly and truthfully about who I am."
            );
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
            assert_bytecat_shaped_signature(signature, model_id);

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
        assert_bytecat_shaped_signature(signature, "claude-opus-4-6");
    }

    #[test]
    fn reasoning_content_event_normalizes_signature_for_opus_47() {
        let mut ctx =
            StreamContext::new_with_thinking("claude-opus-4-7", 1, true, HashMap::new(), None);
        let _ = ctx.generate_initial_events();

        let mut events = ctx.process_kiro_event(&parse_kiro_event(
            "reasoningContentEvent",
            json!({"text":"先想一步"}),
        ));
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
            event.event == "content_block_start"
                && event.data["content_block"]["type"] == "thinking"
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
        assert_bytecat_shaped_signature(signature, "claude-opus-4-7");
        assert!(events.iter().any(|event| {
            event.event == "content_block_delta"
                && event.data["delta"]["type"] == "text_delta"
                && event.data["delta"]["text"] == "最终答案"
        }));

        let blocks = ctx.final_content_blocks();
        assert_eq!(blocks[0]["type"], "thinking");
        assert_eq!(blocks[0]["thinking"], "先想一步");
        assert_bytecat_shaped_signature(
            blocks[0]["signature"]
                .as_str()
                .expect("signature should be string"),
            "claude-opus-4-7",
        );
        assert_eq!(blocks[1]["type"], "text");
        assert_eq!(blocks[1]["text"], "最终答案");
    }

    #[test]
    fn reasoning_content_preserves_long_upstream_chunks_while_capping_signature_body() {
        let mut ctx =
            StreamContext::new_with_thinking("claude-opus-4-7", 1, true, HashMap::new(), None);
        let _ = ctx.generate_initial_events();
        let first_chunk = "alpha ".repeat(36);
        let second_chunk = "beta ".repeat(24);

        let mut events = ctx.process_kiro_event(&parse_kiro_event(
            "reasoningContentEvent",
            json!({"text": first_chunk}),
        ));
        events.extend(ctx.process_kiro_event(&parse_kiro_event(
            "reasoningContentEvent",
            json!({"text": second_chunk}),
        )));
        events.extend(ctx.process_kiro_event(&parse_kiro_event(
            "reasoningContentEvent",
            json!({"signature":"upstream-signature-47"}),
        )));

        let thinking_chunks = events
            .iter()
            .filter(|event| {
                event.event == "content_block_delta"
                    && event.data["delta"]["type"] == "thinking_delta"
            })
            .map(|event| {
                event.data["delta"]["thinking"]
                    .as_str()
                    .expect("thinking should be string")
                    .to_string()
            })
            .collect::<Vec<_>>();
        assert_eq!(thinking_chunks, vec!["alpha ".repeat(36), "beta ".repeat(24)]);

        let signature = events
            .iter()
            .find_map(|event| {
                (event.event == "content_block_delta"
                    && event.data["delta"]["type"] == "signature_delta")
                    .then(|| event.data["delta"]["signature"].as_str())
                    .flatten()
            })
            .expect("signature delta should exist");
        let body_len = assert_bytecat_shaped_signature(signature, "claude-opus-4-7");
        assert_eq!(body_len, 425);

        let blocks = ctx.final_content_blocks();
        assert_eq!(blocks[0]["thinking"], "alpha ".repeat(36) + &"beta ".repeat(24));
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
                event.event == "content_block_delta"
                    && event.data["delta"]["type"] == "signature_delta"
            })
            .expect("thinking signature should be synthesized");
        let text_pos = events
            .iter()
            .position(|event| {
                event.event == "content_block_delta" && event.data["delta"]["type"] == "text_delta"
            })
            .expect("text should still be emitted");
        assert!(signature_pos < text_pos);
        assert_bytecat_shaped_signature(
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
        assert_bytecat_shaped_signature(signature, "claude-opus-4-8");

        let blocks = ctx.final_content_blocks();
        assert_eq!(blocks[0]["type"], "thinking");
        assert_bytecat_shaped_signature(
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
        assert_bytecat_shaped_signature(&signature, "claude-opus-4-6");
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
    fn buffered_stream_context_rewrites_large_message_start_input_tokens_from_upstream_context_usage(
    ) {
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
        assert_eq!(
            message_start.data["message"]["usage"]["input_tokens"],
            serde_json::json!(125000)
        );
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
            !(event.event == "content_block_start"
                && event.data["content_block"]["type"] == "tool_use")
        }));
        let json_text = collect_delta_text(&events, "text_delta", "text");
        assert_eq!(json_text, "{\"expression\":\"4 * 4\",\"result\":16}");
        let assistant = ctx.final_assistant_message();
        assert_eq!(assistant.content, "{\"expression\":\"4 * 4\",\"result\":16}");
        assert!(assistant.tool_uses.is_none());
    }
}
