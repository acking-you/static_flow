# gpt2api-rs Account Proxy Design

## Goal

Give `gpt2api-rs` a self-contained upstream proxy system that:

- stores reusable proxy configs inside `gpt2api-rs` itself
- lets each account independently choose how it reaches ChatGPT Web
- keeps StaticFlow's `/admin/gpt2api-rs` integration as a thin stateless proxy
- preserves current behavior for existing deployments that only use one global
  upstream proxy

## Problem

Today `gpt2api-rs` only supports one service-wide upstream proxy. The proxy URL
is owned by `ChatgptUpstreamClient` and applied to every account uniformly.

That creates three gaps:

1. One bad or rate-limited proxy can affect the whole account pool.
2. Operators cannot pin problematic accounts to specific proxies.
3. StaticFlow cannot offer a real proxy-management UI for `gpt2api-rs` without
   turning itself into the source of truth, which conflicts with the
   self-contained deployment goal.

## Recommended Design

### Self-contained proxy registry

`gpt2api-rs` should own its own reusable proxy registry in SQLite instead of
reusing StaticFlow's LanceDB-backed proxy store.

Add a new `proxy_configs` table in `control.db` with these fields:

- `id`
- `name`
- `proxy_url`
- `proxy_username`
- `proxy_password`
- `status`
- `created_at`
- `updated_at`

This is the only reusable proxy source inside `gpt2api-rs`. Do not add a
provider-level binding layer. `gpt2api-rs` talks to one upstream service, so a
second indirection would be useless complexity.

### Account-level proxy selection

Extend account records with a small proxy-selection model:

- `proxy_mode`
- `proxy_config_id`

The allowed modes are:

- `inherit`
  - use the existing global proxy from `ChatgptUpstreamClient`
- `direct`
  - force this account to bypass the global proxy
- `fixed`
  - force this account to use one row from `proxy_configs`

`fixed` requires a non-empty `proxy_config_id`. All other modes clear
`proxy_config_id`.

This should follow the same conceptual model already used by Kiro account proxy
selection, but implemented locally inside `gpt2api-rs` without any dependency
on `static_flow`.

### Effective proxy resolution

Each outbound upstream request should resolve the effective proxy from the
selected account just before building the HTTP client.

Resolution order:

1. `proxy_mode = fixed`
   - load the referenced proxy config
   - reject the request if the config is missing or disabled
2. `proxy_mode = direct`
   - build a direct client with no proxy
3. `proxy_mode = inherit`
   - use the current service-wide `ChatgptUpstreamClient.proxy_url`

Expose the resolved state back through admin account views:

- `effective_proxy_source`
- `effective_proxy_url`
- `effective_proxy_config_name`

This keeps StaticFlow stateless while still letting the page display what will
actually happen at runtime.

## Storage Design

### Account schema migration

Extend `accounts` with:

- `proxy_mode TEXT NOT NULL DEFAULT 'inherit'`
- `proxy_config_id TEXT`

All existing accounts migrate to `inherit`, so old installations behave exactly
as before until an operator changes one account.

### New proxy config table

Create `proxy_configs` in SQLite:

- primary key: `id`
- unique name is recommended
- `status` supports at least `active` and `disabled`

Passwords remain stored alongside the config, matching the current admin-facing
model already used elsewhere in StaticFlow. This is an operator system, not a
multi-tenant secret manager.

### Delete semantics

Deleting a proxy config that is still referenced by any account should fail
with a conflict error.

Do not silently rewrite bound accounts to `inherit`. Hidden fallback logic will
make operators think an account is still pinned when it is not.

## Runtime Design

### Upstream client surface

`ChatgptUpstreamClient` currently owns one optional global `proxy_url`. That
global field should remain, because it is the backward-compatible default for
`inherit`.

What changes is how request-specific clients are built:

- account requests no longer blindly use the global proxy
- `build_client(...)` should accept an effective per-request proxy setting
- the request path should resolve proxy selection from the account first, then
  build the client

This is the real source-of-truth fix. Do not bolt on post-selection hacks or
secondary transport wrappers.

### No heavy proxy registry cache

Do not copy StaticFlow's full provider registry and client-cache design into
`gpt2api-rs`.

`gpt2api-rs` only needs:

- load one proxy config by id when required
- validate account update requests against existing configs
- optionally probe a config through a lightweight check endpoint

If client reuse later matters, it can be added after the behavior is correct.
The first version should stay simple and obvious.

## Admin API

### New proxy-config endpoints

Add these `gpt2api-rs` admin endpoints:

- `GET /admin/proxy-configs`
- `POST /admin/proxy-configs`
- `PATCH /admin/proxy-configs/:id`
- `DELETE /admin/proxy-configs/:id`
- `POST /admin/proxy-configs/:id/check`

The shape should mirror the existing StaticFlow proxy-config UI closely enough
that the frontend can reuse the same interaction pattern:

- config list returns rows plus timestamps
- create and patch accept `name`, `proxy_url`, `proxy_username`,
  `proxy_password`, `status`
- check returns a small diagnostic payload with `ok`, `status_code`, and a
  human-readable message

The check endpoint only needs to prove that the configured proxy can reach the
ChatGPT upstream host. It does not need provider abstraction.

### Account update endpoint

Extend account update input with:

- `proxy_mode`
- `proxy_config_id`

Validation rules:

- unsupported `proxy_mode` is `400`
- `fixed` without `proxy_config_id` is `400`
- `fixed` pointing to a missing config is `400`
- `fixed` pointing to a disabled config is `400`

The account list response should include both the persisted selection and the
effective resolved values.

## StaticFlow Integration

### Backend

`backend/src/gpt2api_rs.rs` remains a transport proxy only.

Add forwarding routes for the new `gpt2api-rs` admin endpoints, but do not
store, transform, enrich, or merge any proxy data on the StaticFlow side.

StaticFlow stays stateless here:

- it authenticates the local admin user
- it forwards JSON to `gpt2api-rs`
- it returns `gpt2api-rs` responses as-is

### Frontend

Extend `/admin/gpt2api-rs` with two pieces:

1. Proxy config management
   - list existing configs
   - create a new config
   - edit/delete/check one config
2. Account-level proxy selector
   - `inherit`
   - `direct`
   - `fixed:<proxy_config_id>`

The visual and interaction model should borrow from the existing
`admin_llm_gateway` / Kiro pages:

- same proxy config editor structure
- same account-level select control
- same immediate effective-proxy display

But do not extract a generic shared abstraction unless the code naturally
converges. The first goal is correct behavior, not a forced UI framework.

## Compatibility

- Existing deployments keep working because all accounts default to `inherit`.
- Existing global proxy behavior remains the default path.
- StaticFlow remains compatible because its backend already acts as a thin
  proxy for `/admin/gpt2api-rs`.
- No existing public API behavior changes.

## Testing

Add focused tests for:

- SQLite migration of legacy accounts to `proxy_mode = inherit`
- proxy config CRUD round-trip
- account update validation for `inherit`, `direct`, and `fixed`
- delete conflict when a proxy config is still bound
- effective proxy resolution choosing:
  - global proxy for `inherit`
  - no proxy for `direct`
  - bound config for `fixed`
- frontend API/client round-trip for the new proxy-config endpoints

## Risks

- If `fixed` accounts are allowed to point at missing configs, behavior becomes
  non-deterministic. Reject that state early.
- If delete silently falls back to `inherit`, operators lose routing intent.
- If effective proxy is not returned in account views, the admin page becomes
  guesswork.
- If request-time proxy resolution is implemented outside the actual upstream
  client build path, the feature will look correct in admin but be fake in
  production.
