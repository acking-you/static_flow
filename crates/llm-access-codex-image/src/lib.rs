//! Codex image gateway.
//!
//! Provides the shared [`gateway::CodexImageGateway`] dispatch engine plus its
//! routing, concurrency, logging, and request-validation building blocks. It is
//! consumed both by this crate's standalone `serve` binary and, in-process, by
//! the main `llm-access` Codex API service (see [`dispatch::ImageGatewayMode`]).

/// Image-route eligibility and failover classification.
pub mod dispatch;
/// Shared image gateway implementation used by both supported entrypoints.
pub mod gateway;
/// Independent per-account image concurrency limiter.
pub mod limiter;
/// Structured request log event construction.
pub mod logging;
/// JSON request validation and path normalization for the public image API.
pub mod request;
/// Process-local time and lock helpers shared across the gateway modules.
mod util;
