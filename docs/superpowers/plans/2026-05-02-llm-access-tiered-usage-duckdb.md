# llm-access Tiered Usage DuckDB Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move mutable `llm-access` usage writes to a local active DuckDB file while asynchronously archiving immutable DuckDB segments to JuiceFS/R2.

**Architecture:** Keep the existing single-file repository as the compatibility path, then add a tiered repository mode behind additive CLI flags. Tiered mode serializes active DuckDB writes with a mutex, rolls over the active file by size, and hands closed pending files to a background sealer that writes a SQLite catalog plus immutable archive files.

**Tech Stack:** Rust, DuckDB `duckdb` crate, SQLite `rusqlite`, Tokio async traits, Yew frontend admin usage page.

---

### Task 1: Store-Level Tiered Repository

**Files:**
- Modify: `llm-access-store/src/duckdb.rs`
- Test: `llm-access-store/src/duckdb.rs`

- [ ] **Step 1: Write failing tiered rollover tests**

Add tests that create a tiered repository with a tiny rollover threshold, append two events, verify the second append keeps active writes working, wait for archive publication, then assert the first event can be loaded by `event_id` from the archive and both events appear in list results.

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow \
cargo test -p llm-access-store duckdb_tiered_ --features duckdb-runtime --jobs 1
```

Expected: tests fail because tiered repository types do not exist.

- [ ] **Step 3: Implement tiered config and catalog**

Add `TieredDuckDbUsageConfig`, `UsageEventSource`, segment metadata structs, a SQLite catalog under `catalog_dir/usage-segments.sqlite3`, and catalog helpers for segment rows, event locators, and per-segment count lookup.

- [ ] **Step 4: Implement active writer and async sealer**

Add `DuckDbUsageRepository::open_tiered(config)`. In tiered mode, append batches to a local active DuckDB file. When the file reaches `rollover_bytes`, checkpoint and close it, move it into local pending state, immediately create the next active file, and spawn a background sealer that copies the closed segment to `archive_dir`, indexes it, then deletes the pending file.

- [ ] **Step 5: Implement tiered query methods**

Make list/detail/chart/rollup queries combine active data with archived segments. Lists stay capped and newest-first. Detail lookup checks active first, then uses the catalog `event_id` locator to open exactly one archived segment read-only.

- [ ] **Step 6: Run store tests**

Run the command from Step 2 and the existing DuckDB repository tests. Expected: all targeted store tests pass.

### Task 2: Runtime Configuration and Admin API

**Files:**
- Modify: `llm-access/src/config.rs`
- Modify: `llm-access/src/runtime.rs`
- Modify: `llm-access/src/admin.rs`
- Test: `llm-access/src/config.rs`, `llm-access/src/lib.rs`

- [ ] **Step 1: Write failing config/API tests**

Add CLI parsing tests for `--duckdb-active-dir`, `--duckdb-archive-dir`, `--duckdb-catalog-dir`, and `--duckdb-rollover-bytes`. Add an admin router test that `source=hot` reaches the usage list endpoint and preserves backward compatibility when source is omitted.

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow \
cargo test -p llm-access parses_serve_config_with_tiered_duckdb_paths router_handles_llm_gateway_usage_source --features duckdb-runtime --jobs 1
```

Expected: tests fail because the CLI flags and source query do not exist.

- [ ] **Step 3: Implement config and runtime selection**

Extend `StorageConfig` with optional tiered DuckDB config. If tiered flags are present, runtime opens `DuckDbUsageRepository::open_tiered`; otherwise it keeps `open_path`.

- [ ] **Step 4: Implement admin source query**

Add `source` to `ListUsageEventsRequest` and `UsageEventQuery`. Accepted values are `hot`, `archive`, and `all`; omitted source defaults to `hot` in tiered mode and remains equivalent to the existing single-file behavior in legacy mode.

- [ ] **Step 5: Run runtime tests**

Run the command from Step 2 and the existing `llm-access` router tests. Expected: targeted runtime tests pass.

### Task 3: Frontend Usage Source Controls

**Files:**
- Modify: `frontend/src/api.rs`
- Modify: `frontend/src/pages/admin_llm_gateway.rs`

- [ ] **Step 1: Add query field and source control**

Add `source: Option<String>` to `AdminLlmGatewayUsageEventsQuery`. Append it to `/admin/llm-gateway/usage` query params. Add a compact select in the Usage tab for `hot`, `archive`, and `all`, defaulting to `hot`.

- [ ] **Step 2: Preserve existing paging behavior**

Changing source resets page to 1 and reuses existing key/time filters. The table and detail modal keep the same response shape.

- [ ] **Step 3: Run frontend check**

Run:

```bash
CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow \
CARGO_BUILD_JOBS=1 cargo check -p static-flow-frontend --target wasm32-unknown-unknown --jobs 1
```

Expected: frontend crate checks successfully.

### Task 4: Verification and Handoff

**Files:**
- Modify: changed Rust files only

- [ ] **Step 1: Format changed Rust files only**

Run `rustfmt` on changed Rust files. Do not run workspace `cargo fmt`.

- [ ] **Step 2: Run targeted tests and clippy**

Run store/runtime tests, frontend check, and clippy for affected crates:

```bash
CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow \
cargo clippy -p llm-access-store -p llm-access --features duckdb-runtime --jobs 1 -- -D warnings
```

- [ ] **Step 3: Commit**

Commit implementation separately from the design/plan commits with a feature commit message.
