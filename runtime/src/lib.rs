//! Lightweight native runtime helpers shared by StaticFlow binaries.

/// Request/trace id helpers shared by gateway, backend, and standalone
/// services.
pub mod request_ids;

#[cfg(not(target_arch = "wasm32"))]
/// Native runtime logging helpers with app/access log splitting.
pub mod runtime_logging;
