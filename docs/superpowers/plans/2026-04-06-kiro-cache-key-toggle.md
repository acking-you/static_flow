# Kiro Per-Key Cache Toggle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a per-key Kiro toggle that controls whether conservative cache estimation is exposed in protocol usage fields and persisted usage events.

**Architecture:** Extend the persisted Kiro key record with one boolean flag defaulting to `true`, thread it through admin DTOs and the Kiro request pipeline, and gate the existing conservative estimator at the final usage-summary boundary. Keep the behavior purely key-driven so different keys can opt in/out independently without touching global runtime config.

**Tech Stack:** Rust (Axum, Serde, LanceDB schema migration), Yew/WASM frontend, targeted cargo test/check/clippy.

---

## File Map

- `shared/src/llm_gateway_store/types.rs`
  - Add persisted key field defaulting to `true`
- `shared/src/llm_gateway_store/schema.rs`
  - Add LanceDB bool column migration
- `shared/src/llm_gateway_store/codec.rs`
  - Encode/decode the new bool field
- `shared/src/llm_gateway_store/mod.rs`
  - Adjust key test fixtures/round trips
- `backend/src/kiro_gateway/types.rs`
  - Extend admin key view + patch request
- `backend/src/kiro_gateway/mod.rs`
  - Default new keys to `true`, patch existing keys, update comments/tests
- `backend/src/kiro_gateway/anthropic/mod.rs`
  - Gate conservative cache estimation by key toggle
- `frontend/src/api.rs`
  - Expose toggle in Kiro admin DTOs and patch payload
- `frontend/src/pages/admin_kiro_gateway.rs`
  - Add card-level checkbox and submit it with key patches

