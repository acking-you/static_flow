//! Anthropic-compatible SSE stream adapter.
//!
//! Converts Kiro upstream binary events into Anthropic-compatible SSE events,
//! split into focused submodules:
//!
//! ```text
//!  Kiro Event
//!     -> [usage]           input-token resolution + usage JSON
//!     -> [sse_event]       SSE wire value
//!     -> [signature]       synthetic thinking signatures
//!     -> [inline_thinking] <thinking> tag extraction
//!     -> [state]           SSE block state machine
//!     -> [context]         StreamContext / BufferedStreamContext drivers
//! ```

mod context;
mod inline_thinking;
mod signature;
mod sse_event;
mod state;
mod usage;

pub use context::{BufferedStreamContext, StreamContext};
pub use inline_thinking::build_inline_thinking_content_blocks;
pub use sse_event::SseEvent;
pub use state::SseStateManager;
pub use usage::{
    anthropic_usage_json, resolve_input_tokens, resolve_input_tokens_with_threshold,
    KiroInputTokenSource, KIRO_CONTEXT_USAGE_MIN_REQUEST_TOKENS,
};
