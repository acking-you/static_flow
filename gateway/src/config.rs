//! Gateway configuration parsing.

use std::{collections::BTreeMap, fs, path::Path, time::Duration};

use anyhow::{anyhow, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct GatewayFile {
    staticflow: GatewayConfig,
}

/// StaticFlow-specific gateway settings layered on top of Pingora's YAML.
#[derive(Debug, Clone, Deserialize)]
pub struct GatewayConfig {
    listen_addr: String,
    request_id_header: String,
    trace_id_header: String,
    add_forwarded_headers: bool,
    upstreams: BTreeMap<String, String>,
    active_upstream: String,
    connect_timeout_ms: u64,
    read_idle_timeout_ms: u64,
    write_idle_timeout_ms: u64,
    retry_count: usize,
}

impl GatewayConfig {
    /// Effective listen address for the local gateway.
    pub fn listen_addr(&self) -> &str {
        &self.listen_addr
    }

    /// Header name used to propagate request ids.
    pub fn request_id_header(&self) -> &str {
        &self.request_id_header
    }

    /// Header name used to propagate trace ids.
    pub fn trace_id_header(&self) -> &str {
        &self.trace_id_header
    }

    /// Whether `x-forwarded-*` headers should be added upstream.
    pub fn add_forwarded_headers(&self) -> bool {
        self.add_forwarded_headers
    }

    /// Name of the active upstream slot.
    pub fn active_upstream_name(&self) -> &str {
        &self.active_upstream
    }

    /// Resolved socket address for the active upstream slot.
    pub fn active_upstream_addr(&self) -> Result<&str> {
        self.upstreams
            .get(&self.active_upstream)
            .map(String::as_str)
            .ok_or_else(|| {
                anyhow!("active_upstream `{}` missing from upstreams", self.active_upstream)
            })
    }

    /// Connect timeout in milliseconds.
    pub fn connect_timeout_ms(&self) -> u64 {
        self.connect_timeout_ms
    }

    /// Connect timeout as a duration.
    pub fn connect_timeout(&self) -> Duration {
        Duration::from_millis(self.connect_timeout_ms)
    }

    /// Read idle timeout in milliseconds.
    pub fn read_idle_timeout_ms(&self) -> u64 {
        self.read_idle_timeout_ms
    }

    /// Read idle timeout as a duration.
    pub fn read_idle_timeout(&self) -> Duration {
        Duration::from_millis(self.read_idle_timeout_ms)
    }

    /// Write idle timeout in milliseconds.
    pub fn write_idle_timeout_ms(&self) -> u64 {
        self.write_idle_timeout_ms
    }

    /// Write idle timeout as a duration.
    pub fn write_idle_timeout(&self) -> Duration {
        Duration::from_millis(self.write_idle_timeout_ms)
    }

    /// Maximum number of retry attempts for retryable upstream failures.
    pub fn retry_count(&self) -> usize {
        self.retry_count
    }
}

/// Load gateway settings from one YAML file.
pub fn load_gateway_config(path: &Path) -> Result<GatewayConfig> {
    let raw = fs::read_to_string(path)?;
    load_gateway_config_from_str(&raw)
}

/// Parse gateway settings from raw YAML content.
pub fn load_gateway_config_from_str(raw: &str) -> Result<GatewayConfig> {
    let file: GatewayFile = serde_yaml::from_str(raw)?;
    let config = file.staticflow;

    if config.listen_addr.trim().is_empty() {
        return Err(anyhow!("listen_addr must not be empty"));
    }
    if config.request_id_header.trim().is_empty() {
        return Err(anyhow!("request_id_header must not be empty"));
    }
    if config.trace_id_header.trim().is_empty() {
        return Err(anyhow!("trace_id_header must not be empty"));
    }
    for slot in ["blue", "green"] {
        if !config.upstreams.contains_key(slot) {
            return Err(anyhow!("upstreams must contain `{slot}`"));
        }
    }
    if !matches!(config.active_upstream.as_str(), "blue" | "green") {
        return Err(anyhow!("active_upstream must be `blue` or `green`"));
    }
    config.active_upstream_addr()?;

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::load_gateway_config_from_str;

    #[test]
    fn parse_gateway_config_accepts_valid_blue_green_setup() {
        let cfg = load_gateway_config_from_str(
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
        assert_eq!(cfg.active_upstream, "blue");
        assert_eq!(cfg.upstreams["green"], "127.0.0.1:39081");
    }
}
