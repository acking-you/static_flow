# Codex Account Scheduler Design

## Goal

Give Codex upstream accounts the same local scheduling controls Kiro already has:

- per-account max concurrency
- per-account minimum start interval

The gateway should skip locally throttled Codex accounts during routing and
only wait or fail when every eligible account is blocked. Existing key-level
request limits stay in place as a separate second layer.

## Problem

Today Codex only has key-level local request throttling. The request flow first
selects a Codex account and only then applies `request_max_concurrency` /
`request_min_start_interval_ms` from the API key.

That creates two problems:

1. Multiple keys can overload the same upstream account because the account
   itself has no local scheduler state.
2. Auto-routing cannot rotate away from a hot account, because the local
   throttle sits after account selection and is keyed by gateway key instead of
   upstream account.

Kiro already solved this correctly by keeping scheduler state per account and
trying the next eligible account when one is cooling down or concurrency-blocked.

## Design

### Persisted account settings

Extend Codex account settings metadata with:

- `request_max_concurrency: Option<u64>`
- `request_min_start_interval_ms: Option<u64>`

These live beside the existing `map_gpt53_codex_to_spark` and proxy settings in
the Codex account `.meta` file. `None` means unlimited / no pacing constraint.

The admin account summary response also exposes both values so the existing
Codex account list can edit and display them directly.

### Runtime scheduler

Add a dedicated Codex account scheduler that mirrors the Kiro scheduler model:

- local state keyed by account name
- `try_acquire(account_name, max_concurrency, min_start_interval_ms, queued_at)`
- non-blocking rejection when an account is currently blocked
- optional wait duration for start-interval throttles
- RAII lease released when the upstream request completes

This scheduler is account-scoped only. It does not replace the current
key-scoped scheduler.

### Request routing

Codex request flow changes from:

1. resolve account
2. apply key-level request limit
3. send upstream request

to:

1. build the eligible account set from key routing config
2. try candidate accounts in selection order
3. for each candidate, try account-level scheduler acquire
4. pick the first account that acquires successfully
5. apply existing key-level scheduler
6. send upstream request

If a candidate account is locally blocked:

- log the block reason
- remember the shortest wait
- continue trying the next eligible account

If every eligible account is locally blocked:

- wait for the shortest known delay or a release notification
- retry selection

If no usable account exists for non-scheduler reasons, keep the current
service-unavailable behavior.

Legacy fallback to `~/.codex/auth.json` stays unchanged and bypasses the new
account scheduler because it is not part of the managed account pool.

### Admin API and UI

The existing Codex account settings patch endpoint grows two optional fields:

- `request_max_concurrency`
- `request_min_start_interval_ms`

The endpoint also supports clearing them back to unlimited using the current
“missing means unchanged” pattern plus explicit “unlimited” booleans, matching
the key editor semantics already used elsewhere in the admin UI.

Frontend changes stay in the current `/admin/llm-gateway` account list:

- show the current account-level limits
- allow editing and saving them with the other account settings

No new page or separate workflow is needed.

## Compatibility

- Key-level throttling remains unchanged.
- Existing Codex accounts without the new fields continue to behave as
  unlimited.
- Existing routing semantics (`fixed`, `auto`, account groups) remain
  unchanged.
- Kiro code paths are untouched.

## Testing

Add focused tests for:

- Codex account settings round-trip including new fields
- account scheduler concurrency and start-interval enforcement
- auto-routing skipping blocked accounts and selecting another eligible account
- request path returning/handling account-level local throttling correctly when
  all candidates are blocked

## Risks

- Holding the account lease across the full upstream round-trip is required; if
  released too early the concurrency guard becomes fake.
- The new account scheduler must not interfere with legacy auth fallback.
- The admin patch endpoint must distinguish “leave unchanged” from “clear to
  unlimited”, otherwise settings will be hard to manage.
