# LLM Usage Event Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop `llm_gateway_usage_events` from storing full request payloads on successful requests, prevent persistent write failures from turning into request-rate disk churn, tighten default detail retention, and provide a reproducible CLI rebuild path for the existing bloated table.

**Architecture:** Keep the existing `llm_gateway_usage_events` schema and admin APIs, but change write policy: successful events persist only summary fields, failed events keep full diagnostics, and the flusher enters a timer-gated retry mode after one failed append so retries no longer happen on each new request. Add a dedicated CLI maintenance command to redact old success payloads, trim expired failure details, then rebuild the table into a compact stable copy.

**Tech Stack:** Rust, tokio, Axum, LanceDB/Lance, clap

---

### Task 1: Lock Down Success vs Failure Payload Semantics

**Files:**
- Modify: `backend/src/kiro_gateway/mod.rs`
- Modify: `backend/src/llm_gateway.rs`
- Test: `backend/src/kiro_gateway/mod.rs`
- Test: `backend/src/llm_gateway.rs`

- [ ] **Step 1: Write failing tests for Kiro success/failure capture**

Add tests that prove successful Kiro events no longer persist raw request headers/bodies/full request, while failed events still do.

- [ ] **Step 2: Run the Kiro tests and verify they fail**

Run:

```bash
cargo test -p static-flow-backend build_kiro_success_usage_event_skips_full_request_payloads -- --exact
cargo test -p static-flow-backend build_kiro_failure_usage_event_keeps_full_request_payloads -- --exact
```

- [ ] **Step 3: Write failing tests for Codex success/failure capture**

Add one success test and one failure test around `build_gateway_usage_event_record`.

- [ ] **Step 4: Run the Codex tests and verify they fail**

Run:

```bash
cargo test -p static-flow-backend build_gateway_success_usage_event_skips_full_request_payloads -- --exact
cargo test -p static-flow-backend build_gateway_failure_usage_event_keeps_full_request_payloads -- --exact
```

- [ ] **Step 5: Implement the minimal production changes**

Rules:
- Success events must persist `request_headers_json = "{}"`.
- Success events must persist `client_request_body_json = None`.
- Success events must persist `upstream_request_body_json = None`.
- Success events must persist `full_request_json = None`.
- Failed events must still persist the full request/diagnostic payloads.

- [ ] **Step 6: Re-run the focused backend tests until green**

Run the exact tests from steps 2 and 4.

### Task 2: Make Flush Failure Retry Rate Independent From Request Rate

**Files:**
- Modify: `backend/src/llm_gateway/runtime.rs`
- Test: `backend/src/llm_gateway/runtime.rs`

- [ ] **Step 1: Write a failing flusher test**

Add a runtime test proving that after one append failure, incoming requests do not trigger immediate repeated store writes before the timer retry window opens.

- [ ] **Step 2: Run the runtime test and verify it fails**

Run:

```bash
cargo test -p static-flow-backend usage_event_flush_failure_waits_for_timer_before_retry -- --exact
```

- [ ] **Step 3: Implement the minimal flusher state machine**

Implement a small explicit retry state in `spawn_usage_event_flusher`:
- Normal mode consumes `rx` and flushes on batch/size/timer.
- After a failed append, preserve the failed batch and stop consuming new events.
- While failed batch is pending, only the timer and shutdown path may retry.
- A successful retry clears the failed state and resumes normal consumption.

- [ ] **Step 4: Re-run the focused runtime tests**

Run:

```bash
cargo test -p static-flow-backend usage_event_flush_failure_waits_for_timer_before_retry -- --exact
cargo test -p static-flow-backend usage_events_flush_when_buffer_bytes_reach_limit -- --exact
cargo test -p static-flow-backend usage_events_flush_buffer_on_shutdown_and_update_rollup_immediately -- --exact
```

### Task 3: Tighten Default Detail Retention And Add Table Normalization API

**Files:**
- Modify: `shared/src/llm_gateway_store/types.rs`
- Modify: `shared/src/llm_gateway_store/mod.rs`
- Modify: `backend/src/llm_gateway/runtime.rs`
- Test: `shared/src/llm_gateway_store/mod.rs`
- Test: `backend/src/llm_gateway/runtime.rs`

- [ ] **Step 1: Write failing tests for normalization and retention**

Add tests that prove:
- success rows can be redacted in bulk
- old failure rows can be redacted by cutoff
- recent failure rows keep diagnostics
- default runtime retention is no longer `-1`

- [ ] **Step 2: Run the focused tests and verify they fail**

Run:

```bash
cargo test -p static-flow-shared redact_usage_event_details_for_success_rows -- --exact
cargo test -p static-flow-shared redact_usage_event_details_before_cutoff_keeps_recent_failures -- --exact
cargo test -p static-flow-backend usage_event_detail_cutoff_ms_skips_non_positive_retention -- --exact
```

- [ ] **Step 3: Implement store-side normalization helpers and new default**

Implement a dedicated helper in `LlmGatewayStore` that:
- redacts detail fields for all success rows (`status_code < 400`)
- optionally redacts failure rows older than a caller-provided cutoff

Set the default `usage_event_detail_retention_days` to a finite value suitable for diagnostics retention.

- [ ] **Step 4: Re-run the focused tests**

Run the exact commands from step 2.

### Task 4: Add A Reproducible CLI Rebuild Flow For `llm_gateway_usage_events`

**Files:**
- Modify: `cli/src/cli.rs`
- Modify: `cli/src/commands/mod.rs`
- Modify: `cli/src/commands/db_manage.rs`
- Test: `cli/src/commands/db_manage.rs`

- [ ] **Step 1: Write a failing CLI/db-manage test**

Add a test covering the new command flow on a temp DB: redact success rows, trim old failure details, then rebuild the table.

- [ ] **Step 2: Run the focused CLI test and verify it fails**

Run:

```bash
cargo test -p sf-cli rebuild_llm_gateway_usage_events_redacts_success_payloads -- --exact
```

- [ ] **Step 3: Implement the CLI command**

Add a dedicated DB command that:
- loads runtime config from the content DB
- redacts success detail payloads
- trims expired failure payloads using the configured retention cutoff
- rebuilds `llm_gateway_usage_events` with `force=true`
- optimizes/prunes the rebuilt table

- [ ] **Step 4: Re-run the focused CLI test**

Run the exact command from step 2.

### Task 5: Verify, Build, And Run The Real Table Rebuild

**Files:**
- Modify: `docs/superpowers/plans/2026-04-15-llm-usage-event-hardening.md`

- [ ] **Step 1: Format only the changed files**

Run:

```bash
rustfmt backend/src/kiro_gateway/mod.rs \
        backend/src/llm_gateway.rs \
        backend/src/llm_gateway/runtime.rs \
        shared/src/llm_gateway_store/types.rs \
        shared/src/llm_gateway_store/mod.rs \
        cli/src/cli.rs \
        cli/src/commands/mod.rs \
        cli/src/commands/db_manage.rs
```

- [ ] **Step 2: Run targeted crate tests**

Run:

```bash
cargo test -p static-flow-shared llm_gateway_usage
cargo test -p static-flow-backend usage_event
cargo test -p sf-cli llm_gateway_usage
```

- [ ] **Step 3: Run clippy on affected crates and fix everything**

Run:

```bash
cargo clippy -p static-flow-shared -p static-flow-backend -p sf-cli --tests -- -D warnings
```

- [ ] **Step 4: Build the CLI binary used for live repair**

Run:

```bash
cargo build --profile release-backend -p sf-cli
```

- [ ] **Step 5: Execute the real rebuild against production content DB**

Run:

```bash
./target/release-backend/sf-cli db rebuild-llm-gateway-usage-events \
  --db-path /mnt/wsl/data4tb/static-flow-data/lancedb
```

- [ ] **Step 6: Verify resulting size/health**

Run:

```bash
du -sh /mnt/wsl/data4tb/static-flow-data/lancedb/llm_gateway_usage_events.lance
./target/release-backend/sf-cli db describe-table --db-path /mnt/wsl/data4tb/static-flow-data/lancedb llm_gateway_usage_events
```
