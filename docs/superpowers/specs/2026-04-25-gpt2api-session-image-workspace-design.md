# GPT2API Session Image Workspace Design

Date: 2026-04-25

Status: approved design, pending implementation plan

## Purpose

Rebuild the public GPT2API experience into a ChatGPT-style conversation
workspace where every key has durable server-side history. Text chat, image
generation, image editing, prompts, generated images, queue state, and email
notifications are owned by `gpt2api-rs` rather than by browser-local storage or
StaticFlow.

## Goals

- Store every session, message, original prompt, image task, and final image
  artifact under the key that created it.
- Restore full history after logout, browser close, or login from another
  browser.
- Provide one unified ChatGPT-style session UI instead of separate chat and
  image pages.
- Show useful progress while image tasks are queued or running, including queue
  position, coarse ETA, task phase, elapsed time, and activity log updates.
- Send optional email notifications with expiring signed links when image
  generation completes.
- Support admin-role keys that can inspect all sessions and control global
  image queue concurrency.
- Keep existing OpenAI-compatible APIs working while also writing their requests
  into the same session history.

## Non-Goals

- Do not make StaticFlow own GPT2API business data.
- Do not expose image artifact directories as static public files.
- Do not put API keys or admin tokens in emailed URLs.
- Do not break `/v1/images/generations`, `/v1/images/edits`,
  `/v1/chat/completions`, or `/v1/responses`.
- Do not implement automatic artifact retention deletion in the first version.

## Architecture

`gpt2api-rs` becomes the product backend and source of truth. It owns:

- key authentication and role checks
- session and message persistence
- image task queueing and execution
- image artifact storage and access control
- signed share links
- email notifications
- admin queue and session inspection APIs
- OpenAI-compatible API compatibility

StaticFlow remains a deployment shell:

- serve `/gpt2api/*` as the standalone frontend entry
- proxy `/api/gpt2api/*` to `gpt2api-rs`
- keep `/admin/gpt2api-rs` as the local operator control plane

The storage root remains:

```text
/mnt/wsl/data4tb/static-flow-data/gpt2api-rs
```

SQLite stores structured product metadata. Image files live under
`artifacts/images`, and every image read goes through `gpt2api-rs` APIs.

## Authentication And Roles

Web login uses a normal downstream API key from `api_keys.secret_plaintext`.
The service-level `admin_token` remains only for service administration.

Extend `api_keys`:

- `role TEXT NOT NULL DEFAULT 'user'`
- `notification_email TEXT`
- `notification_enabled INTEGER NOT NULL DEFAULT 0`

Role behavior:

- `user`: can read and mutate only sessions, messages, tasks, and artifacts
  owned by that key.
- `admin`: has normal conversation features plus cross-key session inspection,
  queue inspection, key notification settings, role management, and global queue
  configuration.

Every backend endpoint must enforce role and ownership. Frontend hiding is not
authorization.

## Data Model

### api_keys

Existing fields stay intact. Add role and notification fields above.

Defaults for existing rows:

- `role = 'user'`
- `notification_email = NULL`
- `notification_enabled = 0`

### sessions

One durable conversation.

Fields:

- `id TEXT PRIMARY KEY`
- `key_id TEXT NOT NULL`
- `title TEXT NOT NULL`
- `source TEXT NOT NULL` with values `web` or `api`
- `status TEXT NOT NULL` with values `active` or `archived`
- `created_at INTEGER NOT NULL`
- `updated_at INTEGER NOT NULL`
- `last_message_at INTEGER`

Indexes:

- `(key_id, updated_at)`
- `(source, updated_at)`
- `(status, updated_at)`

### messages

Append-only conversation items with mutable status/content updates for pending
assistant messages.

Fields:

- `id TEXT PRIMARY KEY`
- `session_id TEXT NOT NULL`
- `key_id TEXT NOT NULL`
- `role TEXT NOT NULL` with values `user`, `assistant`, `system`, or `tool`
- `content_json TEXT NOT NULL`
- `status TEXT NOT NULL` with values `pending`, `streaming`, `done`, or `failed`
- `created_at INTEGER NOT NULL`
- `updated_at INTEGER NOT NULL`

`content_json` stores structured blocks:

- text content
- original prompt
- requested model
- image mode
- uploaded reference image metadata
- linked task ids
- linked artifact ids
- revised prompts
- error summaries

It never stores final image base64.

Indexes:

- `(session_id, created_at)`
- `(key_id, created_at)`

### image_tasks

The global queue unit.

Fields:

- `id TEXT PRIMARY KEY`
- `session_id TEXT NOT NULL`
- `message_id TEXT NOT NULL`
- `key_id TEXT NOT NULL`
- `status TEXT NOT NULL` with values
  `queued`, `running`, `succeeded`, `failed`, or `cancelled`
- `mode TEXT NOT NULL` with values `generation` or `edit`
- `prompt TEXT NOT NULL`
- `model TEXT NOT NULL`
- `n INTEGER NOT NULL`
- `request_json TEXT NOT NULL`
- `queue_entered_at INTEGER NOT NULL`
- `started_at INTEGER`
- `finished_at INTEGER`
- `position_snapshot INTEGER`
- `estimated_start_after_ms INTEGER`
- `error_code TEXT`
- `error_message TEXT`

Indexes:

- `(status, queue_entered_at)`
- `(key_id, queue_entered_at)`
- `(session_id, queue_entered_at)`

### image_artifacts

Metadata for generated files.

Fields:

- `id TEXT PRIMARY KEY`
- `task_id TEXT NOT NULL`
- `session_id TEXT NOT NULL`
- `message_id TEXT NOT NULL`
- `key_id TEXT NOT NULL`
- `relative_path TEXT NOT NULL`
- `mime_type TEXT NOT NULL`
- `sha256 TEXT NOT NULL`
- `size_bytes INTEGER NOT NULL`
- `width INTEGER`
- `height INTEGER`
- `revised_prompt TEXT`
- `created_at INTEGER NOT NULL`

File path shape:

```text
artifacts/images/<key_id>/<session_id>/<message_id>/<image_id>.png
```

Indexes:

- `(task_id, created_at)`
- `(session_id, created_at)`
- `(key_id, created_at)`

### signed_links

Email share-link access.

Fields:

- `id TEXT PRIMARY KEY`
- `token_hash TEXT NOT NULL UNIQUE`
- `scope TEXT NOT NULL` with values `image_task` or `session`
- `scope_id TEXT NOT NULL`
- `expires_at INTEGER NOT NULL`
- `revoked_at INTEGER`
- `created_at INTEGER NOT NULL`
- `used_at INTEGER`

The plaintext token is shown only in the email URL. SQLite stores only the hash.

### runtime_config

Extend existing runtime configuration with:

- `global_image_concurrency INTEGER NOT NULL DEFAULT 1`
- `signed_link_ttl_seconds INTEGER NOT NULL DEFAULT 604800`
- `queue_eta_window_size INTEGER NOT NULL DEFAULT 20`

## Product API

All product APIs use Bearer key auth.

### Login And User Settings

- `POST /auth/verify`
  - returns key id, name, role, quota, notification email, notification enabled
- `GET /me`
- `PATCH /me/notification`
  - ordinary users can update their own `notification_email` and
    `notification_enabled`

### Sessions

- `GET /sessions?limit=&cursor=`
- `POST /sessions`
- `GET /sessions/:id`
- `PATCH /sessions/:id`
- `GET /sessions/:id/events`
  - SSE stream for message/task/artifact/queue changes

Ordinary users are always scoped to their own `key_id`.

### Messages And Tasks

- `POST /sessions/:id/messages`
  - `kind = text`: create user message and assistant response
  - `kind = image_generation`: create user message, assistant pending message,
    and image task
  - `kind = image_edit`: upload reference image, create messages, and image
    task
- `GET /tasks/:id`
- `POST /tasks/:id/cancel`

Users may cancel only their own queued tasks. Running tasks are not cancelled in
the first version.

### Artifacts And Signed Links

- `GET /artifacts/:id`
  - requires owning key or admin key
- `GET /share/:token`
  - returns session/task summary for a valid, unexpired, unrevoked token
- `GET /share/:token/artifacts/:artifact_id`
  - streams an artifact allowed by that token scope

### Admin-Key Product APIs

These are for `api_keys.role = 'admin'`.

- `GET /admin/sessions?key_id=&q=&limit=&cursor=`
- `GET /admin/keys`
- `PATCH /admin/keys/:id`
  - role, notification email, notification enabled, quota, routing, scheduler
    fields
- `GET /admin/queue`
- `PATCH /admin/queue/config`
  - update `global_image_concurrency`
- `POST /admin/tasks/:id/cancel`

## OpenAI-Compatible APIs

Existing compatibility endpoints keep their current request and response shape:

- `/v1/images/generations`
- `/v1/images/edits`
- `/v1/chat/completions`
- `/v1/responses`

Internal behavior changes:

- authenticate key as today
- resolve `x-gpt2api-session-id` if supplied and owned by the key
- otherwise create or reuse a `source = 'api'` session such as `API Requests`
- create user message and assistant pending message
- create image task when the request asks for image generation/edit
- synchronous compatibility handlers wait for task completion and then return
  the existing JSON format
- text chat handlers also write prompt and assistant response into session
  history

If a compatible request times out or fails, the session still records the user
message and failed assistant message.

## Queue And Task Runner

`gpt2api-rs` runs an `ImageTaskRunner` background worker.

Task lifecycle:

```text
queued -> running -> succeeded
queued -> running -> failed
queued -> cancelled
```

Scheduling rules:

1. enforce `runtime_config.global_image_concurrency`
2. enforce existing key scheduler limits
3. enforce existing account scheduler limits
4. route through existing account selection and quota settlement logic

Queue visibility:

- each queued task has `position_ahead`
- ETA is computed from recent successful task durations and current concurrency
- ETA is explicitly approximate
- queue snapshots are emitted over SSE and are also available via polling

Failure behavior:

- failed tasks update assistant message in place
- original prompt and request parameters are preserved
- successful usage settlement is not applied to failed tasks
- failure events can be recorded for diagnostics without counting billable
  image usage

## Email Notifications

Email is sent only when:

- the task succeeds
- the owning key has `notification_enabled = 1`
- the owning key has a syntactically valid `notification_email`

Email content:

- session title
- original prompt
- model and image count
- signed view link

The signed link:

- points to `/gpt2api/share/<token>`
- expires after `signed_link_ttl_seconds`
- can be revoked
- does not contain the key secret
- is backed by a hashed token in `signed_links`

Email send failure must not mark the image task failed. It should update
notification status metadata and log a warning.

## Frontend Design

The public frontend is a standalone GPT2API app, not a StaticFlow Yew route.

Recommended implementation:

- Vite + TypeScript + React
- output copied into the standalone GPT2API static bundle served from
  `/gpt2api/*`
- no business history stored in IndexedDB

Layout:

- left sidebar: session list, new chat, search, archive, settings
- center: message stream
- bottom: unified composer
- right drawer or panel: task details, queue state, notification settings, admin
  tools
- mobile: session list and right panel become drawers

Composer modes:

- `Chat`
- `Image`
- `Edit`

Image pending experience is a first-class requirement:

- `Queued`: show queue position, tasks ahead, and approximate ETA
- `Starting`: show account/model allocation
- `Running`: show elapsed time and typical duration range
- `Finishing`: show saving artifact and creating share link
- `Done`: replace pending card in place with image grid
- `Failed`: preserve prompt and parameters, show retry

The pending card must not be a static spinner. It should include:

- phase label
- non-precise step indicator
- queue position changes
- ETA updates
- elapsed time
- small activity log such as `queued`, `position changed`, `started`,
  `saving image`

Reloading the browser must recover pending and completed task states from the
server.

Admin UI:

- `All sessions`: cross-key session search and read-only inspection
- `Queue`: running and queued tasks, global concurrency, average duration,
  failures, cancel queued task
- `Keys`: role, notification email, notification enabled, quota, routing

## Migration

Migrations are additive and idempotent.

Steps:

1. add api key role and notification columns
2. add session/message/task/artifact/signed link tables
3. extend runtime config
4. keep existing usage DuckDB untouched
5. default all old keys to ordinary user with notifications off

No existing API key should become admin automatically unless explicitly set by
operator action or a controlled bootstrap command.

## Testing

### gpt2api-rs Unit Tests

- migration idempotency
- key role parsing and defaults
- ordinary key cannot read another key's sessions or artifacts
- admin key can read all sessions
- signed link hash validation
- signed link expiration and revocation
- queue position and ETA calculation
- notification email validation

### gpt2api-rs Integration Tests

- create web session, send image task, complete artifact write
- compatible image API returns existing shape and writes session history
- compatible text API writes session history
- SSE reconnect can recover state via `GET /sessions/:id`
- email disabled does not send
- invalid email does not fail task
- SMTP failure does not fail task

### Frontend Tests

- login restores server-side session list
- new chat creates session
- pending image card updates through queued/running/finishing/done
- browser refresh preserves task state
- admin key can access all sessions and queue tools
- ordinary key cannot access admin UI or APIs

### Smoke Tests

- `/auth/verify`
- create session
- create image task
- SSE task updates
- artifact read
- signed link read
- existing `/v1/images/generations`
- existing `/v1/chat/completions`

## Rollout

1. Implement and test `gpt2api-rs` storage/API/queue/email changes locally.
2. Build `gpt2api-rs` release binary.
3. Run it against `/mnt/wsl/data4tb/static-flow-data/gpt2api-rs`.
4. Build standalone frontend.
5. If StaticFlow route/proxy changes are needed, deploy StaticFlow backend with
   blue-green cutover behind `39180`.
6. Verify local and public paths:
   - `/gpt2api/login`
   - `/api/gpt2api/auth/verify`
   - session APIs
   - image task lifecycle
   - compatible image API
7. Keep old slot alive until `39180` and public health pass repeated checks.

## Risks And Constraints

- Synchronous compatible APIs must have timeouts so callers do not hang forever
  while queued.
- Queue ETA must be clearly approximate.
- Artifact reads must never bypass ownership or signed-link checks.
- Signed tokens must be high entropy and stored only as hashes.
- Admin role enforcement must happen in backend handlers, not only in frontend.
- SMTP failures must not affect image task success.
- Automatic artifact deletion is intentionally deferred to avoid data loss.

## Implementation Slices

1. Storage migrations and model types.
2. Product auth and role enforcement.
3. Session and message CRUD.
4. Image task queue and artifact storage.
5. Product image/chat APIs and SSE.
6. Compatible API history integration.
7. Email notification and signed links.
8. Admin key APIs.
9. Standalone frontend rewrite.
10. Smoke tests and blue-green rollout.
