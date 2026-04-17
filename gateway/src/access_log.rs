//! Gateway access log support.

use std::time::Instant;

use crate::proxy::GatewayRequestContext;

/// Emit one gateway access log entry on the dedicated access target.
pub(crate) fn emit_gateway_access_log(
    ctx: &GatewayRequestContext,
    method: &str,
    path: &str,
    status: u16,
    started_at: Instant,
) {
    tracing::info!(
        target: "staticflow_access",
        request_id = %ctx.request_id,
        trace_id = %ctx.trace_id,
        remote_addr = %ctx.remote_addr,
        active_upstream = %ctx.active_upstream,
        upstream_addr = %ctx.upstream_addr,
        method = %method,
        path = %path,
        status,
        elapsed_ms = started_at.elapsed().as_millis(),
        "gateway access"
    );
}
