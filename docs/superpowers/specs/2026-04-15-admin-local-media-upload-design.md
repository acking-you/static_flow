# Admin Local Media Upload Design

Date: 2026-04-15

## Goal

Add admin-only file upload to `/admin/local-media` so the current browsed
directory becomes the upload destination, upload progress remains visible after
page reload or browser restart, repeated selection of the same local file can
resume from the last persisted byte offset, and duplicate names are resolved by
stable auto-renaming instead of overwrite.

The implementation must preserve the current trust boundary:

- browser talks only to `static-flow-backend`
- backend remains the admin auth gate and reverse proxy
- `static-flow-media` owns filesystem writes, upload task state, and resume
  logic

## User Requirements

1. When the admin is browsing `/admin/local-media?dir=<path>`, uploads land in
   that exact directory under the configured local media root.
2. Upload tasks stay queryable after leaving the page, reloading the SPA, or
   restarting the browser.
3. If the user later selects the same local file again, the upload resumes from
   the persisted server offset instead of restarting from zero.
4. If the destination already contains the same file name, the service must
   auto-rename the new upload (`name (1).ext`, `name (2).ext`, ...).
5. The feature is implemented end-to-end across frontend, backend proxy, and
   media service without exposing new public routes outside `/admin/local-media`.

## Important Constraint

The browser cannot continue uploading bytes after the tab or browser process is
gone because the service no longer has access to the original `File` object.

So “survives browser restart” means:

- the server keeps task records and the uploaded byte offset
- the UI can still show the task later
- the user can re-select the same local file and continue from the saved offset

It does **not** mean the upload keeps transferring bytes while the browser is
closed. Any design pretending otherwise would be a fake state machine.

## Current Architecture

Today `/admin/local-media` already uses a three-hop structure:

- `frontend` renders the admin browser and player pages
- `backend` authenticates admin requests and proxies `/admin/local-media/api/*`
- `media-service` owns local media browsing, playback cache, posters, and job
  state

That ownership split is correct and should stay intact. Upload belongs beside
the existing media-service filesystem logic, not in the main backend and not in
browser-only state.

## Non-Goals

- No direct browser-to-media-service upload path.
- No public `/media/*` upload capability.
- No whole-file buffering in memory.
- No recursive directory upload in this change.
- No filesystem watcher, media indexer, or LanceDB metadata sync in this
  change.
- No fake “background upload” mechanism that claims bytes are still flowing
  after the browser loses the file handle.
- No SSE upload stream in v1; ordinary request/response plus polling is enough.

## Design

### 1. Ownership And Module Boundaries

#### `media-types`

Extend `media-types/src/lib.rs` with upload protocol types shared by backend
and frontend:

- upload task status enum
- create-or-resume request/response types
- task list query/response types
- chunk append response type

These types must stay compact and reflect the actual service contract.

#### `media-service`

Add upload-specific modules instead of expanding playback files:

- `media-service/src/upload.rs`
- `media-service/src/upload_store.rs`

Responsibilities:

- validate target directory under media root
- create or resume upload tasks
- append chunk bytes at an exact offset
- persist task metadata to disk
- finalize successful uploads into the target directory
- expose list/get/delete operations for admin task viewing

`media-service/src/routes.rs` and `media-service/src/handlers.rs` only wire the
new endpoints into the existing router.

#### `backend`

The backend remains thin:

- enforce existing admin auth
- proxy upload JSON endpoints to media-service
- proxy chunk append bodies without taking filesystem ownership

No backend-local upload task database should be introduced.

#### `frontend`

`frontend/src/pages/admin_local_media.rs` remains the page entry but should stop
trying to own upload truth in component memory.

Expected UI split:

- `frontend/src/pages/admin_local_media.rs` keeps route/query wiring and page
  composition
- `frontend/src/components/admin_local_media_uploads.rs` owns file picker,
  upload queue, progress cards, and resume prompts

The page queries server task state for the current directory and only uses
in-memory state for live `File` handles and transient “actively sending chunk”
markers.

### 2. Upload Task Lifecycle

The upload flow is create-or-resume first, then bounded chunk append.

1. User opens `/admin/local-media?dir=movies/2026`.
2. User selects one or more files in the upload panel.
3. Frontend sends one `create-or-resume` request per file containing:
   - `target_dir`
   - `source_file_name`
   - `file_size`
   - `last_modified_ms`
   - optional `mime_type`
4. Media-service either:
   - returns an existing incomplete task with its current `uploaded_bytes`, or
   - creates a new task and reserves a target file name
5. Frontend starts sending sequential chunks beginning at `uploaded_bytes`.
6. Each successful chunk response returns the new persisted byte count.
7. When `uploaded_bytes == file_size`, media-service finalizes the file into the
   target directory and marks the task `completed`.
8. The directory list refresh shows the new file immediately.

On page revisit or browser restart:

- the page loads task records from the server for the current directory
- incomplete tasks remain visible with their persisted byte count
- the user can re-select the same local file and the client resumes from the
  returned server offset

### 3. Truthful Status Model

Persisted task status must describe the durable server state, not optimistic UI
intent.

Use these persisted states:

- `created`: task exists, `uploaded_bytes == 0`
- `partial`: `0 < uploaded_bytes < file_size`
- `completed`: file finalized into destination directory
- `failed`: task hit a write or validation error
- `canceled`: admin explicitly canceled and cleanup completed

Frontend may show an extra local badge like `sending` while it currently owns a
live `File` handle and is pushing chunks, but that local marker is not written
back as durable server truth.

This avoids lying about “uploading” versus “paused” when the browser has
already gone away.

### 4. Task Identity And Resume Matching

Upload resume must be deterministic, not heuristic.

For every file selection, media-service derives a `resume_key` from:

- `target_dir`
- `source_file_name`
- `file_size`
- `last_modified_ms`

`resume_key` is an internal SHA-256 digest over those exact fields.

`POST /uploads/tasks` behaves as create-or-resume:

- if an incomplete task (`created` or `partial`) already exists for the same
  `resume_key`, return that task
- if the matching task is terminal (`completed`, `failed`, `canceled`), create
  a new task

This gives the user the desired behavior:

- selecting the same file again resumes
- uploading the same file again after completion starts a new task

The browser does not need to guess which task to resume. The service remains
the source of truth.

### 5. Target File Naming

New uploads never overwrite existing files.

When a new task is created, media-service resolves the destination name using:

1. desired original name
2. if occupied, append ` (1)` before the extension
3. continue increasing until an unused name is found

Examples:

- `demo.mp4`
- `demo (1).mp4`
- `demo (2).mp4`

The chosen target name is stored in task metadata and reused on resume.

If an out-of-band filesystem change later creates a collision before final
commit, finalization reruns the same auto-rename algorithm once and updates the
task record. This handles real filesystem races without adding placeholder files
inside the user-visible media tree.

### 6. Storage Layout

Task metadata and uploaded bytes have different placement rules.

#### Metadata

Store task metadata under the existing media-service cache root:

```text
<cache_dir>/uploads/tasks/<task_id>/task.json
```

`task.json` includes at minimum:

- `task_id`
- `resume_key`
- `status`
- `target_dir`
- `source_file_name`
- `target_file_name`
- `target_relative_path`
- `file_size`
- `uploaded_bytes`
- `last_modified_ms`
- `mime_type`
- `error`
- `created_at_ms`
- `updated_at_ms`

#### Partial File Bytes

Do **not** stage partial bytes under `cache_dir`.

`cache_dir` may live on a different filesystem from the media root, which would
make final `rename` fail with a cross-device error.

Stage partial data under a hidden directory inside the media root filesystem:

```text
<media_root>/.static-flow-uploads/<task_id>.part
```

This guarantees finalization can use a same-filesystem rename into the target
directory.

The existing directory browser already skips dot-prefixed entries, so this
staging area stays invisible in `/admin/local-media`.

### 7. Crash And Restart Recovery

Durable recovery must trust filesystem facts over stale JSON.

When reading a task from disk:

- if the `.part` file length differs from `task.json.uploaded_bytes`, use the
  actual `.part` length and rewrite the metadata
- if the target file already exists, `.part` is gone, and the task had uploaded
  the full file size, reconcile the task to `completed`

This makes service restart safe without adding a database.

`LocalMediaState` may keep in-memory per-task locks for concurrency control, but
the canonical task record is the disk metadata plus staging file length.

### 8. HTTP Contract

#### Browser-Facing Backend Routes

Add these routes under the existing admin namespace:

- `POST /admin/local-media/api/uploads/tasks`
- `GET /admin/local-media/api/uploads/tasks?dir=<dir>&limit=<n>&offset=<n>`
- `GET /admin/local-media/api/uploads/tasks/:task_id`
- `PUT /admin/local-media/api/uploads/tasks/:task_id/chunks?offset=<offset>`
- `DELETE /admin/local-media/api/uploads/tasks/:task_id`

#### Media-Service Internal Routes

Mirror them behind the internal service prefix:

- `POST /internal/local-media/uploads/tasks`
- `GET /internal/local-media/uploads/tasks`
- `GET /internal/local-media/uploads/tasks/:task_id`
- `PUT /internal/local-media/uploads/tasks/:task_id/chunks`
- `DELETE /internal/local-media/uploads/tasks/:task_id`

#### `POST /uploads/tasks`

Request fields:

- `target_dir`
- `source_file_name`
- `file_size`
- `last_modified_ms`
- `mime_type`

Response fields:

- full task snapshot
- `uploaded_bytes`
- resolved `target_file_name`
- resolved `target_relative_path`

#### `PUT /uploads/tasks/:task_id/chunks?offset=<offset>`

Request body:

- raw bytes for exactly one chunk

Rules:

- offset must match the server’s current `uploaded_bytes`
- the service rejects out-of-order or duplicate append requests
- the service reads only the bounded chunk body, not the whole file

Response:

- updated task snapshot with the new `uploaded_bytes`

#### `GET /uploads/tasks`

Returns current directory task history, newest first, with pagination.

Filtering by `dir` keeps the UI aligned with “show me uploads for the folder I
am browsing now”.

#### `DELETE /uploads/tasks/:task_id`

Allowed for non-completed tasks only.

Behavior:

- delete `.part` bytes if present
- mark task `canceled`
- keep metadata so the UI can still show the terminal result

#### Task Retention

V1 does not introduce automatic retention cleanup.

Reason:

- task counts for this admin-only feature are expected to stay small
- resumable correctness is more important than early garbage collection
- retention policy can be added later as a separate explicit design once real
  usage data exists

### 9. Chunking Strategy

Use fixed-size sequential chunks in the frontend. Start with `8 MiB`.

Rationale:

- bounded request memory
- simple offset math
- no multipart parser required
- good enough throughput for admin-only local uploads

Chunks are strictly sequential per task. No parallel chunk upload in v1.

This keeps correctness simple:

- one task
- one writer
- one monotonic offset

### 10. Frontend Behavior

The page should gain an upload section above the directory cards.

UI responsibilities:

- show current directory upload target
- allow multi-file selection
- create or resume one task per file
- show per-task progress bars from durable server state
- expose cancel/remove controls for incomplete tasks
- surface “choose the same file again to resume” when a task is partial but no
  local file handle is attached

Frontend state is split into:

- durable remote state: fetched upload task snapshots
- ephemeral local state: `task_id -> File` handles currently attached in this
  browser session

On page load and on `dir` change:

- fetch directory listing
- fetch upload tasks for `dir`

While uploading:

- update the visible progress immediately from each chunk response
- periodically refetch task snapshots so the page stays truthful after route
  navigation or another tab resumes the same task

### 11. Backend Proxy Behavior

The main backend must preserve the current security model:

- run `ensure_admin_access`
- forward JSON APIs to media-service
- forward chunk bodies as ordinary proxied requests

The backend must not:

- write upload bytes to disk
- invent its own resume state
- reinterpret task status

It is a trust boundary and transport hop, not the upload engine.

### 12. Error Handling

Principled failures:

- invalid or escaping `target_dir` -> `400`
- target directory not found -> `400`
- offset mismatch -> `409`
- task not found -> `404`
- completed task chunk append -> `409`
- chunk write failure -> `500` and task becomes `failed`

Frontend behavior:

- show the exact task-level failure
- do not silently retry with guessed offsets
- allow the admin to re-select the same file to resume if the task remains
  resumable
- allow cancel to discard a broken partial upload

### 13. Why Polling Is Enough

Upload progress already changes on every chunk response.

So v1 should use:

- direct progress updates after each successful append
- lightweight polling for task list refresh on page re-entry or visibility
  recovery

SSE adds complexity but little value here because there is no independent
server-side background upload worker producing new progress after the browser
stops sending chunks.

### 14. Planned File Changes

Expected primary touch points:

- `media-types/src/lib.rs`
- `media-service/src/routes.rs`
- `media-service/src/handlers.rs`
- `media-service/src/state.rs`
- `media-service/src/config.rs`
- `media-service/src/upload.rs`
- `media-service/src/upload_store.rs`
- `backend/src/media_proxy/handlers.rs`
- `backend/src/routes.rs`
- `frontend/src/api.rs`
- `frontend/src/pages/admin_local_media.rs`
- `frontend/src/components/admin_local_media_uploads.rs`

### 15. Testing Strategy

#### Media-Service Tests

- create-or-resume returns the same incomplete task for matching metadata
- duplicate file names resolve to `name (n).ext`
- chunk append enforces exact offset matching
- persisted `.part` length reconciles metadata after restart
- completion moves the file into the target directory and updates task state
- cancel removes staging bytes and marks the task terminal

#### Backend Tests

- proxy route registration for new upload endpoints
- admin auth remains enforced
- chunk proxy preserves request body and upstream status codes

#### Frontend Tests

- task list renders current-directory upload tasks
- selecting the same file resumes from the server offset returned by create
- partial tasks without a local file handle show a resume prompt instead of a
  fake active state
- completed uploads refresh the directory list and show the new file

## Open Trade-Offs Chosen Explicitly

- No recursive directory upload yet: keeps the task model simple.
- No SSE yet: polling is enough for bounded chunk uploads.
- No database: disk metadata plus staging file length is sufficient.
- No fake “paused/uploading” durable state: persisted status stays faithful to
  what the server can actually know.

## Result

This design adds a real upload subsystem to `/admin/local-media` without
breaking the existing backend/media-service split.

The core principles are:

- backend keeps auth and proxy ownership
- media-service owns resumable upload truth
- frontend shows durable task state instead of pretending page memory is enough
- duplicate names never overwrite
- browser restart recovery is honest: task state survives, bytes resume only
  after the user re-selects the same local file
