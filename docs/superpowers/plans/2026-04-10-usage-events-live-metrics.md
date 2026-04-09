# Usage Events Live Metrics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add live RPM and in-flight metrics to the admin Usage Events page, filtered by the selected key, and persist the full raw client request JSON with each usage event.

**Architecture:** Introduce one shared in-memory request activity tracker that records request starts in a fixed 60-second ring buffer and tracks current in-flight requests with an RAII guard. Extend the usage-event schema with a new canonical `full_request_json` field, plumb it through Codex and Kiro event creation, and surface both the live metrics and the full request in the admin API and UI.

**Tech Stack:** Rust, Axum, Yew, LanceDB store schema/codec, existing LLM gateway runtime state, RAII guards, fixed-size ring buffer, monotonic `Instant`.

---

## File Structure

- Create: `backend/src/llm_gateway/activity.rs`
  - Own the live request activity tracker, ring-buffer window, snapshots, and RAII guard
- Modify: `backend/src/llm_gateway/runtime.rs`
  - Hold shared tracker state and expose helper methods
- Modify: `backend/src/state.rs`
  - Initialize tracker in global state
- Modify: `backend/src/llm_gateway.rs`
  - Start/stop activity guards in request paths and return live metrics from usage API
- Modify: `backend/src/llm_gateway/types.rs`
  - Add live metric fields to `AdminLlmGatewayUsageEventsResponse`
- Modify: `backend/src/kiro_gateway/mod.rs`
  - Populate `full_request_json` in Kiro usage events
- Modify: `backend/src/llm_gateway.rs`
  - Populate `full_request_json` in Codex usage events
- Modify: `shared/src/llm_gateway_store/types.rs`
  - Add `full_request_json` to `LlmGatewayUsageEventRecord`
- Modify: `shared/src/llm_gateway_store/schema.rs`
  - Add the new nullable UTF-8 column
- Modify: `shared/src/llm_gateway_store/codec.rs`
  - Encode/decode the new column
- Modify: `shared/src/llm_gateway_store/mod.rs`
  - Add store round-trip and migration tests
- Modify: `frontend/src/api.rs`
  - Surface `current_rpm`, `current_in_flight`, and `full_request_json`
- Modify: `frontend/src/pages/admin_llm_gateway.rs`
  - Show live metrics in the Usage Events header and `Full Request` in the details modal

## Task 1: Add The Shared Activity Tracker

**Files:**
- Create: `backend/src/llm_gateway/activity.rs`
- Modify: `backend/src/llm_gateway/runtime.rs`
- Modify: `backend/src/state.rs`
- Test: `backend/src/llm_gateway/activity.rs`

- [ ] **Step 1: Write failing tracker tests**

Add tests for the ring buffer and RAII semantics:

```rust
#[test]
fn sliding_second_window_reuses_slots_after_sixty_seconds() {
    let mut window = SlidingSecondWindow::default();

    window.record_at(100);
    window.record_at(100);
    assert_eq!(window.rpm_at(100), 2);

    window.record_at(160);
    assert_eq!(window.rpm_at(160), 1);
}

#[test]
fn request_activity_tracker_counts_total_and_key_in_flight() {
    let tracker = RequestActivityTracker::new();
    let guard_a = tracker.start("key-a", 100);
    let guard_b = tracker.start("key-a", 101);

    assert_eq!(tracker.snapshot(None, 101).in_flight, 2);
    assert_eq!(tracker.snapshot(Some("key-a"), 101).in_flight, 2);

    drop(guard_a);
    drop(guard_b);

    assert_eq!(tracker.snapshot(Some("key-a"), 101).in_flight, 0);
}
```

- [ ] **Step 2: Run tests and verify RED**

Run:

```bash
cargo test -p static-flow-backend sliding_second_window_ -- --nocapture
cargo test -p static-flow-backend request_activity_tracker_counts_total_and_key_in_flight -- --nocapture
```

Expected: FAIL because the tracker module does not exist yet.

- [ ] **Step 3: Implement the minimal tracker**

Create `backend/src/llm_gateway/activity.rs` with:

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RequestActivitySnapshot {
    pub rpm: u32,
    pub in_flight: u32,
}

#[derive(Debug, Clone, Copy, Default)]
struct SlidingSecondBucket {
    tick_sec: u64,
    count: u32,
}

#[derive(Debug, Clone)]
struct SlidingSecondWindow {
    buckets: [SlidingSecondBucket; 60],
}
```

Implement:
- `record_at(tick_sec)`
- `rpm_at(now_sec)`
- `RequestActivityTracker::start(key_id, tick_sec) -> RequestActivityGuard`
- `RequestActivityTracker::snapshot(key_id, now_sec)`

The module must include detailed English comments plus ASCII visualization explaining the lazy slot-reuse cleanup mechanism.

- [ ] **Step 4: Run tests and verify GREEN**

Run:

```bash
cargo test -p static-flow-backend sliding_second_window_ -- --nocapture
cargo test -p static-flow-backend request_activity_tracker_counts_total_and_key_in_flight -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Wire tracker into runtime state**

Expose:

```rust
pub activity_tracker: Arc<RequestActivityTracker>
```

through `backend/src/llm_gateway/runtime.rs` and `backend/src/state.rs`.

- [ ] **Step 6: Commit**

```bash
git add backend/src/llm_gateway/activity.rs backend/src/llm_gateway/runtime.rs backend/src/state.rs
git commit -m "feat: add live request activity tracker"
```

## Task 2: Extend Usage Event Storage With Full Request JSON

**Files:**
- Modify: `shared/src/llm_gateway_store/types.rs`
- Modify: `shared/src/llm_gateway_store/schema.rs`
- Modify: `shared/src/llm_gateway_store/codec.rs`
- Modify: `shared/src/llm_gateway_store/mod.rs`
- Test: `shared/src/llm_gateway_store/mod.rs`

- [ ] **Step 1: Write failing store tests**

Add tests that cover schema migration and round-trip:

```rust
#[tokio::test]
async fn usage_event_round_trip_preserves_full_request_json() {
    let store = temp_store().await;
    let record = sample_usage_event_record();
    let record = LlmGatewayUsageEventRecord {
        full_request_json: Some("{\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}".to_string()),
        ..record
    };

    store.append_usage_event(&record).await.unwrap();
    let loaded = store.list_usage_events(None, Some(1)).await.unwrap();

    assert_eq!(loaded[0].full_request_json, record.full_request_json);
}
```

- [ ] **Step 2: Run test and verify RED**

Run:

```bash
cargo test -p static-flow-shared usage_event_round_trip_preserves_full_request_json -- --nocapture
```

Expected: FAIL because `full_request_json` does not exist yet.

- [ ] **Step 3: Add the new field and schema column**

Extend `LlmGatewayUsageEventRecord`:

```rust
pub full_request_json: Option<String>,
```

Add the nullable UTF-8 column to the usage-events schema and codec encode/decode path.

- [ ] **Step 4: Run test and verify GREEN**

Run:

```bash
cargo test -p static-flow-shared usage_event_round_trip_preserves_full_request_json -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add shared/src/llm_gateway_store/types.rs shared/src/llm_gateway_store/schema.rs shared/src/llm_gateway_store/codec.rs shared/src/llm_gateway_store/mod.rs
git commit -m "feat: persist full usage request json"
```

## Task 3: Populate Full Request JSON In Codex And Kiro Events

**Files:**
- Modify: `backend/src/llm_gateway.rs`
- Modify: `backend/src/kiro_gateway/mod.rs`
- Test: `backend/src/llm_gateway.rs`
- Test: `backend/src/kiro_gateway/mod.rs`

- [ ] **Step 1: Write failing provider integration tests**

Add tests that prove both providers persist the raw request:

```rust
#[test]
fn build_gateway_usage_event_record_captures_full_request_json() {
    let record = build_gateway_usage_event_record(/* ... */);
    assert_eq!(
        record.full_request_json.as_deref(),
        Some("{\"model\":\"gpt-5\",\"messages\":[]}")
    );
}

#[test]
fn build_kiro_usage_event_record_captures_full_request_json() {
    let record = build_kiro_usage_event_record(/* ... */);
    assert_eq!(
        record.full_request_json.as_deref(),
        Some("{\"messages\":[]}")
    );
}
```

- [ ] **Step 2: Run tests and verify RED**

Run:

```bash
cargo test -p static-flow-backend build_gateway_usage_event_record_captures_full_request_json -- --nocapture
cargo test -p static-flow-backend build_kiro_usage_event_record_captures_full_request_json -- --nocapture
```

Expected: FAIL because the new field is still unset.

- [ ] **Step 3: Implement the minimal population logic**

For Codex:
- capture the raw client request body once at request ingress
- thread it into usage event construction

For Kiro:
- reuse the already-captured raw payload and write it into `full_request_json`

Do not change the existing semantics of:
- `client_request_body_json`
- `upstream_request_body_json`

- [ ] **Step 4: Run tests and verify GREEN**

Run:

```bash
cargo test -p static-flow-backend build_gateway_usage_event_record_captures_full_request_json -- --nocapture
cargo test -p static-flow-backend build_kiro_usage_event_record_captures_full_request_json -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add backend/src/llm_gateway.rs backend/src/kiro_gateway/mod.rs
git commit -m "feat: capture full usage request json"
```

## Task 4: Return Live Metrics From The Usage Events API

**Files:**
- Modify: `backend/src/llm_gateway/types.rs`
- Modify: `backend/src/llm_gateway.rs`
- Test: `backend/src/llm_gateway.rs`

- [ ] **Step 1: Write failing API tests**

Add tests that verify filtering behavior:

```rust
#[tokio::test]
async fn admin_usage_events_response_includes_total_live_metrics() {
    let state = test_app_state_with_activity();
    state.llm_gateway.activity_tracker.start("key-a", 100);
    state.llm_gateway.activity_tracker.start("key-b", 100);

    let response = list_admin_usage_events(/* key_id = None */).await.unwrap();
    assert_eq!(response.current_in_flight, 2);
    assert_eq!(response.current_rpm, 2);
}

#[tokio::test]
async fn admin_usage_events_response_filters_live_metrics_by_key() {
    let state = test_app_state_with_activity();
    state.llm_gateway.activity_tracker.start("key-a", 100);
    state.llm_gateway.activity_tracker.start("key-b", 100);

    let response = list_admin_usage_events(/* key_id = Some(\"key-a\") */).await.unwrap();
    assert_eq!(response.current_in_flight, 1);
    assert_eq!(response.current_rpm, 1);
}
```

- [ ] **Step 2: Run tests and verify RED**

Run:

```bash
cargo test -p static-flow-backend admin_usage_events_response_ -- --nocapture
```

Expected: FAIL because the response does not have live metric fields yet.

- [ ] **Step 3: Implement response fields and snapshot lookup**

Extend `AdminLlmGatewayUsageEventsResponse` with:

```rust
pub current_rpm: u32,
pub current_in_flight: u32,
```

Then compute the snapshot in `list_admin_usage_events(...)` from the shared tracker using the query `key_id`.

- [ ] **Step 4: Run tests and verify GREEN**

Run:

```bash
cargo test -p static-flow-backend admin_usage_events_response_ -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add backend/src/llm_gateway/types.rs backend/src/llm_gateway.rs
git commit -m "feat: add live usage metrics to admin api"
```

## Task 5: Hook Activity Guards Into Request Lifecycles

**Files:**
- Modify: `backend/src/llm_gateway.rs`
- Test: `backend/src/llm_gateway.rs`

- [ ] **Step 1: Write failing request-lifecycle tests**

Add tests that prove requests update the tracker only after key resolution:

```rust
#[tokio::test]
async fn codex_request_activity_guard_tracks_in_flight_for_resolved_key() {
    let state = test_app_state_with_key("key-a");
    let _guard = start_request_activity_for_test(&state, "key-a");

    let snapshot = state.llm_gateway.activity_tracker.snapshot(Some("key-a"), current_tick_for_test());
    assert_eq!(snapshot.in_flight, 1);
}
```

- [ ] **Step 2: Run tests and verify RED**

Run:

```bash
cargo test -p static-flow-backend codex_request_activity_guard_tracks_in_flight_for_resolved_key -- --nocapture
```

Expected: FAIL because request handlers do not create tracker guards yet.

- [ ] **Step 3: Implement guard creation at the correct ingress point**

Create the guard only after:
- admin/public auth has passed
- the key is resolved
- the request is officially entering provider handling

The guard must stay alive until the request future settles.

- [ ] **Step 4: Run tests and verify GREEN**

Run:

```bash
cargo test -p static-flow-backend codex_request_activity_guard_tracks_in_flight_for_resolved_key -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add backend/src/llm_gateway.rs
git commit -m "feat: track live request activity by key"
```

## Task 6: Surface Live Metrics And Full Request In The Frontend

**Files:**
- Modify: `frontend/src/api.rs`
- Modify: `frontend/src/pages/admin_llm_gateway.rs`
- Test: `frontend/src/pages/admin_llm_gateway.rs`

- [ ] **Step 1: Write failing frontend tests**

Add tests that cover:

```rust
#[test]
fn usage_events_header_renders_live_metrics() {
    let response = AdminLlmGatewayUsageEventsResponse {
        current_rpm: 42,
        current_in_flight: 3,
        ..Default::default()
    };

    let rendered = render_usage_header(response);
    assert!(rendered.contains("RPM"));
    assert!(rendered.contains("42"));
    assert!(rendered.contains("In Flight"));
    assert!(rendered.contains("3"));
}

#[test]
fn usage_event_modal_renders_full_request_when_present() {
    let event = AdminLlmGatewayUsageEventView {
        full_request_json: Some("{\"messages\":[]}".to_string()),
        ..Default::default()
    };

    let rendered = render_usage_modal(event);
    assert!(rendered.contains("Full Request"));
}
```

- [ ] **Step 2: Run tests and verify RED**

Run:

```bash
cargo test -p static-flow-frontend usage_events_header_renders_live_metrics -- --nocapture
cargo test -p static-flow-frontend usage_event_modal_renders_full_request_when_present -- --nocapture
```

Expected: FAIL because the API types and UI do not expose these fields yet.

- [ ] **Step 3: Implement the UI**

Update API types with:

```rust
pub current_rpm: u32,
pub current_in_flight: u32,
pub full_request_json: Option<String>,
```

Then render:
- live summary in the Usage Events header
- `Full Request` section in the details modal

- [ ] **Step 4: Run tests and verify GREEN**

Run:

```bash
cargo test -p static-flow-frontend usage_events_header_renders_live_metrics -- --nocapture
cargo test -p static-flow-frontend usage_event_modal_renders_full_request_when_present -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/api.rs frontend/src/pages/admin_llm_gateway.rs
git commit -m "feat: show live usage metrics in admin ui"
```

## Final Verification

- [ ] **Step 1: Run backend targeted tests**

```bash
cargo test -p static-flow-backend llm_gateway:: -- --nocapture
```

- [ ] **Step 2: Run shared targeted tests**

```bash
cargo test -p static-flow-shared llm_gateway_store:: -- --nocapture
```

- [ ] **Step 3: Run frontend targeted tests**

```bash
cargo test -p static-flow-frontend admin_llm_gateway -- --nocapture
```

- [ ] **Step 4: Run clippy**

```bash
cargo clippy -p static-flow-backend -p static-flow-shared -- -D warnings
cargo clippy -p static-flow-frontend --target wasm32-unknown-unknown -- -D warnings
```

- [ ] **Step 5: Format only changed files**

```bash
rustfmt backend/src/llm_gateway/activity.rs backend/src/llm_gateway/runtime.rs backend/src/state.rs backend/src/llm_gateway.rs backend/src/llm_gateway/types.rs backend/src/kiro_gateway/mod.rs shared/src/llm_gateway_store/types.rs shared/src/llm_gateway_store/schema.rs shared/src/llm_gateway_store/codec.rs shared/src/llm_gateway_store/mod.rs frontend/src/api.rs frontend/src/pages/admin_llm_gateway.rs
```

- [ ] **Step 6: Confirm clean diff**

```bash
git diff --check
git status --short
```
