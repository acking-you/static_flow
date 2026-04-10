# LLM Usage Retention And Query Slimming Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop admin usage pages from loading heavy request-body data on the list path, add periodic usage-detail retention plus index optimization, and preserve historical usage rollups.

**Architecture:** Keep `llm_gateway_usage_events` as the single source table, but split read projections into summary and detail. Add runtime-config-backed usage maintenance in `LlmGatewayRuntimeState` to clear old detail columns and run `OptimizeAction::Index` on the usage table.

**Tech Stack:** Rust, Axum, Yew, LanceDB/Lance, serde, tokio

---

### Task 1: Add Summary/Detail Usage Store Projections

**Files:**
- Modify: `shared/src/llm_gateway_store/types.rs`
- Modify: `shared/src/llm_gateway_store/schema.rs`
- Modify: `shared/src/llm_gateway_store/codec.rs`
- Modify: `shared/src/llm_gateway_store/mod.rs`
- Test: `shared/src/llm_gateway_store/mod.rs`

- [ ] **Step 1: Write the failing store tests**

Add tests covering summary projection and detail lookup in [mod.rs](/home/ts_user/rust_pro/static_flow/shared/src/llm_gateway_store/mod.rs).

```rust
#[tokio::test]
async fn query_usage_event_summaries_excludes_detail_payloads() {
    let store = temp_store().await;
    let event = sample_usage_event();
    store.append_usage_event(&event).await.unwrap();

    let rows = store
        .query_usage_event_summaries(Some(&event.key_id), None, Some(10), Some(0))
        .await
        .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, event.id);
    assert_eq!(rows[0].billable_tokens, event.billable_tokens);
}

#[tokio::test]
async fn get_usage_event_detail_by_id_returns_heavy_fields() {
    let store = temp_store().await;
    let event = sample_usage_event();
    store.append_usage_event(&event).await.unwrap();

    let detail = store
        .get_usage_event_detail_by_id(&event.id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(detail.full_request_json, event.full_request_json);
    assert_eq!(detail.client_request_body_json, event.client_request_body_json);
}
```

- [ ] **Step 2: Run shared tests to verify they fail**

Run:

```bash
cargo test -p static-flow-shared query_usage_event_summaries_excludes_detail_payloads -- --exact
```

Expected: FAIL because summary/detail store APIs do not exist yet.

- [ ] **Step 3: Add summary/detail store types and projections**

Introduce dedicated summary/detail view structs in [types.rs](/home/ts_user/rust_pro/static_flow/shared/src/llm_gateway_store/types.rs) and separate column lists in [schema.rs](/home/ts_user/rust_pro/static_flow/shared/src/llm_gateway_store/schema.rs).

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmGatewayUsageEventSummaryRecord {
    pub id: String,
    pub key_id: String,
    pub key_name: String,
    pub provider_type: Option<String>,
    pub account_name: Option<String>,
    pub request_method: String,
    pub request_url: String,
    pub latency_ms: i32,
    pub endpoint: String,
    pub model: Option<String>,
    pub status_code: i32,
    pub input_uncached_tokens: u64,
    pub input_cached_tokens: u64,
    pub output_tokens: u64,
    pub billable_tokens: u64,
    pub usage_missing: bool,
    pub credit_usage: Option<f64>,
    pub credit_usage_missing: bool,
    pub client_ip: String,
    pub ip_region: String,
    pub created_at: i64,
}

pub fn usage_event_summary_columns() -> Vec<String> { /* summary columns only */ }
pub fn usage_event_detail_columns() -> Vec<String> { /* summary + detail */ }
```

- [ ] **Step 4: Implement summary/detail query helpers**

Replace list-path full-row reads with dedicated helpers in [mod.rs](/home/ts_user/rust_pro/static_flow/shared/src/llm_gateway_store/mod.rs).

```rust
pub async fn query_usage_event_summaries(
    &self,
    key_id: Option<&str>,
    provider_type: Option<&str>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<Vec<LlmGatewayUsageEventSummaryRecord>> { /* ... */ }

pub async fn get_usage_event_detail_by_id(
    &self,
    event_id: &str,
) -> Result<Option<LlmGatewayUsageEventRecord>> { /* ... */ }
```

- [ ] **Step 5: Run shared tests to verify they pass**

Run:

```bash
cargo test -p static-flow-shared query_usage_event_summaries_excludes_detail_payloads -- --exact
cargo test -p static-flow-shared get_usage_event_detail_by_id_returns_heavy_fields -- --exact
```

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add shared/src/llm_gateway_store/types.rs \
        shared/src/llm_gateway_store/schema.rs \
        shared/src/llm_gateway_store/codec.rs \
        shared/src/llm_gateway_store/mod.rs
git commit -m "feat: split llm usage summary and detail store queries"
```

### Task 2: Add Usage Maintenance Settings And Store Maintenance Operations

**Files:**
- Modify: `backend/src/state.rs`
- Modify: `backend/src/llm_gateway.rs`
- Modify: `backend/src/llm_gateway/types.rs`
- Modify: `shared/src/llm_gateway_store/types.rs`
- Modify: `shared/src/llm_gateway_store/schema.rs`
- Modify: `shared/src/llm_gateway_store/codec.rs`
- Modify: `shared/src/llm_gateway_store/mod.rs`
- Test: `backend/src/llm_gateway.rs`
- Test: `shared/src/llm_gateway_store/mod.rs`

- [ ] **Step 1: Write failing tests for config validation and detail trimming**

Add one handler validation test and one store maintenance test.

```rust
#[test]
fn update_runtime_config_rejects_invalid_usage_detail_retention_days() {
    let request = UpdateLlmGatewayRuntimeConfigRequest {
        usage_event_detail_retention_days: Some(0),
        ..Default::default()
    };
    let err = apply_runtime_update_for_test(request).unwrap_err();
    assert!(err.0 == StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn clear_old_usage_event_details_preserves_summary_fields() {
    let store = temp_store().await;
    let event = sample_usage_event();
    store.append_usage_event(&event).await.unwrap();

    store
        .clear_usage_event_details_before(event.created_at + 1)
        .await
        .unwrap();

    let detail = store.get_usage_event_detail_by_id(&event.id).await.unwrap().unwrap();
    assert_eq!(detail.full_request_json, None);
    assert_eq!(detail.billable_tokens, event.billable_tokens);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p static-flow-backend update_runtime_config_rejects_invalid_usage_detail_retention_days -- --exact
cargo test -p static-flow-shared clear_old_usage_event_details_preserves_summary_fields -- --exact
```

Expected: FAIL because fields and maintenance helpers do not exist.

- [ ] **Step 3: Extend persisted runtime-config records and admin DTOs**

Add the new maintenance fields everywhere runtime config is represented.

```rust
pub struct LlmGatewayRuntimeConfig {
    pub usage_event_maintenance_enabled: bool,
    pub usage_event_maintenance_interval_seconds: u64,
    pub usage_event_detail_retention_days: i64,
}
```

Do the same for:

- `LlmGatewayRuntimeConfigRecord`
- `LlmGatewayRuntimeConfigResponse`
- `UpdateLlmGatewayRuntimeConfigRequest`

- [ ] **Step 4: Implement validation and persistence**

Update runtime-config parsing/upsert logic in [backend/src/llm_gateway.rs](/home/ts_user/rust_pro/static_flow/backend/src/llm_gateway.rs) and [backend/src/state.rs](/home/ts_user/rust_pro/static_flow/backend/src/state.rs).

```rust
if maintenance_interval_seconds == 0 || maintenance_interval_seconds > 7 * 24 * 60 * 60 {
    return Err(bad_request("usage_event_maintenance_interval_seconds is out of range"));
}
if retention_days != -1 && !(1..=3650).contains(&retention_days) {
    return Err(bad_request("usage_event_detail_retention_days is out of range"));
}
```

- [ ] **Step 5: Implement usage maintenance store helpers**

Add helpers to clear old detail columns and optimize usage indices in [mod.rs](/home/ts_user/rust_pro/static_flow/shared/src/llm_gateway_store/mod.rs).

```rust
pub async fn clear_usage_event_details_before(&self, before_ms: i64) -> Result<u64> {
    let table = self.usage_events_table().await?;
    let predicate = format!(
        "created_at < arrow_cast({before_ms}, 'Timestamp(Millisecond, None)')"
    );
    let result = table
        .update()
        .only_if(predicate)
        .column("request_headers_json", "NULL")
        .column("last_message_content", "NULL")
        .column("client_request_body_json", "NULL")
        .column("upstream_request_body_json", "NULL")
        .column("full_request_json", "NULL")
        .execute()
        .await?;
    Ok(result.rows_updated)
}

pub async fn optimize_usage_event_indices(&self) -> Result<()> {
    let table = self.usage_events_table().await?;
    table.optimize(OptimizeAction::Index(Default::default())).await?;
    Ok(())
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run:

```bash
cargo test -p static-flow-backend update_runtime_config_rejects_invalid_usage_detail_retention_days -- --exact
cargo test -p static-flow-shared clear_old_usage_event_details_preserves_summary_fields -- --exact
```

Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add backend/src/state.rs \
        backend/src/llm_gateway.rs \
        backend/src/llm_gateway/types.rs \
        shared/src/llm_gateway_store/types.rs \
        shared/src/llm_gateway_store/schema.rs \
        shared/src/llm_gateway_store/codec.rs \
        shared/src/llm_gateway_store/mod.rs
git commit -m "feat: add llm usage maintenance runtime config"
```

### Task 3: Split Admin Usage APIs Into Summary List And Detail Lookup

**Files:**
- Modify: `backend/src/llm_gateway.rs`
- Modify: `backend/src/kiro_gateway/mod.rs`
- Modify: `backend/src/kiro_gateway/types.rs`
- Modify: `backend/src/routes.rs`
- Modify: `frontend/src/api.rs`
- Test: `backend/src/llm_gateway.rs`
- Test: `backend/src/kiro_gateway/mod.rs`

- [ ] **Step 1: Write failing handler tests**

```rust
#[tokio::test]
async fn admin_llm_usage_list_returns_summary_rows_only() {
    let app = test_app().await;
    let response = get_json(&app, "/api/admin/llm-gateway/usage").await;
    assert!(response["events"][0].get("full_request_json").is_none());
}

#[tokio::test]
async fn admin_kiro_usage_detail_returns_full_request_json() {
    let app = test_app_with_kiro_event().await;
    let event_id = sample_event_id();
    let response = get_json(&app, &format!("/api/admin/kiro-gateway/usage/{event_id}")).await;
    assert_eq!(response["id"], event_id);
    assert!(response.get("full_request_json").is_some());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p static-flow-backend admin_llm_usage_list_returns_summary_rows_only -- --exact
cargo test -p static-flow-backend admin_kiro_usage_detail_returns_full_request_json -- --exact
```

Expected: FAIL because list/detail endpoints are not split.

- [ ] **Step 3: Add admin API DTOs for summary/detail**

Add summary view structs to [backend/src/kiro_gateway/types.rs](/home/ts_user/rust_pro/static_flow/backend/src/kiro_gateway/types.rs) and [frontend/src/api.rs](/home/ts_user/rust_pro/static_flow/frontend/src/api.rs).

```rust
pub struct AdminLlmGatewayUsageEventSummaryView { /* summary fields only */ }
pub struct AdminLlmGatewayUsageEventDetailView { /* summary + detail fields */ }
```

- [ ] **Step 4: Implement list/detail handlers and routes**

Update list handlers to use summary-store helpers and add detail handlers by
`event_id`.

```rust
pub async fn get_admin_usage_event_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(event_id): Path<String>,
) -> Result<Json<AdminLlmGatewayUsageEventDetailView>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let event = state
        .llm_gateway_store
        .get_usage_event_detail_by_id(&event_id)
        .await?;
    /* map or 404 */
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run:

```bash
cargo test -p static-flow-backend admin_llm_usage_list_returns_summary_rows_only -- --exact
cargo test -p static-flow-backend admin_kiro_usage_detail_returns_full_request_json -- --exact
```

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add backend/src/llm_gateway.rs \
        backend/src/kiro_gateway/mod.rs \
        backend/src/kiro_gateway/types.rs \
        backend/src/routes.rs \
        frontend/src/api.rs
git commit -m "feat: split admin usage list and detail apis"
```

### Task 4: Remove Kiro Prefetch And Lazy-Load Usage Detail In Frontend

**Files:**
- Modify: `frontend/src/pages/admin_llm_gateway.rs`
- Modify: `frontend/src/pages/admin_kiro_gateway.rs`
- Modify: `frontend/src/api.rs`
- Test: `frontend/src/pages/admin_llm_gateway.rs`
- Test: `frontend/src/pages/admin_kiro_gateway.rs`

- [ ] **Step 1: Write failing frontend tests**

```rust
#[test]
fn kiro_admin_does_not_prefetch_usage_on_initial_render() {
    let calls = render_kiro_admin_and_capture_usage_calls();
    assert!(calls.is_empty());
}

#[test]
fn llm_usage_detail_modal_fetches_detail_on_demand() {
    let calls = render_llm_usage_modal_flow_and_capture_detail_calls();
    assert_eq!(calls, vec!["detail"]);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p static-flow-frontend kiro_admin_does_not_prefetch_usage_on_initial_render -- --exact
cargo test -p static-flow-frontend llm_usage_detail_modal_fetches_detail_on_demand -- --exact
```

Expected: FAIL because Kiro still prefetches and detail is not lazy-loaded.

- [ ] **Step 3: Change frontend usage state to summary-plus-selected-detail**

Update [admin_llm_gateway.rs](/home/ts_user/rust_pro/static_flow/frontend/src/pages/admin_llm_gateway.rs) and [admin_kiro_gateway.rs](/home/ts_user/rust_pro/static_flow/frontend/src/pages/admin_kiro_gateway.rs).

```rust
let usage_events = use_state(Vec::<AdminLlmGatewayUsageEventSummaryView>::new);
let selected_usage_event_detail = use_state(|| None::<AdminLlmGatewayUsageEventDetailView>);
let usage_detail_loading = use_state(|| false);
```

- [ ] **Step 4: Remove Kiro page-load usage fetch and add detail lazy loading**

Only trigger usage fetch on explicit tab/section access, and fetch detail by
`event_id` when opening the modal.

```rust
let on_open_usage_detail = {
    let selected_usage_event_detail = selected_usage_event_detail.clone();
    Callback::from(move |event_id: String| {
        wasm_bindgen_futures::spawn_local(async move {
            let detail = fetch_admin_llm_gateway_usage_event_detail(&event_id).await.unwrap();
            selected_usage_event_detail.set(Some(detail));
        });
    })
};
```

- [ ] **Step 5: Run tests to verify they pass**

Run:

```bash
cargo test -p static-flow-frontend kiro_admin_does_not_prefetch_usage_on_initial_render -- --exact
cargo test -p static-flow-frontend llm_usage_detail_modal_fetches_detail_on_demand -- --exact
```

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add frontend/src/pages/admin_llm_gateway.rs \
        frontend/src/pages/admin_kiro_gateway.rs \
        frontend/src/api.rs
git commit -m "feat: lazy load admin usage details"
```

### Task 5: Add Runtime Maintenance Loop And Admin Runtime Controls

**Files:**
- Modify: `backend/src/llm_gateway/runtime.rs`
- Modify: `backend/src/llm_gateway.rs`
- Modify: `backend/src/state.rs`
- Modify: `frontend/src/pages/admin_llm_gateway.rs`
- Modify: `frontend/src/api.rs`
- Test: `backend/src/llm_gateway/runtime.rs`
- Test: `frontend/src/pages/admin_llm_gateway.rs`

- [ ] **Step 1: Write failing maintenance-loop tests**

```rust
#[tokio::test]
async fn usage_maintenance_tick_clears_old_detail_and_optimizes_indices() {
    let runtime = test_runtime_with_usage_maintenance();
    runtime.run_usage_maintenance_once().await.unwrap();
    assert_eq!(runtime.store_trim_count().await, 1);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p static-flow-backend usage_maintenance_tick_clears_old_detail_and_optimizes_indices -- --exact
```

Expected: FAIL because the maintenance loop does not exist.

- [ ] **Step 3: Implement dedicated usage maintenance loop**

Add a periodic task in [runtime.rs](/home/ts_user/rust_pro/static_flow/backend/src/llm_gateway/runtime.rs).

```rust
fn spawn_usage_event_maintenance_loop(
    store: Arc<LlmGatewayStore>,
    runtime_config: Arc<RwLock<LlmGatewayRuntimeConfig>>,
    shutdown_rx: watch::Receiver<bool>,
) {
    tokio::spawn(async move {
        loop {
            let cfg = runtime_config.read().clone();
            if cfg.usage_event_maintenance_enabled {
                if cfg.usage_event_detail_retention_days > 0 {
                    let cutoff = chrono::Utc::now().timestamp_millis()
                        - cfg.usage_event_detail_retention_days * 24 * 60 * 60 * 1000;
                    let _ = store.clear_usage_event_details_before(cutoff).await;
                }
                let _ = store.optimize_usage_event_indices().await;
            }
            /* sleep or shutdown */
        }
    });
}
```

- [ ] **Step 4: Expose runtime-config controls in admin UI**

Add form fields and PATCH payload wiring in [admin_llm_gateway.rs](/home/ts_user/rust_pro/static_flow/frontend/src/pages/admin_llm_gateway.rs).

```rust
let usage_maintenance_enabled_input = use_state(|| true);
let usage_maintenance_interval_input = use_state(|| "3600".to_string());
let usage_detail_retention_days_input = use_state(|| "-1".to_string());
```

- [ ] **Step 5: Run targeted tests to verify they pass**

Run:

```bash
cargo test -p static-flow-backend usage_maintenance_tick_clears_old_detail_and_optimizes_indices -- --exact
cargo test -p static-flow-frontend admin_llm_gateway_runtime_config_round_trips_usage_maintenance_fields -- --exact
```

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add backend/src/llm_gateway/runtime.rs \
        backend/src/llm_gateway.rs \
        backend/src/state.rs \
        frontend/src/pages/admin_llm_gateway.rs \
        frontend/src/api.rs
git commit -m "feat: add llm usage maintenance loop"
```

### Task 6: Verification And Cleanup

**Files:**
- Modify: only if verification uncovers issues

- [ ] **Step 1: Format changed Rust files**

Run:

```bash
rustfmt backend/src/state.rs \
        backend/src/llm_gateway.rs \
        backend/src/llm_gateway/runtime.rs \
        backend/src/llm_gateway/types.rs \
        backend/src/kiro_gateway/mod.rs \
        backend/src/kiro_gateway/types.rs \
        backend/src/routes.rs \
        shared/src/llm_gateway_store/types.rs \
        shared/src/llm_gateway_store/schema.rs \
        shared/src/llm_gateway_store/codec.rs \
        shared/src/llm_gateway_store/mod.rs
```

Expected: command succeeds with no output

- [ ] **Step 2: Run frontend fmt for changed frontend files**

Run:

```bash
rustfmt frontend/src/api.rs \
        frontend/src/pages/admin_llm_gateway.rs \
        frontend/src/pages/admin_kiro_gateway.rs
```

Expected: command succeeds with no output

- [ ] **Step 3: Run affected test suites**

Run:

```bash
cargo test -p static-flow-shared
cargo test -p static-flow-backend
cargo test -p static-flow-frontend
```

Expected: PASS

- [ ] **Step 4: Run clippy on affected crates**

Run:

```bash
cargo clippy -p static-flow-shared --all-targets -- -D warnings
cargo clippy -p static-flow-backend --all-targets -- -D warnings
cargo clippy -p static-flow-frontend --all-targets -- -D warnings
```

Expected: PASS with zero warnings

- [ ] **Step 5: Final commit**

```bash
git add backend/src/state.rs \
        backend/src/llm_gateway.rs \
        backend/src/llm_gateway/runtime.rs \
        backend/src/llm_gateway/types.rs \
        backend/src/kiro_gateway/mod.rs \
        backend/src/kiro_gateway/types.rs \
        backend/src/routes.rs \
        shared/src/llm_gateway_store/types.rs \
        shared/src/llm_gateway_store/schema.rs \
        shared/src/llm_gateway_store/codec.rs \
        shared/src/llm_gateway_store/mod.rs \
        frontend/src/api.rs \
        frontend/src/pages/admin_llm_gateway.rs \
        frontend/src/pages/admin_kiro_gateway.rs
git commit -m "fix: slim llm usage queries and retain detail safely"
```
