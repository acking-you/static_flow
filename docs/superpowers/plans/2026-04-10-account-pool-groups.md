# Account Pool Groups Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace per-key direct account subsets with reusable provider-scoped account groups for Codex and Kiro, and migrate existing keys to the new source of truth.

**Architecture:** Add one shared persisted account-group table plus one new `account_group_id` field on keys. Route resolution reads group membership instead of raw key-local account lists. A startup migration rewrites legacy keys into explicit groups once, then admin UIs switch to group management and group selection.

**Tech Stack:** Rust, Axum, Yew/WASM, LanceDB, serde JSON, existing StaticFlow admin UI patterns.

---

### Task 1: Persist account groups in the shared store

**Files:**
- Modify: `shared/src/llm_gateway_store/types.rs`
- Modify: `shared/src/llm_gateway_store/schema.rs`
- Modify: `shared/src/llm_gateway_store/codec.rs`
- Modify: `shared/src/llm_gateway_store/mod.rs`

- [ ] Add `LlmGatewayAccountGroupRecord` and `account_group_id` to `LlmGatewayKeyRecord`.
- [ ] Add the new table schema and key-table column migration.
- [ ] Add encode/decode support for account groups and key `account_group_id`.
- [ ] Add store CRUD for account groups.
- [ ] Add shared-store tests for round-trip and key/account-group persistence.

### Task 2: Write failing migration and routing tests

**Files:**
- Modify: `backend/src/llm_gateway.rs`
- Modify: `backend/src/kiro_gateway/mod.rs`
- Modify: `backend/src/kiro_gateway/provider.rs`

- [ ] Add failing tests for migrating legacy Codex key subsets into account groups.
- [ ] Add failing tests for migrating legacy Kiro key subsets into account groups.
- [ ] Add failing tests for Codex group-based auto/fixed routing.
- [ ] Add failing tests for Kiro group-based auto/fixed routing.

### Task 3: Implement backend migration and group APIs

**Files:**
- Modify: `backend/src/state.rs`
- Modify: `backend/src/llm_gateway.rs`
- Modify: `backend/src/llm_gateway/types.rs`
- Modify: `backend/src/kiro_gateway/mod.rs`
- Modify: `backend/src/kiro_gateway/types.rs`

- [ ] Add backend models and admin JSON payloads for account groups.
- [ ] Add list/create/patch/delete handlers for Codex account groups.
- [ ] Add list/create/patch/delete handlers for Kiro account groups.
- [ ] Add explicit startup migration that rewrites legacy key account selections into groups.
- [ ] Reject deleting groups that are still referenced by keys.

### Task 4: Implement group-based routing and key validation

**Files:**
- Modify: `backend/src/llm_gateway.rs`
- Modify: `backend/src/kiro_gateway/mod.rs`
- Modify: `backend/src/kiro_gateway/provider.rs`

- [ ] Replace direct key-local account subset routing with group resolution in Codex.
- [ ] Replace direct key-local account subset routing with group resolution in Kiro.
- [ ] Validate `account_group_id` for provider match and `fixed` single-account constraints.
- [ ] Remove new writes that directly manipulate legacy `fixed_account_name` / `auto_account_names`.

### Task 5: Update frontend API types and admin pages

**Files:**
- Modify: `frontend/src/api.rs`
- Modify: `frontend/src/pages/admin_llm_gateway.rs`
- Modify: `frontend/src/pages/admin_kiro_gateway.rs`

- [ ] Add frontend API types and fetch/mutate calls for account groups.
- [ ] Add Codex account-group management UI.
- [ ] Add Kiro account-group management UI.
- [ ] Replace key-card raw account selection with group selection.
- [ ] Preserve existing fixed/auto route strategy UX while driving it from groups.

### Task 6: Verify, format, and commit

**Files:**
- Modify: all changed files from tasks above

- [ ] Run targeted backend/shared/frontend tests for groups and migration.
- [ ] Run `cargo clippy -p static-flow-backend -p static-flow-shared -- -D warnings`.
- [ ] Run `cargo clippy -p static-flow-frontend --target wasm32-unknown-unknown -- -D warnings`.
- [ ] Run `rustfmt` on each changed Rust file only.
- [ ] Run `git diff --check`.
- [ ] Commit with one feature commit once the tree is verified.
