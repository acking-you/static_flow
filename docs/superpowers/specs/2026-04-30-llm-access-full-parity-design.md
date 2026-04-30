# LLM Access Full Parity Extraction Design

## Background

`llm-access` must become a standalone service that can fully replace the
current StaticFlow LLM subsystem. It is not a reduced MVP and it must not drop
subtle runtime behavior such as Kiro cache estimation, zero-cache diagnostics,
per-account pacing, cooldown, failover, account routing, usage detail capture,
or the admin APIs used by the existing frontend.

The current `llm-access` crate is only a shell: it can initialize storage and
register provider paths, but provider routes still return a fixed unauthorized
response. The old local Pingora canary design is therefore premature. Gateway
path splitting remains useful, but only after `llm-access` has feature parity
with the existing backend-owned LLM implementation.

## Goals

- Produce a standalone `llm-access` binary that can run without the StaticFlow
  backend process.
- Preserve all public and admin API behavior currently used by the frontend.
- Preserve all provider data-plane behavior for Codex/OpenAI-compatible and
  Kiro/Claude-compatible routes.
- Preserve Kiro cache simulation, cache policy overrides, zero-cache debug
  capture, request validation, body-size guards, context-usage handling,
  scheduler limits, cooldowns, quota failover, proxy cooldowns, and status
  caching.
- Replace LanceDB storage for LLM access with SQLite for control-plane state
  and DuckDB for usage analytics.
- Keep the current frontend usable without feature loss.
- Make the migration reversible until `llm-access` is explicitly promoted to
  the source of truth.

## Non-Goals

- Do not move articles, comments, music, local media, GPT2API image generation,
  or other non-LLM StaticFlow features into `llm-access`.
- Do not implement active-active multi-writer SQLite/DuckDB.
- Do not rewrite provider behavior from memory. Extract or share the existing
  implementation and then adapt storage.
- Do not do gateway cutover before API parity, runtime parity, storage import,
  and local verification pass.
- Do not change frontend UX as part of the extraction unless a compatibility
  break is found and must be fixed.

## Current Route Surface To Preserve

`llm-access` must own the full current LLM route surface.

### Provider And Public Routes

- `POST /api/llm-gateway/v1/*path`
- `GET /api/llm-gateway/access`
- `GET /api/llm-gateway/model-catalog.json`
- `GET /api/llm-gateway/status`
- `POST /api/llm-gateway/public-usage/query`
- `GET /api/llm-gateway/support-config`
- `GET /api/llm-gateway/support-assets/:file_name`
- `GET /api/llm-gateway/account-contributions`
- `GET /api/llm-gateway/sponsors`
- `POST /api/llm-gateway/token-requests/submit`
- `POST /api/llm-gateway/account-contribution-requests/submit`
- `POST /api/llm-gateway/sponsor-requests/submit`
- `GET /api/kiro-gateway/access`
- `GET /api/kiro-gateway/v1/models`
- `POST /api/kiro-gateway/v1/messages`
- `POST /api/kiro-gateway/v1/messages/count_tokens`
- `POST /api/kiro-gateway/cc/v1/messages`
- `POST /api/kiro-gateway/cc/v1/messages/count_tokens`

Standalone provider aliases such as `/v1/*` and `/cc/v1/*` may be supported by
`llm-access`, but they are additive. Compatibility with the existing
`/api/...` paths is mandatory because the current frontend and public docs use
those paths.

### Admin Routes

- `GET|POST /admin/llm-gateway/config`
- `GET|POST /admin/llm-gateway/proxy-configs`
- `POST /admin/llm-gateway/proxy-configs/import-legacy-kiro`
- `PATCH|DELETE /admin/llm-gateway/proxy-configs/:proxy_id`
- `POST /admin/llm-gateway/proxy-configs/:proxy_id/check/:provider_type`
- `GET /admin/llm-gateway/proxy-bindings`
- `POST /admin/llm-gateway/proxy-bindings/:provider_type`
- `GET|POST /admin/llm-gateway/account-groups`
- `PATCH|DELETE /admin/llm-gateway/account-groups/:group_id`
- `GET|POST /admin/llm-gateway/keys`
- `PATCH|DELETE /admin/llm-gateway/keys/:key_id`
- `GET /admin/llm-gateway/usage`
- `GET /admin/llm-gateway/usage/:event_id`
- `GET /admin/llm-gateway/token-requests`
- `POST /admin/llm-gateway/token-requests/:request_id/approve-and-issue`
- `POST /admin/llm-gateway/token-requests/:request_id/reject`
- `GET /admin/llm-gateway/account-contribution-requests`
- `POST /admin/llm-gateway/account-contribution-requests/:request_id/approve-and-issue`
- `POST /admin/llm-gateway/account-contribution-requests/:request_id/reject`
- `GET /admin/llm-gateway/sponsor-requests`
- `POST /admin/llm-gateway/sponsor-requests/:request_id/approve`
- `DELETE /admin/llm-gateway/sponsor-requests/:request_id`
- `GET|POST /admin/llm-gateway/accounts`
- `PATCH|DELETE /admin/llm-gateway/accounts/:name`
- `POST /admin/llm-gateway/accounts/:name/refresh`
- `GET|POST /admin/kiro-gateway/account-groups`
- `PATCH|DELETE /admin/kiro-gateway/account-groups/:group_id`
- `GET|POST /admin/kiro-gateway/keys`
- `PATCH|DELETE /admin/kiro-gateway/keys/:key_id`
- `GET /admin/kiro-gateway/usage`
- `GET /admin/kiro-gateway/usage/:event_id`
- `GET /admin/kiro-gateway/accounts/statuses`
- `GET|POST /admin/kiro-gateway/accounts`
- `POST /admin/kiro-gateway/accounts/import-local`
- `PATCH|DELETE /admin/kiro-gateway/accounts/:name`
- `GET|POST /admin/kiro-gateway/accounts/:name/balance`

## Required Runtime Parity

### Shared Key And Quota Behavior

- Bearer token authentication by key hash.
- Key status, quota, billable token counters, request counters, public/private
  visibility, model mappings, route strategy, fixed-account routing, and
  auto-account subset routing.
- Account groups and group membership.
- Runtime config defaults and per-key override behavior.
- Public usage lookup for private active keys by secret.
- Usage rollups and event counts equivalent to the current admin surfaces.

### Codex / LLM Gateway Behavior

- Codex account import, removal, patching, and refresh.
- Codex auth refresh and account availability logic.
- Account selection based on runtime availability and quota state, not merely
  the presence of auth files.
- Model catalog generation and key-scoped model exposure.
- OpenAI-compatible request normalization and response/SSE conversion.
- Existing error logging on user-reachable Codex paths.
- Usage event persistence for success, failures, missing usage, token counts,
  timing diagnostics, and request/response metadata.

### Kiro Behavior

The Kiro side must be preserved in detail:

- Social and IDC/OIDC auth import and refresh.
- Kiro local import from the known CLI credential stores.
- `profileArn` handling for usage-limit queries, assistant requests, and MCP
  headers.
- Account status cache, persisted cache snapshot, and manual balance refresh.
- Kiro route controls: `route_strategy`, `fixed_account_name`, and
  `auto_account_names`.
- Per-account scheduler settings:
  - max concurrency
  - minimum start interval
  - inherited global defaults for missing account settings
- Upstream cooldown and failover:
  - account-scoped cooldown for 5-minute credit limit signals
  - account-scoped cooldown for `DAILY_REQUEST_COUNT`
  - shortest-cooldown waiting only when all eligible accounts are blocked
  - proxy cooldowns and proxy-aware fallback
- Request validation and local rejection for malformed public requests.
- Exact outbound body-size guard after Kiro request serialization.
- Current-turn image handling and history image rejection behavior.
- Anthropic request conversion, tool/web-search handling, thinking settings,
  and model-name overrides.
- Standard streaming `/v1/messages` behavior.
- Claude Code `/cc/v1/messages` buffered behavior that waits for context usage
  before rewriting input token usage.
- Kiro cache simulation and policy:
  - global cache policy
  - per-key cache policy override JSON
  - cache creation cost adjustment
  - `kiro_cache_estimation_enabled`
  - `kiro_zero_cache_debug_enabled`
  - gated full request body capture for zero-cache diagnostics
- Successful and failed Kiro usage event recording, including quota failover
  count, cache token splits, timing fields, request detail fields, and
  diagnostic flags.

## Storage Design

### SQLite Control Plane

SQLite is the source of truth for mutable operational state:

- runtime config
- keys
- account groups
- proxy configs
- proxy bindings
- Codex accounts and account settings
- Kiro accounts, account scheduler settings, and persisted status cache
- token requests
- account contribution requests
- GPT2API account contribution requests if still owned by the LLM access flow
- sponsor requests
- usage rollup counters needed by hot-path key selection and admin summaries
- CDC apply state

SQLite tables should be normalized where it improves update correctness and
index locality. API response compatibility must be handled in typed repository
methods, not by leaking raw table shapes into handlers.

Required indexes:

- key hash lookup for authentication
- key id lookup
- key status/provider visibility filters
- account provider/status/name lookup
- route/account-group membership lookup
- request queue status/created-at lookup
- usage rollup lookup by key id and provider
- CDC high-water and idempotency lookup

### DuckDB Analytics Plane

DuckDB is the source of truth for append-heavy analytical events:

- usage events
- latency/timing fields
- provider/account/key/model dimensions
- token and billable-token facts
- cache simulation facts
- upstream status/error facts
- request body diagnostics when enabled
- event detail payloads for admin detail views

The primary usage fact table should be wide and denormalized. Do not design the
normal admin usage query around multiple analytical joins. DuckDB joins are
acceptable for rare detail views, one-to-one detail payload lookup, or offline
maintenance, but the common listing and aggregate paths should scan one fact
table with direct filters.

Recommended layout:

- `usage_events`: wide fact table containing event id, timestamps, key/account
  names, provider type, model ids, status, token facts, cache facts, quota
  failover count, stream timings, body sizes, error summaries, and stable
  denormalized labels.
- `usage_event_details`: one-to-one heavy detail payloads keyed by event id for
  request/response JSON and headers. This table is only read by detail views or
  diagnostics.
- `usage_rollups_hourly` and `usage_rollups_daily`: optional materialized
  rollups maintained by background jobs or batch maintenance.

## Code Architecture

### New Runtime Boundary

The extraction should create a real runtime boundary instead of copying large
backend modules into `llm-access` unchanged.

Recommended crate split:

- `llm-access-core`
  - Provider-agnostic key auth, request context, usage event model, timing
    model, route strategy, scheduler traits, and common response types.
- `llm-access-store`
  - SQLite and DuckDB repositories behind typed interfaces.
- `llm-access-codex`
  - Codex auth, account pool, request normalization, upstream dispatch,
    response conversion, models, and usage conversion.
- `llm-access-kiro`
  - Kiro token manager, account store adapter, scheduler, cache simulator,
    Anthropic converter, stream parser, provider dispatch, status cache, and
    usage conversion.
- `llm-access`
  - Axum HTTP server, route registration, admin/public handlers, config,
    lifecycle, shutdown, and health.

StaticFlow backend should stop owning provider internals after extraction. It
can either proxy LLM paths to `llm-access` or leave routing to Caddy/Pingora.

## Implementation Status

- `llm-access-core` owns the route-surface and provider-neutral usage
  contracts.
- `llm-access-store`, `llm-access-migrations`, and `llm-access-migrator` now
  cover the initial SQLite control plane, DuckDB usage fact schema, LanceDB
  snapshot import, and CDC replay tables.
- `llm-access-kiro` owns the pure Kiro scheduler, parser, wire helpers, cache
  simulation, cache policy, billable multipliers, Anthropic conversion,
  streaming, and web-search semantics. Backend modules re-export or delegate to
  those implementations.
- `llm-access-codex` owns Codex/OpenAI-compatible request normalization,
  response/SSE adaptation, usage extraction, embedded default instructions, and
  model catalog normalization. The StaticFlow backend keeps only transport and
  runtime orchestration around those shared helpers.
- `llm-access` now opens the SQLite-backed control store at startup and
  authenticates provider requests by bearer secret before dispatch. Missing,
  malformed, unknown, and non-active keys are rejected before any provider
  runtime is selected.
- `llm-access` now has an explicit provider dispatch boundary after bearer
  authentication. The default dispatcher preserves the current 501 behavior,
  while tests can inject a dispatcher and prove active keys reach the provider
  runtime seam with the authenticated key and original request.
- `llm-access-core` now exposes provider-route requirements for Codex/OpenAI
  and Kiro/Anthropic paths, and `llm-access` rejects keys whose stored
  provider/protocol does not match the requested provider route before
  dispatch.
- `llm-access` now enforces key billable quota before dispatch. Kiro-compatible
  exhausted keys keep the existing 402 behavior, while Codex/OpenAI-compatible
  exhausted keys keep the existing 429 `quota_exceeded` behavior.
- `llm-access` now accepts Kiro data-plane keys from `x-api-key` as well as
  `Authorization: Bearer`, while Codex/OpenAI-compatible routes remain
  Bearer-only.
- `llm-access` now serves Kiro local compatibility endpoints for
  `/api/kiro-gateway/v1/models`,
  `/api/kiro-gateway/v1/messages/count_tokens`, and
  `/api/kiro-gateway/cc/v1/messages/count_tokens` without requiring provider
  key authentication, matching the existing backend route contract.
- `llm-access` now serves the unauthenticated public access compatibility
  endpoints `/api/llm-gateway/access` and `/api/kiro-gateway/access` from the
  standalone runtime state. The LLM access response reads active public keys
  and rollup counters from SQLite; the Kiro response preserves the current
  backend behavior of exposing an empty public account list.
- `llm-access` now serves `/api/llm-gateway/model-catalog.json` directly from
  the standalone service. `llm-access-codex` owns the default public Codex
  catalog construction and injects the embedded base instructions through the
  same normalization path used by imported upstream catalogs.
- `llm-access` now serves `/api/llm-gateway/status` as an unauthenticated
  compatibility endpoint. The response comes from a persisted SQLite Codex
  public status snapshot when present, and otherwise exposes the same `loading`
  empty-cache shape used by the existing backend before its refresh task warms.
- `llm-access` now serves authenticated Codex/OpenAI model-list requests for
  `/v1/models`, `/api/llm-gateway/v1/models`, and
  `/api/codex-gateway/v1/models` from the standalone default Codex catalog.
  Other Codex generation routes still remain behind the explicit provider
  dispatch seam until the real upstream account runtime is wired.
- `llm-access` now serves the public support/community compatibility endpoints
  `/api/llm-gateway/support-config` and
  `/api/llm-gateway/support-assets/:file_name` from the same
  `LLM_ACCESS_SUPPORT_DIR` file layout used by the current backend.
- `llm-access` now serves the public thank-you wall endpoints
  `/api/llm-gateway/account-contributions` and `/api/llm-gateway/sponsors`.
  The standalone SQLite repository lists only issued account contributions and
  approved sponsors, preserving the existing public response shape, imported
  account-name fallback, and processed-time ordering.
- `llm-access` now accepts the public submission endpoints
  `/api/llm-gateway/token-requests/submit`,
  `/api/llm-gateway/account-contribution-requests/submit`, and
  `/api/llm-gateway/sponsor-requests/submit`. The handlers normalize and
  validate inputs using the existing backend limits, apply the same per-client
  public submit rate-limit rule, and persist pending/submitted rows into the
  standalone SQLite control plane. Sponsor payment email remains an explicit
  runtime dependency to wire later; the current standalone path records the
  same no-email-notifier state as the backend does when email is unavailable.
- `llm-access-kiro` now owns Kiro auth-file persistence, local Kiro CLI import,
  deterministic machine-id derivation, and token-count estimation. The backend
  keeps compatibility modules that re-export those implementations.
- `llm-access-kiro` now exposes the Kiro runtime config contract used by
  scheduling, status refresh, and cache simulation. The backend adapts its
  existing shared config into that contract instead of leaking backend state
  types into Kiro logic.
- `llm-access-kiro` now owns Kiro balance/cache status view types, persisted
  status-cache snapshot helpers, account eligibility checks, aggregate snapshot
  summarization, quota-exhausted entries, and duplicate upstream-identity
  grouping. The backend status cache keeps orchestration and logging while
  delegating those reusable rules to the shared crate.

### Store Adapter Rule

Existing backend logic should be extracted behind storage traits before it is
wired to SQLite/DuckDB. This keeps behavior stable while changing persistence.

Do not create a second implementation that merely resembles the old one. Any
logic that decides account eligibility, request conversion, cache estimation,
cooldown, token refresh, or usage accounting must have one source of truth.

## Migration Flow

1. Freeze the full LLM feature matrix in this design.
2. Extract backend LLM/Kiro runtime logic into reusable crates without changing
   behavior.
3. Introduce SQLite/DuckDB repository implementations.
4. Make existing backend tests pass against extracted logic.
5. Add `llm-access` route handlers that use the extracted runtime and new
   repositories.
6. Export a LanceDB snapshot of all existing LLM control and usage state.
7. Import the snapshot into SQLite/DuckDB.
8. Replay CDC until target state catches up.
9. Run local `llm-access` with test-only traffic.
10. Run frontend/admin compatibility checks against `llm-access`.
11. Add gateway or Caddy path split for canary keys.
12. Promote `llm-access` to source of truth only after parity checks pass.

## Frontend Compatibility Contract

The current frontend is the compatibility oracle. Existing pages should keep
working:

- `/admin/llm-gateway`
- `/admin/kiro-gateway`
- `/admin/kiro-gateway/accounts`
- public LLM access pages and usage lookup pages

Rules:

- Keep URL paths compatible.
- Keep response field names compatible.
- Keep `serde(deny_unknown_fields)` mirrored frontend structs compatible.
- Keep admin reload behavior cache-safe.
- Do not remove any route, status, model mapping, cache policy, debug flag, or
  scheduler field exposed by the current UI.

## Verification Matrix

### Unit Tests

- SQLite repository CRUD and index-backed lookup tests.
- DuckDB usage event insert/list/aggregate/detail tests.
- Key auth and quota accounting tests.
- Route strategy tests.
- Kiro scheduler tests for concurrency, start interval, cooldown, proxy
  cooldown, and failover.
- Kiro cache policy and zero-cache debug gating tests.
- Codex request normalization and malformed request rejection tests.
- Kiro Anthropic conversion, body-size guard, and malformed request rejection
  tests.

### Integration Tests

- `llm-access` starts from an isolated state root.
- Existing `/api/llm-gateway/*` and `/api/kiro-gateway/*` routes return the same
  response shape as the backend implementation.
- Admin key/group/config/account routes round-trip through SQLite.
- Usage events write to DuckDB and admin usage APIs can read them.
- Kiro `/cc/v1/messages` preserves buffered context-usage semantics.
- Kiro `/v1/messages` preserves standard streaming semantics.
- Codex and Kiro fake-upstream tests cover success, upstream 400/429/500, retry,
  cooldown, and no-account-available cases.

### Frontend Compatibility

- Compile frontend API types against the new response shapes.
- Run targeted admin page tests for `/admin/llm-gateway` and
  `/admin/kiro-gateway`.
- Verify saved key route/status/model/cache fields reload correctly.

### Operational Checks

- Confirm `llm-access` fails closed when state paths are outside the state root.
- Confirm no live StaticFlow backend process is restarted during local
  verification.
- Confirm local gateway path split remains disabled until parity passes.
- Confirm canary traffic can be rolled back by config only.

## Relationship To Existing Plans

`2026-04-30-llm-access-cloud-migration-design.md` remains valid as the cloud
deployment target, but its implementation order must start with full parity
extraction.

`2026-04-30-pingora-llm-routing-local-canary-design.md` remains valid as the
later canary mechanism, but it must not be executed before `llm-access` is a
real provider target.

## Open Decisions

- Whether to keep GPT2API account contribution requests inside the LLM access
  service or leave them with the image gateway ownership boundary.
- Whether StaticFlow backend should proxy LLM admin APIs to `llm-access` during
  local development, or whether Pingora/Caddy should always own the split.
- Exact DuckDB retention and compaction schedule after production cutover.
- Exact cutover point when LanceDB-backed LLM tables become read-only legacy
  data.
