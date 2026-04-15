# Admin Local Media Upload Design

Date: 2026-04-16

Status: finalized after discussion; supersedes the unimplemented 2026-04-15
draft for upload storage placement and implementation scope.

## Goal

Add admin-only video upload to `/admin/local-media` so the currently browsed
directory becomes the upload destination, progress remains visible after page
reload or browser restart, re-selecting the same local file resumes from the
persisted server offset, and duplicate names are handled by stable auto-rename
instead of overwrite.

The trust boundary remains unchanged:

- browser talks only to `static-flow-backend`
- backend remains the admin auth gate and reverse proxy
- `static-flow-media` owns filesystem writes, upload task state, resume logic,
  and final file publication

## What Is Broken Today

The upload feature is not merely missing UI polish. The chain is incomplete:

- shared upload DTOs exist in `media-types`
- `media-service` has no upload production code, no upload handlers, and no
  upload routes
- backend proxy exposes no upload endpoints
- the admin local-media page exposes no upload UI or upload API helpers

So the correct next step is an end-to-end upload subsystem, not another partial
stub.

## User Requirements

1. When the admin is browsing `/admin/local-media?dir=<path>`, uploads land in
   that exact directory under the configured local media root.
2. Upload tasks remain visible after SPA reload, route navigation, or browser
   restart.
3. Re-selecting the same local file later resumes from the persisted uploaded
   byte offset instead of starting from zero.
4. If the target directory already contains the same file name, the new upload
   auto-renames to `name (1).ext`, `name (2).ext`, and so on.
5. Successful uploads appear in the directory listing immediately and then
   reuse the existing poster and playback flows.
6. Partial uploads must not pollute the normal local-media directory browser.

## Constraint: No Fake Background Upload

The browser cannot continue streaming bytes after it loses the original
`File` handle. So “survives browser restart” means:

- the server persists the task record and uploaded byte count
- the page can still display that task later
- the user can re-select the same local file and continue from the saved
  offset

It does not mean bytes keep transferring while the browser is gone.

## Rejected Approaches

### Single-request multipart upload

Rejected because it does not provide resumability and fails badly for large
videos.

### Direct browser-to-media-service upload

Rejected because it bypasses the current backend auth boundary and duplicates
admin access logic.

### Generic Tus-style protocol

Rejected for v1 because it adds protocol surface and state complexity without
solving a real project-specific problem better than a bounded sequential chunk
protocol.

## Chosen Approach

Use a backend-authenticated, media-service-owned resumable upload flow:

1. frontend creates or resumes a server upload task
2. frontend sends fixed-size chunks sequentially
3. `static-flow-media` persists task metadata and `.part` bytes on disk
4. when the last chunk arrives, `static-flow-media` atomically renames the
   staged file into the target directory

This keeps upload truth in the filesystem owner, not in frontend component
state and not in the main backend.

## Ownership And Module Boundaries

### `media-types`

Keeps compact wire types only:

- upload task status
- create-or-resume request/response
- list/get/delete task responses
- chunk append query/response

### `static-flow-media`

Owns:

- target directory validation
- resume key derivation
- task record persistence
- exact-offset chunk append
- finalize/cancel/recovery logic
- upload HTTP handlers and internal routes

New modules:

- `media-service/src/upload.rs`
- `media-service/src/upload_store.rs`

### `static-flow-backend`

Owns:

- existing admin auth enforcement
- proxying JSON upload APIs to `static-flow-media`
- proxying chunk bodies without taking filesystem ownership

It must not create a separate upload database or infer upload state on its own.

### Frontend

Owns:

- file picker and upload queue
- attaching local `File` handles to durable server task records
- sending sequential chunks
- displaying server-backed task progress and errors

The page remains `frontend/src/pages/admin_local_media.rs`, but upload UI
should live in a focused component such as
`frontend/src/components/admin_local_media_uploads.rs`.

## Storage Layout

Uploads need both durability and same-filesystem finalization. The cleanest
layout is to keep **all** upload state under a hidden directory inside the
media root.

```text
<media_root>/.static-flow/uploads/<task_id>/task.json
<media_root>/.static-flow/uploads/<task_id>/blob.part
```

Why this layout:

- `blob.part` is on the same filesystem as the final destination, so publish
  can use atomic rename
- `task.json` stays beside the staged bytes, so recovery does not depend on a
  separate cache volume
- the existing local-media browser already ignores dot-prefixed names, so this
  service state stays invisible in normal directory listings

`cache_dir` remains owned by poster/transcode/playback caches and is not reused
for uploads.

## Task Model

Persisted task status describes durable server state only:

- `created`: task exists and `uploaded_bytes == 0`
- `partial`: `0 < uploaded_bytes < file_size`
- `completed`: final file is published into the destination directory
- `failed`: validation or write error occurred
- `canceled`: admin canceled and staged bytes were cleaned up

Frontend may show an extra local badge such as `sending`, but that is ephemeral
browser state and must not be written back as durable truth.

## Resume Identity

Resume matching must be deterministic.

`resume_key` is derived from:

- `target_dir`
- `source_file_name`
- `file_size`
- `last_modified_ms`

It is a SHA-256 digest over those exact fields.

`POST /uploads/tasks` behaves as create-or-resume:

- if an incomplete task with the same `resume_key` already exists, return it
- if the matching task is terminal, create a new task

This gives the desired behavior:

- selecting the same file again resumes
- uploading the same file again after completion starts a new task

## File Naming

New uploads never overwrite existing files.

When creating a new task, `static-flow-media` resolves the final target name:

1. try the original file name
2. if occupied, try `name (1).ext`
3. continue until an unused name is found

The chosen target name is persisted in `task.json` and reused on resume.

If a real filesystem race appears before final publish, finalization reruns the
same auto-rename algorithm once and updates the task record before rename.

## Upload Lifecycle

1. Admin opens `/admin/local-media?dir=movies/2026`.
2. Admin selects one or more files in the upload panel.
3. Frontend sends one create-or-resume request per file containing:
   - `target_dir`
   - `source_file_name`
   - `file_size`
   - `last_modified_ms`
   - optional `mime_type`
4. Service returns either:
   - an existing incomplete task with `uploaded_bytes`, or
   - a new task with a resolved `target_file_name`
5. Frontend sends sequential chunks beginning at `uploaded_bytes`.
6. Each successful chunk response returns the updated persisted task snapshot.
7. When `uploaded_bytes == file_size`, the service fsyncs and atomically renames
   `blob.part` into the resolved destination path, then marks the task
   `completed`.
8. The page refreshes the directory list and the new file becomes immediately
   visible to the existing poster and playback paths.

## HTTP Contract

### Browser-facing backend routes

- `POST /admin/local-media/api/uploads/tasks`
- `GET /admin/local-media/api/uploads/tasks?dir=<dir>&limit=<n>&offset=<n>`
- `GET /admin/local-media/api/uploads/tasks/:task_id`
- `PUT /admin/local-media/api/uploads/tasks/:task_id/chunks?offset=<offset>`
- `DELETE /admin/local-media/api/uploads/tasks/:task_id`

### Internal media-service routes

- `POST /internal/local-media/uploads/tasks`
- `GET /internal/local-media/uploads/tasks`
- `GET /internal/local-media/uploads/tasks/:task_id`
- `PUT /internal/local-media/uploads/tasks/:task_id/chunks`
- `DELETE /internal/local-media/uploads/tasks/:task_id`

### Create-or-resume request

Fields:

- `target_dir`
- `source_file_name`
- `file_size`
- `last_modified_ms`
- `mime_type`

Response:

- full task snapshot
- current `uploaded_bytes`
- resolved `target_file_name`
- resolved `target_relative_path`

### Chunk append request

`PUT /uploads/tasks/:task_id/chunks?offset=<offset>`

Rules:

- body is raw bytes for one chunk
- offset must equal the server’s current `uploaded_bytes`
- no out-of-order append
- no duplicate append guessing
- no whole-file buffering in memory

Response:

- full task snapshot after append

### List tasks

Returns tasks for the current directory, newest first, with pagination.

Filtering by `dir` keeps the UI aligned with “show me uploads for the folder I
am browsing now”.

### Delete task

Allowed only for non-completed tasks.

Behavior:

- remove `blob.part` if present
- mark task `canceled`
- keep `task.json` so the UI can still display a terminal result

## Frontend Behavior

The local-media page gains an upload section above the directory cards.

V1 behavior:

- show current directory upload target
- allow multi-file selection
- create or resume one task per file
- upload sequentially per task
- run only one active task at a time in the browser queue
- show per-task progress from server snapshots
- allow cancel for incomplete tasks
- after page load or directory change, fetch both directory listing and upload
  tasks for that directory

Frontend state splits into:

- durable remote state: server task snapshots
- ephemeral local state: browser `File` handles and active send markers

On browser restart, the page can still show the task, but the user must
re-select the same local file to continue sending bytes.

## Chunking Strategy

Use fixed-size `8 MiB` chunks.

Rationale:

- bounded memory and request size
- simple offset math
- good enough throughput for admin-only large video uploads

No parallel chunk upload in v1. Correctness is more important than peak
throughput.

## Recovery Rules

Recovery trusts filesystem facts over stale JSON.

When loading a task:

- if `blob.part` exists and its actual length differs from
  `task.json.uploaded_bytes`, rewrite metadata to the actual length
- if the final target file exists, `blob.part` is gone, and the task had
  already uploaded the full size, reconcile the task to `completed`

This makes restart recovery work without adding a database.

## Validation And Errors

Principled failures:

- invalid or escaping `target_dir` -> `400`
- target directory not found -> `400`
- unsupported file extension -> `400`
- task not found -> `404`
- offset mismatch -> `409`
- append to terminal task -> `409`
- write failure or fsync failure -> `500` and task becomes `failed`

Frontend behavior:

- display the exact server task error
- do not silently retry with guessed offsets
- offer re-select-and-resume when a task is partial but no browser `File`
  handle is attached

## Concurrency

Per task, writes are strictly serialized.

Server-side requirements:

- one task, one monotonic offset
- append requests for the same task cannot race
- different tasks may proceed independently

This only requires per-task locks in `static-flow-media`; no distributed queue
or global scheduler is needed.

## Non-Goals

- no direct browser-to-media-service upload path
- no public upload routes outside `/admin/local-media`
- no directory upload
- no background transfer after the browser loses the file handle
- no filesystem watcher or LanceDB import
- no SSE/websocket upload stream in v1
- no upload retention cleanup in v1

Task counts for this admin-only feature are expected to stay small. Cleanup can
be added later as a separate explicit design once real usage data exists.

## Testing Requirements

### `static-flow-media`

- create-or-resume returns existing partial task
- duplicate destination names auto-rename
- chunk append rejects wrong offsets
- final chunk finalizes by atomic rename
- cancel removes staged bytes
- hidden `.static-flow` tree stays invisible in normal listings
- restart reconciliation repairs stale `uploaded_bytes`

### backend

- upload routes require admin auth
- JSON proxying preserves request and response bodies
- chunk proxying preserves raw body and content type

### frontend

- upload task URLs use the admin local-media prefix
- queue state transitions are correct
- page reload restores task cards from server state
- successful completion refreshes the current directory listing

## Rollout

1. Implement media-service upload core first.
2. Expose internal routes.
3. Expose backend admin proxy routes.
4. Add frontend upload APIs and upload component.
5. Verify upload into the current directory and immediate playback reuse on a
   canary local media root.

## Summary

This design adds a truthful, resumable, admin-only video upload path to
`/admin/local-media` without changing the current backend trust boundary.

The key design choice is simple and deliberate:

- `static-flow-media` owns upload truth
- upload metadata and staged bytes live together under a hidden directory in
  the media root
- backend stays a pure auth gate and proxy
- frontend becomes a queue and observer, not the source of truth
