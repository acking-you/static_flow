# Media Service Split Design

Date: 2026-04-12

## Summary

Move all admin local-media implementation out of `static-flow-backend` into a new
workspace crate and binary, tentatively named `static-flow-media`.

The browser-facing contract stays unchanged:

- the SPA still navigates to `/admin/local-media` and `/admin/local-media/player`
- the frontend still calls `/admin/local-media/api/*`
- admin auth stays enforced by the main backend

The main backend becomes an authenticated reverse proxy for local-media
requests. The new media binary owns ffmpeg/ffprobe, media cache, HLS
generation, poster generation, and playback job state.

This keeps media-specific process cost and dependency weight out of the main
backend startup path while preserving current userspace behavior.

## Goals

- Preserve existing `/admin/local-media/*` browser behavior.
- Keep admin-only access anchored in the main backend.
- Remove ffmpeg/media cache/media job ownership from `static-flow-backend`.
- Allow the media service to be started independently.
- Keep memory behavior streaming-first and disk-cache-based.
- Keep feature toggles working in both enabled and disabled builds.

## Non-Goals

- No public `/media/*` exposure.
- No gRPC, Unix socket protocol, or custom IPC layer.
- No change to frontend route names or query shape.
- No broad media indexing/database work.
- No fallback mode where the backend silently emulates media behavior when the
  media service is unavailable.

## Current Problems

Today the local-media implementation is compiled directly into
`static-flow-backend`.

The direct coupling points are:

- backend module registration in `backend/src/main.rs`
- media state initialization in `backend/src/state.rs`
- media routes in `backend/src/routes.rs`
- startup script env injection in `scripts/start_backend_from_tmp.sh`

Operationally this means:

- main backend startup initializes media-specific state even when the only user
  intent is “run the site”
- media dependencies and ffmpeg-related env are mixed into the main backend
  process model
- the backend binary remains responsible for media cache, job coordination,
  poster generation, and HLS generation

This is the wrong ownership boundary.

## Recommended Architecture

### High-Level Shape

Add a new workspace member:

- `media-service/`

Package:

- crate name: `static-flow-media`

Outputs:

- `lib` for shared request/response/config types and helper code
- `bin` for the standalone media HTTP service

Move the current media implementation from `backend/src/local_media/` into this
crate with minimal structural change.

The main backend keeps only:

- admin route exposure at `/admin/local-media/*`
- admin auth checks
- reverse-proxy forwarding to the media service
- media service base URL config

The media service owns:

- filesystem browsing under configured media root
- path guard enforcement
- ffprobe/ffmpeg resolution
- HLS generation
- poster generation
- cache layout
- playback job state
- media-local concurrency limits

### Network Topology

Browser:

- talks only to `static-flow-backend`

Main backend:

- listens on normal site/admin port
- proxies `/admin/local-media/api/*` to media service

Media service:

- listens on localhost only by default
- example default: `127.0.0.1:39085`

This preserves the current trust boundary:

- browser never talks directly to media service
- media service does not own admin auth
- main backend remains the security gate

## Crate Layout

Recommended new crate structure:

```text
media-service/
  Cargo.toml
  src/
    lib.rs
    main.rs
    config.rs
    routes.rs
    handlers.rs
    state.rs
    cache.rs
    ffmpeg.rs
    fs.rs
    jobs.rs
    path_guard.rs
    playback.rs
    poster.rs
    probe.rs
    types.rs
```

`lib.rs` exports:

- config parsing
- request/response types
- state construction helpers
- route builder for the media binary

`main.rs` does only:

- env loading
- logger setup
- media state init
- router binding

The current `backend/src/local_media/` code should migrate nearly file-for-file.
Do not redesign it during extraction unless the move exposes an actual boundary
problem.

## Route Design

### Browser-Facing Routes (unchanged)

These remain in the main backend:

- `/admin/local-media/api/list`
- `/admin/local-media/api/playback/open`
- `/admin/local-media/api/playback/jobs/:job_id`
- `/admin/local-media/api/playback/raw`
- `/admin/local-media/api/playback/hls/:job_id/:file_name`
- `/admin/local-media/api/poster`

### Media-Service Internal Routes

Recommended media-service routes:

- `/internal/local-media/list`
- `/internal/local-media/playback/open`
- `/internal/local-media/playback/jobs/:job_id`
- `/internal/local-media/playback/raw`
- `/internal/local-media/playback/hls/:job_id/:file_name`
- `/internal/local-media/poster`

The main backend maps public admin-local-media routes to these internal routes.

Reason:

- keeps media-service route namespace explicit
- avoids confusion between browser contract and service-internal contract
- makes it obvious that direct browser usage is not intended

## Proxy Behavior

### Main Backend Responsibilities

For any `/admin/local-media/api/*` request:

1. run existing admin auth logic
2. build media-service upstream URL
3. forward method, path, query, and request body
4. stream the upstream response back to the browser

### JSON Endpoints

For:

- `list`
- `playback/open`
- `playback/jobs/:job_id`

The backend should:

- forward status code
- forward `content-type`
- forward response body

No media-specific interpretation belongs in the backend.

### Streaming Endpoints

For:

- `playback/raw`
- `playback/hls/:job_id/:file_name`
- `poster`

The backend must:

- forward `Range` and relevant request headers
- stream upstream response body directly
- preserve these headers when present:
  - `Content-Type`
  - `Content-Length`
  - `Content-Range`
  - `Accept-Ranges`
  - `Cache-Control`

The proxy must not buffer media bodies into memory.

## Configuration

### Main Backend Config

Main backend stops reading media-root/cache/ffmpeg settings.

It reads only media-proxy settings:

- `STATICFLOW_MEDIA_PROXY_BASE_URL`
  - example: `http://127.0.0.1:39085`
- `STATICFLOW_MEDIA_PROXY_TIMEOUT_SECONDS`
  - optional, for JSON-style endpoints only

If the proxy base URL is unset and the media proxy feature is enabled:

- `/admin/local-media` frontend route may still exist
- `/admin/local-media/api/*` requests should fail clearly with `503`
- do not silently emulate the old in-process implementation

### Media Service Config

Media-service-specific env remains there:

- `STATICFLOW_LOCAL_MEDIA_ROOT`
- `STATICFLOW_LOCAL_MEDIA_CACHE_DIR`
- `STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG`
- `STATICFLOW_LOCAL_MEDIA_MAX_TRANSCODE_JOBS`
- `STATICFLOW_LOCAL_MEDIA_MAX_POSTER_JOBS`
- `STATICFLOW_LOCAL_MEDIA_LIST_PAGE_SIZE`
- `STATICFLOW_FFMPEG_BIN`
- `STATICFLOW_FFPROBE_BIN`
- `HOST` / `PORT`

This is the key cleanup: media config no longer belongs to the main backend
process.

## Feature Design

### Backend Feature

Keep a backend feature named `local-media`, but change its meaning.

Old meaning:

- compile full media implementation into backend

New meaning:

- compile admin local-media frontend contract and proxy handlers into backend

The backend `local-media` feature should depend only on:

- proxy code
- shared request/response types
- `reqwest` streaming support if not already sufficient

It should no longer depend on:

- `ffmpeg-sidecar`
- `tokio-util` for media file reading
- `mime_guess2`
- media cache implementation

### Media Service Feature

`static-flow-media` should own the actual media implementation features. It can
use default features for the common self-hosted path.

### Frontend Feature

Frontend `local-media` feature stays as-is.

The frontend should not care whether the backend implementation is in-process
or proxied.

## Script Design

### New Scripts

Add:

- `scripts/start_media_service_from_tmp.sh`
- `scripts/start_media_service_canary.sh`
- `scripts/start_backend_with_media_from_tmp.sh`

Responsibilities:

`start_media_service_from_tmp.sh`

- build or resolve `static-flow-media`
- choose a free localhost port
- pass media-specific env
- start only the media service

`start_media_service_canary.sh`

- same idea as existing backend canary
- separate port from site backend
- used for real verification against real media directory

`start_backend_with_media_from_tmp.sh`

- start media service first
- derive `STATICFLOW_MEDIA_PROXY_BASE_URL`
- start main backend

### Existing Scripts

Keep:

- `scripts/start_backend_from_tmp.sh`
- `scripts/start_backend_from_tmp_no_media.sh`
- `scripts/start_backend_selfhosted_canary.sh`

But change meanings:

`start_backend_from_tmp.sh`

- starts only main backend
- does not directly receive media root/cache/ffmpeg env

`start_backend_from_tmp_no_media.sh`

- starts backend without media proxy feature or with media proxy disabled

`start_backend_selfhosted_canary.sh`

- stays focused on backend canary behavior
- should gain the ability to pair with an already-running media-service canary

### Default Ports

Recommended defaults:

- main backend: existing behavior
- backend canary: `39081`
- media service: `39085`
- media service canary: `39086`

Ports must remain overrideable via env.

## Migration Plan

### Phase 1: Extract Without Behavior Change

Create `static-flow-media` and migrate current local-media implementation into
it with minimal code movement.

At the end of this phase:

- media binary can serve the current API behavior by itself
- backend local-media still exists temporarily

### Phase 2: Add Backend Proxy

Implement backend proxy handlers and wire them to the same browser-facing
routes.

At the end of this phase:

- frontend browser contract is served via proxy
- media binary is the actual execution engine

### Phase 3: Remove Old Backend Implementation

Delete in-process local-media implementation from backend.

At the end of this phase:

- backend owns only proxy logic
- media service owns all actual media behavior

### Phase 4: Script Cleanup

Update startup/canary scripts to reflect the new split.

At the end of this phase:

- media-enabled startup is explicit
- media-disabled startup is simple
- no media-specific env is leaked into backend-only startup paths

## Error Handling

If media service is unavailable:

- backend returns `502 Bad Gateway` or `503 Service Unavailable`
- response body should clearly say media service is unavailable

If media service returns a valid application error:

- backend forwards status/body unchanged

Do not add local compatibility fallbacks.

If media service startup config is invalid:

- media binary should fail fast at startup
- do not defer root-dir validation indefinitely

## Performance and Memory Constraints

The split must not regress current streaming behavior.

Required invariants:

- raw video responses stay streaming
- HLS artifact responses stay streaming
- poster responses stay file-streaming
- proxy layer does not aggregate full bodies in memory
- cache remains disk-based
- no full library pre-scan is introduced

The proxy adds one extra localhost hop. This is acceptable. The correctness
rule is more important than shaving a localhost round-trip.

## Security

- media service listens on localhost only by default
- browser never sees internal media-service routes
- admin auth stays in main backend
- media service still enforces path guard under configured root
- no public route exposure is added

Optional hardening later:

- shared secret header from backend to media service
- allowlist origin/host checks

These are not required for the first split because the service is localhost-only.

## Testing Strategy

### Media Service

Port existing local-media tests into `static-flow-media`.

Keep coverage for:

- config parsing
- path guard
- HLS cache/job readiness
- poster command generation
- poster cache path behavior

Add service-level integration tests for:

- `list`
- `playback/open`
- `playback/jobs`
- `playback/raw`
- `playback/hls`
- `poster`

### Main Backend

Add proxy-focused tests for:

- auth enforced before upstream call
- JSON response passthrough
- `Range` header passthrough
- key response-header passthrough
- upstream unavailable handling

### Build Matrix

Required checks:

- `cargo check -p static-flow-backend`
- `cargo check -p static-flow-backend --no-default-features`
- `cargo check -p static-flow-frontend --target wasm32-unknown-unknown`
- `cargo check -p static-flow-frontend --target wasm32-unknown-unknown --no-default-features`
- `cargo check -p static-flow-media`
- `cargo clippy` for affected crates to zero warnings

### Real Verification

Use the real media directory:

- `/mnt/e/videos/static/未归类`

Run:

- media-service canary
- backend canary

Verify:

- list loads through main backend
- mp4 playback works through proxy
- mkv playback works through proxy
- poster works through proxy
- backend without media still starts normally

## Risks

### Proxy Buffering Mistake

If proxy handlers accidentally call `.bytes()` or similar whole-body reads,
memory usage will regress badly.

Mitigation:

- keep streaming endpoints explicitly implemented as streamed passthrough
- test with `Range` and large files

### Contract Drift

If backend and media service do not share types, response drift will happen.

Mitigation:

- keep shared request/response types in `static-flow-media` `lib`
- backend depends on that `lib`

### Startup Complexity

Two processes are more operationally complex than one.

Mitigation:

- wrapper scripts own coordination
- do not hide process boundaries in application code

### Incremental Migration Breakage

Deleting in-process backend media too early would break current behavior.

Mitigation:

- land the split in phases
- only remove backend-local media after proxy path is verified

## Open Decision

No major open decisions remain for the split itself.

Assumptions:

- browser contract must remain `/admin/local-media/*`
- main backend remains the auth gate
- media service is localhost-only
- feature-disabled backend builds must still work

## Recommendation

Proceed with the split using:

- new workspace crate `static-flow-media`
- main-backend reverse proxy under existing admin routes
- script-level process composition

This is the smallest principled change that separates media concerns without
breaking the existing admin UI contract.
