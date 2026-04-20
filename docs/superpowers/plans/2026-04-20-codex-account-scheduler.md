# Codex Account Scheduler Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Codex account-level local concurrency and request-start interval scheduling so the gateway can rotate away from hot accounts before applying existing key-level limits.

**Architecture:** Persist scheduler settings in Codex account metadata, add a dedicated account-scoped scheduler in the Codex runtime, and move request-time account selection onto a loop that can skip locally throttled accounts. Keep the existing key-level scheduler as a second-stage guard.

**Tech Stack:** Rust (`axum`, `tokio`, `parking_lot`), Yew frontend admin page, Lance/shared API models, existing StaticFlow LLM gateway runtime.

---

## File Structure

- Modify: `backend/src/llm_gateway/accounts.rs`
  Responsibility: Codex account metadata model, account summary snapshot, account selection helpers.
- Modify: `backend/src/llm_gateway/runtime.rs`
  Responsibility: Codex runtime state plus new account-scoped scheduler.
- Modify: `backend/src/llm_gateway.rs`
  Responsibility: admin handlers, request routing flow, account patch parsing, throttling responses.
- Modify: `backend/src/llm_gateway/types.rs`
  Responsibility: admin API request/response models for Codex account settings.
- Modify: `frontend/src/api.rs`
  Responsibility: frontend API models and patch payload for Codex account settings.
- Modify: `frontend/src/pages/admin_llm_gateway.rs`
  Responsibility: Codex account settings UI.

## Task 1: Persist Codex Account Scheduler Settings

**Files:**
- Modify: `backend/src/llm_gateway/accounts.rs`
- Modify: `backend/src/llm_gateway/types.rs`
- Modify: `frontend/src/api.rs`

- [ ] Add optional scheduler fields to the Codex account settings metadata model and account summary projection.
- [ ] Extend the account settings patch request/response models to carry these fields and explicit unlimited toggles.
- [ ] Add or update backend unit tests covering account settings serialization / round-trip.

## Task 2: Add Codex Account Scheduler

**Files:**
- Modify: `backend/src/llm_gateway/runtime.rs`

- [ ] Add a new account-scoped scheduler type modeled after Kiro’s scheduler, with `try_acquire`, `wait_for_available`, and RAII lease release behavior.
- [ ] Store the scheduler in `LlmGatewayRuntimeState`.
- [ ] Add focused unit tests for per-account concurrency isolation and start-interval enforcement.

## Task 3: Route Codex Requests Through Account Scheduler

**Files:**
- Modify: `backend/src/llm_gateway.rs`
- Modify: `backend/src/llm_gateway/accounts.rs`

- [ ] Refactor Codex account selection so the request path can iterate candidate accounts instead of resolving only one account eagerly.
- [ ] When an eligible account is locally throttled, skip it and continue to the next candidate.
- [ ] When all eligible accounts are locally throttled, wait on the shortest local delay / release notification and retry.
- [ ] Carry the account scheduler lease through the full upstream request lifetime.
- [ ] Keep legacy unmanaged auth fallback behavior unchanged.
- [ ] Add or update backend tests for account rotation / blocked-account behavior.

## Task 4: Wire Admin Endpoint and UI

**Files:**
- Modify: `backend/src/llm_gateway.rs`
- Modify: `frontend/src/api.rs`
- Modify: `frontend/src/pages/admin_llm_gateway.rs`

- [ ] Update the Codex account patch handler to validate and persist the new fields.
- [ ] Show current account scheduler settings in the admin page.
- [ ] Allow editing/saving/clearing the values using the existing account row controls.

## Task 5: Verify and Clean Up

**Files:**
- Modify: `backend/src/llm_gateway.rs`
- Modify: `backend/src/llm_gateway/runtime.rs`
- Modify: `backend/src/llm_gateway/accounts.rs`
- Modify: `backend/src/llm_gateway/types.rs`
- Modify: `frontend/src/api.rs`
- Modify: `frontend/src/pages/admin_llm_gateway.rs`

- [ ] Run targeted Rust tests for the touched gateway/account modules.
- [ ] Run `rustfmt` only on changed Rust files.
- [ ] Run `cargo clippy` for affected crates and fix warnings to zero.
- [ ] Sanity-check the frontend build or compile path affected by the API/UI model changes.

## Self-Review

- Spec coverage: covered persisted settings, runtime scheduler, routing flow, admin API, and UI.
- Placeholder scan: no `TODO`/`TBD` markers or implied steps remain.
- Type consistency: backend and frontend must use the same field names for account scheduler settings and unlimited toggles.
