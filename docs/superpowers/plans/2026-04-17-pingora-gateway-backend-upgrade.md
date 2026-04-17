# Pingora Gateway Backend Upgrade Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a local Pingora gateway binary in front of `static-flow-backend`, switch backend traffic between `blue` and `green` slots with graceful reload, and unify backend/gateway runtime logs under library-backed hourly rotation with 24-hour retention.

**Architecture:** Introduce a new workspace member `gateway/` that owns Pingora config loading, request correlation, proxying, and gateway access logging. Move request-id generation and native runtime logging setup into `shared/` so backend and gateway share one tracing-based implementation. Extend the backend with runtime metadata and `/api/healthz`, then add shell scripts that perform explicit config-driven reload and controlled backend cutover.

**Tech Stack:** Rust, Pingora, Axum, Tokio, tracing, tracing-subscriber, tracing-appender, shell scripts

---

## File Structure Map

**Workspace and manifests**
- Modify: `/home/ts_user/rust_pro/static_flow/Cargo.toml`
- Modify: `/home/ts_user/rust_pro/static_flow/shared/Cargo.toml`
- Create: `/home/ts_user/rust_pro/static_flow/gateway/Cargo.toml`

**Shared observability**
- Modify: `/home/ts_user/rust_pro/static_flow/shared/src/lib.rs`
- Create: `/home/ts_user/rust_pro/static_flow/shared/src/request_ids.rs`
- Create: `/home/ts_user/rust_pro/static_flow/shared/src/runtime_logging.rs`

**Backend runtime integration**
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/main.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/state.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/request_context.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/routes.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/health.rs`

**Gateway crate**
- Create: `/home/ts_user/rust_pro/static_flow/gateway/src/lib.rs`
- Create: `/home/ts_user/rust_pro/static_flow/gateway/src/main.rs`
- Create: `/home/ts_user/rust_pro/static_flow/gateway/src/config.rs`
- Create: `/home/ts_user/rust_pro/static_flow/gateway/src/proxy.rs`
- Create: `/home/ts_user/rust_pro/static_flow/gateway/src/access_log.rs`

**Config and scripts**
- Create: `/home/ts_user/rust_pro/static_flow/conf/pingora/staticflow-gateway.yaml`
- Create: `/home/ts_user/rust_pro/static_flow/scripts/pingora_gateway.sh`
- Create: `/home/ts_user/rust_pro/static_flow/scripts/backend_gateway_upgrade.sh`
- Modify: `/home/ts_user/rust_pro/static_flow/scripts/start_backend_selfhosted.sh`
- Modify: `/home/ts_user/rust_pro/static_flow/scripts/start_backend_selfhosted_canary.sh`

**Plan-only docs**
- Do not touch the design spec unless implementation reveals a real contradiction

---

### Task 1: Add Gateway Workspace Member And Crate Skeleton

**Files:**
- Modify: `/home/ts_user/rust_pro/static_flow/Cargo.toml`
- Create: `/home/ts_user/rust_pro/static_flow/gateway/Cargo.toml`
- Create: `/home/ts_user/rust_pro/static_flow/gateway/src/lib.rs`
- Create: `/home/ts_user/rust_pro/static_flow/gateway/src/main.rs`

- [ ] **Step 1: Write the failing workspace checks**

Run:

```bash
cargo metadata --no-deps --format-version 1 | jq -r '.packages[].name' | rg '^staticflow-pingora-gateway$'
cargo check -p staticflow-pingora-gateway
test -f /home/ts_user/rust_pro/static_flow/gateway/Cargo.toml
```

Expected:
- `cargo metadata ...` prints nothing for `staticflow-pingora-gateway`
- `cargo check -p staticflow-pingora-gateway` fails with `package ID specification 'staticflow-pingora-gateway' did not match any packages`
- `test -f .../gateway/Cargo.toml` fails because the crate does not exist

- [ ] **Step 2: Verify the current failure modes**

Run:

```bash
cargo check -p staticflow-pingora-gateway || echo "missing gateway package"
test -f /home/ts_user/rust_pro/static_flow/gateway/Cargo.toml || echo "missing gateway manifest"
```

Expected:
- both checks fail for the reasons above

- [ ] **Step 3: Add the workspace member and gateway crate skeleton**

In `/home/ts_user/rust_pro/static_flow/Cargo.toml`, ensure the workspace member list includes `gateway` and the workspace dependency table includes the following additions:

```toml
[workspace]
resolver = "2"
members = ["frontend", "shared", "backend", "cli", "media-service", "media-types", "gateway"]
exclude = ["deps/lance", "deps/lancedb"]

[workspace.dependencies]
tracing = "0.1"
tracing-appender = "0.2"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt", "registry"] }
```

Create `/home/ts_user/rust_pro/static_flow/gateway/Cargo.toml`:

```toml
[package]
name = "staticflow-pingora-gateway"
version = "0.1.0"
edition = "2021"
publish = false

[lints]
workspace = true

[dependencies]
anyhow = { workspace = true }
async-trait = "0.1"
serde = { workspace = true }
serde_yaml = "0.9"
tokio = { workspace = true }
tracing = { workspace = true }
static-flow-shared = { path = "../shared" }
pingora = { path = "../deps/pingora/pingora", features = ["proxy"] }
pingora-core = { path = "../deps/pingora/pingora-core" }
pingora-http = { path = "../deps/pingora/pingora-http" }
pingora-proxy = { path = "../deps/pingora/pingora-proxy" }

[dev-dependencies]
tempfile = "3.23"
```

Create `/home/ts_user/rust_pro/static_flow/gateway/src/lib.rs`:

```rust
pub mod access_log;
pub mod config;
pub mod proxy;
```

Create `/home/ts_user/rust_pro/static_flow/gateway/src/main.rs`:

```rust
use anyhow::Result;
use pingora::server::configuration::Opt;

#[tokio::main]
async fn main() -> Result<()> {
    let _opt = Opt::parse_args();
    anyhow::bail!("gateway not implemented yet")
}
```

- [ ] **Step 4: Verify the package is now discoverable**

Run:

```bash
cargo metadata --no-deps --format-version 1 | jq -r '.packages[].name' | rg '^staticflow-pingora-gateway$'
cargo check -p staticflow-pingora-gateway || true
```

Expected:
- `cargo metadata` lists `staticflow-pingora-gateway`
- `cargo check -p staticflow-pingora-gateway` reaches `gateway/src/main.rs` and fails only because `main` intentionally bails

- [ ] **Step 5: Commit**

```bash
git add /home/ts_user/rust_pro/static_flow/Cargo.toml \
        /home/ts_user/rust_pro/static_flow/gateway
git commit -m "feat: add pingora gateway crate skeleton"
```

### Task 2: Add Shared Request-ID And Runtime Logging Helpers

**Files:**
- Modify: `/home/ts_user/rust_pro/static_flow/shared/Cargo.toml`
- Modify: `/home/ts_user/rust_pro/static_flow/shared/src/lib.rs`
- Create: `/home/ts_user/rust_pro/static_flow/shared/src/request_ids.rs`
- Create: `/home/ts_user/rust_pro/static_flow/shared/src/runtime_logging.rs`

- [ ] **Step 1: Write the failing shared tests**

Create `/home/ts_user/rust_pro/static_flow/shared/src/request_ids.rs` with only the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::read_or_generate_id;

    #[test]
    fn read_or_generate_id_keeps_existing_value() {
        let value = read_or_generate_id(Some("req-existing"), "req");
        assert_eq!(value, "req-existing");
    }
}
```

Create `/home/ts_user/rust_pro/static_flow/shared/src/runtime_logging.rs` with only the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::RuntimeLogOptions;

    #[test]
    fn runtime_log_options_default_to_24_files() {
        let opts = RuntimeLogOptions::for_service("backend");
        assert_eq!(opts.max_files, 24);
    }
}
```

Expose both modules from `/home/ts_user/rust_pro/static_flow/shared/src/lib.rs`:

```rust
pub mod request_ids;

#[cfg(not(target_arch = "wasm32"))]
pub mod runtime_logging;
```

Run:

```bash
cargo test -p static-flow-shared read_or_generate_id_keeps_existing_value -- --exact
cargo test -p static-flow-shared runtime_log_options_default_to_24_files -- --exact
```

Expected:
- both tests fail with unresolved imports or missing types/functions

- [ ] **Step 2: Verify the failures are about missing implementation**

Run:

```bash
cargo test -p static-flow-shared read_or_generate_id_keeps_existing_value -- --exact || true
cargo test -p static-flow-shared runtime_log_options_default_to_24_files -- --exact || true
```

Expected:
- failure output mentions `read_or_generate_id` and `RuntimeLogOptions` are missing

- [ ] **Step 3: Implement `request_ids.rs` and `runtime_logging.rs`**

In `/home/ts_user/rust_pro/static_flow/shared/Cargo.toml`, add the native logging dependencies shown below without removing the existing native dependencies:

```toml
[dependencies]
anyhow = { workspace = true }
serde = { version = "1.0", features = ["derive"] }
tracing = { workspace = true }

[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
tracing-appender = { workspace = true }
tracing-subscriber = { workspace = true }
```

Write `/home/ts_user/rust_pro/static_flow/shared/src/request_ids.rs`:

```rust
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

pub const REQUEST_ID_HEADER: &str = "x-request-id";
pub const TRACE_ID_HEADER: &str = "x-trace-id";

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn read_or_generate_id(raw_value: Option<&str>, prefix: &str) -> String {
    raw_value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| generate_id(prefix))
}

pub fn generate_id(prefix: &str) -> String {
    let now_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    let counter = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{now_ns:032x}-{counter:016x}")
}

#[cfg(test)]
mod tests {
    use super::{generate_id, read_or_generate_id};

    #[test]
    fn read_or_generate_id_keeps_existing_value() {
        let value = read_or_generate_id(Some("req-existing"), "req");
        assert_eq!(value, "req-existing");
    }

    #[test]
    fn generate_id_uses_prefix() {
        let value = generate_id("trace");
        assert!(value.starts_with("trace-"));
    }
}
```

Write `/home/ts_user/rust_pro/static_flow/shared/src/runtime_logging.rs`:

```rust
use std::{env, fs, path::PathBuf};

use anyhow::Result;
use tracing_appender::{
    non_blocking::{self, WorkerGuard},
    rolling::{RollingFileAppender, Rotation},
};
use tracing_subscriber::{
    filter::{filter_fn, EnvFilter, Targets},
    layer::SubscriberExt,
    util::SubscriberInitExt,
    Layer,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLogOptions {
    pub root_dir: PathBuf,
    pub service: String,
    pub max_files: usize,
    pub stdout: bool,
}

impl RuntimeLogOptions {
    pub fn for_service(service: &str) -> Self {
        let root_dir = env::var("STATICFLOW_LOG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("tmp/runtime-logs"));
        let stdout = env::var("STATICFLOW_LOG_STDOUT")
            .map(|value| value != "0")
            .unwrap_or(true);
        Self {
            root_dir,
            service: service.to_string(),
            max_files: 24,
            stdout,
        }
    }
}

pub struct RuntimeLogGuards {
    _app_guard: WorkerGuard,
    _access_guard: WorkerGuard,
}

pub fn init_runtime_logging(service: &str, default_filter: &str) -> Result<RuntimeLogGuards> {
    let opts = RuntimeLogOptions::for_service(service);
    let app_dir = opts.root_dir.join(service).join("app");
    let access_dir = opts.root_dir.join(service).join("access");
    fs::create_dir_all(&app_dir)?;
    fs::create_dir_all(&access_dir)?;

    let app_writer = RollingFileAppender::builder()
        .rotation(Rotation::HOURLY)
        .max_log_files(opts.max_files)
        .filename_prefix("current")
        .filename_suffix("log")
        .build(&app_dir)?;
    let access_writer = RollingFileAppender::builder()
        .rotation(Rotation::HOURLY)
        .max_log_files(opts.max_files)
        .filename_prefix("current")
        .filename_suffix("log")
        .build(&access_dir)?;

    let (app_writer, app_guard) = non_blocking(app_writer);
    let (access_writer, access_guard) = non_blocking(access_writer);

    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    let app_layer = tracing_subscriber::fmt::layer()
        .compact()
        .with_writer(app_writer)
        .with_filter(filter_fn(|metadata| metadata.target() != "staticflow_access"));
    let access_layer = tracing_subscriber::fmt::layer()
        .compact()
        .with_writer(access_writer)
        .with_filter(Targets::new().with_target("staticflow_access", tracing::Level::TRACE));

    let subscriber = tracing_subscriber::registry()
        .with(env_filter)
        .with(app_layer)
        .with(access_layer);

    if opts.stdout {
        subscriber
            .with(tracing_subscriber::fmt::layer().compact())
            .init();
    } else {
        subscriber.init();
    }

    Ok(RuntimeLogGuards {
        _app_guard: app_guard,
        _access_guard: access_guard,
    })
}

#[cfg(test)]
mod tests {
    use super::RuntimeLogOptions;

    #[test]
    fn runtime_log_options_default_to_24_files() {
        let opts = RuntimeLogOptions::for_service("backend");
        assert_eq!(opts.max_files, 24);
    }
}
```

- [ ] **Step 4: Verify the shared tests now pass**

Run:

```bash
cargo test -p static-flow-shared read_or_generate_id_keeps_existing_value -- --exact
cargo test -p static-flow-shared runtime_log_options_default_to_24_files -- --exact
```

Expected:
- both tests pass

- [ ] **Step 5: Commit**

```bash
git add /home/ts_user/rust_pro/static_flow/shared/Cargo.toml \
        /home/ts_user/rust_pro/static_flow/shared/src/lib.rs \
        /home/ts_user/rust_pro/static_flow/shared/src/request_ids.rs \
        /home/ts_user/rust_pro/static_flow/shared/src/runtime_logging.rs
git commit -m "feat: add shared runtime logging helpers"
```

### Task 3: Integrate Backend Runtime Metadata, Healthz, And Shared Logging

**Files:**
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/main.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/state.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/request_context.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/routes.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/health.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/scripts/start_backend_selfhosted.sh`
- Modify: `/home/ts_user/rust_pro/static_flow/scripts/start_backend_selfhosted_canary.sh`

- [ ] **Step 1: Write the failing backend tests**

Create `/home/ts_user/rust_pro/static_flow/backend/src/health.rs` with only the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::HealthzResponse;

    #[test]
    fn healthz_response_serializes_runtime_metadata() {
        let body = HealthzResponse {
            status: "ok".to_string(),
            pid: 123,
            port: 39080,
            started_at: 1,
            version: "test-build".to_string(),
        };
        let json = serde_json::to_value(&body).expect("serialize healthz");
        assert_eq!(json["status"], "ok");
        assert_eq!(json["pid"], 123);
        assert_eq!(json["port"], 39080);
    }
}
```

Run:

```bash
cargo test -p static-flow-backend healthz_response_serializes_runtime_metadata -- --exact
```

Expected:
- the test fails because `HealthzResponse` does not exist yet

- [ ] **Step 2: Verify the failure is about missing health runtime types**

Run:

```bash
cargo test -p static-flow-backend healthz_response_serializes_runtime_metadata -- --exact || true
```

Expected:
- failure output mentions missing `HealthzResponse`

- [ ] **Step 3: Add runtime metadata, `/api/healthz`, and backend access logging**

Update `/home/ts_user/rust_pro/static_flow/backend/src/state.rs` by adding runtime metadata:

```rust
#[derive(Debug, Clone)]
pub struct RuntimeMetadata {
    pub started_at_ms: i64,
    pub build_id: String,
}

#[derive(Clone)]
pub struct AppState {
    // existing fields...
    pub(crate) runtime_metadata: Arc<RuntimeMetadata>,
}
```

Initialize it inside `AppState::new(...)`:

```rust
let runtime_metadata = Arc::new(RuntimeMetadata {
    started_at_ms: chrono::Utc::now().timestamp_millis(),
    build_id: option_env!("STATICFLOW_BUILD_ID")
        .unwrap_or(env!("CARGO_PKG_VERSION"))
        .to_string(),
});

Ok(Self {
    // existing fields...
    runtime_metadata,
})
```

Write `/home/ts_user/rust_pro/static_flow/backend/src/health.rs`:

```rust
use axum::{extract::State, Json};
use serde::Serialize;

use crate::state::AppState;

#[derive(Debug, Clone, Serialize)]
pub struct HealthzResponse {
    pub status: String,
    pub pid: u32,
    pub port: u16,
    pub started_at: i64,
    pub version: String,
}

pub async fn get_healthz(State(state): State<AppState>) -> Json<HealthzResponse> {
    let port = std::env::var("PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(3000);

    Json(HealthzResponse {
        status: "ok".to_string(),
        pid: std::process::id(),
        port,
        started_at: state.runtime_metadata.started_at_ms,
        version: state.runtime_metadata.build_id.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::HealthzResponse;

    #[test]
    fn healthz_response_serializes_runtime_metadata() {
        let body = HealthzResponse {
            status: "ok".to_string(),
            pid: 123,
            port: 39080,
            started_at: 1,
            version: "test-build".to_string(),
        };
        let json = serde_json::to_value(&body).expect("serialize healthz");
        assert_eq!(json["status"], "ok");
        assert_eq!(json["pid"], 123);
        assert_eq!(json["port"], 39080);
    }
}
```

Hook the route in `/home/ts_user/rust_pro/static_flow/backend/src/routes.rs`:

```rust
use crate::{
    behavior_analytics, handlers, health, kiro_gateway, llm_gateway, request_context, seo,
    state::AppState,
};

// inside api_router
.route("/api/healthz", get(health::get_healthz))
```

Replace request-id generation in `/home/ts_user/rust_pro/static_flow/backend/src/request_context.rs` with shared helpers and emit access logs to the dedicated target:

```rust
use std::{net::SocketAddr, time::Instant};

use axum::{
    extract::{connect_info::ConnectInfo, Request},
    http::{header::HeaderName, HeaderMap, HeaderValue},
    middleware::Next,
    response::Response,
};
use static_flow_shared::request_ids::{
    read_or_generate_id, REQUEST_ID_HEADER, TRACE_ID_HEADER,
};

pub async fn request_context_middleware(request: Request, next: Next) -> Response {
    let request_id = read_or_generate_id(
        request.headers().get(REQUEST_ID_HEADER).and_then(|value| value.to_str().ok()),
        "req",
    );
    let trace_id = read_or_generate_id(
        request.headers().get(TRACE_ID_HEADER).and_then(|value| value.to_str().ok()),
        "trace",
    );
    let remote_addr = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|value| value.0.to_string())
        .unwrap_or_else(|| "-".to_string());

    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let started_at = Instant::now();
    let span = tracing::info_span!(
        "http_request",
        request_id = %request_id,
        trace_id = %trace_id,
        method = %method,
        path = %path,
    );

    let mut response = next.run(request).instrument(span.clone()).await;
    set_response_header(response.headers_mut(), REQUEST_ID_HEADER, request_id.as_str());
    set_response_header(response.headers_mut(), TRACE_ID_HEADER, trace_id.as_str());

    tracing::info!(
        target: "staticflow_access",
        request_id = %request_id,
        trace_id = %trace_id,
        remote_addr = %remote_addr,
        method = %method,
        path = %path,
        status = response.status().as_u16(),
        elapsed_ms = started_at.elapsed().as_millis(),
        "backend access"
    );

    response
}
```

Update `/home/ts_user/rust_pro/static_flow/backend/src/main.rs`:

```rust
use std::{env, net::SocketAddr, time::Duration};

use static_flow_shared::runtime_logging::init_runtime_logging;

#[tokio::main]
async fn main() -> Result<()> {
    MiMalloc::init();
    let _log_guards = init_runtime_logging("backend", DEFAULT_LOG_FILTER)?;
    let mem_profiler = memory_profiler::init_from_env();
    // existing startup...
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let app_state_ref = app_state.clone();
    let app = routes::create_router(app_state);
    let mut server = tokio::spawn(async move {
        axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
            .with_graceful_shutdown(async move {
                let _ = server_shutdown_rx.await;
            })
            .await
    });
    // existing shutdown logic...
}
```

Update the two self-hosted scripts so the backend process owns business logs and the wrapper only controls stdout duplication:

```bash
# start_backend_selfhosted.sh and start_backend_selfhosted_canary.sh
STATICFLOW_LOG_DIR="${STATICFLOW_LOG_DIR:-$ROOT_DIR/tmp/runtime-logs}"
export STATICFLOW_LOG_DIR
export STATICFLOW_LOG_STDOUT="${STATICFLOW_LOG_STDOUT:-1}"

if [[ "$DAEMON" == "true" ]]; then
  export STATICFLOW_LOG_STDOUT=0
  nohup "$BACKEND_BIN_PATH" >/dev/null 2>&1 &
else
  export STATICFLOW_LOG_STDOUT=1
  exec "$BACKEND_BIN_PATH"
fi
```

- [ ] **Step 4: Verify backend health and logging integration**

Run:

```bash
cargo test -p static-flow-backend healthz_response_serializes_runtime_metadata -- --exact
cargo check -p static-flow-backend
bash /home/ts_user/rust_pro/static_flow/scripts/start_backend_selfhosted.sh --help
bash /home/ts_user/rust_pro/static_flow/scripts/start_backend_selfhosted_canary.sh --help
```

Expected:
- the health test passes
- backend compiles with the shared logging bootstrap
- both scripts still print usable help output

- [ ] **Step 5: Commit**

```bash
git add /home/ts_user/rust_pro/static_flow/backend/src/main.rs \
        /home/ts_user/rust_pro/static_flow/backend/src/state.rs \
        /home/ts_user/rust_pro/static_flow/backend/src/request_context.rs \
        /home/ts_user/rust_pro/static_flow/backend/src/routes.rs \
        /home/ts_user/rust_pro/static_flow/backend/src/health.rs \
        /home/ts_user/rust_pro/static_flow/scripts/start_backend_selfhosted.sh \
        /home/ts_user/rust_pro/static_flow/scripts/start_backend_selfhosted_canary.sh
git commit -m "feat: add backend health and shared logging"
```

### Task 4: Implement Gateway Config Loading, Proxying, And Access Logs

**Files:**
- Create: `/home/ts_user/rust_pro/static_flow/gateway/src/config.rs`
- Create: `/home/ts_user/rust_pro/static_flow/gateway/src/proxy.rs`
- Create: `/home/ts_user/rust_pro/static_flow/gateway/src/access_log.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/gateway/src/lib.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/gateway/src/main.rs`

- [ ] **Step 1: Write the failing gateway tests**

Create `/home/ts_user/rust_pro/static_flow/gateway/src/config.rs` with only the test module first:

```rust
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
```

Create `/home/ts_user/rust_pro/static_flow/gateway/src/proxy.rs` with only the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::GatewayRequestContext;

    #[test]
    fn proxy_ctx_keeps_existing_request_ids() {
        let ctx = GatewayRequestContext::new(
            "req-existing".to_string(),
            "trace-existing".to_string(),
            "blue".to_string(),
            "127.0.0.1:39080".to_string(),
        );
        assert_eq!(ctx.request_id, "req-existing");
        assert_eq!(ctx.trace_id, "trace-existing");
    }
}
```

Run:

```bash
cargo test -p staticflow-pingora-gateway parse_gateway_config_accepts_valid_blue_green_setup -- --exact
cargo test -p staticflow-pingora-gateway proxy_ctx_keeps_existing_request_ids -- --exact
```

Expected:
- both tests fail because the gateway modules are not implemented yet

- [ ] **Step 2: Verify the failures are about missing gateway internals**

Run:

```bash
cargo test -p staticflow-pingora-gateway parse_gateway_config_accepts_valid_blue_green_setup -- --exact || true
cargo test -p staticflow-pingora-gateway proxy_ctx_keeps_existing_request_ids -- --exact || true
```

Expected:
- failure output mentions missing `load_gateway_config_from_str` and `GatewayRequestContext`

- [ ] **Step 3: Implement config parsing, `--test`, proxy service, and access logging**

Write `/home/ts_user/rust_pro/static_flow/gateway/src/config.rs`:

```rust
use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{anyhow, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct GatewayFile {
    staticflow: GatewayConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GatewayConfig {
    pub listen_addr: String,
    pub request_id_header: String,
    pub trace_id_header: String,
    pub add_forwarded_headers: bool,
    pub upstreams: BTreeMap<String, String>,
    pub active_upstream: String,
    pub connect_timeout_ms: u64,
    pub read_idle_timeout_ms: u64,
    pub write_idle_timeout_ms: u64,
    pub retry_count: usize,
}

impl GatewayConfig {
    pub fn active_upstream_addr(&self) -> Result<&str> {
        self.upstreams
            .get(&self.active_upstream)
            .map(String::as_str)
            .ok_or_else(|| anyhow!("active_upstream `{}` missing from upstreams", self.active_upstream))
    }
}

pub fn load_gateway_config(path: &Path) -> Result<GatewayConfig> {
    let raw = fs::read_to_string(path)?;
    load_gateway_config_from_str(&raw)
}

pub fn load_gateway_config_from_str(raw: &str) -> Result<GatewayConfig> {
    let file: GatewayFile = serde_yaml::from_str(raw)?;
    if !matches!(file.staticflow.active_upstream.as_str(), "blue" | "green") {
        return Err(anyhow!("active_upstream must be `blue` or `green`"));
    }
    file.staticflow.active_upstream_addr()?;
    Ok(file.staticflow)
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
```

Write `/home/ts_user/rust_pro/static_flow/gateway/src/access_log.rs`:

```rust
use std::time::Instant;

use crate::proxy::GatewayRequestContext;

pub fn emit_gateway_access_log(
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
        active_upstream = %ctx.active_upstream,
        upstream_addr = %ctx.upstream_addr,
        method = %method,
        path = %path,
        status,
        elapsed_ms = started_at.elapsed().as_millis(),
        "gateway access"
    );
}
```

Write `/home/ts_user/rust_pro/static_flow/gateway/src/proxy.rs`:

```rust
use std::{sync::Arc, time::Instant};

use anyhow::Result;
use async_trait::async_trait;
use pingora_core::upstreams::peer::HttpPeer;
use pingora_http::RequestHeader;
use pingora_proxy::{ProxyHttp, Session};
use static_flow_shared::request_ids::{read_or_generate_id, REQUEST_ID_HEADER, TRACE_ID_HEADER};

use crate::{access_log::emit_gateway_access_log, config::GatewayConfig};

#[derive(Debug, Clone)]
pub struct GatewayRequestContext {
    pub request_id: String,
    pub trace_id: String,
    pub active_upstream: String,
    pub upstream_addr: String,
    pub method: String,
    pub path: String,
    pub started_at: Instant,
}

impl GatewayRequestContext {
    pub fn new(
        request_id: String,
        trace_id: String,
        active_upstream: String,
        upstream_addr: String,
    ) -> Self {
        Self {
            request_id,
            trace_id,
            active_upstream,
            upstream_addr,
            method: String::new(),
            path: String::new(),
            started_at: Instant::now(),
        }
    }
}

pub struct StaticFlowGateway {
    config: Arc<GatewayConfig>,
}

impl StaticFlowGateway {
    pub fn new(config: Arc<GatewayConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl ProxyHttp for StaticFlowGateway {
    type CTX = GatewayRequestContext;

    fn new_ctx(&self) -> Self::CTX {
        let upstream_addr = self.config.active_upstream_addr().unwrap_or("").to_string();
        GatewayRequestContext::new(
            "req-pending".to_string(),
            "trace-pending".to_string(),
            self.config.active_upstream.clone(),
            upstream_addr,
        )
    }

    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> Result<bool> {
        let req = session.req_header();
        ctx.request_id = read_or_generate_id(
            req.headers.get(REQUEST_ID_HEADER).and_then(|value| value.to_str().ok()),
            "req",
        );
        ctx.trace_id = read_or_generate_id(
            req.headers.get(TRACE_ID_HEADER).and_then(|value| value.to_str().ok()),
            "trace",
        );
        ctx.method = req.method.as_str().to_string();
        ctx.path = req.uri.path().to_string();
        ctx.started_at = Instant::now();
        Ok(false)
    }

    async fn upstream_peer(&self, _session: &mut Session, ctx: &mut Self::CTX) -> Result<Box<HttpPeer>> {
        ctx.upstream_addr = self.config.active_upstream_addr()?.to_string();
        Ok(Box::new(HttpPeer::new(ctx.upstream_addr.as_str(), false, String::new())))
    }

    async fn upstream_request_filter(
        &self,
        _session: &mut Session,
        upstream_request: &mut RequestHeader,
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        upstream_request.insert_header(REQUEST_ID_HEADER, ctx.request_id.as_str())?;
        upstream_request.insert_header(TRACE_ID_HEADER, ctx.trace_id.as_str())?;
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

    #[test]
    fn proxy_ctx_keeps_existing_request_ids() {
        let ctx = GatewayRequestContext::new(
            "req-existing".to_string(),
            "trace-existing".to_string(),
            "blue".to_string(),
            "127.0.0.1:39080".to_string(),
        );
        assert_eq!(ctx.request_id, "req-existing");
        assert_eq!(ctx.trace_id, "trace-existing");
    }
}
```

Update `/home/ts_user/rust_pro/static_flow/gateway/src/main.rs`:

```rust
use std::{path::PathBuf, sync::Arc};

use anyhow::{anyhow, Result};
use pingora::server::{configuration::Opt, Server};
use pingora_proxy::http_proxy_service;
use static_flow_shared::runtime_logging::init_runtime_logging;

use staticflow_pingora_gateway::{config::load_gateway_config, proxy::StaticFlowGateway};

#[tokio::main]
async fn main() -> Result<()> {
    let opt = Opt::parse_args();
    let conf_path = PathBuf::from(opt.conf.clone());
    if conf_path.as_os_str().is_empty() {
        return Err(anyhow!("--conf is required"));
    }

    let gateway_config = load_gateway_config(&conf_path)?;
    if opt.test {
        println!("listen_addr={}", gateway_config.listen_addr);
        println!("active_upstream={}", gateway_config.active_upstream);
        println!("connect_timeout_ms={}", gateway_config.connect_timeout_ms);
        println!("read_idle_timeout_ms={}", gateway_config.read_idle_timeout_ms);
        println!("write_idle_timeout_ms={}", gateway_config.write_idle_timeout_ms);
        println!(
            "log_root={}",
            std::env::var("STATICFLOW_LOG_DIR").unwrap_or_else(|_| "tmp/runtime-logs".to_string())
        );
        return Ok(());
    }

    let _log_guards = init_runtime_logging("gateway", "warn,staticflow_pingora_gateway=info")?;
    let mut server = Server::new(Some(opt)).unwrap();
    server.bootstrap();

    let mut proxy = http_proxy_service(
        &server.configuration,
        StaticFlowGateway::new(Arc::new(gateway_config.clone())),
    );
    proxy.add_tcp(gateway_config.listen_addr.as_str());
    server.add_service(proxy);
    server.run_forever();
}
```

Update `/home/ts_user/rust_pro/static_flow/gateway/src/lib.rs`:

```rust
pub mod access_log;
pub mod config;
pub mod proxy;
```

- [ ] **Step 4: Verify gateway parsing and proxy context tests**

Run:

```bash
cargo test -p staticflow-pingora-gateway parse_gateway_config_accepts_valid_blue_green_setup -- --exact
cargo test -p staticflow-pingora-gateway proxy_ctx_keeps_existing_request_ids -- --exact
cargo check -p staticflow-pingora-gateway
```

Expected:
- both gateway tests pass
- the gateway crate compiles

- [ ] **Step 5: Commit**

```bash
git add /home/ts_user/rust_pro/static_flow/gateway/src/lib.rs \
        /home/ts_user/rust_pro/static_flow/gateway/src/main.rs \
        /home/ts_user/rust_pro/static_flow/gateway/src/config.rs \
        /home/ts_user/rust_pro/static_flow/gateway/src/proxy.rs \
        /home/ts_user/rust_pro/static_flow/gateway/src/access_log.rs
git commit -m "feat: add pingora gateway proxy implementation"
```

### Task 5: Add Gateway Config, Reload Script, Upgrade Script, And Smoke Verification

**Files:**
- Create: `/home/ts_user/rust_pro/static_flow/conf/pingora/staticflow-gateway.yaml`
- Create: `/home/ts_user/rust_pro/static_flow/scripts/pingora_gateway.sh`
- Create: `/home/ts_user/rust_pro/static_flow/scripts/backend_gateway_upgrade.sh`

- [ ] **Step 1: Write the failing file-existence checks**

Run:

```bash
test -f /home/ts_user/rust_pro/static_flow/conf/pingora/staticflow-gateway.yaml
test -f /home/ts_user/rust_pro/static_flow/scripts/pingora_gateway.sh
test -f /home/ts_user/rust_pro/static_flow/scripts/backend_gateway_upgrade.sh
```

Expected:
- all three checks fail because the files do not exist yet

- [ ] **Step 2: Verify the current absence**

Run:

```bash
test -f /home/ts_user/rust_pro/static_flow/conf/pingora/staticflow-gateway.yaml || echo "missing gateway yaml"
test -f /home/ts_user/rust_pro/static_flow/scripts/pingora_gateway.sh || echo "missing gateway script"
test -f /home/ts_user/rust_pro/static_flow/scripts/backend_gateway_upgrade.sh || echo "missing upgrade script"
```

Expected:
- all three commands print the corresponding missing message

- [ ] **Step 3: Add the config file and scripts**

Create `/home/ts_user/rust_pro/static_flow/conf/pingora/staticflow-gateway.yaml`:

```yaml
version: 1
daemon: false
threads: 2
pid_file: /home/ts_user/rust_pro/static_flow/tmp/staticflow-gateway.pid
error_log: /home/ts_user/rust_pro/static_flow/tmp/runtime-logs/gateway/pingora-error/current.log
upgrade_sock: /home/ts_user/rust_pro/static_flow/tmp/staticflow-gateway-upgrade.sock

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
```

Create `/home/ts_user/rust_pro/static_flow/scripts/pingora_gateway.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

CONF_FILE="${CONF_FILE:-$ROOT_DIR/conf/pingora/staticflow-gateway.yaml}"
GATEWAY_BIN="${GATEWAY_BIN:-$ROOT_DIR/target/release-backend/staticflow-pingora-gateway}"

log() { echo "[gateway] $*"; }
fail() { echo "[gateway][ERROR] $*" >&2; exit 1; }

build_gateway_bin() {
  cargo build -p staticflow-pingora-gateway --profile release-backend >/dev/null
}

check_gateway() {
  build_gateway_bin
  "$GATEWAY_BIN" --conf "$CONF_FILE" --test
}

pid_file() {
  rg '^pid_file:' "$CONF_FILE" | awk '{print $2}'
}

current_pid() {
  local file
  file="$(pid_file)"
  [[ -f "$file" ]] && cat "$file"
}

reload_gateway() {
  local old_pid=""
  old_pid="$(current_pid || true)"
  "$GATEWAY_BIN" --daemon --upgrade --conf "$CONF_FILE"
  if [[ -n "$old_pid" ]]; then
    kill -QUIT "$old_pid"
  fi
}

switch_upstream() {
  local next="$1"
  python3 - "$CONF_FILE" "$next" <<'PY'
import pathlib, re, sys
path = pathlib.Path(sys.argv[1])
next_value = sys.argv[2]
text = path.read_text()
text, count = re.subn(r'(^\s*active_upstream:\s*).+$', rf'\1{next_value}', text, flags=re.M)
if count != 1:
    raise SystemExit("expected exactly one active_upstream line")
path.write_text(text)
PY
}

case "${1:-}" in
  run) build_gateway_bin; exec "$GATEWAY_BIN" --conf "$CONF_FILE" ;;
  start) build_gateway_bin; exec "$GATEWAY_BIN" --daemon --conf "$CONF_FILE" ;;
  check) check_gateway ;;
  reload) check_gateway >/dev/null; reload_gateway ;;
  status) check_gateway; echo "pid=$(current_pid || true)" ;;
  stop)
    pid="$(current_pid || true)"
    [[ -n "$pid" ]] || fail "gateway is not running"
    kill -TERM "$pid"
    ;;
  switch)
    [[ $# -eq 2 ]] || fail "usage: $0 switch <blue|green>"
    [[ "$2" == "blue" || "$2" == "green" ]] || fail "slot must be blue or green"
    switch_upstream "$2"
    check_gateway >/dev/null
    reload_gateway
    ;;
  *) fail "usage: $0 {run|start|check|reload|status|stop|switch}" ;;
esac
```

Create `/home/ts_user/rust_pro/static_flow/scripts/backend_gateway_upgrade.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

GATEWAY_URL="${GATEWAY_URL:-http://127.0.0.1:39180}"

log() { echo "[upgrade] $*"; }
fail() { echo "[upgrade][ERROR] $*" >&2; exit 1; }

active_slot() {
  rg '^  active_upstream:' "$ROOT_DIR/conf/pingora/staticflow-gateway.yaml" | awk '{print $2}'
}

slot_port() {
  case "$1" in
    blue) echo 39080 ;;
    green) echo 39081 ;;
    *) fail "unknown slot $1" ;;
  esac
}

other_slot() {
  case "$1" in
    blue) echo green ;;
    green) echo blue ;;
    *) fail "unknown slot $1" ;;
  esac
}

wait_health() {
  local url="$1"
  for _ in $(seq 1 80); do
    if curl -fsS "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  return 1
}

json_field() {
  local field="$1"
  python3 - "$field" <<'PY'
import json, sys
field = sys.argv[1]
print(json.load(sys.stdin)[field])
PY
}

old_slot="$(active_slot)"
new_slot="$(other_slot "$old_slot")"
old_port="$(slot_port "$old_slot")"
new_port="$(slot_port "$new_slot")"

log "old_slot=$old_slot new_slot=$new_slot old_port=$old_port new_port=$new_port"

if [[ "$new_slot" == "blue" ]]; then
  bash "$ROOT_DIR/scripts/start_backend_selfhosted.sh" --daemon --port "$new_port"
else
  bash "$ROOT_DIR/scripts/start_backend_selfhosted_canary.sh" --daemon --port "$new_port"
fi

wait_health "http://127.0.0.1:${new_port}/api/healthz" || fail "candidate backend failed healthz"
old_pid="$(curl -fsS "http://127.0.0.1:${old_port}/api/healthz" | json_field pid)"

bash "$ROOT_DIR/scripts/pingora_gateway.sh" switch "$new_slot"
wait_health "${GATEWAY_URL}/api/healthz" || fail "gateway did not recover after switch"

gateway_port="$(curl -fsS "${GATEWAY_URL}/api/healthz" | json_field port)"
[[ "$gateway_port" == "$new_port" ]] || fail "gateway still points to old backend"

kill -TERM "$old_pid" || log "warning: failed to stop old pid=$old_pid"
log "upgrade completed"
```

Mark scripts executable:

```bash
chmod +x /home/ts_user/rust_pro/static_flow/scripts/pingora_gateway.sh
chmod +x /home/ts_user/rust_pro/static_flow/scripts/backend_gateway_upgrade.sh
```

- [ ] **Step 4: Run smoke verification and final quality gates**

Run:

```bash
bash /home/ts_user/rust_pro/static_flow/scripts/pingora_gateway.sh check
cargo test -p static-flow-shared
cargo test -p static-flow-backend healthz_response_serializes_runtime_metadata -- --exact
cargo test -p staticflow-pingora-gateway
cargo check -p staticflow-pingora-gateway -p static-flow-backend -p static-flow-shared
cargo clippy -p static-flow-shared -p static-flow-backend -p staticflow-pingora-gateway --all-targets -- -D warnings
rustfmt /home/ts_user/rust_pro/static_flow/shared/src/request_ids.rs \
        /home/ts_user/rust_pro/static_flow/shared/src/runtime_logging.rs \
        /home/ts_user/rust_pro/static_flow/backend/src/main.rs \
        /home/ts_user/rust_pro/static_flow/backend/src/state.rs \
        /home/ts_user/rust_pro/static_flow/backend/src/request_context.rs \
        /home/ts_user/rust_pro/static_flow/backend/src/routes.rs \
        /home/ts_user/rust_pro/static_flow/backend/src/health.rs \
        /home/ts_user/rust_pro/static_flow/gateway/src/main.rs \
        /home/ts_user/rust_pro/static_flow/gateway/src/config.rs \
        /home/ts_user/rust_pro/static_flow/gateway/src/proxy.rs \
        /home/ts_user/rust_pro/static_flow/gateway/src/access_log.rs
```

Expected:
- gateway config check succeeds
- shared/backend/gateway targeted tests pass
- `cargo check` succeeds for the three affected crates
- `cargo clippy ... -D warnings` exits cleanly
- `rustfmt` formats only the changed Rust files

- [ ] **Step 5: Commit**

```bash
git add /home/ts_user/rust_pro/static_flow/conf/pingora/staticflow-gateway.yaml \
        /home/ts_user/rust_pro/static_flow/scripts/pingora_gateway.sh \
        /home/ts_user/rust_pro/static_flow/scripts/backend_gateway_upgrade.sh
git commit -m "feat: add gateway upgrade orchestration scripts"
```
