//! Anthropic-compatible Server-Sent Event value type.

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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::SseEvent;

    #[test]
    fn sse_event_format_is_valid() {
        let event = SseEvent::new("message_start", json!({"type": "message_start"}));
        let sse = event.to_sse_string();
        assert!(sse.starts_with("event: message_start\n"));
        assert!(sse.contains("data: "));
        assert!(sse.ends_with("\n\n"));
    }
}
