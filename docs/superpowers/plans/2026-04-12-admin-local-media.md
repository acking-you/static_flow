# Admin Local Media Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an admin-only local media browser and mobile-first video player, with `mkv` normalization, compile-time feature gating, dual startup modes, and canary validation against a real media directory without disturbing the running backend.

**Architecture:** Introduce a dedicated `local-media` feature in both backend and frontend. The backend owns filesystem browsing, path safety, ffprobe/ffmpeg integration, cache/job coordination, and `/admin/local-media/api/*` endpoints. The frontend owns a separate admin browser page and a dedicated player page backed by xgplayer through a small JS bridge. Build/start scripts become feature-aware so both `with local media` and `without local media` variants are first-class and testable.

**Tech Stack:** Rust, Axum, Tokio, Yew, Trunk, shell scripts, ffmpeg-sidecar, xgplayer

---

## File Structure Map

**Root / build plumbing**
- Modify: `/home/ts_user/rust_pro/static_flow/.gitmodules`
- Modify: `/home/ts_user/rust_pro/static_flow/Cargo.toml`
- Modify: `/home/ts_user/rust_pro/static_flow/Makefile`
- Create: `/home/ts_user/rust_pro/static_flow/deps/ffmpeg-sidecar` (git submodule)

**Backend**
- Modify: `/home/ts_user/rust_pro/static_flow/backend/Cargo.toml`
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/main.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/state.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/routes.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/mod.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/config.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/types.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/path_guard.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/fs.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/cache.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/ffmpeg.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/probe.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/jobs.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/playback.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/handlers.rs`

**Frontend**
- Modify: `/home/ts_user/rust_pro/static_flow/frontend/Cargo.toml`
- Modify: `/home/ts_user/rust_pro/static_flow/frontend/index.html`
- Modify: `/home/ts_user/rust_pro/static_flow/frontend/src/api.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/frontend/src/router.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/frontend/src/pages/mod.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/frontend/src/pages/admin.rs`
- Create: `/home/ts_user/rust_pro/static_flow/frontend/src/pages/admin_local_media.rs`
- Create: `/home/ts_user/rust_pro/static_flow/frontend/src/pages/admin_local_media_player.rs`
- Create: `/home/ts_user/rust_pro/static_flow/frontend/src/components/local_media_breadcrumbs.rs`
- Create: `/home/ts_user/rust_pro/static_flow/frontend/src/components/local_media_grid.rs`
- Create: `/home/ts_user/rust_pro/static_flow/frontend/src/components/local_media_preview_tile.rs`
- Create: `/home/ts_user/rust_pro/static_flow/frontend/static/local_media_player_bridge.js`
- Create: `/home/ts_user/rust_pro/static_flow/frontend/static/vendor/xgplayer/`

**Scripts**
- Modify: `/home/ts_user/rust_pro/static_flow/scripts/build_frontend_selfhosted.sh`
- Modify: `/home/ts_user/rust_pro/static_flow/scripts/start_backend_from_tmp.sh`
- Create: `/home/ts_user/rust_pro/static_flow/scripts/start_backend_from_tmp_no_media.sh`
- Modify: `/home/ts_user/rust_pro/static_flow/scripts/start_backend_selfhosted_canary.sh`

---

### Task 1: Add Feature-Aware Dependency And Build Plumbing

**Files:**
- Modify: `/home/ts_user/rust_pro/static_flow/.gitmodules`
- Modify: `/home/ts_user/rust_pro/static_flow/Cargo.toml`
- Modify: `/home/ts_user/rust_pro/static_flow/backend/Cargo.toml`
- Modify: `/home/ts_user/rust_pro/static_flow/frontend/Cargo.toml`
- Modify: `/home/ts_user/rust_pro/static_flow/Makefile`
- Modify: `/home/ts_user/rust_pro/static_flow/scripts/build_frontend_selfhosted.sh`
- Modify: `/home/ts_user/rust_pro/static_flow/scripts/start_backend_from_tmp.sh`
- Create: `/home/ts_user/rust_pro/static_flow/scripts/start_backend_from_tmp_no_media.sh`
- Modify: `/home/ts_user/rust_pro/static_flow/scripts/start_backend_selfhosted_canary.sh`

- [ ] **Step 1: Write the failing shell checks**

Create a temporary verification script or run these direct checks first.

```bash
test -f deps/ffmpeg-sidecar/Cargo.toml
./scripts/start_backend_from_tmp_no_media.sh --help
BACKEND_DEFAULT_FEATURES=0 make -n bin-backend
FRONTEND_DEFAULT_FEATURES=0 ./scripts/build_frontend_selfhosted.sh --help | rg "FRONTEND_DEFAULT_FEATURES"
```

Expected:
- `deps/ffmpeg-sidecar/Cargo.toml` is missing
- `start_backend_from_tmp_no_media.sh` is missing
- `make -n bin-backend` ignores feature env vars
- build script help does not mention frontend feature toggles

- [ ] **Step 2: Verify the current checks fail**

Run:

```bash
test -f deps/ffmpeg-sidecar/Cargo.toml || echo "missing sidecar"
./scripts/start_backend_from_tmp_no_media.sh --help || echo "missing no-media script"
BACKEND_DEFAULT_FEATURES=0 make -n bin-backend
```

Expected:
- The first two commands report missing pieces.
- `make -n bin-backend` still emits a plain `cargo build -p static-flow-backend --profile release-backend`.

- [ ] **Step 3: Add the submodule, optional deps, and feature-aware build/start plumbing**

Add the sidecar submodule and wire backend/frontend features plus build toggles.

```bash
git submodule add git@github.com:acking-you/ffmpeg-sidecar.git deps/ffmpeg-sidecar
```

Update `.gitmodules`:

```ini
[submodule "deps/ffmpeg-sidecar"]
	path = deps/ffmpeg-sidecar
	url = git@github.com:acking-you/ffmpeg-sidecar.git
```

Update root/backend/frontend cargo feature declarations:

```toml
# backend/Cargo.toml
[features]
default = ["local-media"]
local-media = ["dep:ffmpeg-sidecar", "dep:tokio-util", "dep:mime_guess2"]

[dependencies]
ffmpeg-sidecar = { path = "../deps/ffmpeg-sidecar", default-features = false, optional = true }
tokio-util = { version = "0.7", features = ["io"], optional = true }
mime_guess2 = { version = "2.3", optional = true }
```

```toml
# frontend/Cargo.toml
[features]
default = ["local-media"]
local-media = []
mock = []
```

Make `bin-backend` feature-aware:

```make
BACKEND_DEFAULT_FEATURES ?= 1
BACKEND_FEATURES ?=
BACKEND_BIN_NAME ?= static-flow-backend

bin-backend:
	@cmd="cargo build -p static-flow-backend --profile release-backend"; \
	if [ "$(BACKEND_DEFAULT_FEATURES)" = "0" ]; then cmd="$$cmd --no-default-features"; fi; \
	if [ -n "$(BACKEND_FEATURES)" ]; then cmd="$$cmd --features $(BACKEND_FEATURES)"; fi; \
	echo "📦 $$cmd"; \
	eval "$$cmd"; \
	mkdir -p $(BIN_DIR); \
	cp ./target/release-backend/static-flow-backend $(BIN_DIR)/$(BACKEND_BIN_NAME)
```

Expose frontend feature toggles in `build_frontend_selfhosted.sh`:

```bash
FRONTEND_DEFAULT_FEATURES="${FRONTEND_DEFAULT_FEATURES:-1}"
FRONTEND_FEATURES="${FRONTEND_FEATURES:-}"
TRUNK_ARGS=(build --release)
if [[ "$FRONTEND_DEFAULT_FEATURES" == "0" ]]; then
  TRUNK_ARGS+=(--no-default-features)
fi
if [[ -n "$FRONTEND_FEATURES" ]]; then
  TRUNK_ARGS+=(--features "$FRONTEND_FEATURES")
fi
STATICFLOW_API_BASE="/api" trunk "${TRUNK_ARGS[@]}"
```

Make tmp/canary startup scripts build `bin/static-flow-backend` first and support no-media mode.

```bash
# scripts/start_backend_from_tmp_no_media.sh
#!/usr/bin/env bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export BACKEND_DEFAULT_FEATURES=0
export FRONTEND_DEFAULT_FEATURES=0
exec "$ROOT_DIR/scripts/start_backend_from_tmp.sh" "$@"
```

```bash
# canary mode snippet
LOCAL_MEDIA_MODE="${LOCAL_MEDIA_MODE:-enabled}"
if [[ "$LOCAL_MEDIA_MODE" == "disabled" ]]; then
  export BACKEND_DEFAULT_FEATURES=0
  export FRONTEND_DEFAULT_FEATURES=0
  CANARY_BIN_PATH="${CANARY_BIN_PATH:-$ROOT_DIR/bin/static-flow-backend-canary-no-media}"
fi
```

- [ ] **Step 4: Verify the new build plumbing works before touching app code**

Run:

```bash
git submodule status deps/ffmpeg-sidecar
BACKEND_DEFAULT_FEATURES=0 make -n bin-backend
FRONTEND_DEFAULT_FEATURES=0 ./scripts/build_frontend_selfhosted.sh --help
./scripts/start_backend_from_tmp_no_media.sh --help
```

Expected:
- submodule status shows `deps/ffmpeg-sidecar`
- `make -n` now prints `--no-default-features`
- frontend build help references the new feature env vars
- no-media wrapper prints help instead of “file not found”

- [ ] **Step 5: Commit**

```bash
git add .gitmodules Cargo.toml backend/Cargo.toml frontend/Cargo.toml Makefile \
        scripts/build_frontend_selfhosted.sh scripts/start_backend_from_tmp.sh \
        scripts/start_backend_from_tmp_no_media.sh scripts/start_backend_selfhosted_canary.sh \
        deps/ffmpeg-sidecar
git commit -m "feat: add feature-aware local media build plumbing"
```

### Task 2: Add Backend Local-Media Config, State, And Path Guards

**Files:**
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/main.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/state.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/mod.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/config.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/types.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/path_guard.rs`
- Test: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/config.rs`
- Test: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/path_guard.rs`

- [ ] **Step 1: Write failing config and path-guard tests**

Add tests for missing config tolerance, traversal rejection, and symlink escape rejection.

```rust
#[test]
fn read_local_media_config_from_env_allows_missing_root() {
    let cfg = read_local_media_config_for_test(&[]);
    assert!(cfg.root.is_none());
    assert_eq!(cfg.max_transcode_jobs, 1);
}

#[test]
fn resolve_relative_path_rejects_parent_traversal() {
    let root = PathBuf::from("/tmp/staticflow-media-root");
    let err = resolve_media_path(&root, "../secret.mkv").unwrap_err();
    assert!(err.to_string().contains("outside media root"));
}

#[cfg(unix)]
#[test]
fn resolve_relative_path_rejects_symlink_escape() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", root.join("escape")).unwrap();

    let err = resolve_media_path(&root, "escape").unwrap_err();
    assert!(err.to_string().contains("outside media root"));
}
```

- [ ] **Step 2: Run the failing backend tests**

Run:

```bash
cargo test -p static-flow-backend read_local_media_config_from_env_allows_missing_root -- --exact
cargo test -p static-flow-backend resolve_relative_path_rejects_parent_traversal -- --exact
```

Expected: FAIL because the local-media module and helpers do not exist yet.

- [ ] **Step 3: Implement the local-media module skeleton and state wiring**

Create the feature-gated module tree and attach optional runtime state to `AppState`.

```rust
// backend/src/local_media/mod.rs
pub mod cache;
pub mod config;
pub mod ffmpeg;
pub mod fs;
pub mod handlers;
pub mod jobs;
pub mod path_guard;
pub mod playback;
pub mod probe;
pub mod types;

pub use config::{read_local_media_config_from_env, LocalMediaConfig};
pub use types::LocalMediaRuntime;
```

```rust
// backend/src/state.rs
#[cfg(feature = "local-media")]
pub(crate) local_media: Option<Arc<crate::local_media::LocalMediaRuntime>>,
```

```rust
// backend/src/main.rs
#[cfg(feature = "local-media")]
mod local_media;
```

```rust
// backend/src/local_media/config.rs
#[derive(Debug, Clone)]
pub struct LocalMediaConfig {
    pub root: Option<PathBuf>,
    pub cache_dir: PathBuf,
    pub max_list_entries: usize,
    pub max_transcode_jobs: usize,
    pub auto_download_ffmpeg: bool,
    pub ffmpeg_dir: Option<PathBuf>,
    pub ffmpeg_bin: Option<PathBuf>,
    pub ffprobe_bin: Option<PathBuf>,
}
```

- [ ] **Step 4: Re-run the tests and a feature-off compile smoke check**

Run:

```bash
cargo test -p static-flow-backend read_local_media_config_from_env_allows_missing_root -- --exact
cargo test -p static-flow-backend resolve_relative_path_rejects_parent_traversal -- --exact
cargo check -p static-flow-backend --no-default-features
```

Expected:
- the new tests PASS
- `cargo check --no-default-features` succeeds, proving the backend still compiles without local media

- [ ] **Step 5: Commit**

```bash
git add backend/src/main.rs backend/src/state.rs \
        backend/src/local_media/mod.rs backend/src/local_media/config.rs \
        backend/src/local_media/types.rs backend/src/local_media/path_guard.rs
git commit -m "feat: add backend local media config and path guards"
```

### Task 3: Implement Filesystem Listing And Admin List API

**Files:**
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/routes.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/fs.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/handlers.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/types.rs`
- Test: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/fs.rs`
- Test: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/handlers.rs`

- [ ] **Step 1: Write failing listing tests**

Add one filesystem ordering test and one handler-state test.

```rust
#[tokio::test]
async fn list_directory_returns_directories_before_supported_videos() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir(root.join("series")).unwrap();
    std::fs::write(root.join("movie.mkv"), b"mkv").unwrap();
    std::fs::write(root.join("notes.txt"), b"skip").unwrap();

    let items = list_directory(&LocalMediaConfig::for_root(root), "").await.unwrap();

    assert_eq!(items.entries.len(), 2);
    assert_eq!(items.entries[0].kind, LocalMediaEntryKind::Directory);
    assert_eq!(items.entries[1].display_name, "movie.mkv");
}

#[tokio::test]
async fn list_handler_returns_not_configured_when_root_is_missing() {
    let state = app_state_without_local_media_root();
    let response = list_local_media(state, Query(ListQuery { dir: None })).await.unwrap();
    assert_eq!(response.0.status, "not_configured");
}
```

- [ ] **Step 2: Run the failing listing tests**

Run:

```bash
cargo test -p static-flow-backend list_directory_returns_directories_before_supported_videos -- --exact
cargo test -p static-flow-backend list_handler_returns_not_configured_when_root_is_missing -- --exact
```

Expected: FAIL because list logic and handler contracts do not exist yet.

- [ ] **Step 3: Implement listing types, filesystem walking, and `/admin/local-media/api/list`**

Add small DTOs and a shallow per-directory listing path.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum LocalMediaEntryKind {
    Directory,
    Video,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalMediaEntry {
    pub kind: LocalMediaEntryKind,
    pub relative_path: String,
    pub display_name: String,
    pub size_bytes: Option<u64>,
    pub modified_at_ms: Option<i64>,
}

pub async fn list_directory(cfg: &LocalMediaConfig, dir: &str) -> Result<LocalMediaListResponse> {
    // resolve path, read one directory, filter by supported video extensions,
    // sort directories first, then names
}
```

Wire the handler in `routes.rs`:

```rust
#[cfg(feature = "local-media")]
.route("/admin/local-media/api/list", get(crate::local_media::handlers::list_local_media))
```

- [ ] **Step 4: Re-run the listing tests and a handler smoke check**

Run:

```bash
cargo test -p static-flow-backend list_directory_returns_directories_before_supported_videos -- --exact
cargo test -p static-flow-backend list_handler_returns_not_configured_when_root_is_missing -- --exact
cargo check -p static-flow-backend
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add backend/src/routes.rs backend/src/local_media/fs.rs \
        backend/src/local_media/handlers.rs backend/src/local_media/types.rs
git commit -m "feat: add admin local media listing api"
```

### Task 4: Add Probe, Cache Keying, And Job Scheduler

**Files:**
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/cache.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/ffmpeg.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/probe.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/jobs.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/types.rs`
- Test: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/cache.rs`
- Test: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/jobs.rs`

- [ ] **Step 1: Write failing cache and job tests**

```rust
#[test]
fn cache_key_changes_when_source_metadata_changes() {
    let a = build_cache_key("videos/demo.mkv", 100, 1_700_000_000_000, "hls-h264-aac");
    let b = build_cache_key("videos/demo.mkv", 101, 1_700_000_000_000, "hls-h264-aac");
    assert_ne!(a, b);
}

#[tokio::test]
async fn enqueue_prepare_job_deduplicates_same_source() {
    let jobs = LocalMediaJobRegistry::new(1);
    let first = jobs.enqueue("videos/demo.mkv", "cache-a").await.unwrap();
    let second = jobs.enqueue("videos/demo.mkv", "cache-a").await.unwrap();
    assert_eq!(first.job_id, second.job_id);
}

#[tokio::test]
async fn enqueue_prepare_job_rejects_when_capacity_is_full() {
    let jobs = LocalMediaJobRegistry::new(1);
    let _ = jobs.enqueue("videos/a.mkv", "cache-a").await.unwrap();
    let err = jobs.enqueue("videos/b.mkv", "cache-b").await.unwrap_err();
    assert!(err.to_string().contains("busy"));
}
```

- [ ] **Step 2: Run the failing cache/job tests**

Run:

```bash
cargo test -p static-flow-backend cache_key_changes_when_source_metadata_changes -- --exact
cargo test -p static-flow-backend enqueue_prepare_job_deduplicates_same_source -- --exact
```

Expected: FAIL because cache and job modules do not exist.

- [ ] **Step 3: Implement cache-key helpers, ffmpeg path resolution, ffprobe decisions, and bounded jobs**

Use `ffmpeg-sidecar` only for ffmpeg/ffprobe discovery and download setup.

```rust
pub fn build_cache_key(
    relative_path: &str,
    size_bytes: u64,
    modified_at_ms: i64,
    profile: &str,
) -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(relative_path.as_bytes());
    hasher.update(size_bytes.to_le_bytes());
    hasher.update(modified_at_ms.to_le_bytes());
    hasher.update(profile.as_bytes());
    hex::encode(hasher.finalize())
}
```

```rust
pub enum PlaybackPlan {
    Raw { content_type: String },
    HlsCached { playlist_path: PathBuf },
    HlsPrepare { cache_dir: PathBuf, job_id: String },
}
```

```rust
pub struct LocalMediaJobRegistry {
    limit: usize,
    inner: Arc<Mutex<HashMap<String, LocalMediaJobState>>>,
}
```

Keep job state small and disk-backed. Do not store transcoded bytes in memory.

- [ ] **Step 4: Re-run tests and compile checks**

Run:

```bash
cargo test -p static-flow-backend cache_key_changes_when_source_metadata_changes -- --exact
cargo test -p static-flow-backend enqueue_prepare_job_deduplicates_same_source -- --exact
cargo test -p static-flow-backend enqueue_prepare_job_rejects_when_capacity_is_full -- --exact
cargo check -p static-flow-backend
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add backend/src/local_media/cache.rs backend/src/local_media/ffmpeg.rs \
        backend/src/local_media/probe.rs backend/src/local_media/jobs.rs \
        backend/src/local_media/types.rs
git commit -m "feat: add local media probe cache and job scheduler"
```

### Task 5: Implement Playback Open/Status/Raw/HLS Handlers

**Files:**
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/routes.rs`
- Create: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/playback.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/handlers.rs`
- Test: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/playback.rs`
- Test: `/home/ts_user/rust_pro/static_flow/backend/src/local_media/handlers.rs`

- [ ] **Step 1: Write failing playback tests**

```rust
#[tokio::test]
async fn open_playback_returns_preparing_for_uncached_mkv() {
    let runtime = fake_runtime_for_uncached_mkv();
    let response = runtime.open_playback("videos/demo.mkv").await.unwrap();
    assert_eq!(response.mode, "preparing");
    assert!(response.job_id.is_some());
}

#[tokio::test]
async fn stream_raw_file_honors_range_requests_without_buffering_whole_file() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("clip.mp4");
    std::fs::write(&file, b"0123456789").unwrap();

    let response = stream_raw_file(&file, Some("bytes=2-5")).await.unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()["content-range"], "bytes 2-5/10");
}
```

- [ ] **Step 2: Run the failing playback tests**

Run:

```bash
cargo test -p static-flow-backend open_playback_returns_preparing_for_uncached_mkv -- --exact
cargo test -p static-flow-backend stream_raw_file_honors_range_requests_without_buffering_whole_file -- --exact
```

Expected: FAIL because open/raw/hls paths are not implemented.

- [ ] **Step 3: Implement playback planning and handlers**

Add `open`, `job status`, `raw`, and HLS file serving endpoints.

```rust
pub async fn open_playback(
    State(state): State<AppState>,
    Json(req): Json<OpenPlaybackRequest>,
) -> Result<Json<OpenPlaybackResponse>, ApiError> {
    let runtime = require_local_media(&state)?;
    let response = runtime.open_playback(&req.file).await?;
    Ok(Json(response))
}
```

```rust
pub async fn stream_raw_file(path: &Path, range_header: Option<&str>) -> Result<Response, ApiError> {
    let file = tokio::fs::File::open(path).await?;
    // seek + ReaderStream over the selected range, never read the full file
}
```

```rust
#[cfg(feature = "local-media")]
.route("/admin/local-media/api/playback/open", post(crate::local_media::handlers::open_playback))
.route("/admin/local-media/api/playback/jobs/:job_id", get(crate::local_media::handlers::get_playback_job))
.route("/admin/local-media/api/playback/raw/:path_token", get(crate::local_media::handlers::stream_raw_playback))
.route("/admin/local-media/api/playback/hls/:cache_key/index.m3u8", get(crate::local_media::handlers::stream_hls_playlist))
.route("/admin/local-media/api/playback/hls/:cache_key/:segment_name", get(crate::local_media::handlers::stream_hls_segment))
```

- [ ] **Step 4: Re-run playback tests and full backend smoke checks**

Run:

```bash
cargo test -p static-flow-backend open_playback_returns_preparing_for_uncached_mkv -- --exact
cargo test -p static-flow-backend stream_raw_file_honors_range_requests_without_buffering_whole_file -- --exact
cargo check -p static-flow-backend
cargo check -p static-flow-backend --no-default-features
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add backend/src/routes.rs backend/src/local_media/playback.rs \
        backend/src/local_media/handlers.rs
git commit -m "feat: add local media playback handlers"
```

### Task 6: Add Frontend Feature Gates, API Helpers, And Browser Page

**Files:**
- Modify: `/home/ts_user/rust_pro/static_flow/frontend/src/api.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/frontend/src/router.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/frontend/src/pages/mod.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/frontend/src/pages/admin.rs`
- Create: `/home/ts_user/rust_pro/static_flow/frontend/src/pages/admin_local_media.rs`
- Create: `/home/ts_user/rust_pro/static_flow/frontend/src/components/local_media_breadcrumbs.rs`
- Create: `/home/ts_user/rust_pro/static_flow/frontend/src/components/local_media_grid.rs`
- Create: `/home/ts_user/rust_pro/static_flow/frontend/src/components/local_media_preview_tile.rs`
- Test: `/home/ts_user/rust_pro/static_flow/frontend/src/api.rs`

- [ ] **Step 1: Write failing frontend URL/helper tests**

Add pure helper tests for query-string and admin API URL building.

```rust
#[test]
fn build_admin_local_media_list_url_encodes_utf8_dir() {
    let url = build_admin_local_media_list_url(Some("未归类/动漫"));
    assert!(url.contains("%E6%9C%AA%E5%BD%92%E7%B1%BB"));
}

#[test]
fn build_admin_local_media_player_route_keeps_relative_file_query() {
    let route = build_local_media_player_href("movies/demo file.mkv");
    assert!(route.contains("movies%2Fdemo%20file.mkv"));
}
```

- [ ] **Step 2: Run the failing frontend tests/checks**

Run:

```bash
cargo test -p static-flow-frontend build_admin_local_media_list_url_encodes_utf8_dir -- --exact
cargo check -p static-flow-frontend --target wasm32-unknown-unknown --no-default-features
```

Expected: FAIL because the helpers and feature gates do not exist.

- [ ] **Step 3: Implement API helpers, router entries, admin entry link, and the browser page**

Add feature-gated routes:

```rust
#[cfg(all(not(feature = "mock"), feature = "local-media"))]
#[at("/admin/local-media")]
AdminLocalMedia,

#[cfg(all(not(feature = "mock"), feature = "local-media"))]
#[at("/admin/local-media/player")]
AdminLocalMediaPlayer,
```

Add browser page fetch logic:

```rust
#[function_component(AdminLocalMediaPage)]
pub fn admin_local_media_page() -> Html {
    let entries = use_state(Vec::<LocalMediaEntry>::new);
    let current_dir = use_state(String::new);
    let loading = use_state(|| true);
    let error = use_state(|| None::<String>);
    // fetch /admin/local-media/api/list?dir=...
}
```

Add a small admin entry in `admin.rs` only when `feature = "local-media"`:

```rust
#[cfg(feature = "local-media")]
html! {
    <Link<Route> to={Route::AdminLocalMedia} classes="admin-quick-link">
        <i class="fas fa-film"></i>
        { "Local Media" }
    </Link<Route>>
}
```

- [ ] **Step 4: Re-run frontend tests and compile checks**

Run:

```bash
cargo test -p static-flow-frontend build_admin_local_media_list_url_encodes_utf8_dir -- --exact
cargo check -p static-flow-frontend --target wasm32-unknown-unknown
cargo check -p static-flow-frontend --target wasm32-unknown-unknown --no-default-features
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add frontend/src/api.rs frontend/src/router.rs frontend/src/pages/mod.rs \
        frontend/src/pages/admin.rs frontend/src/pages/admin_local_media.rs \
        frontend/src/components/local_media_breadcrumbs.rs \
        frontend/src/components/local_media_grid.rs \
        frontend/src/components/local_media_preview_tile.rs
git commit -m "feat: add frontend admin local media browser"
```

### Task 7: Add xgplayer Bridge And Dedicated Player Page

**Files:**
- Modify: `/home/ts_user/rust_pro/static_flow/frontend/index.html`
- Create: `/home/ts_user/rust_pro/static_flow/frontend/src/pages/admin_local_media_player.rs`
- Create: `/home/ts_user/rust_pro/static_flow/frontend/static/local_media_player_bridge.js`
- Create: `/home/ts_user/rust_pro/static_flow/frontend/static/vendor/xgplayer/`
- Test: `/home/ts_user/rust_pro/static_flow/frontend/src/pages/admin_local_media_player.rs`

- [ ] **Step 1: Write failing player helper tests**

Add pure tests for playback progress keys and sibling navigation helpers.

```rust
#[test]
fn playback_progress_key_is_stable_for_relative_file() {
    assert_eq!(
        playback_progress_key("未归类/demo.mkv"),
        "local-media-progress:未归类/demo.mkv"
    );
}

#[test]
fn sibling_lookup_ignores_current_file() {
    let files = vec!["a.mkv".to_string(), "b.mkv".to_string(), "c.mkv".to_string()];
    let nav = find_siblings(&files, "b.mkv");
    assert_eq!(nav.prev.as_deref(), Some("a.mkv"));
    assert_eq!(nav.next.as_deref(), Some("c.mkv"));
}
```

- [ ] **Step 2: Run the failing player tests**

Run:

```bash
cargo test -p static-flow-frontend playback_progress_key_is_stable_for_relative_file -- --exact
```

Expected: FAIL because the player page and helpers do not exist.

- [ ] **Step 3: Implement the static bridge and the player page**

Expose xgplayer through static assets and a tiny bridge.

```html
<!-- frontend/index.html -->
<script src="/static/vendor/xgplayer/xgplayer.min.js"></script>
<script src="/static/vendor/xgplayer/xgplayer-hls.min.js"></script>
<script src="/static/local_media_player_bridge.js"></script>
```

```javascript
// frontend/static/local_media_player_bridge.js
window.StaticFlowLocalMediaPlayer = {
  mount(containerId, options) {
    const target = document.getElementById(containerId);
    const player = new window.Player({
      id: containerId,
      url: options.url,
      playsinline: true,
      autoplay: false,
      lang: 'zh-cn',
      fluid: true,
      videoInit: true,
      playbackRate: [0.75, 1, 1.25, 1.5, 2],
      ...options.extra
    });
    return player;
  }
};
```

```rust
#[function_component(AdminLocalMediaPlayerPage)]
pub fn admin_local_media_player_page() -> Html {
    let playback = use_state(|| None::<OpenPlaybackResponse>);
    let preparing = use_state(|| false);
    let error = use_state(|| None::<String>);
    // call open endpoint, poll job status when needed, mount xgplayer bridge
}
```

- [ ] **Step 4: Re-run player tests and frontend compile checks**

Run:

```bash
cargo test -p static-flow-frontend playback_progress_key_is_stable_for_relative_file -- --exact
cargo check -p static-flow-frontend --target wasm32-unknown-unknown
cargo check -p static-flow-frontend --target wasm32-unknown-unknown --no-default-features
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add frontend/index.html frontend/src/pages/admin_local_media_player.rs \
        frontend/static/local_media_player_bridge.js frontend/static/vendor/xgplayer
git commit -m "feat: add local media player page and xgplayer bridge"
```

### Task 8: Run Formatting, Linting, And Real Canary Validation

**Files:**
- Modify if needed after verification: any files touched in Tasks 1-7

- [ ] **Step 1: Format only the modified Rust files**

Run `rustfmt` only on changed Rust files, never workspace-wide.

```bash
rustfmt backend/src/main.rs backend/src/state.rs backend/src/routes.rs \
        backend/src/local_media/*.rs \
        frontend/src/api.rs frontend/src/router.rs frontend/src/pages/mod.rs \
        frontend/src/pages/admin.rs frontend/src/pages/admin_local_media.rs \
        frontend/src/pages/admin_local_media_player.rs \
        frontend/src/components/local_media_breadcrumbs.rs \
        frontend/src/components/local_media_grid.rs \
        frontend/src/components/local_media_preview_tile.rs
```

Expected: formatting changes only in touched files.

- [ ] **Step 2: Run clippy and compile checks for both feature modes**

Run:

```bash
cargo clippy -p static-flow-backend --all-targets -- -D warnings
cargo clippy -p static-flow-frontend --target wasm32-unknown-unknown -- -D warnings
cargo clippy -p static-flow-backend --all-targets --no-default-features -- -D warnings
cargo check -p static-flow-frontend --target wasm32-unknown-unknown --no-default-features
```

Expected: zero warnings/errors.

- [ ] **Step 3: Build and start the media-enabled canary against the real directory**

Run:

```bash
STATICFLOW_LOCAL_MEDIA_ROOT="/mnt/e/videos/static/未归类" \
STATICFLOW_LOCAL_MEDIA_CACHE_DIR="/home/ts_user/rust_pro/static_flow/tmp/local-media-cache-canary" \
STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG=1 \
LOCAL_MEDIA_MODE=enabled \
./scripts/start_backend_selfhosted_canary.sh --build --build-frontend --daemon --port 39081
```

Then verify:

```bash
curl -fsS "http://127.0.0.1:39081/admin/local-media/api/list"
curl -fsS -X POST "http://127.0.0.1:39081/admin/local-media/api/playback/open" \
  -H "Content-Type: application/json" \
  -d '{"file":"<replace-with-real-relative-file-from-list>"}'
```

Expected:
- canary starts on `39081`
- list endpoint returns real directory entries from `/mnt/e/videos/static/未归类`
- opening at least one `mkv` returns `preparing` or `hls`

- [ ] **Step 4: Start the no-media canary and verify it stays healthy**

Run:

```bash
LOCAL_MEDIA_MODE=disabled \
./scripts/start_backend_selfhosted_canary.sh --build --build-frontend --daemon --port 39082
curl -fsS "http://127.0.0.1:39082/api/articles" >/dev/null
curl -sS -o /dev/null -w "%{http_code}\n" "http://127.0.0.1:39082/admin/local-media/api/list"
```

Expected:
- backend starts on `39082`
- public/article APIs still work
- local-media route is absent or returns a controlled non-existent result rather than crashing the backend

- [ ] **Step 5: Inspect logs and ensure the existing backend was not disturbed**

Run:

```bash
tail -n 200 /home/ts_user/rust_pro/static_flow/tmp/staticflow-backend-canary.log
ss -tlnp | rg "39081|39082|39080"
```

Expected:
- canary logs show only the canary process
- existing main backend port remains separate

- [ ] **Step 6: Commit**

```bash
git add backend frontend scripts Makefile Cargo.toml .gitmodules
git commit -m "feat: add admin local media browser and player"
```

---

## Self-Review

### Spec Coverage

- Admin-only routing: covered in Tasks 3, 5, 6, and 7.
- Dedicated modules and feature split: covered in Tasks 1 and 2.
- `mkv` normalization via ffprobe/ffmpeg: covered in Tasks 4 and 5.
- Memory constraints and disk cache only: covered in Tasks 4 and 5, plus canary verification in Task 8.
- Dual startup modes and canary validation: covered in Tasks 1 and 8.
- Real media directory verification: covered in Task 8.

### Placeholder Scan

- No `TODO`/`TBD` placeholders remain.
- Every code-changing task names exact files and includes concrete code snippets.
- Every verification task has exact commands and expected outcomes.

### Type Consistency

- Feature name is consistently `local-media`.
- Runtime env names are consistently prefixed with `STATICFLOW_LOCAL_MEDIA_`.
- The playback flow consistently uses `open -> job status -> raw/hls`.
