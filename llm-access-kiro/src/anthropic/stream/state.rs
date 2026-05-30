//! SSE protocol state machine.
//!
//! Tracks open content blocks and enforces start/delta/stop sequencing, then
//! emits the closing `message_delta` + `message_stop` events.

use std::collections::HashMap;

use serde_json::json;

use super::sse_event::SseEvent;

// Tracks the lifecycle of a single content block (text, thinking, tool_use).
#[derive(Debug, Clone)]
struct BlockState {
    block_type: String,
    started: bool,
    stopped: bool,
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

    /// Whether the block at `index` is currently open and of `expected_type`.
    pub fn is_block_open_of_type(&self, index: i32, expected_type: &str) -> bool {
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

    /// Whether any currently-open block is something other than `thinking`.
    pub fn has_non_thinking_blocks(&self) -> bool {
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
