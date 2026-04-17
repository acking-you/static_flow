//! Gateway proxy runtime.

use std::{sync::Arc, time::Instant};

use async_trait::async_trait;
use pingora_core::{upstreams::peer::HttpPeer, Error, ErrorType::InternalError, Result};
use pingora_http::{RequestHeader, ResponseHeader};
use pingora_proxy::{ProxyHttp, Session};
use static_flow_shared::request_ids::read_or_generate_id;

use crate::{
    access_log::emit_gateway_access_log,
    config::{GatewayConfig, GatewayConfigStore},
};

fn internal_error(message: impl Into<String>) -> pingora_core::BError {
    Error::explain(InternalError, message.into())
}

/// Per-request proxy metadata carried across Pingora filter phases.
#[derive(Debug, Clone)]
pub struct GatewayRequestContext {
    pub(crate) config: GatewayConfig,
    pub(crate) request_id: String,
    pub(crate) trace_id: String,
    pub(crate) remote_addr: String,
    pub(crate) active_upstream: String,
    pub(crate) upstream_addr: String,
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) started_at: Instant,
}

impl GatewayRequestContext {
    pub(crate) fn new(config: GatewayConfig) -> Self {
        let active_upstream = config.active_upstream_name().to_string();
        let upstream_addr = config.active_upstream_addr().unwrap_or("").to_string();
        Self {
            config,
            request_id: "req-pending".to_string(),
            trace_id: "trace-pending".to_string(),
            remote_addr: "-".to_string(),
            active_upstream,
            upstream_addr,
            method: String::new(),
            path: String::new(),
            started_at: Instant::now(),
        }
    }
}

/// Pingora proxy service for the local StaticFlow backend.
pub struct StaticFlowGateway {
    config: Arc<GatewayConfigStore>,
}

impl StaticFlowGateway {
    /// Create one gateway service from loaded config.
    pub fn new(config: Arc<GatewayConfigStore>) -> Self {
        Self {
            config,
        }
    }
}

#[async_trait]
impl ProxyHttp for StaticFlowGateway {
    type CTX = GatewayRequestContext;

    fn new_ctx(&self) -> Self::CTX {
        GatewayRequestContext::new(self.config.snapshot())
    }

    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> Result<bool> {
        ctx.config = self.config.snapshot();
        ctx.active_upstream = ctx.config.active_upstream_name().to_string();
        ctx.upstream_addr = ctx.config.active_upstream_addr().unwrap_or("").to_string();
        let req = session.req_header();
        ctx.request_id = read_or_generate_id(
            req.headers
                .get(ctx.config.request_id_header())
                .and_then(|value| value.to_str().ok()),
            "req",
        );
        ctx.trace_id = read_or_generate_id(
            req.headers
                .get(ctx.config.trace_id_header())
                .and_then(|value| value.to_str().ok()),
            "trace",
        );
        ctx.remote_addr = session
            .client_addr()
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string());
        ctx.method = req.method.as_str().to_string();
        ctx.path = req.uri.path().to_string();
        ctx.started_at = Instant::now();
        Ok(false)
    }

    async fn upstream_peer(
        &self,
        _session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>> {
        ctx.active_upstream = ctx.config.active_upstream_name().to_string();
        ctx.upstream_addr = ctx
            .config
            .active_upstream_addr()
            .map_err(|err| internal_error(err.to_string()))?
            .to_string();

        let mut peer = Box::new(HttpPeer::new(ctx.upstream_addr.as_str(), false, String::new()));
        peer.options.connection_timeout = Some(ctx.config.connect_timeout());
        peer.options.total_connection_timeout = Some(ctx.config.connect_timeout());
        peer.options.read_timeout = Some(ctx.config.read_idle_timeout());
        peer.options.idle_timeout = Some(ctx.config.read_idle_timeout());
        peer.options.write_timeout = Some(ctx.config.write_idle_timeout());
        Ok(peer)
    }

    async fn upstream_request_filter(
        &self,
        session: &mut Session,
        upstream_request: &mut RequestHeader,
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        let request_id_header = ctx.config.request_id_header().to_string();
        let trace_id_header = ctx.config.trace_id_header().to_string();
        upstream_request.insert_header(request_id_header, ctx.request_id.as_str())?;
        upstream_request.insert_header(trace_id_header, ctx.trace_id.as_str())?;

        if ctx.config.add_forwarded_headers() {
            upstream_request.insert_header("x-forwarded-proto", "http")?;
            if let Some(host) = session
                .req_header()
                .headers
                .get("host")
                .and_then(|value| value.to_str().ok())
            {
                upstream_request.insert_header("x-forwarded-host", host)?;
            }
            if let Some(addr) = session.client_addr().and_then(|value| value.as_inet()) {
                upstream_request.insert_header("x-forwarded-for", addr.ip().to_string())?;
            }
        }

        Ok(())
    }

    async fn response_filter(
        &self,
        _session: &mut Session,
        downstream_response: &mut ResponseHeader,
        ctx: &mut Self::CTX,
    ) -> Result<()>
    where
        Self::CTX: Send + Sync,
    {
        let request_id_header = ctx.config.request_id_header().to_string();
        let trace_id_header = ctx.config.trace_id_header().to_string();
        downstream_response.insert_header(request_id_header, ctx.request_id.as_str())?;
        downstream_response.insert_header(trace_id_header, ctx.trace_id.as_str())?;
        Ok(())
    }

    async fn logging(
        &self,
        session: &mut Session,
        _error: Option<&pingora_core::Error>,
        ctx: &mut Self::CTX,
    ) {
        let status = session
            .response_written()
            .map(|resp| resp.status.as_u16())
            .unwrap_or(502);
        emit_gateway_access_log(ctx, &ctx.method, &ctx.path, status, ctx.started_at);
    }
}

#[cfg(test)]
mod tests {
    use super::GatewayRequestContext;
    use crate::config::load_gateway_config_from_str;

    #[test]
    fn proxy_ctx_uses_config_snapshot_for_active_upstream() {
        let config = load_gateway_config_from_str(
            r#"
version: 1
staticflow:
  listen_addr: 127.0.0.1:39180
  request_id_header: x-request-id
  trace_id_header: x-trace-id
  add_forwarded_headers: true
  upstreams:
    blue: 127.0.0.1:39080
    green: 127.0.0.1:39081
  active_upstream: blue
  connect_timeout_ms: 3000
  read_idle_timeout_ms: 1800000
  write_idle_timeout_ms: 1800000
  retry_count: 0
"#,
        )
        .expect("valid config");
        let ctx = GatewayRequestContext::new(config);
        assert_eq!(ctx.active_upstream, "blue");
        assert_eq!(ctx.upstream_addr, "127.0.0.1:39080");
    }
}
