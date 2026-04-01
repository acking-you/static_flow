# Kiro Gateway Per-Key Account Routing Subset

## Goal

Add per-key account routing controls to `/admin/kiro-gateway` so each Kiro key
can either:

- use the existing default strategy and choose from the full account pool
- bind to one fixed account
- restrict automatic routing to a configured account subset

The default behavior must stay unchanged for existing keys: if no accounts are
specified, routing continues to select from all Kiro accounts using the current
fairness-based strategy.

## Current Problems

1. Kiro key records already expose `route_strategy`, `fixed_account_name`, and
   `auto_account_names`, but `/admin/kiro-gateway` does not let admins edit them
2. Kiro admin patch requests do not accept these fields, so the data path is
   incomplete even if the UI were added
3. Kiro runtime routing in `provider.rs` always iterates the full account pool
   and ignores per-key routing metadata entirely
4. Codex already supports the desired semantics, so Kiro currently behaves
   inconsistently with the rest of the gateway admin model

## Design

### Route Semantics

Kiro will use the same routing contract as Codex:

- `fixed` + `fixed_account_name`
  - route only to that account
  - if the account does not exist or is not usable, fail the request
- `auto` + empty `auto_account_names`
  - use the full Kiro account pool
  - preserve the current Kiro fairness/quota/cooldown selection behavior
- `auto` + non-empty `auto_account_names`
  - restrict candidate accounts to that subset
  - apply the existing Kiro fairness/quota/cooldown logic only within that
    subset
  - if the configured subset has no existing or no usable accounts, fail the
    request

No widening fallback is allowed when a subset is configured. A configured subset
must mean a real constraint, not a hint.

### Admin UI

**File:** `frontend/src/pages/admin_kiro_gateway.rs`

Extend the Kiro key editor card with the same core controls already used on the
Codex admin page:

- route strategy selector: `auto` or `fixed`
- fixed-account selector when strategy is `fixed`
- account checkbox list when strategy is `auto`
- summary text describing the effective mode:
  - fixed binding
  - auto over all accounts
  - auto over a restricted subset

The Kiro page should reuse the Codex interaction model, but only the minimum
necessary controls should be added. This change does not need to expand the
create-key form; route settings remain editable after key creation.

Sanitization rules in the UI:

- drop unknown account names when refreshing editor state from server data
- keep empty subset as the explicit representation of “auto over full pool”
- keep comments and labels simple; do not invent a new routing vocabulary

### Frontend API Contract

**File:** `frontend/src/api.rs`

Extend `patch_admin_kiro_key(...)` so it can send:

- `route_strategy`
- `fixed_account_name`
- `auto_account_names`

`create_admin_kiro_key(...)` remains minimal and continues to create keys with
no routing constraints by default.

### Backend Admin Patch Contract

**Files:**

- `backend/src/kiro_gateway/types.rs`
- `backend/src/kiro_gateway/mod.rs`

Extend `PatchKiroKeyRequest` with:

- `route_strategy: Option<String>`
- `fixed_account_name: Option<String>`
- `auto_account_names: Option<Vec<String>>`

Patch handling rules:

- normalize and validate `route_strategy`
- for `fixed`
  - require a non-empty `fixed_account_name`
  - ensure that account exists in Kiro auth records
  - clear `auto_account_names`
- for `auto`
  - clear `fixed_account_name`
  - normalize `auto_account_names` by trimming, deduplicating, and dropping
    empty entries
  - if a non-empty configured subset is provided, intersect it with existing
    Kiro accounts
  - if the resulting subset is empty, reject the patch

This keeps persistence semantics aligned with Codex while using Kiro’s own
account source of truth.

### Runtime Routing

**Files:**

- `backend/src/kiro_gateway/anthropic/mod.rs`
- `backend/src/kiro_gateway/anthropic/websearch.rs`
- `backend/src/kiro_gateway/provider.rs`

The Kiro provider must receive the authenticated key record so runtime routing
can apply per-key restrictions.

Implementation shape:

- thread `&LlmGatewayKeyRecord` from the authenticated request handler into
  provider entry points
- derive the candidate account set from key routing metadata before the current
  account ordering step
- reuse the existing selection machinery after candidate filtering

Filtering rules:

- `fixed`
  - candidate set is the single bound account
- `auto` with subset
  - candidate set is exactly the configured subset
- `auto` without subset
  - candidate set is all accounts

The existing Kiro routing behavior stays intact after filtering:

- fairness ordering by least recently started routing identity
- cached remaining balance as a secondary tiebreaker
- cooldown skipping
- local concurrency and start-interval throttling
- token refresh retry
- quota exhausted and rate-limit handling

This is the key compatibility point: the change only constrains the candidate
pool; it does not rewrite Kiro’s actual scheduling algorithm.

### Error Semantics

Errors must stay explicit and deterministic:

- fixed account missing or unavailable
  - request fails
- configured auto subset has no existing accounts
  - request fails
- configured auto subset has existing accounts but none are usable
  - request fails
- no subset configured
  - request keeps using the full pool

Do not silently widen routing from a configured subset back to the full pool.

## Files Changed

| File | Change |
|------|--------|
| `frontend/src/pages/admin_kiro_gateway.rs` | Add per-key route strategy and account subset UI |
| `frontend/src/api.rs` | Send Kiro route fields in key patch requests |
| `backend/src/kiro_gateway/types.rs` | Extend patch request type with route fields |
| `backend/src/kiro_gateway/mod.rs` | Validate and persist Kiro key route settings |
| `backend/src/kiro_gateway/anthropic/mod.rs` | Pass authenticated key record into provider calls |
| `backend/src/kiro_gateway/anthropic/websearch.rs` | Pass authenticated key record into MCP provider path |
| `backend/src/kiro_gateway/provider.rs` | Filter candidate accounts by key routing metadata before selection |

## Files Not Changed

- `shared/src/llm_gateway_store/*`
  - existing schema already stores `route_strategy`, `fixed_account_name`, and
    `auto_account_names`
- Kiro account auth file format
- public Kiro access page
- create-key API payload shape

## Tests And Verification

Backend tests should cover:

- patch validation for `fixed` and `auto`
- fixed-account routing
- auto routing with no subset
- auto routing with a configured subset
- configured subset with no existing accounts
- configured subset with no usable accounts

Verification before implementation is considered complete:

- targeted Rust tests for affected Kiro modules pass
- `cargo clippy` passes for affected crates with zero warnings/errors
- changed Rust files are formatted with targeted `rustfmt`

## Risks And Assumptions

- **Assumption:** Kiro key records already persist routing metadata correctly in
  LanceDB because they share the existing `LlmGatewayKeyRecord` schema
- **Assumption:** Kiro request handlers already hold the authenticated key
  record, so threading it into provider calls is a straightforward interface
  change rather than a data-model change
- **Risk:** provider entry point signature changes touch both normal message
  requests and MCP websearch requests; both paths must be updated together
- **Risk:** Kiro runtime currently has no dedicated helper equivalent to Codex
  `resolve_auth_for_key`, so the new filtering logic must be introduced without
  duplicating the full provider loop
- **Compatibility constraint:** existing Kiro keys with unset routing fields
  must remain behaviorally identical to today
