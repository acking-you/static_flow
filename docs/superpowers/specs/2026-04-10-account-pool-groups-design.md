# Account Pool Groups Design

## Goal

Replace per-key direct account selection with reusable account-pool groups for both Codex and Kiro. Keys should bind to groups, not raw account lists. Existing keys that already carry manual account subsets must be migrated into equivalent groups without changing effective routing behavior.

## Constraints

- Do not break existing keys or current routing semantics.
- Keep the routing model simple enough to understand from the admin UI.
- Avoid runtime compatibility shims as the long-term source of truth.
- Preserve the existing `route_strategy` semantics:
  - `auto` means "route inside a pool"
  - `fixed` means "bind to exactly one account"
- Support both Codex and Kiro from one shared persistence model.

## Current Problems

### Per-key subset selection does not scale

Both Codex and Kiro keys currently store raw account routing fields directly on the key:

- `fixed_account_name`
- `auto_account_names`

This makes key cards large and hard to manage when the account list grows. It also duplicates the same account subsets across multiple keys.

### The same routing pool must be re-created over and over

If several keys should route through the same pool, the operator currently repeats the same manual checkbox selection on each key.

### Existing data already depends on the legacy fields

There are keys in production data that already rely on the legacy subset fields, especially in Kiro. The new design must migrate these keys rather than silently dropping their routing scope.

## Recommended Design

### Shared persisted account-group table

Add a new shared LanceDB table:

- `llm_gateway_account_groups`

Each row represents one reusable routing pool:

- `id`
- `provider_type`
- `name`
- `account_names_json`
- `created_at`
- `updated_at`

The table is shared across providers, but every group is namespaced by `provider_type`, so Codex and Kiro groups remain isolated.

### Keys bind to groups instead of raw account subsets

Extend `LlmGatewayKeyRecord` with:

- `account_group_id: Option<String>`

After migration, the routing source of truth becomes:

- `route_strategy = "auto"` + `account_group_id = Some(...)`
  - route only within that group
- `route_strategy = "auto"` + `account_group_id = None`
  - route within the full provider pool
- `route_strategy = "fixed"` + `account_group_id = Some(...)`
  - the group must contain exactly one account
- `route_strategy = "fixed"` + `account_group_id = None`
  - invalid configuration

The legacy per-key fields remain in storage only as migration-era compatibility input. New UI and new writes should no longer treat them as editable state.

### Group-based admin UX

Each provider admin page gets a dedicated "Account Groups" management section:

- list existing groups
- create a group with a name and selected member accounts
- edit group name and membership
- delete a group if no key references it

Each key card changes from raw account selection to group selection:

- `auto` route shows a group picker plus a "full pool" option
- `fixed` route shows only groups that contain exactly one account

This removes large checkbox blocks from key cards and makes pool reuse explicit.

## Migration Strategy

### Explicit key migration on backend startup

Run a one-time explicit migration during backend startup:

1. Load all Codex and Kiro keys.
2. For each key with legacy routing fields but no `account_group_id`:
   - `fixed_account_name` -> create a single-account group
   - `auto_account_names` -> create a group containing that subset
3. Set the key's `account_group_id` to the new group.
4. Clear the legacy routing fields on the key:
   - `fixed_account_name = None`
   - `auto_account_names = None`
5. Persist the rewritten key.

Groups created by migration should be key-local, not globally deduplicated by identical membership. That avoids surprising cross-key coupling after migration.

Recommended migrated group naming:

- `Migrated <key_name>`

If needed for uniqueness:

- `Migrated <key_name> <short_id>`

### Why startup migration instead of lazy runtime fallback

Startup migration is the correct mechanism because:

- it converts old data into the new source of truth once
- it avoids indefinite dual-source routing logic
- it makes the admin UI immediately consistent after upgrade

## Backend Behavior Changes

### Codex routing

Replace direct use of `fixed_account_name` / `auto_account_names` in `resolve_auth_for_key(...)` with group resolution:

- resolve the referenced group by `account_group_id`
- validate provider match
- derive allowed account names from the group
- for `fixed`, require exactly one account in the group

### Kiro routing

Replace direct use of `fixed_account_name` / `auto_account_names` in `filter_auths_for_key_route(...)` with group resolution:

- resolve the referenced group by `account_group_id`
- for `fixed`, require a single-account group
- for `auto`, filter to the group members
- if the configured group has no currently available members, fail the request without widening scope

### Key validation and patch APIs

Patch/create handlers must validate group references:

- group must exist
- provider must match key provider
- `fixed` may only target a single-account group

For `auto`, `account_group_id = None` remains a valid "full pool" configuration.

## Frontend Behavior Changes

### Codex admin page

- Add an account-group management panel.
- Replace direct raw account selection on key cards with group selection.
- Keep existing request limit and quota controls unchanged.

### Kiro admin page

- Add a parallel account-group management panel.
- Replace the current route UI that directly manipulates account names with group selection.
- Preserve existing Kiro-only controls such as request validation and cache estimation toggles.

## Error Handling

- Deleting a group referenced by one or more keys must be rejected with a clear admin error.
- If a key references a missing group, routing should fail clearly instead of widening to the full pool.
- If startup migration finds invalid legacy names, it should still migrate the surviving names when possible and log what was dropped.
- If a migrated `fixed` key has no surviving account, leave it without `account_group_id` and mark the key invalid at validation time instead of guessing.

## Testing

### Shared store

- account-group round-trip encode/decode
- schema migration for the new table and new key column

### Migration

- legacy fixed key becomes single-account group + rewritten key
- legacy auto subset key becomes subset group + rewritten key
- keys without legacy subset remain unchanged

### Backend routing

- Codex auto group limits selection to group members
- Codex fixed group requires one account
- Kiro auto group filters auth pool correctly
- Kiro fixed group requires one account
- deleting an in-use group is rejected

### Frontend

- group editor sanitizes membership against available accounts
- key editor renders group selection instead of raw account checkbox pools
- saved payloads send `account_group_id`

## Non-Goals

- No nested groups.
- No weighted routing across groups.
- No automatic group generation from account metadata.
- No cross-provider mixed groups.
