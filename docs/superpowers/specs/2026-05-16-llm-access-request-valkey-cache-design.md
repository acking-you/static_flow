# llm-access request-path Valkey cache design

Date: 2026-05-16
Status: approved in brainstorming, pending implementation plan

## Summary

Move `llm-access` request-time account selection and dispatch preparation off
the hot Postgres path and onto a shared Valkey cache. Postgres remains the
single durable source of truth. Valkey becomes the primary request-time read
surface for:

- authenticated key lookup;
- provider request-route snapshots;
- per-account selection status views;
- per-account dispatch auth payloads.

This design intentionally accepts bounded staleness:

- request-time selection may lag durable truth by up to roughly `5-10s`;
- cache entries may live for several hours;
- freshness is maintained by explicit cache update/invalidation on writes and
  background refreshes, not by short TTLs.

The deployed shared cache target is the Tencent Cloud Hong Kong Valkey node on
`lb7666.top:16379`, with local private connection material stored under:

- `.local/common/valkey/lb7666.env`

## Problem

The current largest avoidable Neon traffic cost is no longer the admin UI. It
is the request hot path itself.

Today, a typical request still triggers Postgres control reads for data that is
mostly slow-changing:

- key bundle lookup by `key_id`;
- Codex route candidate listing;
- Kiro route candidate listing;
- Kiro cached status map listing;
- selected-account auth hydration.

The current request path also splits route resolution into multiple reads:

1. load key bundle;
2. load route candidates;
3. inspect cached status;
4. select one account;
5. re-load the selected account with full auth payload.

That design keeps correctness simple, but it makes every provider request pay
for control-plane reads against Neon even when the underlying routing state has
not changed.

## Goals

- remove Postgres from the normal request hot path for provider selection;
- keep Postgres as the single durable source of truth;
- preserve current request correctness and routing semantics;
- accept `5-10s` of bounded staleness on request-time selection state;
- use long-lived cache entries with deterministic TTL jitter to prevent cache
  stampedes;
- keep cache invalidation operationally simple and explicit;
- use the already deployed shared Valkey node rather than a local-only cache.

## Non-goals

- no attempt to make Valkey a durable source of truth;
- no dual-write control-plane migration away from Postgres;
- no admin page caching redesign in this task;
- no redesign of usage analytics, usage worker, or DuckDB persistence;
- no strong distributed consistency across multiple API nodes;
- no requirement that request-time routing reflect sub-second status changes.

## Scope boundary

In scope:

- request authentication cache;
- request-time route snapshot cache;
- per-account selection-state cache;
- per-account dispatch-auth cache;
- invalidation/update rules on control-plane writes and background refreshes;
- rollout verification focused on request-path Neon traffic reduction.

Out of scope:

- admin list/summary caches;
- public status API caching beyond existing local behavior;
- usage-event persistence;
- worker-side analytics storage;
- cross-region failover design for the Valkey node itself.

## Current state

### Request hot path

The current code still depends on Postgres during request dispatch:

- key bundle load:
  `llm-access-store/src/postgres.rs:631`
- Codex route candidates:
  `llm-access-store/src/postgres.rs:1713`
- Kiro route candidates:
  `llm-access-store/src/postgres.rs:2377`
- Kiro cached status map:
  `llm-access-store/src/postgres.rs:2442`
- selected-account full-auth hydration:
  `llm-access/src/provider.rs:831`
  `llm-access/src/provider.rs:863`

Codex already has one limited cache:

- a short-lived in-process cached public status snapshot:
  `llm-access-store/src/postgres.rs:539`

That existing cache is not enough to remove Postgres from request-time route
selection.

### Existing routing semantics that must remain true

- Postgres remains the durable truth for keys, accounts, groups, proxies, and
  status cache rows.
- Request-time selection still depends on runtime status surfaces, not only on
  auth-file presence.
- Selected account auth hydration must still return the same effective payload
  that current dispatch uses.

## Chosen approach

Use a standard cache-aside design with explicit write-side update/invalidation:

1. Postgres remains the only durable source of truth.
2. Valkey becomes the default primary read source for request-time dispatch
   inputs.
3. Cache entries use long TTLs with deterministic jitter.
4. Freshness comes from explicit write-side maintenance and background refresh
   updates, not from short expirations.
5. Cache misses rebuild from Postgres under a short single-flight lock to avoid
   stampedes.

This is intentionally simpler than a distributed event-stream or consensus
design because the user has already accepted bounded staleness and prioritizes
lower Neon traffic over stronger real-time consistency.

## Alternatives considered

### 1. Keep Postgres as the main request read path

Rejected because it preserves the current traffic problem. Narrow SQL and
pagination improvements help admin/UI traffic, but do not solve hot-path control
reads.

### 2. Cache full dispatch objects per key, including auth and dynamic status

Rejected because it creates large, highly coupled cache entries:

- one account auth refresh would invalidate many keys;
- one status refresh would invalidate many keys;
- one proxy/runtime change would invalidate many keys.

This shape is easy to read but expensive to keep coherent.

### 3. Split cache into stable request snapshots plus account-level dynamic
views

Chosen because it minimizes invalidation blast radius while keeping the read
path simple.

## Cache model

The request cache is split into four primary entry classes plus one generation
namespace.

### 1. Auth key cache

Key:

- `llma:auth:{secret_hash}`

Payload:

- `key_id`
- `key_name`
- `provider_type`
- `protocol_family`
- `status`
- `quota_billable_limit`
- `billable_tokens_used`

Purpose:

- authenticate incoming secrets without touching Postgres on a cache hit.

Negative cache:

- nonexistent or disabled key auth misses are cached separately with short TTL.

### 2. Request snapshot cache

Key:

- `llma:req:{provider}:{key_id}`

Payload:

- provider-specific route strategy;
- fixed-account name;
- auto-account names;
- account-group id or resolved account-name membership;
- model-map data;
- per-key request limiter settings;
- provider-specific request toggles;
- cache generation number captured when the snapshot was built.

Purpose:

- represent the key-scoped static routing view used to begin request selection.

Explicit rule:

- this payload does **not** embed full auth payloads;
- this payload should avoid embedding rapidly changing account status fields.

### 3. Account selection view cache

Key:

- `llma:acct:view:{provider}:{account_name}`

Payload:

- account status / disabled state;
- request concurrency and start-interval settings;
- cached last error;
- cached remaining quota/credits used for ordering;
- proxy reference fields needed for selection and routing identity;
- Kiro cached balance/cache view fields needed for request-time eligibility;
- Codex cached usage error / remaining state needed for request-time
  eligibility.

Purpose:

- provide the dynamic, per-account view required during selection.

### 4. Account dispatch auth cache

Key:

- `llma:acct:auth:{provider}:{account_name}`

Payload:

- auth payload required to actually dispatch upstream;
- small auth-only metadata needed for request construction.

Purpose:

- avoid the final selected-account Postgres hydrate read.

### 5. Dispatch generation keys

Key:

- `llma:gen:dispatch:{provider}`

Payload:

- monotonically increasing integer version.

Purpose:

- invalidate all key-scoped request snapshots for a provider class without
  deleting every key entry individually.

No TTL.

## TTL strategy

### Rules

- cache TTLs are long;
- expirations are jittered;
- jitter is deterministic per cache key;
- read hits do not extend TTL;
- freshness is maintained by explicit writes/invalidation.

### Recommended defaults

- `auth key`: base `6h`, jitter range `[-20%, +20%]`
- `request snapshot`: base `6h`, jitter range `[-20%, +20%]`
- `account selection view`: base `4h`, jitter range `[-25%, +25%]`
- `account dispatch auth`: base `4h`, jitter range `[-25%, +25%]`
- negative auth cache: base `5m`, jitter range `[-40%, +40%]`
- miss lock: fixed `5s`

### Deterministic jitter

Use a stable hash of the final Redis key to derive the TTL offset, for example:

`ttl = base_ttl * (0.8 + 0.4 * normalized_hash(cache_key))`

This prevents synchronized expiry while keeping TTL behavior stable for the
same key across refreshes and deployments.

## Request flow after redesign

### Auth phase

1. Hash incoming secret.
2. Read `llma:auth:{secret_hash}` from Valkey.
3. On hit, continue.
4. On miss, acquire short single-flight lock, rebuild from Postgres, write
   cache, then continue.
5. On not-found, write short negative cache and reject.

### Route selection phase

1. Read `llma:gen:dispatch:{provider}`.
2. Read `llma:req:{provider}:{key_id}`.
3. If missing or generation mismatch, rebuild the snapshot from Postgres under
   a short lock.
4. From the snapshot, collect candidate account names.
5. Bulk-read all corresponding `llma:acct:view:*` entries.
6. If a subset is missing, bulk-rebuild only the missing account views.
7. Run the existing in-memory selection logic against the cached account views.

### Dispatch phase

1. After one account is selected, read `llma:acct:auth:*`.
2. If missing, rebuild only that account auth payload from Postgres.
3. Dispatch upstream using the cached auth payload and the already selected
   route view.

Normal request hits should not require Postgres after this redesign.

## Write and invalidation rules

### Key changes

When key metadata or secret changes:

- invalidate `llma:auth:{secret_hash}` for old and new secret hashes as needed;
- invalidate `llma:req:{provider}:{key_id}`.

### Key route changes

When route strategy, account selection mode, model map, account-group binding,
or provider-specific key toggles change:

- `INCR llma:gen:dispatch:{provider}`;
- optionally delete the changed key’s `llma:req:*` entry immediately.

### Group membership changes

When account groups change in a way that affects candidate membership:

- `INCR llma:gen:dispatch:{provider}`.

### Proxy/runtime config changes

When a change affects request-time provider route resolution:

- `INCR llma:gen:dispatch:{provider}`.

### Account auth refresh changes

When persisted account auth changes:

- overwrite `llma:acct:auth:{provider}:{account_name}`;
- if the auth change also affects request-time routing identity, overwrite
  `llma:acct:view:{provider}:{account_name}` too.

### Account status refresh changes

When background refresh updates remaining credits, usage errors, or cached
status:

- overwrite only `llma:acct:view:{provider}:{account_name}`.

Explicit rule:

- account status refresh must **not** bump provider generation;
- otherwise high-frequency refresh traffic would invalidate every key snapshot.

## Rebuild and stampede control

Use a standard short Redis single-flight lock per rebuild target.

Recommended lock keys:

- `llma:lock:auth:{secret_hash}`
- `llma:lock:req:{provider}:{key_id}`
- `llma:lock:acct:view:{provider}:{account_name}`
- `llma:lock:acct:auth:{provider}:{account_name}`

Rules:

- acquire with `SET NX EX 5`;
- loser requests briefly poll/re-read cache before falling back to a second
  rebuild attempt;
- if lock holder crashes, expiration bounds the wait;
- rebuild code should write the value and TTL atomically before releasing the
  lock.

## Multi-node behavior

This design is safe for multiple API nodes in the limited sense required here:

- all nodes share the same Valkey cache;
- each node may rebuild a missing entry if its short lock attempt wins;
- explicit writes and generation bumps propagate through the shared cache;
- no cross-node strong ordering is promised beyond eventual convergence inside
  the accepted `5-10s` window.

This is intentionally not a distributed consensus system.

## Failure behavior

### Valkey unavailable

If Valkey is unavailable:

- request path may fall back to direct Postgres reads for correctness;
- such fallback should be explicit, observable, and rate-limited where needed;
- the goal is degraded performance/cost, not request failure, when Postgres is
  still healthy.

This is the only fallback explicitly retained in the design because it preserves
userspace behavior while isolating the cache layer as an optimization.

### Postgres unavailable during cache miss

If both Valkey miss and Postgres read fail:

- return the same class of request failure the current implementation would
  return for unresolved routing/auth state;
- do not synthesize guessed routes.

### Stale cache entries

Bounded staleness is acceptable. The correctness rule is:

- cache may be stale by seconds;
- cache may not invent state that durable truth never had;
- explicit writes/refreshes must eventually converge Valkey toward Postgres.

## Security and secret handling

- Redis auth payloads contain provider auth material, so the cache is sensitive.
- Use authenticated Valkey access only.
- Do not log cached auth payloads.
- Prefer dedicated key prefixes and a dedicated DB if operationally convenient.
- Private connection material stays only in ignored local env files and GCP
  service env, not in tracked repo files.

## Rollout plan

### Phase 1: cache infrastructure

- add Valkey client wiring and health checks;
- keep request path behavior unchanged;
- verify the deployed Valkey node from GCP and local development.

### Phase 2: auth cache

- move secret-hash authentication to Valkey first;
- verify key auth hit rate and correctness.

### Phase 3: request snapshot cache

- cache provider key-scoped route snapshots;
- keep selected-account auth hydrate on Postgres temporarily.

### Phase 4: account view cache

- cache request-time account selection status surfaces;
- move route selection fully off Postgres on cache hits.

### Phase 5: account auth cache

- cache selected-account dispatch auth payloads;
- eliminate final selected-account hydrate reads on cache hits.

### Phase 6: optimize and tighten observability

- add hit/miss metrics;
- confirm Neon traffic drops on the hot path;
- remove any now-unused request-time Postgres reads that remain.

## Verification expectations

Implementation should prove:

- authenticated requests can complete without Postgres on a cache hit;
- request routing still selects the same account set as the current logic;
- per-account status refresh updates account-view cache without invalidating all
  key snapshots;
- admin/config writes correctly invalidate or update the relevant cache keys;
- deterministic jitter produces a spread of TTLs across otherwise similar keys;
- Neon control traffic falls materially under real request load.

## Risks

- the biggest correctness risk is caching the wrong data boundary, especially by
  embedding fast-changing status or auth into coarse key snapshots;
- the biggest operational risk is silent cache drift if write-side invalidation
  misses one mutation path;
- the biggest security risk is over-broad logging or debugging of cached auth
  payloads.

This design reduces those risks by:

- splitting static key snapshots from dynamic account views;
- using explicit invalidation rules tied to concrete mutation classes;
- keeping Postgres as the only durable truth.

## Open implementation constraints

- The first implementation should prioritize the Postgres-backed live path only.
  It does not need to preserve a parallel SQLite-optimized cache path.
- Existing Codex public status caching can remain as-is initially; request-path
  Redis caching is an additional layer focused on hot dispatch reads.
- The Valkey node is already live and validated for connectivity; implementation
  should consume local private config rather than embedding connection data into
  tracked files.
