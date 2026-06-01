//! Gateway configuration parsing.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::RwLock,
    time::Duration,
};

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
    #[serde(default = "default_downstream_h2c")]
    downstream_h2c: bool,
    upstreams: BTreeMap<String, String>,
    active_upstream: String,
    #[serde(default)]
    routing_policy: Option<GatewayRoutingPolicy>,
    connect_timeout_ms: u64,
    read_idle_timeout_ms: u64,
    write_idle_timeout_ms: u64,
    retry_count: usize,
}

/// Optional request routing policy. When absent, the gateway keeps using
/// `active_upstream` exactly as before.
#[derive(Debug, Clone, Deserialize)]
pub struct GatewayRoutingPolicy {
    #[serde(default)]
    mode: GatewayRoutingMode,
    #[serde(default)]
    backends: Vec<GatewayRoutingBackend>,
}

/// Routing strategy used by [`GatewayRoutingPolicy`].
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GatewayRoutingMode {
    /// Always use `active_upstream`.
    #[default]
    Active,
    /// Always use the first listed backend. This is useful for priority
    /// rollout configs without changing `active_upstream`.
    Priority,
    /// Pick one backend by request-level weighted hashing.
    Weighted,
}

/// One backend candidate in a routing policy.
#[derive(Debug, Clone, Deserialize)]
pub struct GatewayRoutingBackend {
    upstream: String,
    #[serde(default = "default_routing_weight")]
    weight: u32,
}

/// Resolved upstream selected for one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedUpstream {
    /// Upstream name from the config.
    pub name: String,
    /// Socket address for the selected upstream.
    pub addr: String,
}

/// Shared gateway config state that can be reloaded from disk in-process.
#[derive(Debug)]
pub struct GatewayConfigStore {
    path: PathBuf,
    current: RwLock<GatewayConfig>,
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

    /// Whether the cleartext downstream listener should accept HTTP/2
    /// prior-knowledge.
    pub fn downstream_h2c(&self) -> bool {
        self.downstream_h2c
    }

    /// Name of the active upstream slot.
    pub fn active_upstream_name(&self) -> &str {
        &self.active_upstream
    }

    /// Human-readable routing policy name for diagnostics.
    pub fn routing_policy_name(&self) -> &'static str {
        self.routing_policy
            .as_ref()
            .map(|policy| policy.mode.name())
            .unwrap_or("active")
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

    /// Select the upstream for one request. `route_key` is only used by the
    /// weighted policy; callers should provide a request-scoped hash.
    pub fn select_upstream(&self, route_key: u64) -> Result<SelectedUpstream> {
        let Some(policy) = self.routing_policy.as_ref() else {
            return self.selected_active_upstream();
        };

        match policy.mode {
            GatewayRoutingMode::Active => self.selected_active_upstream(),
            GatewayRoutingMode::Priority => {
                let backend = policy
                    .backends
                    .first()
                    .ok_or_else(|| anyhow!("priority routing policy has no backends"))?;
                self.selected_named_upstream(&backend.upstream)
            },
            GatewayRoutingMode::Weighted => {
                let total_weight = policy
                    .backends
                    .iter()
                    .map(|backend| u64::from(backend.weight))
                    .sum::<u64>();
                if total_weight == 0 {
                    return Err(anyhow!("weighted routing policy total weight must be positive"));
                }

                let mut slot = route_key % total_weight;
                for backend in &policy.backends {
                    let weight = u64::from(backend.weight);
                    if slot < weight {
                        return self.selected_named_upstream(&backend.upstream);
                    }
                    slot = slot.saturating_sub(weight);
                }

                // The validation above makes this unreachable, but keeping the
                // error explicit avoids hiding future arithmetic mistakes.
                Err(anyhow!("weighted routing policy failed to select an upstream"))
            },
        }
    }

    fn selected_active_upstream(&self) -> Result<SelectedUpstream> {
        self.selected_named_upstream(&self.active_upstream)
    }

    fn selected_named_upstream(&self, upstream: &str) -> Result<SelectedUpstream> {
        let addr = self
            .upstreams
            .get(upstream)
            .ok_or_else(|| anyhow!("routing upstream `{upstream}` missing from upstreams"))?;
        Ok(SelectedUpstream {
            name: upstream.to_string(),
            addr: addr.clone(),
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

impl GatewayConfigStore {
    /// Load one config file and prepare it for future hot reloads.
    pub fn load(path: &Path) -> Result<Self> {
        let config = load_gateway_config(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            current: RwLock::new(config),
        })
    }

    /// Return the current in-memory snapshot used for new requests.
    pub fn snapshot(&self) -> GatewayConfig {
        self.current
            .read()
            .expect("gateway config store poisoned")
            .clone()
    }

    /// Reload the config from disk and atomically publish it for new requests.
    pub fn reload(&self) -> Result<GatewayConfig> {
        let next = load_gateway_config(&self.path)?;
        *self.current.write().expect("gateway config store poisoned") = next.clone();
        Ok(next)
    }

    /// Path of the backing YAML config file.
    pub fn path(&self) -> &Path {
        &self.path
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
    validate_routing_policy(&config)?;

    Ok(config)
}

fn default_downstream_h2c() -> bool {
    true
}

fn default_routing_weight() -> u32 {
    1
}

fn validate_routing_policy(config: &GatewayConfig) -> Result<()> {
    let Some(policy) = config.routing_policy.as_ref() else {
        return Ok(());
    };

    if policy.mode != GatewayRoutingMode::Active && policy.backends.is_empty() {
        return Err(anyhow!(
            "routing_policy.backends must not be empty when mode is `{}`",
            policy.mode.name()
        ));
    }

    for backend in &policy.backends {
        if backend.upstream.trim().is_empty() {
            return Err(anyhow!("routing_policy backend upstream must not be empty"));
        }
        if !config.upstreams.contains_key(&backend.upstream) {
            return Err(anyhow!(
                "routing_policy upstream `{}` missing from upstreams",
                backend.upstream
            ));
        }
    }

    if policy.mode == GatewayRoutingMode::Weighted {
        let total_weight = policy
            .backends
            .iter()
            .map(|backend| u64::from(backend.weight))
            .sum::<u64>();
        if total_weight == 0 {
            return Err(anyhow!(
                "routing_policy weighted mode requires at least one positive weight"
            ));
        }
    }

    Ok(())
}

impl GatewayRoutingMode {
    fn name(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Priority => "priority",
            Self::Weighted => "weighted",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{load_gateway_config_from_str, GatewayConfigStore};

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
        assert!(cfg.downstream_h2c(), "h2c should be enabled by default for existing configs");
        assert_eq!(cfg.routing_policy_name(), "active");
    }

    #[test]
    fn parse_gateway_config_allows_disabling_downstream_h2c() {
        let cfg = load_gateway_config_from_str(
            r#"
version: 1
staticflow:
  listen_addr: 127.0.0.1:39180
  request_id_header: x-request-id
  trace_id_header: x-trace-id
  add_forwarded_headers: true
  downstream_h2c: false
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
        assert!(!cfg.downstream_h2c());
    }

    #[test]
    fn weighted_routing_policy_selects_by_configured_weights() {
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
    llm_external: 127.0.0.1:39082
  active_upstream: green
  routing_policy:
    mode: weighted
    backends:
      - upstream: green
        weight: 80
      - upstream: llm_external
        weight: 20
  connect_timeout_ms: 3000
  read_idle_timeout_ms: 1800000
  write_idle_timeout_ms: 1800000
  retry_count: 0
"#,
        )
        .expect("valid weighted config");

        assert_eq!(cfg.routing_policy_name(), "weighted");
        assert_eq!(cfg.select_upstream(0).expect("select").name, "green");
        assert_eq!(cfg.select_upstream(79).expect("select").name, "green");
        assert_eq!(cfg.select_upstream(80).expect("select").name, "llm_external");
        assert_eq!(cfg.select_upstream(99).expect("select").name, "llm_external");
        assert_eq!(cfg.select_upstream(100).expect("select").name, "green");
    }

    #[test]
    fn priority_routing_policy_selects_first_backend() {
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
    llm_external: 127.0.0.1:39082
  active_upstream: green
  routing_policy:
    mode: priority
    backends:
      - upstream: llm_external
      - upstream: green
  connect_timeout_ms: 3000
  read_idle_timeout_ms: 1800000
  write_idle_timeout_ms: 1800000
  retry_count: 0
"#,
        )
        .expect("valid priority config");

        assert_eq!(cfg.routing_policy_name(), "priority");
        assert_eq!(cfg.select_upstream(0).expect("select").name, "llm_external");
        assert_eq!(cfg.select_upstream(99).expect("select").name, "llm_external");
    }

    #[test]
    fn routing_policy_rejects_unknown_upstream() {
        let err = load_gateway_config_from_str(
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
  active_upstream: green
  routing_policy:
    mode: weighted
    backends:
      - upstream: missing
        weight: 20
  connect_timeout_ms: 3000
  read_idle_timeout_ms: 1800000
  write_idle_timeout_ms: 1800000
  retry_count: 0
"#,
        )
        .expect_err("unknown upstream should be rejected");

        assert!(err.to_string().contains("missing from upstreams"), "unexpected error: {err:#}");
    }

    #[test]
    fn gateway_config_store_reload_switches_active_upstream() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("gateway.yaml");
        fs::write(
            &path,
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
        .expect("write config");

        let store = GatewayConfigStore::load(&path).expect("load config store");
        assert_eq!(store.snapshot().active_upstream_name(), "blue");

        fs::write(
            &path,
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
  active_upstream: green
  connect_timeout_ms: 3000
  read_idle_timeout_ms: 1800000
  write_idle_timeout_ms: 1800000
  retry_count: 0
"#,
        )
        .expect("write updated config");

        store.reload().expect("reload config");
        assert_eq!(store.snapshot().active_upstream_name(), "green");
    }

    #[test]
    fn gateway_config_store_reload_keeps_previous_config_on_invalid_update() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("gateway.yaml");
        fs::write(
            &path,
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
        .expect("write config");

        let store = GatewayConfigStore::load(&path).expect("load config store");
        fs::write(
            &path,
            r#"
version: 1
staticflow:
  listen_addr: 127.0.0.1:39180
  request_id_header: x-request-id
  trace_id_header: x-trace-id
  add_forwarded_headers: true
  upstreams:
    blue: 127.0.0.1:39080
  active_upstream: green
  connect_timeout_ms: 3000
  read_idle_timeout_ms: 1800000
  write_idle_timeout_ms: 1800000
  retry_count: 0
"#,
        )
        .expect("write invalid config");

        assert!(store.reload().is_err());
        assert_eq!(store.snapshot().active_upstream_name(), "blue");
    }
}
