# Pingora LLM Routing Local Canary Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add reloadable local LLM canary routing to the existing Pingora gateway so selected LLM requests can go to local `llm-access` while all other traffic keeps using the active StaticFlow backend.

**Architecture:** Extend gateway config with optional `llm_routing`, implement route selection in a pure helper module, then wire the selected upstream into Pingora request context and access logs. Default config keeps current blue/green behavior, and rollback is config-only by setting `llm_routing.enabled: false`.

**Tech Stack:** Rust, Pingora, serde/serde_yaml, sha2, existing StaticFlow gateway config reload, existing shell scripts.

---

## File Structure

- Modify `gateway/Cargo.toml`
  - Add `sha2 = { workspace = true }` for bearer token SHA-256 matching.
- Modify `gateway/src/lib.rs`
  - Export a new private `llm_routing` module.
- Modify `gateway/src/config.rs`
  - Add optional `llm_routing` config parsing and validation.
  - Add methods used by route selection.
- Create `gateway/src/llm_routing.rs`
  - Own all path/token matching and route decision logic.
  - Keep this pure and directly unit-testable.
- Modify `gateway/src/proxy.rs`
  - Evaluate route selection once per request after reading the request header.
  - Use selected upstream in `upstream_peer`.
- Modify `gateway/src/access_log.rs`
  - Log selected upstream and LLM route reason.
- Modify `conf/pingora/staticflow-gateway.yaml`
  - Add `llm_access_local` upstream and disabled `llm_routing` block.
- Create `scripts/start_llm_access_local.sh`
  - Start local `llm-access` using `/mnt/wsl/data4tb/static-flow-data/llm-access-local`.
- Create `scripts/verify_pingora_llm_local_canary.sh`
  - Read-only curl checks for health, non-LLM fallback, and test-key LLM routing behavior.
- Modify `docs/superpowers/specs/2026-04-30-pingora-llm-routing-local-canary-design.md`
  - Add implementation status after code lands.

---

### Task 1: Add Gateway Config Support For Optional LLM Routing

**Files:**
- Modify: `gateway/src/config.rs`
- Test: `gateway/src/config.rs`

- [ ] **Step 1: Add failing config tests**

Append these tests inside `#[cfg(test)] mod tests` in `gateway/src/config.rs`:

```rust
    #[test]
    fn parse_gateway_config_defaults_llm_routing_to_disabled() {
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

        assert!(!cfg.llm_routing().enabled());
        assert_eq!(cfg.llm_routing().path_prefixes(), super::default_llm_path_prefixes().as_slice());
    }

    #[test]
    fn parse_gateway_config_accepts_disabled_llm_routing_with_extra_upstream() {
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
    llm_access_local: 127.0.0.1:19080
  active_upstream: green
  llm_routing:
    enabled: false
    upstream: llm_access_local
    bearer_token_sha256_allowlist:
      - 2bb80d537b1da3e38bd30361aa855686bde0ba545d2edc419e2ed0868858c55
  connect_timeout_ms: 3000
  read_idle_timeout_ms: 1800000
  write_idle_timeout_ms: 1800000
  retry_count: 0
"#,
        )
        .expect("valid config");

        assert!(!cfg.llm_routing().enabled());
        assert_eq!(cfg.upstream_addr("llm_access_local").expect("llm upstream"), "127.0.0.1:19080");
    }

    #[test]
    fn enabled_llm_routing_requires_existing_upstream() {
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
  llm_routing:
    enabled: true
    upstream: llm_access_local
    bearer_token_sha256_allowlist:
      - 2bb80d537b1da3e38bd30361aa855686bde0ba545d2edc419e2ed0868858c55
  connect_timeout_ms: 3000
  read_idle_timeout_ms: 1800000
  write_idle_timeout_ms: 1800000
  retry_count: 0
"#,
        )
        .expect_err("missing llm upstream must fail");

        assert!(err.to_string().contains("llm_routing upstream"));
    }

    #[test]
    fn enabled_llm_routing_rejects_invalid_token_hash() {
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
    llm_access_local: 127.0.0.1:19080
  active_upstream: green
  llm_routing:
    enabled: true
    upstream: llm_access_local
    bearer_token_sha256_allowlist:
      - not-a-sha256
  connect_timeout_ms: 3000
  read_idle_timeout_ms: 1800000
  write_idle_timeout_ms: 1800000
  retry_count: 0
"#,
        )
        .expect_err("invalid token hash must fail");

        assert!(err.to_string().contains("bearer_token_sha256_allowlist"));
    }
```

- [ ] **Step 2: Run config tests and verify they fail**

Run:

```bash
pgrep -af 'cargo|rustc|trunk|ld|lld|mold|c\+\+' || true
ps -eo pid,ppid,stat,rss,etime,cmd --sort=-rss | head -40
cargo test -p staticflow-pingora-gateway --jobs 1 config::tests::parse_gateway_config_defaults_llm_routing_to_disabled -- --nocapture
```

Expected:

- The test fails to compile because `llm_routing`, `LlmRoutingConfig`, or `upstream_addr` does not exist yet.

- [ ] **Step 3: Add config structs and methods**

In `gateway/src/config.rs`, update the config structs and methods with this code shape:

```rust
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
    llm_routing: LlmRoutingConfig,
    connect_timeout_ms: u64,
    read_idle_timeout_ms: u64,
    write_idle_timeout_ms: u64,
    retry_count: usize,
}

/// Optional LLM-specific route split configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct LlmRoutingConfig {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    upstream: String,
    #[serde(default = "default_llm_path_prefixes")]
    path_prefixes: Vec<String>,
    #[serde(default)]
    bearer_token_sha256_allowlist: Vec<String>,
}

impl Default for LlmRoutingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            upstream: String::new(),
            path_prefixes: default_llm_path_prefixes(),
            bearer_token_sha256_allowlist: Vec::new(),
        }
    }
}
```

Add methods to the existing `impl GatewayConfig`:

```rust
    /// Return one named upstream socket address.
    pub fn upstream_addr(&self, name: &str) -> Result<&str> {
        self.upstreams
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| anyhow!("upstream `{name}` missing from upstreams"))
    }

    /// Optional LLM routing configuration.
    pub fn llm_routing(&self) -> &LlmRoutingConfig {
        &self.llm_routing
    }
```

Change `active_upstream_addr()` to call `upstream_addr()`:

```rust
    pub fn active_upstream_addr(&self) -> Result<&str> {
        self.upstream_addr(&self.active_upstream)
            .map_err(|_| anyhow!("active_upstream `{}` missing from upstreams", self.active_upstream))
    }
```

Add methods for `LlmRoutingConfig`:

```rust
impl LlmRoutingConfig {
    /// Whether LLM routing is enabled.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Configured upstream name for LLM canary traffic.
    pub fn upstream(&self) -> &str {
        &self.upstream
    }

    /// Path prefixes owned by LLM routing.
    pub fn path_prefixes(&self) -> &[String] {
        &self.path_prefixes
    }

    /// SHA-256 hex digests allowed to route to the LLM upstream.
    pub fn bearer_token_sha256_allowlist(&self) -> &[String] {
        &self.bearer_token_sha256_allowlist
    }
}
```

Add the default path helper near `default_downstream_h2c()`:

```rust
fn default_llm_path_prefixes() -> Vec<String> {
    [
        "/v1/",
        "/cc/v1/",
        "/api/llm-gateway/",
        "/api/kiro-gateway/",
        "/api/codex-gateway/",
        "/api/llm-access/",
    ]
    .into_iter()
    .map(ToOwned::to_owned)
    .collect()
}
```

Add validation inside `load_gateway_config_from_str()` after active upstream validation:

```rust
    validate_llm_routing(&config)?;
```

Add the validation helpers:

```rust
fn validate_llm_routing(config: &GatewayConfig) -> Result<()> {
    let routing = config.llm_routing();
    if routing.path_prefixes().iter().any(|prefix| prefix.trim().is_empty()) {
        return Err(anyhow!("llm_routing path_prefixes must not contain empty values"));
    }
    for hash in routing.bearer_token_sha256_allowlist() {
        if !is_sha256_hex(hash) {
            return Err(anyhow!(
                "llm_routing bearer_token_sha256_allowlist contains invalid SHA-256 hex digest"
            ));
        }
    }
    if routing.enabled() {
        if routing.upstream().trim().is_empty() {
            return Err(anyhow!("llm_routing upstream must not be empty when enabled"));
        }
        config
            .upstream_addr(routing.upstream())
            .map_err(|_| anyhow!("llm_routing upstream `{}` missing from upstreams", routing.upstream()))?;
        if routing.bearer_token_sha256_allowlist().is_empty() {
            return Err(anyhow!(
                "llm_routing bearer_token_sha256_allowlist must not be empty when enabled"
            ));
        }
    }
    Ok(())
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.as_bytes().iter().all(|byte| byte.is_ascii_hexdigit())
}
```

- [ ] **Step 4: Run config tests**

Run:

```bash
cargo test -p staticflow-pingora-gateway --jobs 1 config::tests -- --nocapture
```

Expected:

- Existing tests pass.
- New config tests pass.

- [ ] **Step 5: Commit config support**

Run:

```bash
rustfmt gateway/src/config.rs
cargo test -p staticflow-pingora-gateway --jobs 1 config::tests -- --nocapture
cargo clippy -p staticflow-pingora-gateway --jobs 1 -- -D warnings
git diff --check
git add gateway/src/config.rs
git commit -m "feat: add gateway llm routing config"
```

---

### Task 2: Add Pure LLM Route Decision Module

**Files:**
- Create: `gateway/src/llm_routing.rs`
- Modify: `gateway/src/lib.rs`
- Modify: `gateway/Cargo.toml`
- Test: `gateway/src/llm_routing.rs`

- [ ] **Step 1: Add the new module and failing tests**

Add to `gateway/src/lib.rs`:

```rust
mod llm_routing;
```

Add to `gateway/Cargo.toml` dependencies:

```toml
sha2 = { workspace = true }
```

Create `gateway/src/llm_routing.rs` with tests first:

```rust
//! LLM-specific gateway route selection.

#[cfg(test)]
mod tests {
    use super::{decide_route, RouteReason, RouteTarget};
    use crate::config::load_gateway_config_from_str;

    fn config(enabled: bool) -> crate::config::GatewayConfig {
        load_gateway_config_from_str(&format!(
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
    llm_access_local: 127.0.0.1:19080
  active_upstream: green
  llm_routing:
    enabled: {enabled}
    upstream: llm_access_local
    bearer_token_sha256_allowlist:
      - 2bb80d537b1da3e38bd30361aa855686bde0ba545d2edc419e2ed0868858c55
  connect_timeout_ms: 3000
  read_idle_timeout_ms: 1800000
  write_idle_timeout_ms: 1800000
  retry_count: 0
"#
        ))
        .expect("valid config")
    }

    #[test]
    fn disabled_routing_selects_active_staticflow_backend() {
        let decision = decide_route(&config(false), "/v1/chat/completions", Some("Bearer secret"))
            .expect("route decision");

        assert_eq!(decision.route_target, RouteTarget::StaticFlow);
        assert_eq!(decision.reason, RouteReason::Disabled);
        assert_eq!(decision.selected_upstream, "green");
        assert_eq!(decision.selected_upstream_addr, "127.0.0.1:39081");
    }

    #[test]
    fn matching_path_and_token_selects_llm_access() {
        let decision = decide_route(&config(true), "/v1/chat/completions", Some("Bearer secret"))
            .expect("route decision");

        assert_eq!(decision.route_target, RouteTarget::LlmAccess);
        assert_eq!(decision.reason, RouteReason::TokenMatch);
        assert_eq!(decision.selected_upstream, "llm_access_local");
        assert_eq!(decision.selected_upstream_addr, "127.0.0.1:19080");
    }

    #[test]
    fn non_llm_path_selects_active_staticflow_backend() {
        let decision = decide_route(&config(true), "/api/articles", Some("Bearer secret"))
            .expect("route decision");

        assert_eq!(decision.route_target, RouteTarget::StaticFlow);
        assert_eq!(decision.reason, RouteReason::PathMiss);
        assert_eq!(decision.selected_upstream, "green");
    }

    #[test]
    fn missing_bearer_selects_active_staticflow_backend() {
        let decision = decide_route(&config(true), "/v1/chat/completions", None)
            .expect("route decision");

        assert_eq!(decision.route_target, RouteTarget::StaticFlow);
        assert_eq!(decision.reason, RouteReason::MissingBearer);
    }

    #[test]
    fn non_matching_token_selects_active_staticflow_backend() {
        let decision = decide_route(&config(true), "/v1/chat/completions", Some("Bearer wrong"))
            .expect("route decision");

        assert_eq!(decision.route_target, RouteTarget::StaticFlow);
        assert_eq!(decision.reason, RouteReason::TokenMiss);
    }
}
```

- [ ] **Step 2: Run route decision tests and verify they fail**

Run:

```bash
pgrep -af 'cargo|rustc|trunk|ld|lld|mold|c\+\+' || true
ps -eo pid,ppid,stat,rss,etime,cmd --sort=-rss | head -40
cargo test -p staticflow-pingora-gateway --jobs 1 llm_routing::tests -- --nocapture
```

Expected:

- The test fails to compile because `decide_route`, `RouteReason`, and `RouteTarget` are not implemented.

- [ ] **Step 3: Implement route decision logic**

Replace `gateway/src/llm_routing.rs` with:

```rust
//! LLM-specific gateway route selection.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::config::GatewayConfig;

/// Selected high-level routing target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RouteTarget {
    /// Route to the active StaticFlow backend.
    StaticFlow,
    /// Route to the configured local llm-access service.
    LlmAccess,
}

impl RouteTarget {
    /// Stable access-log value.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::StaticFlow => "staticflow",
            Self::LlmAccess => "llm_access",
        }
    }
}

/// Why a route target was selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RouteReason {
    /// LLM routing is disabled.
    Disabled,
    /// Request path is not LLM-owned.
    PathMiss,
    /// Request path is LLM-owned but no bearer token was present.
    MissingBearer,
    /// Bearer token hash was not allowlisted.
    TokenMiss,
    /// Bearer token hash was allowlisted.
    TokenMatch,
}

impl RouteReason {
    /// Stable access-log value.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::PathMiss => "path_miss",
            Self::MissingBearer => "missing_bearer",
            Self::TokenMiss => "token_miss",
            Self::TokenMatch => "token_match",
        }
    }
}

/// Complete route decision for one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RouteDecision {
    /// Selected upstream name.
    pub(crate) selected_upstream: String,
    /// Selected upstream socket address.
    pub(crate) selected_upstream_addr: String,
    /// Selected logical target.
    pub(crate) route_target: RouteTarget,
    /// Reason for the decision.
    pub(crate) reason: RouteReason,
}

/// Decide where one request should be proxied.
pub(crate) fn decide_route(
    config: &GatewayConfig,
    path: &str,
    authorization: Option<&str>,
) -> Result<RouteDecision> {
    let routing = config.llm_routing();
    if !routing.enabled() {
        return staticflow_decision(config, RouteReason::Disabled);
    }
    if !routing.path_prefixes().iter().any(|prefix| path.starts_with(prefix)) {
        return staticflow_decision(config, RouteReason::PathMiss);
    }
    let Some(token) = extract_bearer_token(authorization) else {
        return staticflow_decision(config, RouteReason::MissingBearer);
    };
    let hash = sha256_hex(token);
    if !routing
        .bearer_token_sha256_allowlist()
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(&hash))
    {
        return staticflow_decision(config, RouteReason::TokenMiss);
    }
    let addr = config
        .upstream_addr(routing.upstream())
        .with_context(|| format!("failed to resolve llm upstream `{}`", routing.upstream()))?;
    Ok(RouteDecision {
        selected_upstream: routing.upstream().to_string(),
        selected_upstream_addr: addr.to_string(),
        route_target: RouteTarget::LlmAccess,
        reason: RouteReason::TokenMatch,
    })
}

fn staticflow_decision(config: &GatewayConfig, reason: RouteReason) -> Result<RouteDecision> {
    Ok(RouteDecision {
        selected_upstream: config.active_upstream_name().to_string(),
        selected_upstream_addr: config.active_upstream_addr()?.to_string(),
        route_target: RouteTarget::StaticFlow,
        reason,
    })
}

fn extract_bearer_token(authorization: Option<&str>) -> Option<&str> {
    authorization
        .map(str::trim)
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn sha256_hex(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
```

Keep the tests from Step 1 at the bottom of the same file.

- [ ] **Step 4: Run route decision tests**

Run:

```bash
cargo test -p staticflow-pingora-gateway --jobs 1 llm_routing::tests -- --nocapture
```

Expected:

- All `llm_routing::tests` pass.

- [ ] **Step 5: Commit route decision module**

Run:

```bash
rustfmt gateway/src/lib.rs gateway/src/llm_routing.rs
cargo test -p staticflow-pingora-gateway --jobs 1 llm_routing::tests -- --nocapture
cargo clippy -p staticflow-pingora-gateway --jobs 1 -- -D warnings
git diff --check
git add gateway/Cargo.toml Cargo.lock gateway/src/lib.rs gateway/src/llm_routing.rs
git commit -m "feat: add gateway llm route decisions"
```

---

### Task 3: Wire Selected Upstream Into Pingora Proxy And Logs

**Files:**
- Modify: `gateway/src/proxy.rs`
- Modify: `gateway/src/access_log.rs`
- Test: `gateway/src/proxy.rs`

- [ ] **Step 1: Add failing proxy context tests**

In `gateway/src/proxy.rs`, update the test module imports:

```rust
use super::GatewayRequestContext;
use crate::{
    config::load_gateway_config_from_str,
    llm_routing::{RouteReason, RouteTarget},
};
```

Add this test:

```rust
    #[test]
    fn proxy_ctx_records_selected_llm_upstream() {
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
    llm_access_local: 127.0.0.1:19080
  active_upstream: green
  llm_routing:
    enabled: true
    upstream: llm_access_local
    bearer_token_sha256_allowlist:
      - 2bb80d537b1da3e38bd30361aa855686bde0ba545d2edc419e2ed0868858c55
  connect_timeout_ms: 3000
  read_idle_timeout_ms: 1800000
  write_idle_timeout_ms: 1800000
  retry_count: 0
"#,
        )
        .expect("valid config");
        let mut ctx = GatewayRequestContext::new(config.clone());

        ctx.apply_route_decision("/v1/chat/completions", Some("Bearer secret"))
            .expect("apply route decision");

        assert_eq!(ctx.active_upstream, "green");
        assert_eq!(ctx.selected_upstream, "llm_access_local");
        assert_eq!(ctx.upstream_addr, "127.0.0.1:19080");
        assert_eq!(ctx.route_target, RouteTarget::LlmAccess);
        assert_eq!(ctx.route_reason, RouteReason::TokenMatch);
    }
```

- [ ] **Step 2: Run proxy test and verify it fails**

Run:

```bash
pgrep -af 'cargo|rustc|trunk|ld|lld|mold|c\+\+' || true
ps -eo pid,ppid,stat,rss,etime,cmd --sort=-rss | head -40
cargo test -p staticflow-pingora-gateway --jobs 1 proxy::tests::proxy_ctx_records_selected_llm_upstream -- --nocapture
```

Expected:

- The test fails to compile because context selected-upstream fields and `apply_route_decision` do not exist.

- [ ] **Step 3: Extend request context and proxy selection**

In `gateway/src/proxy.rs`, update imports:

```rust
use crate::{
    access_log::emit_gateway_access_log,
    config::{GatewayConfig, GatewayConfigStore},
    llm_routing::{decide_route, RouteReason, RouteTarget},
};
```

Add fields to `GatewayRequestContext`:

```rust
    pub(crate) selected_upstream: String,
    pub(crate) route_target: RouteTarget,
    pub(crate) route_reason: RouteReason,
```

Update `GatewayRequestContext::new()`:

```rust
        let active_upstream = config.active_upstream_name().to_string();
        let upstream_addr = config.active_upstream_addr().unwrap_or("").to_string();
        Self {
            config,
            request_id: "req-pending".to_string(),
            trace_id: "trace-pending".to_string(),
            remote_addr: "-".to_string(),
            active_upstream: active_upstream.clone(),
            selected_upstream: active_upstream,
            upstream_addr,
            route_target: RouteTarget::StaticFlow,
            route_reason: RouteReason::Disabled,
            method: String::new(),
            path: String::new(),
            started_at: Instant::now(),
        }
```

Add this method on `GatewayRequestContext`:

```rust
    pub(crate) fn apply_route_decision(
        &mut self,
        path: &str,
        authorization: Option<&str>,
    ) -> anyhow::Result<()> {
        self.active_upstream = self.config.active_upstream_name().to_string();
        let decision = decide_route(&self.config, path, authorization)?;
        self.selected_upstream = decision.selected_upstream;
        self.upstream_addr = decision.selected_upstream_addr;
        self.route_target = decision.route_target;
        self.route_reason = decision.reason;
        Ok(())
    }
```

Update `request_filter()` after setting `ctx.path`:

```rust
        let authorization = req
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok());
        let path = ctx.path.clone();
        ctx.apply_route_decision(&path, authorization)
            .map_err(|err| internal_error(err.to_string()))?;
```

Update `upstream_peer()` so it does not overwrite the selected route:

```rust
    async fn upstream_peer(
        &self,
        _session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>> {
        if ctx.upstream_addr.is_empty() {
            ctx.apply_route_decision(ctx.path.as_str(), None)
                .map_err(|err| internal_error(err.to_string()))?;
        }

        let mut peer = Box::new(HttpPeer::new(ctx.upstream_addr.as_str(), false, String::new()));
        peer.options.connection_timeout = Some(ctx.config.connect_timeout());
        peer.options.total_connection_timeout = Some(ctx.config.connect_timeout());
        peer.options.read_timeout = Some(ctx.config.read_idle_timeout());
        peer.options.idle_timeout = Some(ctx.config.read_idle_timeout());
        peer.options.write_timeout = Some(ctx.config.write_idle_timeout());
        Ok(peer)
    }
```

If the borrow checker rejects `ctx.apply_route_decision(ctx.path.as_str(), None)`, use:

```rust
            let path = ctx.path.clone();
            ctx.apply_route_decision(&path, None)
                .map_err(|err| internal_error(err.to_string()))?;
```

- [ ] **Step 4: Add selected route fields to access log**

In `gateway/src/access_log.rs`, add fields to the existing `tracing::info!` call:

```rust
        selected_upstream = %ctx.selected_upstream,
        route_target = %ctx.route_target.as_str(),
        route_reason = %ctx.route_reason.as_str(),
```

Keep existing `active_upstream` and `upstream_addr` fields for compatibility. After Task 3, `upstream_addr` is the actual selected upstream address.

- [ ] **Step 5: Run proxy tests**

Run:

```bash
cargo test -p staticflow-pingora-gateway --jobs 1 proxy::tests -- --nocapture
```

Expected:

- Existing proxy context test passes.
- New selected-upstream test passes.

- [ ] **Step 6: Commit proxy integration**

Run:

```bash
rustfmt gateway/src/proxy.rs gateway/src/access_log.rs
cargo test -p staticflow-pingora-gateway --jobs 1 proxy::tests -- --nocapture
cargo clippy -p staticflow-pingora-gateway --jobs 1 -- -D warnings
git diff --check
git add gateway/src/proxy.rs gateway/src/access_log.rs
git commit -m "feat: route selected llm traffic in gateway"
```

---

### Task 4: Add Disabled Default Config And Local Verification Scripts

**Files:**
- Modify: `conf/pingora/staticflow-gateway.yaml`
- Create: `scripts/start_llm_access_local.sh`
- Create: `scripts/verify_pingora_llm_local_canary.sh`
- Test: scripts syntax and gateway config tests

- [ ] **Step 1: Update gateway YAML with disabled LLM routing**

Modify `conf/pingora/staticflow-gateway.yaml`:

```yaml
  upstreams:
    blue: 127.0.0.1:39080
    green: 127.0.0.1:39081
    llm_access_local: 127.0.0.1:19080
  active_upstream: green
  llm_routing:
    enabled: false
    upstream: llm_access_local
    path_prefixes:
      - /v1/
      - /cc/v1/
      - /api/llm-gateway/
      - /api/kiro-gateway/
      - /api/codex-gateway/
      - /api/llm-access/
    bearer_token_sha256_allowlist: []
```

Keep all existing timeout and retry fields unchanged.

- [ ] **Step 2: Add local llm-access startup script**

Create `scripts/start_llm_access_local.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STATE_ROOT="${LLM_ACCESS_STATE_ROOT:-/mnt/wsl/data4tb/static-flow-data/llm-access-local}"
BIND_ADDR="${LLM_ACCESS_BIND_ADDR:-127.0.0.1:19080}"
SQLITE_CONTROL="${LLM_ACCESS_SQLITE_CONTROL:-$STATE_ROOT/control/llm-access.sqlite3}"
DUCKDB_PATH="${LLM_ACCESS_DUCKDB:-$STATE_ROOT/analytics/usage.duckdb}"

mkdir -p "$STATE_ROOT/control" "$STATE_ROOT/analytics" "$STATE_ROOT/auths/kiro" "$STATE_ROOT/auths/codex" "$STATE_ROOT/cdc" "$STATE_ROOT/logs"

cd "$ROOT_DIR"
exec cargo run -p llm-access -- serve \
  --bind "$BIND_ADDR" \
  --state-root "$STATE_ROOT" \
  --sqlite-control "$SQLITE_CONTROL" \
  --duckdb "$DUCKDB_PATH"
```

- [ ] **Step 3: Add local canary verification script**

Create `scripts/verify_pingora_llm_local_canary.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

BASE_URL="${BASE_URL:-http://127.0.0.1:39180}"
TEST_TOKEN="${TEST_TOKEN:-}"

curl_common=(
  -o /dev/null
  -sS
  -w 'code=%{http_code} start=%{time_starttransfer} total=%{time_total}\n'
)

echo "[staticflow] health through gateway"
curl "${curl_common[@]}" "$BASE_URL/api/healthz"

echo "[staticflow] non-llm article path through gateway"
curl "${curl_common[@]}" "$BASE_URL/api/articles"

if [[ -n "$TEST_TOKEN" ]]; then
  echo "[llm-routing] test-token LLM path through gateway"
  curl "${curl_common[@]}" \
    -H "Authorization: Bearer $TEST_TOKEN" \
    -H 'Content-Type: application/json' \
    -d '{"model":"canary","messages":[]}' \
    "$BASE_URL/v1/chat/completions"
else
  echo "[llm-routing] set TEST_TOKEN to exercise allowlisted LLM routing"
fi
```

- [ ] **Step 4: Validate scripts and config**

Run:

```bash
chmod +x scripts/start_llm_access_local.sh scripts/verify_pingora_llm_local_canary.sh
bash -n scripts/start_llm_access_local.sh
bash -n scripts/verify_pingora_llm_local_canary.sh
cargo test -p staticflow-pingora-gateway --jobs 1 config::tests::parse_gateway_config_accepts_valid_blue_green_setup -- --nocapture
```

Expected:

- Both scripts pass shell syntax validation.
- Existing config parses with the added disabled LLM routing block.

- [ ] **Step 5: Commit config and scripts**

Run:

```bash
git diff --check
git add conf/pingora/staticflow-gateway.yaml scripts/start_llm_access_local.sh scripts/verify_pingora_llm_local_canary.sh
git commit -m "chore: add local llm access canary config"
```

---

### Task 5: Update Design Status And Run Final Verification

**Files:**
- Modify: `docs/superpowers/specs/2026-04-30-pingora-llm-routing-local-canary-design.md`
- Test: full gateway checks plus script syntax

- [ ] **Step 1: Update design status**

Append this section to `docs/superpowers/specs/2026-04-30-pingora-llm-routing-local-canary-design.md`:

```markdown
## Implementation Status

Implemented:

- Optional `llm_routing` gateway config with disabled default behavior.
- SHA-256 bearer-token allowlist matching for local canary routing.
- Per-request selected upstream metadata in Pingora request context.
- Access log fields for selected upstream, route target, and route reason.
- Local `llm-access` startup script using `/mnt/wsl/data4tb/static-flow-data/llm-access-local`.
- Local read-only canary verification script.

Not implemented in this step:

- Moving Kiro/Codex provider runtime into `llm-access`.
- GCP deployment.
- JuiceFS-backed state.
```

- [ ] **Step 2: Run final verification**

Run build pressure check first:

```bash
pgrep -af 'cargo|rustc|trunk|ld|lld|mold|c\+\+' || true
ps -eo pid,ppid,stat,rss,etime,cmd --sort=-rss | head -40
```

Then run:

```bash
rustfmt gateway/src/config.rs gateway/src/lib.rs gateway/src/llm_routing.rs gateway/src/proxy.rs gateway/src/access_log.rs
cargo test -p staticflow-pingora-gateway --jobs 1 -- --nocapture
cargo clippy -p staticflow-pingora-gateway --jobs 1 -- -D warnings
bash -n scripts/start_llm_access_local.sh
bash -n scripts/verify_pingora_llm_local_canary.sh
git diff --check
```

Expected:

- Gateway tests pass.
- Clippy reports zero warnings.
- Shell scripts pass syntax checks.
- `git diff --check` reports no whitespace errors.

- [ ] **Step 3: Commit docs**

Run:

```bash
git add docs/superpowers/specs/2026-04-30-pingora-llm-routing-local-canary-design.md
git commit -m "docs: update pingora llm routing status"
```

---

## Execution Notes

- Do not restart the live gateway during implementation.
- Do not change `active_upstream` as part of these tasks.
- Do not add plaintext API keys to `conf/pingora/staticflow-gateway.yaml`.
- To compute a test token hash locally, use:

```bash
printf '%s' "$TEST_TOKEN" | sha256sum | awk '{print $1}'
```

- Live local canary requires a separate explicit approval after the code is merged and verified.
