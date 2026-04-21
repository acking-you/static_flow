# gpt2api-rs Design

## Goal

Build a standalone Rust image-generation gateway in `deps/gpt2api_rs` that:

- exposes the same public image endpoints `chatgpt2api` already supports
- imports existing ChatGPT `access_token` credentials instead of implementing
  real account login
- uses Kiro/Codex-style account routing, local scheduling, and key-based quota
  controls
- runs as one independent binary with its own storage directory and no runtime
  dependency on `static_flow`
- ships a CLI that manages the service only through admin REST APIs

## Constraints

- Do not depend on `static_flow` runtime services, LanceDB tables, or backend
  routes.
- Account import only needs these sources:
  - direct `access_token`
  - Session JSON containing `accessToken`
  - CPA JSON containing `access_token` / `accessToken`
- Public compatibility surface should match `chatgpt2api` v1 behavior:
  - `POST /v1/images/generations`
  - `POST /v1/chat/completions`
  - `POST /v1/responses`
  - `GET /v1/models`
- Downstream callers authenticate with multiple managed API keys, not one
  global shared secret.
- Admin APIs authenticate separately with an `admin` bearer token.
- Successful billing is image-count based:
  - one generated image consumes one unit
  - partial success only bills the number of images actually returned
- Account state should use startup warmup plus background periodic refresh,
  matching the existing Kiro gateway operational model.
- No management web UI in v1.

## Recommended Design

### One binary, two surfaces

The project should produce one executable with two command families:

- `gpt2api-rs serve`
  - starts the HTTP service
- `gpt2api-rs admin ...`
  - calls the admin REST APIs

The HTTP service itself exposes two distinct API surfaces:

- public OpenAI-compatible image endpoints
- private admin management endpoints

CLI never reads local storage directly. It always talks to admin REST. That
keeps the admin contract stable for both future skills and automation.

### Internal module boundaries

The codebase should be structured around stable responsibilities instead of one
large request handler:

- `config`
  - CLI args, env overrides, storage path resolution
- `storage`
  - SQLite control database, DuckDB event store, migrations, outbox
- `accounts`
  - account import, normalization, persisted account state, cached status
- `scheduler`
  - key-level and account-level local request schedulers
- `routing`
  - account-group resolution, route strategy selection, candidate ordering
- `upstream::chatgpt`
  - unofficial ChatGPT image-generation transport
- `public_api`
  - OpenAI-compatible request/response adaptation
- `admin_api`
  - account/key/group/config/usage management endpoints
- `usage`
  - quota deduction, usage rollups, outbox enqueue, event flush worker

The unofficial ChatGPT upstream behavior must stay isolated in
`upstream::chatgpt`. Public handlers and routing logic should not know about
`chat-requirements`, proof-of-work, SSE parsing, or file download endpoints.

## Storage Design

### Storage root

`gpt2api-rs serve` receives a required `--storage-dir`. The service creates and
owns all files under that directory.

Recommended v1 layout:

- `control.db`
- `events.duckdb`
- `event-blobs/`
- `logs/` if local file logging is enabled later

### Control-plane storage in SQLite

Use `control.db` for transactional operational truth:

- `accounts`
- `account_groups`
- `account_group_members`
- `api_keys`
- `runtime_config`
- `event_outbox`

This database handles:

- key quota deduction
- account status updates
- route configuration
- runtime scheduler configuration
- reliable event handoff to the event store

`accounts` should persist at least:

- `name`
- `access_token`
- `source_kind`
- `email`
- `user_id`
- `plan_type`
- `default_model_slug`
- `status`
- `quota_remaining`
- `restore_at`
- `last_refresh_at`
- `last_used_at`
- `last_error`
- `success_count`
- `fail_count`
- `request_max_concurrency`
- `request_min_start_interval_ms`
- `browser_profile_json`

`api_keys` should persist at least:

- `id`
- `name`
- `secret_hash`
- `status`
- `quota_total_images`
- `quota_used_images`
- `route_strategy`
- `account_group_id`
- `request_max_concurrency`
- `request_min_start_interval_ms`
- timestamps

`runtime_config` should hold the singleton service configuration:

- refresh interval min/max
- per-account jitter
- request size limits
- default scheduler limits
- event flush settings

### Event storage in DuckDB

Use `events.duckdb` only for usage-event summaries and operator queries.

Recommended `usage_events` columns:

- `event_id`
- `request_id`
- `key_id`
- `key_name`
- `account_name`
- `endpoint`
- `requested_model`
- `resolved_upstream_model`
- `requested_n`
- `generated_n`
- `billable_images`
- `status_code`
- `latency_ms`
- `error_code`
- `error_message`
- `detail_ref`
- `created_at`

Successful requests should default to summary-only rows. Large diagnostics do
not belong in DuckDB.

### Failure detail sidecars

If an event needs full diagnostics, write a compressed JSON sidecar under
`event-blobs/` and store only `detail_ref` in DuckDB.

This keeps the event store append-friendly while preserving operator access to
rare high-value failure payloads.

### Reliable outbox

Avoid direct synchronous dual writes to SQLite and DuckDB on the request path.

Instead:

1. commit quota/state changes plus one `event_outbox` row in a single SQLite
   transaction
2. let a background flusher batch `event_outbox` rows into DuckDB
3. mark flushed outbox rows complete only after the DuckDB write succeeds

This keeps control-plane truth authoritative even if the analytics store is
temporarily unavailable.

## Public Request Lifecycle

All public endpoints should normalize into one internal request type such as
`ImageGenerationRequest`.

### Ingress normalization

Supported external shapes:

- `/v1/images/generations`
- `/v1/chat/completions`
- `/v1/responses`

All three should normalize into:

- `prompt`
- `requested_model`
- `n`
- `response_mode`
- `source_endpoint`

Then one shared service method handles image generation for all protocols.

### Downstream key validation

For each public request:

1. extract presented key from `Authorization: Bearer ...` or `x-api-key`
2. load the managed key from SQLite
3. validate:
   - `status == active`
   - quota can still cover the request
   - key-level local scheduler can start the request

### Route strategy

v1 should only support:

- `auto`
- `fixed`

Behavior:

- `fixed`
  - requires one account group containing exactly one account
- `auto`
  - if a key references an account group, route within that group
  - otherwise route across the full managed account pool

### Account eligibility and selection

Candidate accounts should first be filtered by cached status:

- exclude `disabled`
- exclude `invalid`
- exclude exhausted accounts
- exclude accounts in short failure cooldown

Then apply account-level scheduler checks:

- `request_max_concurrency`
- `request_min_start_interval_ms`

Candidate ordering should prefer:

1. available accounts with healthy remaining quota
2. higher remaining image quota
3. fair rotation using least-recently-routed preference

This intentionally favors healthy accounts over simple round-robin.

### Upstream ChatGPT transport

The upstream adapter should encapsulate the current `chatgpt2api` image flow:

- session bootstrap / browser impersonation
- `chat-requirements`
- proof-of-work handling
- `backend-api/conversation`
- SSE parsing
- conversation polling when needed
- image download URL resolution
- binary download and base64 encoding

It should return a normalized result containing:

- generated image payloads
- revised prompt
- resolved upstream model
- latency
- classified upstream observations

### Success settlement

On success:

- bill only the number of images actually returned
- update the key quota in SQLite
- update account usage fields
- decrement cached account quota locally
- enqueue one usage event to the outbox

### Failure handling

On failure:

- do not charge the downstream key
- record failure counters and diagnostics
- classify account state conservatively:
  - mark `invalid` only for clear token invalidation
  - mark `limited` only for clear upstream quota exhaustion
  - otherwise keep the account usable after cooldown

Retry policy should stay conservative in v1:

- invalid token: fail that account immediately
- exhausted account: try another candidate
- transient network or download failure: allow at most one alternate-account
  retry for the same request
- no infinite whole-pool retry loops

## Scheduling and Limits

The service should keep the same two-layer local scheduling model already used
in the existing gateway design.

### Key-level scheduler

Each downstream API key gets:

- `request_max_concurrency`
- `request_min_start_interval_ms`

This prevents one client from monopolizing the service.

### Account-level scheduler

Each upstream ChatGPT account gets:

- `request_max_concurrency`
- `request_min_start_interval_ms`

This protects individual upstream accounts from local overload and reduces the
chance of triggering upstream anti-abuse behavior.

The key scheduler and account scheduler must stay separate. One does not
replace the other.

## Background Refresh

Use the same operational model as the Kiro gateway:

- run one initial warmup refresh during startup
- run a background refresh loop afterward
- use configurable min/max interval windows
- add per-account jitter within a refresh round

Refresh should query the same upstream metadata `chatgpt2api` already uses:

- `/backend-api/me`
- `/backend-api/conversation/init`

The refresh result updates:

- `email`
- `user_id`
- `plan_type`
- `default_model_slug`
- `quota_remaining`
- `restore_at`
- account status

Routing should consult this cached state instead of probing upstream for every
selection decision.

## Admin REST and CLI

### Admin authentication

Admin APIs use a dedicated bearer token and never reuse downstream API keys.

### Admin endpoints

Recommended v1 admin routes:

- accounts
  - `GET /admin/accounts`
  - `GET /admin/accounts/:name`
  - `POST /admin/accounts/import/token`
  - `POST /admin/accounts/import/session`
  - `POST /admin/accounts/import/cpa`
  - `POST /admin/accounts/:name/refresh`
  - `PATCH /admin/accounts/:name`
  - `DELETE /admin/accounts/:name`
- account groups
  - `GET /admin/account-groups`
  - `POST /admin/account-groups`
  - `PATCH /admin/account-groups/:group_id`
  - `DELETE /admin/account-groups/:group_id`
- keys
  - `GET /admin/keys`
  - `POST /admin/keys`
  - `PATCH /admin/keys/:key_id`
  - `DELETE /admin/keys/:key_id`
- runtime config
  - `GET /admin/runtime-config`
  - `PATCH /admin/runtime-config`
- usage
  - `GET /admin/usage`
  - `GET /admin/usage/:event_id`
- status
  - `GET /admin/status`

### CLI shape

Recommended command shape:

```bash
gpt2api-rs serve --listen 127.0.0.1:8787 --storage-dir /data/gpt2api
gpt2api-rs admin --base-url http://127.0.0.1:8787 --admin-token xxx accounts list
gpt2api-rs admin --base-url ... --admin-token ... accounts import-token --name a1 --token '...'
gpt2api-rs admin --base-url ... --admin-token ... accounts import-session --name a2 --file session.json
gpt2api-rs admin --base-url ... --admin-token ... accounts import-cpa --name a3 --file cpa.json
gpt2api-rs admin --base-url ... --admin-token ... accounts refresh --name a1
gpt2api-rs admin --base-url ... --admin-token ... keys create --name prod --quota-images 1000 --route-strategy auto --account-group default
gpt2api-rs admin --base-url ... --admin-token ... usage list --limit 50
```

CLI should support:

- human-readable default output
- `--json` machine-readable output

That keeps the CLI usable for both humans and future skills.

## Account Import

All import routes should converge into one internal flow:

1. parse source input
2. extract token
3. normalize and deduplicate
4. upsert account rows
5. immediately trigger upstream refresh

Parsing rules:

- token import
  - use the provided token string directly
- session import
  - extract `accessToken`
- cpa import
  - extract `access_token` or `accessToken`

Naming rules:

- explicit `name` wins
- otherwise generate a stable derived name as `acct_` plus the first 12 hex
  characters of the token SHA-1

Refresh failure should not discard the imported account automatically. Instead:

- keep the row
- set a visible error status
- record the failure detail for operator review

## Errors and Observability

### Public error contract

Recommended public status mapping:

- `401` / `403`
  - invalid or disabled downstream key
- `429`
  - local key/account scheduling block
- `502` / `503`
  - upstream generation failed or no usable account exists

Internal upstream error categories should include at least:

- `token_invalid`
- `account_limited`
- `upstream_rate_limited`
- `pow_failed`
- `conversation_failed`
- `download_failed`
- `network_error`
- `unknown_upstream_error`

### Structured logging

Every request should log structured fields including:

- `request_id`
- `key_id`
- `account_name`
- `source_endpoint`
- `requested_model`
- `resolved_upstream_model`
- `requested_n`
- `generated_n`
- `latency_ms`
- `error_code`

Never log raw tokens in normal request logs.

### Health and operator status

Recommended v1 lightweight operational endpoints:

- `GET /healthz`
  - process-level liveness
- `GET /admin/status`
  - account counts by status
  - recent refresh time
  - outbox backlog
  - last event flush health

## Testing

v1 testing should focus on stable local logic first.

### Parser and protocol tests

- token/session/cpa import parsing
- endpoint-specific request normalization
- public response adaptation back into OpenAI-compatible shapes

### Routing and scheduler tests

- key-level concurrency and pacing
- account-level concurrency and pacing
- `auto` and `fixed` route behavior
- account-group filtering
- candidate ranking and fairness

### Storage tests

- SQLite migrations
- DuckDB schema bootstrap
- quota deduction plus outbox insert transaction consistency
- outbox flush into DuckDB

### Upstream adapter tests

Use fixtures or mocks for:

- `chat-requirements`
- SSE event parsing
- conversation polling
- download URL extraction

CI should not require live ChatGPT integration as a hard dependency.

## Non-Goals

- no real ChatGPT email/password login flow
- no management web UI
- no image editing, masks, or variations
- no promise of native official `gpt-image-2` semantics beyond
  `chatgpt2api`-compatible behavior
- no multi-provider general gateway framework in v1
- no distributed multi-instance consistency design in v1
- no encrypted secret-management subsystem in v1

## Risks

- The unofficial ChatGPT transport can change frequently, so the upstream
  adapter boundary must stay narrow and well-tested.
- Account quota signals are inferred from unofficial endpoints and may drift;
  routing must tolerate stale cached values.
- The outbox-to-DuckDB pipeline must never block quota truth in SQLite.
- Holding account scheduler leases for the full upstream round-trip is required
  for concurrency limits to remain real instead of cosmetic.
