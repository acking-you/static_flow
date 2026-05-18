# LLM Access Usage Analysis Query Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade the existing admin/public usage-event query flow into a real analyzable pagination interface with rich filters, full-result token sums, and catalog-pruned archive access.

**Architecture:** Keep the existing `/admin/.../usage` routes stable, extend the backend query contract with additional filters and totals, compute count/page/sums from the worker-backed DuckDB analytics store, and update the frontend usage pages to expose denser filtering and correct full-result summaries. Keep catalog-first pruning as the archive access strategy and do not introduce free-form SQL.

**Tech Stack:** Rust, Axum, DuckDB, SQLite catalog, Yew/WASM frontend

---

## File Map

- Modify: `llm-access-core/src/store.rs`
  - extend `UsageEventQuery`
  - extend `UsageEventPage`
  - add `UsageEventTotals`
- Modify: `llm-access/src/usage_query.rs`
  - accept new query params
  - normalize filters
  - return totals in response
- Modify: `llm-access-store/src/duckdb.rs`
  - apply row filters
  - compute full filtered totals
  - keep catalog-first pruning
- Modify: `frontend/src/api.rs`
  - extend usage query request/response types
  - serialize new filter params
- Modify: `frontend/src/pages/admin_llm_gateway.rs`
  - add filter controls
  - render totals strip
  - keep correct pagination
- Modify: `frontend/src/pages/admin_kiro_gateway.rs`
  - align usage preview request shape if needed
- Modify: `frontend/src/pages/llm_access_usage.rs`
  - remove artificial page cap
  - parse totals if public response includes them
- Verify/update docs:
  - `docs/ops-runbook.md`
  - `docs/llm-access-tiered-usage-analytics.md`

## Task 1: Extend Core Query/Response Types

**Files:**
- Modify: `llm-access-core/src/store.rs`
- Test: `llm-access/src/usage_query.rs`

- [ ] **Step 1: Add new filter fields to `UsageEventQuery`**

Add:

```rust
pub model: Option<String>,
pub account_name: Option<String>,
pub endpoint: Option<String>,
pub status_code: Option<i32>,
```

- [ ] **Step 2: Add a totals struct**

Define:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UsageEventTotals {
    pub event_count: usize,
    pub input_uncached_tokens: u64,
    pub input_cached_tokens: u64,
    pub output_tokens: u64,
    pub billable_tokens: u64,
}
```

- [ ] **Step 3: Add totals to `UsageEventPage`**

Add:

```rust
pub totals: UsageEventTotals,
```

- [ ] **Step 4: Compile-fail check via existing consumers**

Run:

```bash
export CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow
cargo test -p llm-access usage_query --jobs 4
```

Expected: failures in callers that still construct `UsageEventPage` without `totals`.

- [ ] **Step 5: Commit**

```bash
git add llm-access-core/src/store.rs
git commit -m "feat: extend usage query core types"
```

## Task 2: Add Backend Query Params And Response Totals

**Files:**
- Modify: `llm-access/src/usage_query.rs`
- Test: `llm-access/src/usage_query.rs`

- [ ] **Step 1: Extend request/response JSON structs**

Add request fields:

```rust
model: Option<String>,
account_name: Option<String>,
endpoint: Option<String>,
status_code: Option<i32>,
```

Add response totals:

```rust
pub(crate) totals: AdminUsageTotalsView,
```

- [ ] **Step 2: Add a response totals view**

Define:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct AdminUsageTotalsView {
    pub(crate) event_count: usize,
    pub(crate) input_uncached_tokens: u64,
    pub(crate) input_cached_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) billable_tokens: u64,
}
```

- [ ] **Step 3: Normalize new filters**

Map request fields into `UsageEventQuery`, using the same exact-match trimming
rules already used for optional strings.

- [ ] **Step 4: Remove artificial offset clamp and keep bounded page size**

Keep:

```rust
const DEFAULT_ADMIN_USAGE_LIMIT: usize = 20;
const MAX_ADMIN_USAGE_LIMIT: usize = 200;
```

and keep:

```rust
offset: request.offset.unwrap_or(0),
```

- [ ] **Step 5: Add failing tests for new filter normalization**

Add tests that prove:

- large offsets survive normalization;
- `model`, `account_name`, and `endpoint` are trimmed and preserved;
- response JSON requires/deserializes `totals`.

- [ ] **Step 6: Run tests**

Run:

```bash
export CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow
cargo test -p llm-access usage_query --jobs 4
```

Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add llm-access/src/usage_query.rs
git commit -m "feat: extend usage query api filters and totals"
```

## Task 3: Implement DuckDB Row Filters And Full Totals

**Files:**
- Modify: `llm-access-store/src/duckdb.rs`
- Test: `llm-access-store/src/duckdb.rs`

- [ ] **Step 1: Add row-filter predicates to count/list SQL**

Extend the count/list SQL to honor:

- `model`
- `account_name`
- `endpoint`
- `status_code`

with exact-match optional predicates.

- [ ] **Step 2: Add totals SQL**

Introduce one totals query per connection:

```sql
SELECT
  count(*) AS event_count,
  COALESCE(sum(input_uncached_tokens), 0) AS input_uncached_tokens,
  COALESCE(sum(input_cached_tokens), 0) AS input_cached_tokens,
  COALESCE(sum(output_tokens), 0) AS output_tokens,
  COALESCE(sum(billable_tokens), 0) AS billable_tokens
FROM usage_events
WHERE ...
```

- [ ] **Step 3: Return full totals from single-file queries**

Update `list_usage_events_from_conn(...)` so it returns:

- total row count
- page rows
- totals across the whole filtered result

- [ ] **Step 4: Return full totals from tiered queries**

For tiered mode:

- sum active totals;
- sum each candidate archived segment totals;
- keep page row fetching separate from totals accumulation;
- keep catalog-first pruning intact.

- [ ] **Step 5: Preserve conservative catalog pruning**

Do not use row-level filters for catalog exclusion unless catalog already knows
that dimension. Use catalog only for time/key/provider pruning.

- [ ] **Step 6: Add failing tests**

Add tests proving:

- offsets beyond 200 still page correctly;
- totals are computed from all filtered rows, not just the current page;
- tiered queries with archived segments return correct totals;
- model/account/status filters work.

- [ ] **Step 7: Run targeted tests**

Run:

```bash
export CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow
cargo test -p llm-access-store list_usage_events --jobs 4
```

Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add llm-access-store/src/duckdb.rs
git commit -m "feat: add filtered usage totals over duckdb analytics"
```

## Task 4: Extend Frontend API Contracts

**Files:**
- Modify: `frontend/src/api.rs`
- Test: `frontend/src/api.rs`

- [ ] **Step 1: Extend admin usage query struct**

Add:

```rust
pub model: Option<String>,
pub account_name: Option<String>,
pub endpoint: Option<String>,
pub status_code: Option<i32>,
```

- [ ] **Step 2: Extend usage response struct**

Add:

```rust
pub totals: AdminUsageTotalsView,
```

- [ ] **Step 3: Serialize new query params in fetchers**

Update:

- `fetch_admin_llm_gateway_usage_events`
- `fetch_admin_kiro_usage_events`

to include the new optional params.

- [ ] **Step 4: Add serde/url tests**

Add tests that prove:

- new fields serialize into URLs correctly;
- usage response deserializes totals.

- [ ] **Step 5: Run tests**

Run:

```bash
export CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow
cargo test -p static-flow-frontend api::tests::admin_usage_events_response_deserializes_retention_days --jobs 4
```

Expected: update that test or add adjacent tests and keep them PASS.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/api.rs
git commit -m "feat: extend frontend usage query contracts"
```

## Task 5: Upgrade Admin Usage Filters And Totals UI

**Files:**
- Modify: `frontend/src/pages/admin_llm_gateway.rs`
- Test: `frontend/src/pages/admin_llm_gateway.rs`

- [ ] **Step 1: Add filter state**

Add local state for:

- `usage_model_filter`
- `usage_account_filter`
- `usage_endpoint_filter`
- `usage_status_filter`

- [ ] **Step 2: Wire filters into reload request**

When building `AdminLlmGatewayUsageEventsQuery`, pass the new filters.

- [ ] **Step 3: Render filter controls**

Add a denser filter row above the table with:

- existing key filter
- existing time range
- existing source selector if missing from visible UI
- model input/select
- account input/select
- endpoint select
- status input/select

- [ ] **Step 4: Render totals summary strip**

Show:

- matching event count
- uncached input tokens
- cached input tokens
- output tokens
- billable tokens

using the backend-provided totals.

- [ ] **Step 5: Remove admin usage page cap**

Keep:

```rust
let usage_total_pages = (*usage_total).max(1).div_ceil(USAGE_PAGE_SIZE);
```

and do not reintroduce any hard max page count.

- [ ] **Step 6: Add page-behavior tests**

Add tests proving:

- total pages are unbounded by the old 11-page cap;
- filter changes reset to page 1;
- summary values read from backend totals rather than current page rows.

- [ ] **Step 7: Run frontend tests**

Run:

```bash
export CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow
cargo test -p static-flow-frontend admin_llm_gateway --jobs 4
```

Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add frontend/src/pages/admin_llm_gateway.rs
git commit -m "feat: add richer admin usage filters and totals"
```

## Task 6: Align Public Usage Page Pagination

**Files:**
- Modify: `frontend/src/pages/llm_access_usage.rs`
- Test: `frontend/src/pages/llm_access_usage.rs`

- [ ] **Step 1: Remove the public page-count cap**

Keep:

```rust
let total_pages = total.div_ceil(limit);
```

- [ ] **Step 2: Keep behavior compatible**

Do not add the full admin filter surface to the public page. This task only
aligns pagination semantics and response parsing.

- [ ] **Step 3: Add/update tests**

Add a test proving public usage pagination is no longer capped by a fixed page
maximum.

- [ ] **Step 4: Run frontend tests**

Run:

```bash
export CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow
cargo test -p static-flow-frontend llm_access_usage --jobs 4
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add frontend/src/pages/llm_access_usage.rs
git commit -m "fix: align public usage pagination with backend totals"
```

## Task 7: Verification And Docs

**Files:**
- Modify: `docs/ops-runbook.md`
- Modify: `docs/llm-access-tiered-usage-analytics.md`

- [ ] **Step 1: Update docs**

Document:

- available admin usage filters;
- full-result totals behavior;
- catalog-first pruning and row-level filtering split;
- absence of artificial offset cap.

- [ ] **Step 2: Run final verification**

Run:

```bash
export CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow
rustfmt llm-access-core/src/store.rs llm-access/src/usage_query.rs llm-access-store/src/duckdb.rs frontend/src/api.rs frontend/src/pages/admin_llm_gateway.rs frontend/src/pages/admin_kiro_gateway.rs frontend/src/pages/llm_access_usage.rs
cargo test -p llm-access -p llm-access-store -p static-flow-frontend --jobs 4
cargo clippy -p llm-access -p llm-access-store -p static-flow-frontend --jobs 4 -- -D warnings
git diff --check
```

Expected: all pass.

- [ ] **Step 3: Final commit**

```bash
git add docs/ops-runbook.md docs/llm-access-tiered-usage-analytics.md
git commit -m "feat: add analyzable usage query filtering and totals"
```
