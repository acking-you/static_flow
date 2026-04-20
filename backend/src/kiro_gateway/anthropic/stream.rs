//! SSE event stream adapter for the Anthropic-compatible endpoint.
//!
//! Converts Kiro upstream binary events into Anthropic-compatible SSE events.
//! Handles `<thinking>` block extraction from inline content, tool_use block
//! interleaving, and a buffered mode for Claude Code that collects all events
//! before flushing (to rewrite input_tokens from context-usage feedback).

use std::collections::HashMap;

use serde_json::json;
use uuid::Uuid;

use super::{anthropic_usage_json, converter::get_context_window_size};
use crate::kiro_gateway::wire::{AssistantMessage, Event, ToolUseEntry};

/// A single Server-Sent Event with an event type and JSON data payload.
#[derive(Debug, Clone)]
pub struct SseEvent {
    pub event: String,
    pub data: serde_json::Value,
}

impl SseEvent {
    pub fn new(event: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            event: event.into(),
            data,
        }
    }

    /// Formats this event as a standard SSE text frame (`event: ...\ndata:
    /// ...\n\n`).
    pub fn to_sse_string(&self) -> String {
        format!(
            "event: {}\ndata: {}\n\n",
            self.event,
            serde_json::to_string(&self.data).unwrap_or_default()
        )
    }
}

// Tracks the lifecycle of a single content block (text, thinking, tool_use).
#[derive(Debug, Clone)]
struct BlockState {
    block_type: String,
    started: bool,
    stopped: bool,
}

#[derive(Debug, Clone)]
struct ToolUseAccumulator {
    start_order: usize,
    name: String,
    input_buffer: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum InlineThinkingBlock {
    Thinking(String),
    Text(String),
}

impl BlockState {
    fn new(block_type: impl Into<String>) -> Self {
        Self {
            block_type: block_type.into(),
            started: false,
            stopped: false,
        }
    }
}

/// Manages SSE protocol state: tracks which blocks are open, ensures
/// proper start/delta/stop sequencing, and generates final message events.
#[derive(Debug)]
pub struct SseStateManager {
    message_started: bool,
    message_delta_sent: bool,
    active_blocks: HashMap<i32, BlockState>,
    message_ended: bool,
    next_block_index: i32,
    stop_reason: Option<String>,
    has_tool_use: bool,
}

impl Default for SseStateManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SseStateManager {
    pub fn new() -> Self {
        Self {
            message_started: false,
            message_delta_sent: false,
            active_blocks: HashMap::new(),
            message_ended: false,
            next_block_index: 0,
            stop_reason: None,
            has_tool_use: false,
        }
    }

    fn is_block_open_of_type(&self, index: i32, expected_type: &str) -> bool {
        self.active_blocks.get(&index).is_some_and(|block| {
            block.started && !block.stopped && block.block_type == expected_type
        })
    }

    pub fn next_block_index(&mut self) -> i32 {
        let index = self.next_block_index;
        self.next_block_index += 1;
        index
    }

    pub fn set_has_tool_use(&mut self, has_tool_use: bool) {
        self.has_tool_use = has_tool_use;
    }

    pub fn set_stop_reason(&mut self, reason: impl Into<String>) {
        self.stop_reason = Some(reason.into());
    }

    fn has_non_thinking_blocks(&self) -> bool {
        self.active_blocks
            .values()
            .any(|block| block.block_type != "thinking")
    }

    pub fn get_stop_reason(&self) -> String {
        if let Some(reason) = &self.stop_reason {
            reason.clone()
        } else if self.has_tool_use {
            "tool_use".to_string()
        } else {
            "end_turn".to_string()
        }
    }

    pub fn handle_message_start(&mut self, event: serde_json::Value) -> Option<SseEvent> {
        if self.message_started {
            return None;
        }
        self.message_started = true;
        Some(SseEvent::new("message_start", event))
    }

    pub fn handle_content_block_start(
        &mut self,
        index: i32,
        block_type: &str,
        data: serde_json::Value,
    ) -> Vec<SseEvent> {
        let mut events = Vec::new();
        if block_type == "tool_use" {
            self.has_tool_use = true;
            for (block_index, block) in self.active_blocks.iter_mut() {
                if block.block_type == "text" && block.started && !block.stopped {
                    events.push(SseEvent::new(
                        "content_block_stop",
                        json!({"type":"content_block_stop","index":block_index}),
                    ));
                    block.stopped = true;
                }
            }
        }
        if let Some(block) = self.active_blocks.get_mut(&index) {
            if block.started {
                return events;
            }
            block.started = true;
        } else {
            let mut block = BlockState::new(block_type);
            block.started = true;
            self.active_blocks.insert(index, block);
        }
        events.push(SseEvent::new("content_block_start", data));
        events
    }

    pub fn handle_content_block_delta(
        &mut self,
        index: i32,
        data: serde_json::Value,
    ) -> Option<SseEvent> {
        let block = self.active_blocks.get(&index)?;
        if !block.started || block.stopped {
            return None;
        }
        Some(SseEvent::new("content_block_delta", data))
    }

    pub fn handle_content_block_stop(&mut self, index: i32) -> Option<SseEvent> {
        let block = self.active_blocks.get_mut(&index)?;
        if block.stopped {
            return None;
        }
        block.stopped = true;
        Some(SseEvent::new(
            "content_block_stop",
            json!({"type":"content_block_stop","index":index}),
        ))
    }

    /// Closes any still-open blocks and emits `message_delta` + `message_stop`.
    pub fn generate_final_events(
        &mut self,
        input_tokens: i32,
        output_tokens: i32,
    ) -> Vec<SseEvent> {
        let mut events = Vec::new();
        for (index, block) in self.active_blocks.iter_mut() {
            if block.started && !block.stopped {
                events.push(SseEvent::new(
                    "content_block_stop",
                    json!({"type":"content_block_stop","index":index}),
                ));
                block.stopped = true;
            }
        }
        if !self.message_delta_sent {
            self.message_delta_sent = true;
            events.push(SseEvent::new(
                "message_delta",
                json!({
                    "type":"message_delta",
                    "delta":{"stop_reason":self.get_stop_reason(),"stop_sequence":null},
                    "usage":{"input_tokens":input_tokens,"output_tokens":output_tokens}
                }),
            ));
        }
        if !self.message_ended {
            self.message_ended = true;
            events.push(SseEvent::new("message_stop", json!({"type":"message_stop"})));
        }
        events
    }
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
    pub output_tokens: i32,
    pub credit_usage: f64,
    pub credit_usage_observed: bool,
    pub tool_block_indices: HashMap<String, i32>,
    pub tool_name_map: HashMap<String, String>,
    assistant_content: String,
    tool_use_accumulators: HashMap<String, ToolUseAccumulator>,
    completed_tool_uses: Vec<(usize, ToolUseEntry)>,
    next_tool_use_order: usize,
    pub thinking_enabled: bool,
    pub thinking_buffer: String,
    pub in_thinking_block: bool,
    pub thinking_extracted: bool,
    pub thinking_block_index: Option<i32>,
    pub text_block_index: Option<i32>,
    strip_thinking_leading_newline: bool,
}

impl StreamContext {
    pub fn new_with_thinking(
        model: impl Into<String>,
        input_tokens: i32,
        thinking_enabled: bool,
        tool_name_map: HashMap<String, String>,
    ) -> Self {
        Self {
            state_manager: SseStateManager::new(),
            model: model.into(),
            message_id: format!("msg_{}", Uuid::new_v4().simple()),
            input_tokens,
            context_input_tokens: None,
            output_tokens: 0,
            credit_usage: 0.0,
            credit_usage_observed: false,
            tool_block_indices: HashMap::new(),
            tool_name_map,
            assistant_content: String::new(),
            tool_use_accumulators: HashMap::new(),
            completed_tool_uses: Vec::new(),
            next_tool_use_order: 0,
            thinking_enabled,
            thinking_buffer: String::new(),
            in_thinking_block: false,
            thinking_extracted: false,
            thinking_block_index: None,
            text_block_index: None,
            strip_thinking_leading_newline: false,
        }
    }

    pub fn final_usage(&self) -> (i32, i32) {
        let (input_tokens, _) =
            super::resolve_input_tokens(self.input_tokens, self.context_input_tokens);
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

    pub fn final_assistant_message(&self) -> AssistantMessage {
        let mut completed_tool_uses = self.completed_tool_uses.clone();
        completed_tool_uses.sort_by_key(|(start_order, _)| *start_order);
        let mut assistant = AssistantMessage::new(self.assistant_content.clone());
        let tool_uses = completed_tool_uses
            .into_iter()
            .map(|(_, tool_use)| tool_use)
            .collect::<Vec<_>>();
        if !tool_uses.is_empty() {
            assistant = assistant.with_tool_uses(tool_uses);
        }
        assistant
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
        if self.thinking_enabled {
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
                    self.state_manager.set_stop_reason("max_tokens");
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
        self.assistant_content.push_str(content);
        self.output_tokens += estimate_tokens(content);
        if self.thinking_enabled {
            return self.process_content_with_thinking(content);
        }
        self.create_text_delta_events(content)
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
                    let index = self.state_manager.next_block_index();
                    self.thinking_block_index = Some(index);
                    events.extend(self.state_manager.handle_content_block_start(
                        index,
                        "thinking",
                        json!({"type":"content_block_start","index":index,"content_block":{"type":"thinking","thinking":""}}),
                    ));
                } else {
                    let target_len = self
                        .thinking_buffer
                        .len()
                        .saturating_sub("<thinking>".len());
                    let safe_len = find_char_boundary(&self.thinking_buffer, target_len);
                    if safe_len > 0 {
                        let safe = self.thinking_buffer[..safe_len].to_string();
                        if !safe.trim().is_empty() {
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
                    if !thinking.is_empty() {
                        if let Some(index) = self.thinking_block_index {
                            events.push(self.create_thinking_delta_event(index, &thinking));
                        }
                    }
                    self.in_thinking_block = false;
                    self.thinking_extracted = true;
                    if let Some(index) = self.thinking_block_index {
                        events.push(self.create_thinking_delta_event(index, ""));
                        if let Some(stop) = self.state_manager.handle_content_block_stop(index) {
                            events.push(stop);
                        }
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
                        if !safe.is_empty() {
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

    fn create_thinking_delta_event(&self, index: i32, thinking: &str) -> SseEvent {
        SseEvent::new(
            "content_block_delta",
            json!({"type":"content_block_delta","index":index,"delta":{"type":"thinking_delta","thinking":thinking}}),
        )
    }

    // Handles a tool_use event: closes any open thinking block, flushes
    // buffered text, then emits tool_use block start/delta/stop events.
    fn process_tool_use(
        &mut self,
        tool_use: &crate::kiro_gateway::wire::ToolUseEvent,
    ) -> Vec<SseEvent> {
        let mut events = Vec::new();
        self.state_manager.set_has_tool_use(true);

        if self.thinking_enabled && self.in_thinking_block {
            if let Some(end_pos) = find_real_thinking_end_tag_at_buffer_end(&self.thinking_buffer) {
                let thinking = self.thinking_buffer[..end_pos].to_string();
                if !thinking.is_empty() {
                    if let Some(index) = self.thinking_block_index {
                        events.push(self.create_thinking_delta_event(index, &thinking));
                    }
                }

                self.in_thinking_block = false;
                self.thinking_extracted = true;

                if let Some(index) = self.thinking_block_index {
                    events.push(self.create_thinking_delta_event(index, ""));
                    if let Some(stop) = self.state_manager.handle_content_block_stop(index) {
                        events.push(stop);
                    }
                }

                let after_pos = end_pos + "</thinking>".len();
                let remaining = self.thinking_buffer[after_pos..].trim_start().to_string();
                self.thinking_buffer.clear();
                if !remaining.is_empty() {
                    events.extend(self.create_text_delta_events(&remaining));
                }
            }
        }

        if self.thinking_enabled
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
        if self.thinking_enabled && !self.thinking_buffer.is_empty() {
            if self.in_thinking_block {
                if let Some(end_pos) =
                    find_real_thinking_end_tag_at_buffer_end(&self.thinking_buffer)
                {
                    let thinking = self.thinking_buffer[..end_pos].to_string();
                    if !thinking.is_empty() {
                        if let Some(index) = self.thinking_block_index {
                            events.push(self.create_thinking_delta_event(index, &thinking));
                        }
                    }

                    if let Some(index) = self.thinking_block_index {
                        events.push(self.create_thinking_delta_event(index, ""));
                        if let Some(stop) = self.state_manager.handle_content_block_stop(index) {
                            events.push(stop);
                        }
                    }

                    let after_pos = end_pos + "</thinking>".len();
                    let remaining = self.thinking_buffer[after_pos..].trim_start().to_string();
                    self.thinking_buffer.clear();
                    self.in_thinking_block = false;
                    self.thinking_extracted = true;
                    if !remaining.is_empty() {
                        events.extend(self.create_text_delta_events(&remaining));
                    }
                } else {
                    if let Some(index) = self.thinking_block_index {
                        events.push(self.create_thinking_delta_event(index, &self.thinking_buffer));
                    }
                    if let Some(index) = self.thinking_block_index {
                        events.push(self.create_thinking_delta_event(index, ""));
                        if let Some(stop) = self.state_manager.handle_content_block_stop(index) {
                            events.push(stop);
                        }
                    }
                }
            } else {
                let buffer_content = self.thinking_buffer.clone();
                events.extend(self.create_text_delta_events(&buffer_content));
            }
            self.thinking_buffer.clear();
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
    ) -> Self {
        Self {
            inner: StreamContext::new_with_thinking(
                model,
                estimated_input_tokens,
                thinking_enabled,
                tool_name_map,
            ),
            event_buffer: Vec::new(),
            estimated_input_tokens,
            initial_events_generated: false,
        }
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

// Characters that indicate a tag is inside a quoted/escaped context
// and should not be treated as a real thinking boundary.
const QUOTE_CHARS: &[u8] = b"`\"'\\";

// Checks whether the byte at `pos` is a quote/escape character.
fn is_quote_char(buffer: &str, pos: usize) -> bool {
    buffer
        .as_bytes()
        .get(pos)
        .map(|value| QUOTE_CHARS.contains(value))
        .unwrap_or(false)
}

// Finds `<thinking>` that is not inside quotes. Skips false positives
// where the tag is adjacent to quote characters.
fn find_real_thinking_start_tag(buffer: &str) -> Option<usize> {
    find_real_tag(buffer, "<thinking>", false)
}

// Finds `</thinking>` followed by `\n\n` (mid-stream boundary).
// Returns None if the double-newline hasn't arrived yet (partial buffer).
fn find_real_thinking_end_tag(buffer: &str) -> Option<usize> {
    const TAG: &str = "</thinking>";
    let mut search_start = 0usize;
    while let Some(pos) = buffer[search_start..].find(TAG) {
        let absolute_pos = search_start + pos;
        let after_pos = absolute_pos + TAG.len();
        if (absolute_pos > 0 && is_quote_char(buffer, absolute_pos - 1))
            || is_quote_char(buffer, after_pos)
        {
            search_start = absolute_pos + 1;
            continue;
        }
        let after_content = &buffer[after_pos..];
        if after_content.len() < 2 {
            return None;
        }
        if after_content.starts_with("\n\n") {
            return Some(absolute_pos);
        }
        search_start = absolute_pos + 1;
    }
    None
}

// Finds `</thinking>` at the end of the buffer (for tool_use or final flush),
// where the double-newline requirement is relaxed to trailing whitespace.
fn find_real_thinking_end_tag_at_buffer_end(buffer: &str) -> Option<usize> {
    const TAG: &str = "</thinking>";
    let mut search_start = 0usize;

    while let Some(pos) = buffer[search_start..].find(TAG) {
        let absolute_pos = search_start + pos;
        let after_pos = absolute_pos + TAG.len();
        if (absolute_pos > 0 && is_quote_char(buffer, absolute_pos - 1))
            || is_quote_char(buffer, after_pos)
        {
            search_start = absolute_pos + 1;
            continue;
        }
        if buffer[after_pos..].trim().is_empty() {
            return Some(absolute_pos);
        }
        search_start = absolute_pos + 1;
    }

    None
}

fn find_real_tag(buffer: &str, tag: &str, require_double_newline_after: bool) -> Option<usize> {
    let mut search_start = 0usize;
    while let Some(pos) = buffer[search_start..].find(tag) {
        let absolute_pos = search_start + pos;
        let after_pos = absolute_pos + tag.len();
        if (absolute_pos > 0 && is_quote_char(buffer, absolute_pos - 1))
            || is_quote_char(buffer, after_pos)
        {
            search_start = absolute_pos + 1;
            continue;
        }
        if require_double_newline_after {
            let after_content = &buffer[after_pos..];
            if after_content.len() < 2 {
                return None;
            }
            if !after_content.starts_with("\n\n") {
                search_start = absolute_pos + 1;
                continue;
            }
        }
        return Some(absolute_pos);
    }
    None
}

pub(super) fn split_inline_thinking_content(
    content: &str,
    thinking_enabled: bool,
) -> Vec<InlineThinkingBlock> {
    if content.is_empty() {
        return Vec::new();
    }
    if !thinking_enabled {
        return vec![InlineThinkingBlock::Text(content.to_string())];
    }

    let Some(start_pos) = find_real_thinking_start_tag(content) else {
        return vec![InlineThinkingBlock::Text(content.to_string())];
    };

    let mut blocks = Vec::new();
    let before = &content[..start_pos];
    if !before.trim().is_empty() {
        blocks.push(InlineThinkingBlock::Text(before.to_string()));
    }

    let mut remaining = &content[start_pos + "<thinking>".len()..];
    if remaining.starts_with('\n') {
        remaining = &remaining[1..];
    }

    let end_pos = if let Some(end_pos) = find_real_thinking_end_tag(remaining) {
        end_pos
    } else if let Some(end_pos) = find_real_thinking_end_tag_at_buffer_end(remaining) {
        end_pos
    } else {
        return vec![InlineThinkingBlock::Text(content.to_string())];
    };

    blocks.push(InlineThinkingBlock::Thinking(remaining[..end_pos].to_string()));

    let after_tag = &remaining[end_pos + "</thinking>".len()..];
    let after_thinking = after_tag.strip_prefix("\n\n").unwrap_or(after_tag);
    if !after_thinking.is_empty() {
        blocks.push(InlineThinkingBlock::Text(after_thinking.to_string()));
    }

    blocks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kiro_gateway::wire::{ContextUsageEvent, Event, MeteringEvent, ToolUseEvent};

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
        let blocks = split_inline_thinking_content(
            "<thinking>\nCount carefully.\n</thinking>\n\nbeta",
            true,
        );

        assert_eq!(blocks, vec![
            InlineThinkingBlock::Thinking("Count carefully.\n".to_string()),
            InlineThinkingBlock::Text("beta".to_string()),
        ]);
    }

    #[test]
    fn text_delta_after_tool_use_restarts_text_block() {
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, false, HashMap::new());
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
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new());
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
        assert!(pos_text_delta.is_some());
        assert!(pos_text_stop.is_some());
        assert!(pos_tool_start.is_some());
        assert!(pos_text_delta.unwrap() < pos_text_stop.unwrap());
        assert!(pos_text_stop.unwrap() < pos_tool_start.unwrap());
        assert!(events.iter().any(|event| {
            event.event == "content_block_delta"
                && event.data["delta"]["type"] == "text_delta"
                && event.data["delta"]["text"] == "有修改："
        }));
    }

    #[test]
    fn tool_use_after_thinking_closes_block_and_filters_end_tag() {
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new());
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
        assert!(pos_thinking_stop.is_some());
        assert!(pos_tool_start.is_some());
        assert!(pos_thinking_stop.unwrap() < pos_tool_start.unwrap());
    }

    #[test]
    fn thinking_strips_leading_newline_across_chunks() {
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new());
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
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new());
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
    fn thinking_with_tool_use_keeps_tool_use_stop_reason() {
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new());
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
    fn buffered_stream_context_rewrites_message_start_input_tokens_from_upstream_context_usage() {
        let mut ctx = BufferedStreamContext::new("claude-sonnet-4-6", 123, false, HashMap::new());
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
        let ctx = StreamContext::new_with_thinking("claude-sonnet-4-6", 123, false, HashMap::new());
        let event = ctx.create_message_start_event();
        assert_eq!(event["message"]["usage"]["input_tokens"], serde_json::json!(62));
        assert_eq!(event["message"]["usage"]["cache_creation_input_tokens"], serde_json::json!(61));
        assert_eq!(event["message"]["usage"]["cache_read_input_tokens"], serde_json::json!(0));
    }

    #[test]
    fn metering_event_accumulates_credit_usage() {
        let mut ctx =
            StreamContext::new_with_thinking("claude-sonnet-4-6", 123, false, HashMap::new());
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
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, false, tool_name_map);
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
}
