# LLM Access Full Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring standalone `llm-access` Codex and Kiro API behavior to parity with the existing StaticFlow backend before any cutover or broad testing.

**Architecture:** Keep the existing request/response conversion crates (`llm-access-codex`, `llm-access-kiro`) as the source of truth for protocol transforms, and move the missing runtime orchestration into focused `llm-access` modules. The runtime must preserve old routing, per-key and per-account limiting, upstream headers, retry/failover, refresh, MCP/web_search, admin routes, and usage diagnostics.

**Tech Stack:** Rust, Axum, Reqwest, SQLite control store, DuckDB usage sink, existing `llm-access-*` crates.

---

### Task 1: Codex Runtime Parity

**Files:**
- Modify: `llm-access-core/src/store.rs`
- Modify: `llm-access-store/src/sqlite.rs`
- Modify: `llm-access-store/src/repository.rs`
- Create: `llm-access/src/codex_runtime.rs`
- Modify: `llm-access/src/provider.rs`
- Modify: `llm-access/src/lib.rs`

- [ ] Add route-candidate store APIs that return the full Codex candidate set for fixed, auto, account-group, and explicit auto-account routes.
- [ ] Add non-blocking key/account scheduler APIs with rejection metadata and wait durations.
- [ ] Rebuild Codex upstream headers to match the old backend, including session, turn-state, account-id, trace, beta, and FedRAMP headers.
- [ ] Implement transport and account-bound non-success failover over remaining candidates.
- [ ] Persist Codex failure usage events for the old failure stages.
- [ ] Add focused unit tests for route candidate ordering, local throttle skipping, upstream header preservation, and failover classification.

### Task 2: Codex Refresh Parity

**Files:**
- Modify: `llm-access-core/src/store.rs`
- Modify: `llm-access-store/src/sqlite.rs`
- Modify: `llm-access-store/src/repository.rs`
- Create: `llm-access/src/codex_refresh.rs`
- Modify: `llm-access/src/runtime.rs`
- Modify: `llm-access/src/admin.rs`

- [ ] Parse Codex auth JSON into access, refresh, id token, account id, FedRAMP, and last-refresh fields.
- [ ] Add single-flight per-account refresh locks.
- [ ] Implement manual refresh and 401 forced refresh.
- [ ] Persist refreshed auth JSON and account health fields back to SQLite.
- [ ] Replace timestamp-only admin refresh with real refresh.

### Task 3: Kiro Runtime Parity

**Files:**
- Modify: `llm-access-core/src/store.rs`
- Modify: `llm-access-store/src/sqlite.rs`
- Modify: `llm-access-store/src/repository.rs`
- Create: `llm-access/src/kiro_runtime.rs`
- Modify: `llm-access/src/provider.rs`
- Modify: `llm-access/src/lib.rs`

- [ ] Add Kiro route-candidate store APIs for fixed, auto, account-group, and explicit account lists.
- [ ] Recreate account selection using status, quota exhausted state, minimum remaining threshold, local throttle, upstream cooldown, proxy cooldown, and fairness.
- [ ] Implement per-account attempt loop with three attempts, refresh-on-401/403, 402 quota failover, 429 cooldown, transient invalid-model cooldown, and 408/5xx retry.
- [ ] Rebuild Kiro upstream headers from the old provider, including machine-id-derived user agents, host, connection close, and profile ARN.
- [ ] Persist routing diagnostics and failed request events.

### Task 4: Kiro Refresh And MCP Parity

**Files:**
- Create: `llm-access/src/kiro_refresh.rs`
- Modify: `llm-access/src/kiro_runtime.rs`
- Modify: `llm-access/src/provider.rs`
- Modify: `llm-access/src/kiro.rs`

- [ ] Parse social and IdC Kiro auth JSON variants.
- [ ] Implement single-flight Kiro refresh against social refresh and IdC OIDC token endpoints.
- [ ] Implement `/mcp` upstream calls with the same routing/failover logic as messages.
- [ ] Restore pure `web_search` MCP shim behavior for stream and non-stream Anthropic responses.

### Task 5: Kiro Admin Compatibility

**Files:**
- Modify: `llm-access-core/src/store.rs`
- Modify: `llm-access-store/src/sqlite.rs`
- Modify: `llm-access-store/src/repository.rs`
- Modify: `llm-access/src/admin.rs`
- Modify: `llm-access/src/lib.rs`

- [ ] Register every `/admin/kiro-gateway/*` route declared in `llm-access-core/src/routes.rs`.
- [ ] Map Kiro admin key, group, usage, account, import-local, status, and balance APIs to the unified SQLite store while preserving old response shapes.
- [ ] Validate route settings against existing Kiro accounts and account groups.
- [ ] Keep `/admin/llm-gateway/*` Codex behavior backward-compatible.

### Task 6: Verification

**Files:**
- Modify: affected tests near the changed modules only.

- [ ] Run `rustfmt` only on changed Rust files.
- [ ] Run focused unit tests for `llm-access`, `llm-access-core`, and `llm-access-store`.
- [ ] Run `cargo clippy --jobs 1` for affected crates with warnings denied.
- [ ] Do not start, stop, or restart production backend, gateway, Caddy, or pbmapper.
