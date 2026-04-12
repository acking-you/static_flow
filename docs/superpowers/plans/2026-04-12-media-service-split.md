# Media Service Split Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split admin local-media out of `static-flow-backend` into a standalone `static-flow-media` binary while keeping the browser contract at `/admin/local-media/*` unchanged through authenticated reverse proxying in the main backend.

**Architecture:** Introduce a new workspace member `media-service/` that owns all media implementation details: browsing, ffprobe/ffmpeg, HLS, poster generation, job state, and cache management. Convert the main backend into a thin proxy for `/admin/local-media/api/*`, preserving admin auth and existing frontend behavior while removing media execution from backend startup.

**Tech Stack:** Rust, Axum, Tokio, reqwest, ffmpeg-sidecar, shell scripts, Yew frontend unchanged

---

## File Structure Map

**Workspace / build plumbing**
- Modify: `/home/ts_user/rust_pro/static_flow/Cargo.toml`
- Modify: `/home/ts_user/rust_pro/static_flow/Makefile`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/Cargo.toml`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/lib.rs`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/main.rs`

**Media service crate**
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/config.rs`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/state.rs`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/routes.rs`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/handlers.rs`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/cache.rs`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/ffmpeg.rs`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/fs.rs`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/jobs.rs`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/path_guard.rs`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/playback.rs`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/poster.rs`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/probe.rs`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/types.rs`
- Move then delete: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/`

**Backend proxy**
- Modify: `/home/ts_user/rust_pro/static_flow/backend/Cargo.toml`
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/main.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/state.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/routes.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/media_proxy/mod.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/media_proxy/config.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/media_proxy/forward.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/media_proxy/handlers.rs`

**Scripts**
- Modify: `/home/ts_user/rust_pro/static_flow/scripts/start_backend_from_tmp.sh`
- Modify: `/home/ts_user/rust_pro/static_flow/scripts/start_backend_from_tmp_no_media.sh`
- Modify: `/home/ts_user/rust_pro/static_flow/scripts/start_backend_selfhosted_canary.sh`
- Create: `/home/ts_user/rust_pro/static_flow/scripts/start_media_service_from_tmp.sh`
- Create: `/home/ts_user/rust_pro/static_flow/scripts/start_media_service_canary.sh`
- Create: `/home/ts_user/rust_pro/static_flow/scripts/start_backend_with_media_from_tmp.sh`

**Docs / verification**
- Modify: `/home/ts_user/rust_pro/static_flow/docs/superpowers/specs/2026-04-12-media-service-split-design.md` only if the implementation reveals a real contradiction

---

### Task 1: Add Workspace Member And Media Crate Skeleton

**Files:**
- Modify: `/home/ts_user/rust_pro/static_flow/Cargo.toml`
- Modify: `/home/ts_user/rust_pro/static_flow/Makefile`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/Cargo.toml`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/lib.rs`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/main.rs`

- [ ] **Step 1: Write the failing package checks**

Run:

```bash
cargo check -p static-flow-media
make -n bin-media
test -f /home/ts_user/rust_pro/static_flow/media-service/Cargo.toml
```

Expected:
- `cargo check -p static-flow-media` fails with `package ID specification 'static-flow-media' did not match any packages`
- `make -n bin-media` fails because the target does not exist
- `test -f .../media-service/Cargo.toml` fails because the crate does not exist

- [ ] **Step 2: Verify the current failure modes**

Run:

```bash
cargo check -p static-flow-media || echo "missing media-service crate"
make -n bin-media || echo "missing bin-media target"
test -f /home/ts_user/rust_pro/static_flow/media-service/Cargo.toml || echo "missing manifest"
```

Expected:
- all three checks fail for the reasons above

- [ ] **Step 3: Add the new workspace member and build target**

Update the root workspace list:

```toml
[workspace]
resolver = "2"
members = ["frontend", "shared", "backend", "cli", "media-service"]
exclude = ["deps/lance", "deps/lancedb"]
```

Add a `bin-media` target to `Makefile` mirroring `bin-backend`:

```make
MEDIA_BIN_NAME ?= static-flow-media

bin-media:
	@cmd="cargo build -p static-flow-media --profile release-backend"; \
	echo "📦 $$cmd"; \
	eval "$$cmd"; \
	mkdir -p $(BIN_DIR); \
	cp ./target/release-backend/static-flow-media $(BIN_DIR)/$(MEDIA_BIN_NAME)
```

Create the crate manifest:

```toml
[package]
name = "static-flow-media"
version = "0.1.0"
edition = "2021"
publish = false

[features]
default = []

[lints]
workspace = true

[dependencies]
axum = { workspace = true }
tokio = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
anyhow = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
reqwest = { workspace = true }
sha2 = { workspace = true }
urlencoding = "2.1"
dashmap = "6.1"
ffmpeg-sidecar = { path = "../deps/ffmpeg-sidecar", default-features = false, features = ["download_ffmpeg"] }
tokio-util = { version = "0.7", features = ["io"] }
mime_guess2 = "2.3"
```

Create the initial library skeleton:

```rust
// media-service/src/lib.rs
pub mod cache;
pub mod config;
pub mod ffmpeg;
pub mod fs;
pub mod handlers;
pub mod jobs;
pub mod path_guard;
pub mod playback;
pub mod poster;
pub mod probe;
pub mod routes;
pub mod state;
pub mod types;
```

Create the initial binary skeleton:

```rust
// media-service/src/main.rs
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().compact().init();
    anyhow::bail!("media service not implemented yet")
}
```

- [ ] **Step 4: Verify the new package is discoverable**

Run:

```bash
cargo metadata --no-deps --format-version 1 | jq -r '.packages[].name' | rg '^static-flow-media$'
cargo check -p static-flow-media
make -n bin-media
```

Expected:
- `cargo metadata` lists `static-flow-media`
- `cargo check -p static-flow-media` reaches `main.rs` and fails only because the binary intentionally bails or modules are still empty
- `make -n bin-media` prints a `cargo build -p static-flow-media` command

- [ ] **Step 5: Commit**

```bash
git add /home/ts_user/rust_pro/static_flow/Cargo.toml \
        /home/ts_user/rust_pro/static_flow/Makefile \
        /home/ts_user/rust_pro/static_flow/media-service
git commit -m "feat: add media service crate skeleton"
```

### Task 2: Move The Media Implementation Into The New Crate

**Files:**
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/config.rs`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/state.rs`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/routes.rs`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/handlers.rs`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/cache.rs`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/ffmpeg.rs`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/fs.rs`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/jobs.rs`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/path_guard.rs`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/playback.rs`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/poster.rs`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/probe.rs`
- Create: `/home/ts_user/rust_pro/static_flow/media-service/src/types.rs`
- Move then delete: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/`

- [ ] **Step 1: Write the failing media-service tests**

Before moving code, create a small smoke test file in the new crate that references the migrated API:

```rust
// media-service/src/config.rs
#[cfg(test)]
mod tests {
    use super::read_local_media_config_for_test;

    #[test]
    fn read_local_media_config_from_env_allows_missing_root() {
        let cfg = read_local_media_config_for_test(&[]).expect("config should parse");
        assert!(cfg.root.is_none());
    }
}
```

Then run:

```bash
cargo test -p static-flow-media read_local_media_config_from_env_allows_missing_root -- --nocapture
```

Expected:
- fail with unresolved import or missing function because the module has not been migrated yet

- [ ] **Step 2: Verify the failure is about missing implementation**

Run:

```bash
cargo test -p static-flow-media read_local_media_config_from_env_allows_missing_root -- --nocapture || true
```

Expected:
- failure references missing module items, not unrelated build errors

- [ ] **Step 3: Copy the current media implementation into the new crate and normalize module paths**

Use the current backend implementation as the source of truth:

```bash
cp /home/ts_user/rust_pro/static_flow/backend/src/local_media/*.rs \
   /home/ts_user/rust_pro/static_flow/media-service/src/
```

Adjust module imports from backend-local paths to crate-local paths. The new
`state.rs` should contain the current `LocalMediaState` implementation:

```rust
// media-service/src/state.rs
#[derive(Clone)]
pub struct LocalMediaState {
    config: LocalMediaConfig,
    root_dir: PathBuf,
    cache_dir: PathBuf,
    transcode_limiter: Arc<Semaphore>,
    poster_limiter: Arc<Semaphore>,
    jobs: Arc<DashMap<String, Arc<PlaybackJobHandle>>>,
}
```

Create a crate-local route builder:

```rust
// media-service/src/routes.rs
pub fn create_router(state: Arc<LocalMediaState>) -> Router {
    Router::new()
        .route("/internal/local-media/list", get(handlers::list_local_media))
        .route("/internal/local-media/playback/open", post(handlers::open_local_media_playback))
        .route("/internal/local-media/playback/jobs/:job_id", get(handlers::get_local_media_job_status))
        .route("/internal/local-media/playback/raw", get(handlers::stream_local_media_raw))
        .route("/internal/local-media/playback/hls/:job_id/:file_name", get(handlers::stream_local_media_hls_artifact))
        .route("/internal/local-media/poster", get(handlers::stream_local_media_poster))
        .with_state(state)
}
```

Create a crate-local `main.rs` that binds `HOST` and `PORT`:

```rust
use std::env;

use anyhow::Result;
use static_flow_media::{routes, state::LocalMediaState};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().compact().init();
    let state = LocalMediaState::from_env()
        .await?
        .ok_or_else(|| anyhow::anyhow!("local media root is not configured"))?;
    let host = env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = env::var("PORT").unwrap_or_else(|_| "39085".to_string());
    let listener = tokio::net::TcpListener::bind(format!("{host}:{port}")).await?;
    axum::serve(listener, routes::create_router(state)).await?;
    Ok(())
}
```

- [ ] **Step 4: Run the migrated test suite in the new crate**

Run:

```bash
cargo test -p static-flow-media -- --nocapture
```

Expected:
- migrated config/path-guard/playback/ffmpeg tests pass inside `static-flow-media`

- [ ] **Step 5: Remove backend-local media implementation references only after the new crate compiles**

Delete the old backend module directory after the new crate is green:

```bash
rm -rf /home/ts_user/rust_pro/static_flow/backend/src/local_media
```

Do not update backend imports yet in this task. The backend is expected to fail
until proxy code is added.

- [ ] **Step 6: Commit**

```bash
git add /home/ts_user/rust_pro/static_flow/media-service \
        /home/ts_user/rust_pro/static_flow/backend/src/local_media
git commit -m "feat: move local media implementation into media service crate"
```

### Task 3: Add Media-Service HTTP Startup And Real Route Coverage

**Files:**
- Modify: `/home/ts_user/rust_pro/static_flow/media-service/src/main.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/media-service/src/routes.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/media-service/src/handlers.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/media-service/src/types.rs`
- Test: `/home/ts_user/rust_pro/static_flow/media-service/src/routes.rs`

- [ ] **Step 1: Write a failing route smoke test**

Add a tiny integration-style test that proves one internal route exists:

```rust
#[tokio::test]
async fn media_router_registers_internal_list_route() {
    let app = create_router(test_state()).into_service();
    let response = app
        .oneshot(
            http::Request::builder()
                .uri("/internal/local-media/list")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(response.status(), http::StatusCode::NOT_FOUND);
}
```

Run:

```bash
cargo test -p static-flow-media media_router_registers_internal_list_route -- --nocapture
```

Expected:
- fail until the router and test helper exist

- [ ] **Step 2: Verify the failure is route-related**

Run the same command and confirm the failure is `cannot find function create_router`
or `not found`, not unrelated env/config issues.

- [ ] **Step 3: Finish the internal HTTP surface**

Make handlers service-internal, with no admin auth logic. The list handler
should become:

```rust
pub async fn list_local_media(
    State(state): State<Arc<LocalMediaState>>,
    Query(query): Query<LocalMediaListQuery>,
) -> Result<Json<LocalMediaListResponse>, ErrorResponse> {
    let limit = query.limit.unwrap_or(state.config().list_page_size).clamp(1, 500);
    let offset = query.offset.unwrap_or(0);
    let response = list_directory(state.as_ref(), query.dir.as_deref(), limit, offset).await?;
    Ok(Json(response))
}
```

All current media logic should remain here:

- list
- playback/open
- playback/jobs
- raw
- hls artifact
- poster

- [ ] **Step 4: Verify the media binary can boot standalone**

Run:

```bash
STATICFLOW_LOCAL_MEDIA_ROOT='/mnt/e/videos/static/未归类' \
HOST=127.0.0.1 PORT=39085 \
cargo run -p static-flow-media
```

Expected:
- service binds successfully
- log shows the configured root and cache dir

- [ ] **Step 5: Verify one internal API call directly**

In another terminal:

```bash
curl -sS 'http://127.0.0.1:39085/internal/local-media/list?limit=2'
```

Expected:
- JSON response with `configured: true`
- entries populated from `/mnt/e/videos/static/未归类`

- [ ] **Step 6: Commit**

```bash
git add /home/ts_user/rust_pro/static_flow/media-service/src
git commit -m "feat: add standalone media service routes"
```

### Task 4: Replace Backend In-Process Media With Proxy Config And Handlers

**Files:**
- Modify: `/home/ts_user/rust_pro/static_flow/backend/Cargo.toml`
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/main.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/state.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/routes.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/media_proxy/mod.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/media_proxy/config.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/media_proxy/forward.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/media_proxy/handlers.rs`

- [ ] **Step 1: Write the failing backend proxy tests**

Add one pure config test and one forwarding test.

Config test:

```rust
#[test]
fn media_proxy_config_reads_base_url() {
    let cfg = read_media_proxy_config_for_test(&[
        ("STATICFLOW_MEDIA_PROXY_BASE_URL", "http://127.0.0.1:39085"),
    ]).unwrap();
    assert_eq!(cfg.base_url.as_str(), "http://127.0.0.1:39085/");
}
```

Forwarding test:

```rust
#[tokio::test]
async fn forward_raw_request_preserves_range_header() {
    let upstream = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/internal/local-media/playback/raw"))
        .and(wiremock::matchers::header("range", "bytes=0-15"))
        .respond_with(
            wiremock::ResponseTemplate::new(206)
                .insert_header("content-range", "bytes 0-15/100")
                .set_body_bytes(b"0123456789abcdef"),
        )
        .mount(&upstream)
        .await;
    // call forward_raw_request(...) here
}
```

Run:

```bash
cargo test -p static-flow-backend media_proxy -- --nocapture
```

Expected:
- fail because proxy config and forwarding modules do not exist yet

- [ ] **Step 2: Verify the failures point at missing proxy pieces**

Run the same command and confirm failures are about missing modules/functions,
not the removed local-media implementation.

- [ ] **Step 3: Add backend proxy config and state**

Create a tiny proxy state:

```rust
// backend/src/media_proxy/config.rs
#[derive(Debug, Clone)]
pub struct MediaProxyConfig {
    pub base_url: reqwest::Url,
}
```

```rust
// backend/src/media_proxy/mod.rs
pub mod config;
pub mod forward;
pub mod handlers;

#[derive(Clone)]
pub struct MediaProxyState {
    pub client: reqwest::Client,
    pub config: config::MediaProxyConfig,
}
```

Wire it into `AppState`:

```rust
#[cfg(feature = "local-media")]
pub(crate) media_proxy: Option<Arc<MediaProxyState>>,
```

and initialize it in `AppState::new()` instead of `LocalMediaState::from_env()`.

- [ ] **Step 4: Add streaming and JSON proxy forwarding**

The forwarding helper should look like:

```rust
pub async fn forward(
    client: &reqwest::Client,
    upstream: reqwest::RequestBuilder,
) -> Result<axum::response::Response, ProxyError> {
    let upstream = upstream.send().await?;
    let status = upstream.status();
    let mut builder = axum::response::Response::builder().status(status);
    for name in ["content-type", "content-length", "content-range", "accept-ranges", "cache-control"] {
        if let Some(value) = upstream.headers().get(name) {
            builder = builder.header(name, value);
        }
    }
    Ok(builder.body(axum::body::Body::from_stream(upstream.bytes_stream())).unwrap())
}
```

Handler rule:

- run existing `ensure_admin_access(...)`
- map `/admin/local-media/api/...` to `/internal/local-media/...`
- pass query/body/`Range` through

- [ ] **Step 5: Replace backend route wiring**

In `backend/src/main.rs`, replace:

```rust
#[cfg(feature = "local-media")]
mod local_media;
```

with:

```rust
#[cfg(feature = "local-media")]
mod media_proxy;
```

In `backend/src/routes.rs`, keep the existing public URLs but point them to
proxy handlers instead of in-process handlers.

- [ ] **Step 6: Verify backend proxy mode**

Run:

```bash
cargo check -p static-flow-backend
cargo check -p static-flow-backend --no-default-features
cargo test -p static-flow-backend media_proxy -- --nocapture
```

Expected:
- backend compiles in both feature modes
- proxy tests pass
- backend no longer depends on local-media implementation files

- [ ] **Step 7: Commit**

```bash
git add /home/ts_user/rust_pro/static_flow/backend/Cargo.toml \
        /home/ts_user/rust_pro/static_flow/backend/src/main.rs \
        /home/ts_user/rust_pro/static_flow/backend/src/state.rs \
        /home/ts_user/rust_pro/static_flow/backend/src/routes.rs \
        /home/ts_user/rust_pro/static_flow/backend/src/media_proxy
git commit -m "feat: proxy local media through media service"
```

### Task 5: Split Startup Scripts Into Backend-Only, Media-Only, And Combined Modes

**Files:**
- Modify: `/home/ts_user/rust_pro/static_flow/Makefile`
- Modify: `/home/ts_user/rust_pro/static_flow/scripts/start_backend_from_tmp.sh`
- Modify: `/home/ts_user/rust_pro/static_flow/scripts/start_backend_from_tmp_no_media.sh`
- Modify: `/home/ts_user/rust_pro/static_flow/scripts/start_backend_selfhosted_canary.sh`
- Create: `/home/ts_user/rust_pro/static_flow/scripts/start_media_service_from_tmp.sh`
- Create: `/home/ts_user/rust_pro/static_flow/scripts/start_media_service_canary.sh`
- Create: `/home/ts_user/rust_pro/static_flow/scripts/start_backend_with_media_from_tmp.sh`

- [ ] **Step 1: Write the failing script checks**

Run:

```bash
./scripts/start_media_service_from_tmp.sh --help
./scripts/start_media_service_canary.sh --help
./scripts/start_backend_with_media_from_tmp.sh --help
```

Expected:
- all three fail because the scripts do not exist

- [ ] **Step 2: Verify the missing-script failures**

Run:

```bash
for s in start_media_service_from_tmp.sh start_media_service_canary.sh start_backend_with_media_from_tmp.sh; do
  ./scripts/$s --help || echo "missing $s"
done
```

Expected:
- each command prints `missing ...`

- [ ] **Step 3: Add a standalone media-service tmp startup script**

Create:

```bash
#!/usr/bin/env bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"
HOST="${HOST:-127.0.0.1}"
PORT="${PORT:-39085}"
STATICFLOW_LOCAL_MEDIA_ROOT="${STATICFLOW_LOCAL_MEDIA_ROOT:-}"
STATICFLOW_LOCAL_MEDIA_CACHE_DIR="${STATICFLOW_LOCAL_MEDIA_CACHE_DIR:-$ROOT_DIR/tmp/local-media-cache}"
make bin-media >/dev/null
exec "$ROOT_DIR/bin/static-flow-media"
```

- [ ] **Step 4: Add a combined backend+media wrapper**

Create:

```bash
#!/usr/bin/env bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MEDIA_HOST="${MEDIA_HOST:-127.0.0.1}"
MEDIA_PORT="${MEDIA_PORT:-39085}"
"$ROOT_DIR/scripts/start_media_service_from_tmp.sh" &
export STATICFLOW_MEDIA_PROXY_BASE_URL="http://${MEDIA_HOST}:${MEDIA_PORT}"
exec "$ROOT_DIR/scripts/start_backend_from_tmp.sh"
```

Refactor `start_backend_from_tmp.sh` so it no longer exports:

- `STATICFLOW_LOCAL_MEDIA_ROOT`
- `STATICFLOW_LOCAL_MEDIA_CACHE_DIR`
- `STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG`

and instead only uses:

- `STATICFLOW_MEDIA_PROXY_BASE_URL`

- [ ] **Step 5: Add canary support**

`start_media_service_canary.sh` should mirror backend canary:

```bash
PORT="${PORT:-39086}"
LOG_FILE="${LOG_FILE:-$ROOT_DIR/tmp/staticflow-media-canary.log}"
PID_FILE="${PID_FILE:-$ROOT_DIR/tmp/staticflow-media-canary.pid}"
```

`start_backend_selfhosted_canary.sh` should accept:

```bash
STATICFLOW_MEDIA_PROXY_BASE_URL="${STATICFLOW_MEDIA_PROXY_BASE_URL:-http://127.0.0.1:39086}"
```

and should stop talking about local-media root/cache env directly.

- [ ] **Step 6: Verify the new process split**

Run:

```bash
./scripts/start_media_service_from_tmp.sh --help
./scripts/start_backend_with_media_from_tmp.sh --help || true
PORT=39086 ./scripts/start_media_service_canary.sh --daemon
STATICFLOW_MEDIA_PROXY_BASE_URL=http://127.0.0.1:39086 PORT=39081 ./scripts/start_backend_selfhosted_canary.sh --daemon
```

Expected:
- media scripts exist and print sane help or start successfully
- backend canary starts without direct media-root env
- media canary owns the media-specific env

- [ ] **Step 7: Commit**

```bash
git add /home/ts_user/rust_pro/static_flow/Makefile \
        /home/ts_user/rust_pro/static_flow/scripts/start_backend_from_tmp.sh \
        /home/ts_user/rust_pro/static_flow/scripts/start_backend_from_tmp_no_media.sh \
        /home/ts_user/rust_pro/static_flow/scripts/start_backend_selfhosted_canary.sh \
        /home/ts_user/rust_pro/static_flow/scripts/start_media_service_from_tmp.sh \
        /home/ts_user/rust_pro/static_flow/scripts/start_media_service_canary.sh \
        /home/ts_user/rust_pro/static_flow/scripts/start_backend_with_media_from_tmp.sh
git commit -m "feat: split media and backend startup flows"
```

### Task 6: End-To-End Validation With Real Media Directory

**Files:**
- Modify only if verification reveals a real bug: exact file depends on failure

- [ ] **Step 1: Start the media-service canary with the real directory**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow
PORT=39086 \
STATICFLOW_LOCAL_MEDIA_ROOT='/mnt/e/videos/static/未归类' \
STATICFLOW_LOCAL_MEDIA_CACHE_DIR="$PWD/tmp/local-media-cache-canary-split" \
./scripts/start_media_service_canary.sh --daemon
```

Expected:
- media-service canary binds `127.0.0.1:39086`

- [ ] **Step 2: Start the backend canary pointed at the media-service canary**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow
PORT=39081 \
STATICFLOW_MEDIA_PROXY_BASE_URL='http://127.0.0.1:39086' \
./scripts/start_backend_selfhosted_canary.sh --build --build-frontend --daemon
```

Expected:
- backend canary binds `127.0.0.1:39081`
- no direct media-root env is required by backend startup

- [ ] **Step 3: Verify API passthrough manually**

Run:

```bash
curl -sS 'http://127.0.0.1:39081/admin/local-media/api/list?limit=2'
curl -sS -D - -o /dev/null 'http://127.0.0.1:39081/admin/local-media/api/poster?file=89688-sc-1080p.mp4'
curl -sS -X POST 'http://127.0.0.1:39081/admin/local-media/api/playback/open' \
  -H 'Content-Type: application/json' \
  --data '{"file":"UCDownload_temp(8).realesrgan.mkv"}'
```

Expected:
- list returns entries
- poster returns `200 OK` with `content-type: image/jpeg`
- playback open returns `ready` or `preparing` with a stable job id

- [ ] **Step 4: Verify browser behavior through the backend only**

Open:

```text
http://127.0.0.1:39081/admin/local-media
http://127.0.0.1:39081/admin/local-media/player?file=89688-sc-1080p.mp4
```

Expected:
- list page loads through backend
- poster images appear
- mp4 playback works
- mkv playback and HLS work through proxy

- [ ] **Step 5: Run mandatory quality gates**

Run:

```bash
cargo check -p static-flow-media
cargo test -p static-flow-media -- --nocapture
cargo check -p static-flow-backend
cargo check -p static-flow-backend --no-default-features
cargo check -p static-flow-frontend --target wasm32-unknown-unknown
cargo check -p static-flow-frontend --target wasm32-unknown-unknown --no-default-features
cargo clippy -p static-flow-media --all-targets -- -D warnings
cargo clippy -p static-flow-backend --all-targets -- -D warnings
cargo clippy -p static-flow-frontend --target wasm32-unknown-unknown -- -D warnings
```

Expected:
- all checks pass
- no warnings remain

- [ ] **Step 6: Shut down canaries cleanly**

Run:

```bash
kill $(cat /home/ts_user/rust_pro/static_flow/tmp/staticflow-media-canary.pid)
kill $(cat /home/ts_user/rust_pro/static_flow/tmp/staticflow-backend-canary.pid)
rm -f /home/ts_user/rust_pro/static_flow/tmp/staticflow-media-canary.pid
rm -f /home/ts_user/rust_pro/static_flow/tmp/staticflow-backend-canary.pid
```

Expected:
- both processes stop
- no canary pid files remain

- [ ] **Step 7: Commit**

```bash
git add /home/ts_user/rust_pro/static_flow
git commit -m "feat: split local media into standalone service"
```

## Self-Review

### Spec Coverage

- New workspace crate and binary: covered by Tasks 1-3.
- Main backend reverse proxy under existing admin routes: covered by Task 4.
- Script/process split: covered by Task 5.
- Real canary validation against `/mnt/e/videos/static/未归类`: covered by Task 6.
- Feature-enabled and feature-disabled builds: covered by Task 4 and Task 6 quality gates.
- Streaming and memory boundary preservation: covered by Task 4 forwarding step and Task 6 API verification.

### Placeholder Scan

- No unresolved placeholder markers remain.
- Every task contains exact file paths and concrete commands.
- Proxy behavior and script behavior are specified explicitly, not by reference to another task.

### Type Consistency

- Browser-facing route prefix remains `/admin/local-media/*`.
- Media-service internal route prefix remains `/internal/local-media/*`.
- Backend config key is consistently `STATICFLOW_MEDIA_PROXY_BASE_URL`.
- Media service keeps `STATICFLOW_LOCAL_MEDIA_*` env keys.
