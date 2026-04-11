# Admin Local Media Design

## Goal

Add a private admin-only local media browser and mobile-first video player to
StaticFlow without changing public `/media/*` behavior, without breaking the
existing self-hosted deployment model, and without introducing high steady-state
memory usage.

The feature must satisfy these constraints:

1. All UI routes and HTTP APIs live under `/admin/...`.
2. The implementation is isolated into dedicated modules instead of expanding
   the existing large admin page and generic backend handlers.
3. The feature is compile-time gated so both `with local media` and
   `without local media` builds run normally.
4. The default build enables the feature, but a separate startup script must
   support running StaticFlow without it.
5. Playback must work on mobile browsers, including `mkv` sources, by
   normalizing playback output instead of assuming browsers can play arbitrary
   containers directly.
6. The implementation must be memory-conscious: no whole-file buffering, no
   in-memory transcoding cache, no eager library-wide scan, and bounded
   transcode concurrency.
7. Validation must use a separate canary backend process and real media data at
   `/mnt/e/videos/static/未归类` so the existing running backend is not
   disturbed.

## Current Problem

StaticFlow already has:

- public media routes such as `/media/video`, `/media/audio`, and `/media/image`
- a large monolithic `frontend/src/pages/admin.rs`
- a backend startup/script story split across several shell scripts
- an audio streaming path that proves Axum + range-friendly media serving works

But it does not have:

- an admin-only local filesystem media browser
- a mobile-optimized video player page
- a principled `mkv` handling path
- a compile-time feature split for optional local media support
- a startup/build workflow that cleanly produces separate with/without-media
  binaries while keeping the running production backend untouched

The current `/media/video` route is only a placeholder, so reusing it would
conflate public media semantics with private filesystem browsing. That violates
the requirement that local media stay under `/admin` only.

## Non-Goals

- Do not expose local filesystem media through public `/media/*` routes.
- Do not import the local media library into LanceDB in this change.
- Do not build a general-purpose media indexer, search engine, or background
  scanner for the entire filesystem.
- Do not add compatibility shims that try to “make random browser/container
  combinations work” after the fact. The source-of-truth fix is normalized
  playback output.
- Do not implement full photo/audio library browsing in the same change.
- Do not modify the active production backend process during validation.

## Design

### 1. Route And Ownership Boundaries

All new functionality lives under `/admin/local-media`.

Frontend routes:

- `/admin/local-media`
- `/admin/local-media/player`

Backend routes:

- `GET /admin/local-media/api/list`
- `POST /admin/local-media/api/playback/open`
- `GET /admin/local-media/api/playback/jobs/:job_id`
- `GET /admin/local-media/api/playback/raw/*encoded_path`
- `GET /admin/local-media/api/playback/hls/*encoded_path/index.m3u8`
- `GET /admin/local-media/api/playback/hls/*encoded_path/:segment_name`
- optional: `GET /admin/local-media/api/poster/*encoded_path`

The routes intentionally stay separate from existing public `/media/*` routes.
There is no alias and no compatibility layer.

The UI is also split into dedicated pages rather than extending the existing
admin mega-page with another large tab. The admin landing page may contain an
entry link, but directory browsing and playback state live in standalone page
modules.

### 2. Module Layout And Compile-Time Feature Split

The feature is isolated behind a new crate feature named `local-media`.

#### Backend feature layout

Add backend feature flags in `backend/Cargo.toml`:

- `default = ["local-media"]`
- `local-media = ["dep:ffmpeg-sidecar", "dep:tokio-util", "dep:mime_guess2"]`

Introduce a dedicated module tree:

- `backend/src/local_media/mod.rs`
- `backend/src/local_media/config.rs`
- `backend/src/local_media/types.rs`
- `backend/src/local_media/fs.rs`
- `backend/src/local_media/path_guard.rs`
- `backend/src/local_media/cache.rs`
- `backend/src/local_media/ffmpeg.rs`
- `backend/src/local_media/probe.rs`
- `backend/src/local_media/playback.rs`
- `backend/src/local_media/jobs.rs`
- `backend/src/local_media/handlers.rs`

`routes.rs`, `main.rs`, and `state.rs` only wire the feature when
`cfg(feature = "local-media")` is enabled. When the feature is off, those code
paths compile out cleanly and the rest of the backend still starts normally.

#### Frontend feature layout

Add frontend feature flags in `frontend/Cargo.toml`:

- `default = ["local-media"]`
- `local-media = []`
- keep existing `mock`

Add dedicated page modules:

- `frontend/src/pages/admin_local_media.rs`
- `frontend/src/pages/admin_local_media_player.rs`

Optional supporting modules:

- `frontend/src/components/local_media_grid.rs`
- `frontend/src/components/local_media_breadcrumbs.rs`
- `frontend/src/components/local_media_preview_tile.rs`
- `frontend/src/local_media_bridge.rs`

The router wires these pages only under `cfg(feature = "local-media")`. When
the feature is disabled, the routes disappear and the admin entry link is not
rendered.

This keeps both build modes honest:

- feature on: full local media UI and backend support
- feature off: no dead menu entries, no missing API handlers, no unused code
  silently left behind

### 3. Runtime Config Model

The feature stays compile-time optional, but the enabled build still uses
runtime config for deployment and operational tuning.

New environment variables:

- `STATICFLOW_LOCAL_MEDIA_ROOT`
- `STATICFLOW_LOCAL_MEDIA_CACHE_DIR`
- `STATICFLOW_LOCAL_MEDIA_MAX_LIST_ENTRIES`
- `STATICFLOW_LOCAL_MEDIA_MAX_TRANSCODE_JOBS`
- `STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG`
- `STATICFLOW_LOCAL_MEDIA_FFMPEG_DIR`
- `STATICFLOW_LOCAL_MEDIA_FFMPEG_BIN`
- `STATICFLOW_LOCAL_MEDIA_FFPROBE_BIN`
- `STATICFLOW_LOCAL_MEDIA_ENABLE_POSTER`
- `STATICFLOW_LOCAL_MEDIA_CACHE_RETENTION_HOURS`

Defaults:

- feature enabled build: local media support compiled in
- scripts with media: `STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG=1`
- scripts without media: feature compiled out, so runtime env is irrelevant
- `STATICFLOW_LOCAL_MEDIA_ROOT` has no implicit production default; startup
  scripts set it explicitly when the media-enabled workflow is intended

If the feature is compiled in but `STATICFLOW_LOCAL_MEDIA_ROOT` is absent, the
backend still starts. The admin page shows a clear “not configured” state
instead of failing the whole process.

### 4. Filesystem Model

The local media library is a filesystem view rooted at
`STATICFLOW_LOCAL_MEDIA_ROOT`.

Rules:

- Every request path is relative to the root.
- `..` traversal is rejected.
- Symlink escape is rejected after canonicalization.
- Directory listing is shallow and on-demand.
- No background full-root scan.
- No persistent DB table is introduced.

The listing endpoint returns only the current directory snapshot with separate
directory rows and video rows. It includes small metadata only:

- relative path
- display name
- file size
- modified time
- optional duration/resolution if already cheaply available
- poster availability flag

Directory browsing remains cheap because the system only touches the requested
directory.

### 5. Playback Decision Model

The browser never directly decides whether to use raw playback or HLS. The
backend owns that decision.

`POST /admin/local-media/api/playback/open` receives a relative file path and
returns one of three modes:

- `raw`
- `hls`
- `preparing`

#### Raw mode

Used when the source is already browser-friendly enough for mobile playback.
The server exposes a `raw` playback URL and supports range requests without
loading the whole file into memory.

#### HLS mode

Used when cached normalized output already exists on disk.
The response returns the playlist URL immediately.

#### Preparing mode

Used when normalized output is required but not yet ready. The response returns
a `job_id`, and the frontend polls a lightweight job-status endpoint until the
playlist is ready or the job fails.

This avoids a long hanging HTTP request and keeps transcode scheduling explicit.

### 6. `mkv` Handling Policy

`mkv` support is solved by output normalization, not by assuming browsers can
play arbitrary Matroska sources directly.

Decision order:

1. If the source is already safe for direct mobile playback, use `raw`.
2. If the container/stream combination can be cheaply rewrapped, normalize via
   copy/remux path.
3. Otherwise transcode to normalized HLS output:
   - video: `H.264`
   - audio: `AAC`

The backend uses `ffprobe` to inspect streams and decide among these paths.

The design does not attempt a maze of browser-specific heuristics. The
principled answer is a normalized playback format the player can consume
reliably on mobile.

### 7. `ffmpeg-sidecar` Responsibility Boundary

Use `acking-you/ffmpeg-sidecar` as a submodule under `deps/ffmpeg-sidecar`.

Dependency model:

- backend path dependency to the submodule
- `default-features = false` in Cargo
- runtime auto-download behavior controlled by
  `STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG=1`

`ffmpeg-sidecar` is responsible for:

- locating `ffmpeg` and `ffprobe`
- managing download/install when enabled
- reducing binary-discovery boilerplate

It is not responsible for:

- Axum response streaming lifecycle
- cache invalidation policy
- request/job scheduling
- in-memory media buffering

The backend’s local media module remains the owner of job state, cache
directory structure, and HTTP behavior.

### 8. Cache Design

Normalized output is cached on disk only.

Cache root:

- `STATICFLOW_LOCAL_MEDIA_CACHE_DIR`
- default recommendation: `tmp/local-media-cache`

Cache key inputs:

- source relative path
- source file size
- source modified time
- transform profile

Cache contents:

- HLS playlist
- HLS segment files
- optional poster image
- small metadata/status file if needed

No transcode output is kept in memory after it is written.

If the source file changes, the cache key changes, so old cache entries simply
stop being used. A lightweight cleanup path may prune aged cache directories,
but the first version does not require a complex LRU service.

### 9. Memory And Concurrency Rules

These are hard requirements, not follow-up optimizations.

Rules:

- Never read an entire video into memory.
- Never aggregate an entire transcode output in memory before responding.
- Never pre-scan the full media root.
- Never hold an in-memory cache of transcoded bytes.
- Limit concurrent transcode jobs with
  `STATICFLOW_LOCAL_MEDIA_MAX_TRANSCODE_JOBS`.
- Deduplicate concurrent requests for the same source so only one normalize job
  runs at a time.

Request behavior under load:

- same source already preparing: attach to existing job status
- different source over concurrency limit: reject with explicit busy/preparing
  state instead of opportunistically starting more jobs

This protects the backend process from CPU and memory spikes caused by several
mobile clients or multiple player tabs.

### 10. Frontend Page Design

#### Browser page: `/admin/local-media`

Responsibilities:

- show breadcrumbs
- show current directory
- render directories before files
- render a mobile-friendly list/grid
- provide a single inline muted preview slot at most
- navigate to the dedicated player page

The browser page is intentionally not a multi-video wall. At most one preview
tile can actively mount a `<video>` element at a time.

#### Player page: `/admin/local-media/player`

Responsibilities:

- fetch/open playback session
- mount xgplayer
- show back navigation
- show previous/next within the same directory
- restore playback progress
- surface preparing/error states cleanly

The player page owns playback state. The browser page owns browsing state.
These concerns remain separate to keep mobile memory, scroll performance, and
state handling manageable.

### 11. Player Integration

Use xgplayer for the actual mobile playback surface.

Integration direction:

- ship xgplayer browser assets through frontend static output
- add a small JS bridge file under `frontend/static/`
- let Yew mount/unmount the player and pass config
- avoid introducing a new frontend bundler pipeline just for this feature

Expected mobile behavior:

- tap to show controls
- double tap play/pause
- swipe seek
- long-press speed boost
- lock-friendly player UI
- `playsinline`

The backend chooses `raw` vs `hls`. The frontend only consumes the returned
player URL and mode.

### 12. Build And Script Design

The current `make bin-backend` target already exports
`bin/static-flow-backend`. The local media design standardizes around that
artifact instead of ad hoc debug/release binary discovery.

#### Backend build toggles

Extend `Makefile` so `bin-backend` can be driven by feature env:

- `BACKEND_DEFAULT_FEATURES=1|0`
- `BACKEND_FEATURES=...`

Default:

- with-media build uses default features, so `local-media` stays on

No-media build:

- `BACKEND_DEFAULT_FEATURES=0`
- pass only the explicitly required non-local-media features, if any exist

#### Frontend build toggles

Extend `scripts/build_frontend_selfhosted.sh` so trunk can receive feature
selection:

- `FRONTEND_DEFAULT_FEATURES=1|0`
- `FRONTEND_FEATURES=...`

This keeps the delivered self-hosted SPA aligned with the backend binary.

#### Startup scripts

For tmp/dev-style local startup:

- keep `scripts/start_backend_from_tmp.sh` as the media-enabled default
- make it always run `make bin-backend` before start
- make it run `bin/static-flow-backend`
- make it set:
  - `STATICFLOW_LOCAL_MEDIA_ROOT`
  - `STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG=1`

Add a wrapper:

- `scripts/start_backend_from_tmp_no_media.sh`

The no-media wrapper builds/runs the feature-disabled variant instead of merely
setting a runtime env flag. That proves the compile-time separation works.

#### Canary script

Update `scripts/start_backend_selfhosted_canary.sh` similarly:

- accept feature-mode aware build path
- continue using a separate port from the live backend
- optionally build frontend with matching feature set
- support media-root env wiring for real validation

This script is the required validation path because it does not collide with the
existing running backend instance.

### 13. Verification Plan

Implementation verification must use the real media directory:

- `/mnt/e/videos/static/未归类`

Required runtime validation environment:

- canary backend only
- separate port, for example `39081`
- existing backend remains untouched

Required end-to-end checks:

1. Start canary with local media feature enabled and
   `STATICFLOW_LOCAL_MEDIA_ROOT=/mnt/e/videos/static/未归类`.
2. Open `/admin/local-media` on the canary port and verify directory listing.
3. Open at least one direct-play file.
4. Open at least one `mkv` file and verify the prepare/job/HLS path.
5. Confirm repeated playback reuses disk cache rather than redoing the full
   conversion.
6. Confirm memory stays bounded during playback and conversion.
7. Start the no-media build and verify the backend still starts cleanly and the
   local-media UI/routes are absent.

The validation must run against the real backend process, not mocked tests
alone.

### 14. Testing Requirements

Backend unit/integration coverage should include:

- path normalization and traversal rejection
- symlink escape rejection
- directory listing shaping
- cache key calculation
- job deduplication
- transcode concurrency cap
- raw vs hls decision behavior

Frontend verification should include:

- route presence only when the feature is enabled
- admin entry visibility only when enabled
- browser page empty/config-error states
- player page preparing/error/rendered states

Before claiming the task complete:

- run `cargo clippy` for affected crates to zero warnings
- run `rustfmt` only on modified Rust files
- run the canary backend with the real media directory
- verify that the existing live backend process is not affected

## Design Summary

This design adds a private local media subsystem, not a tweak to the existing
public media layer. The system is separated at every boundary that matters:

- separate routes
- separate backend modules
- separate frontend pages
- separate compile-time feature
- separate startup scripts
- separate canary validation port

It defaults to the useful mode, but it still proves that the no-media variant is
healthy. It also treats `mkv` support as a real delivery problem solved by
normalized playback output and disk-backed caching, not by wishful direct-play
assumptions.
