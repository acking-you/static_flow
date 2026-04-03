# LanceDB Memory And Polling Maintenance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild the hot LanceDB event tables into a healthy layout, reduce version churn and `count_rows` pressure in the LLM gateway paths, and replace fixed 60-second Codex/Kiro status polling with configurable randomized refresh windows.

**Architecture:** Keep the existing API shapes and event schema semantics intact. Extend the persisted LLM gateway runtime-config row to hold polling and usage-flush controls, move exact usage totals into an in-memory cache rebuilt from usage events, and upgrade both event flushers from tiny fixed batches to threshold-driven batching. Use the existing `sf-cli db rebuild-table-stable` workflow for the maintenance window instead of inventing a one-off migrator.

**Tech Stack:** Rust (Axum backend, shared LanceDB store, Yew frontend), LanceDB SQL aggregation, `cargo test`, `cargo clippy`, targeted `rustfmt`, `sf-cli`.

---

## File Map

- `shared/src/llm_gateway_store/types.rs`
  - Owns persisted runtime-config defaults and row shape
  - Add polling + usage-flush config fields and constants
- `shared/src/llm_gateway_store/schema.rs`
  - Owns Lance schema + nullable backfill for the runtime-config table
  - Add the new persisted columns
- `shared/src/llm_gateway_store/codec.rs`
  - Owns Arrow serialization/deserialization for runtime-config rows
  - Encode/decode the new fields
- `shared/src/llm_gateway_store/mod.rs`
  - Owns runtime-config load/save and usage-event aggregation helpers
  - Select the new fields and add exact usage-count aggregation from the dataset
- `backend/src/state.rs`
  - Owns backend defaults, env-backed compaction/behavior config, startup wiring
  - Raise compaction defaults and add behavior flusher thresholds
- `backend/src/llm_gateway/types.rs`
  - Owns admin request/response payloads for `/admin/llm-gateway/config`
  - Expose the new runtime-config fields without changing response shape style
- `backend/src/llm_gateway.rs`
  - Owns LLM gateway admin config handlers and public rate-limit status refresh
  - Validate/save the new config and report the configured refresh cadence
- `backend/src/llm_gateway/runtime.rs`
  - Owns usage-event batching, rollup rebuild, and in-memory gateway state
  - Add exact usage-event count cache and config-driven usage flush thresholds
- `backend/src/llm_gateway/token_refresh.rs`
  - Owns Codex account refresh / usage polling loop
  - Replace fixed ticker cadence with randomized interval + per-account jitter
- `backend/src/kiro_gateway/status_cache.rs`
  - Owns Kiro cached status refresh loop
  - Replace fixed ticker cadence with randomized interval + per-account jitter
- `backend/src/kiro_gateway/mod.rs`
  - Owns public/admin Kiro status surfaces and usage list endpoint
  - Read exact usage totals from runtime cache and expose the configured refresh cadence
- `backend/src/handlers.rs`
  - Owns compaction config validation tests
  - Update expectations after raising defaults where test fixtures mention them
- `frontend/src/api.rs`
  - Owns admin API runtime-config structs and HTTP payload serialization
  - Add the new fields and update mock defaults
- `frontend/src/pages/admin_llm_gateway.rs`
  - Owns `/admin/llm-gateway` runtime-config form
  - Add inputs for polling + usage-flush settings

---

### Task 1: Persist the new LLM gateway runtime-config fields end to end

**Files:**
- Modify: `shared/src/llm_gateway_store/types.rs`
- Modify: `shared/src/llm_gateway_store/schema.rs`
- Modify: `shared/src/llm_gateway_store/codec.rs`
- Modify: `shared/src/llm_gateway_store/mod.rs`
- Modify: `backend/src/state.rs`
- Modify: `backend/src/llm_gateway/types.rs`
- Modify: `backend/src/llm_gateway.rs`
- Test: `shared/src/llm_gateway_store/mod.rs`
- Test: `backend/src/llm_gateway.rs`

- [ ] **Step 1: Write the failing shared-store round-trip test**

Extend the existing runtime-config tests in `shared/src/llm_gateway_store/mod.rs` with:

```rust
    #[tokio::test]
    async fn runtime_config_round_trip_preserves_polling_and_usage_flush_fields() {
        let dir = temp_store_dir("runtime-config-polling-and-flush");
        let store = LlmGatewayStore::connect(&dir.to_string_lossy())
            .await
            .expect("connect llm gateway store");

        let config = LlmGatewayRuntimeConfigRecord {
            codex_status_refresh_min_interval_seconds: 240,
            codex_status_refresh_max_interval_seconds: 300,
            codex_status_account_jitter_max_seconds: 10,
            kiro_status_refresh_min_interval_seconds: 240,
            kiro_status_refresh_max_interval_seconds: 300,
            kiro_status_account_jitter_max_seconds: 10,
            usage_event_flush_batch_size: 256,
            usage_event_flush_interval_seconds: 15,
            usage_event_flush_max_buffer_bytes: 8 * 1024 * 1024,
            updated_at: now_ms(),
            ..LlmGatewayRuntimeConfigRecord::default()
        };

        store
            .upsert_runtime_config(&config)
            .await
            .expect("upsert runtime config");

        let loaded = store
            .get_runtime_config_or_default()
            .await
            .expect("load runtime config");

        assert_eq!(loaded.codex_status_refresh_min_interval_seconds, 240);
        assert_eq!(loaded.codex_status_refresh_max_interval_seconds, 300);
        assert_eq!(loaded.codex_status_account_jitter_max_seconds, 10);
        assert_eq!(loaded.kiro_status_refresh_min_interval_seconds, 240);
        assert_eq!(loaded.kiro_status_refresh_max_interval_seconds, 300);
        assert_eq!(loaded.kiro_status_account_jitter_max_seconds, 10);
        assert_eq!(loaded.usage_event_flush_batch_size, 256);
        assert_eq!(loaded.usage_event_flush_interval_seconds, 15);
        assert_eq!(loaded.usage_event_flush_max_buffer_bytes, 8 * 1024 * 1024);

        let _ = fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 2: Write the failing backend validation test**

Add a focused validation test near the existing `backend/src/llm_gateway.rs` tests:

```rust
    #[test]
    fn update_runtime_config_rejects_invalid_refresh_ranges() {
        let err = validate_runtime_refresh_window(301, 300)
            .expect_err("min > max should fail");
        assert!(err.0 == StatusCode::BAD_REQUEST);

        let err = validate_runtime_refresh_window(239, 300)
            .expect_err("too-small min should fail");
        assert!(err.0 == StatusCode::BAD_REQUEST);
    }
```

- [ ] **Step 3: Run the narrow failing tests**

Run:

```bash
cargo test -p static-flow-shared runtime_config_round_trip_preserves_polling_and_usage_flush_fields -- --nocapture
cargo test -p static-flow-backend update_runtime_config_rejects_invalid_refresh_ranges -- --nocapture
```

Expected:

- The shared test fails because the new fields do not exist in `LlmGatewayRuntimeConfigRecord`
- The backend test fails because the validation helper does not exist yet

- [ ] **Step 4: Extend the shared runtime-config model, schema, and codec**

In `shared/src/llm_gateway_store/types.rs`, add new defaults and fields:

```rust
pub const DEFAULT_CODEX_STATUS_REFRESH_MIN_INTERVAL_SECONDS: u64 = 240;
pub const DEFAULT_CODEX_STATUS_REFRESH_MAX_INTERVAL_SECONDS: u64 = 300;
pub const DEFAULT_CODEX_STATUS_ACCOUNT_JITTER_MAX_SECONDS: u64 = 10;
pub const DEFAULT_KIRO_STATUS_REFRESH_MIN_INTERVAL_SECONDS: u64 = 240;
pub const DEFAULT_KIRO_STATUS_REFRESH_MAX_INTERVAL_SECONDS: u64 = 300;
pub const DEFAULT_KIRO_STATUS_ACCOUNT_JITTER_MAX_SECONDS: u64 = 10;
pub const DEFAULT_LLM_GATEWAY_USAGE_EVENT_FLUSH_BATCH_SIZE: u64 = 256;
pub const DEFAULT_LLM_GATEWAY_USAGE_EVENT_FLUSH_INTERVAL_SECONDS: u64 = 15;
pub const DEFAULT_LLM_GATEWAY_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES: u64 = 8 * 1024 * 1024;

pub struct LlmGatewayRuntimeConfigRecord {
    pub id: String,
    pub auth_cache_ttl_seconds: u64,
    pub max_request_body_bytes: u64,
    pub account_failure_retry_limit: u64,
    pub kiro_channel_max_concurrency: u64,
    pub kiro_channel_min_start_interval_ms: u64,
    pub codex_status_refresh_min_interval_seconds: u64,
    pub codex_status_refresh_max_interval_seconds: u64,
    pub codex_status_account_jitter_max_seconds: u64,
    pub kiro_status_refresh_min_interval_seconds: u64,
    pub kiro_status_refresh_max_interval_seconds: u64,
    pub kiro_status_account_jitter_max_seconds: u64,
    pub usage_event_flush_batch_size: u64,
    pub usage_event_flush_interval_seconds: u64,
    pub usage_event_flush_max_buffer_bytes: u64,
    pub updated_at: i64,
}
```

Then update:

- `shared/src/llm_gateway_store/schema.rs` to add `UInt64` columns and `ensure_nullable_u64_column(...)`
- `shared/src/llm_gateway_store/codec.rs` to encode/decode every new field with defaults
- `shared/src/llm_gateway_store/mod.rs` to select every new column in `get_runtime_config()`

- [ ] **Step 5: Extend the backend runtime-config structs and validation**

In `backend/src/state.rs`, extend `LlmGatewayRuntimeConfig`:

```rust
pub struct LlmGatewayRuntimeConfig {
    pub auth_cache_ttl_seconds: u64,
    pub max_request_body_bytes: u64,
    pub account_failure_retry_limit: u64,
    pub kiro_channel_max_concurrency: u64,
    pub kiro_channel_min_start_interval_ms: u64,
    pub codex_status_refresh_min_interval_seconds: u64,
    pub codex_status_refresh_max_interval_seconds: u64,
    pub codex_status_account_jitter_max_seconds: u64,
    pub kiro_status_refresh_min_interval_seconds: u64,
    pub kiro_status_refresh_max_interval_seconds: u64,
    pub kiro_status_account_jitter_max_seconds: u64,
    pub usage_event_flush_batch_size: u64,
    pub usage_event_flush_interval_seconds: u64,
    pub usage_event_flush_max_buffer_bytes: u64,
}
```

In `backend/src/llm_gateway/types.rs`, extend request/response structs:

```rust
pub struct UpdateLlmGatewayRuntimeConfigRequest {
    pub auth_cache_ttl_seconds: Option<u64>,
    pub max_request_body_bytes: Option<u64>,
    pub account_failure_retry_limit: Option<u64>,
    pub codex_status_refresh_min_interval_seconds: Option<u64>,
    pub codex_status_refresh_max_interval_seconds: Option<u64>,
    pub codex_status_account_jitter_max_seconds: Option<u64>,
    pub kiro_status_refresh_min_interval_seconds: Option<u64>,
    pub kiro_status_refresh_max_interval_seconds: Option<u64>,
    pub kiro_status_account_jitter_max_seconds: Option<u64>,
    pub usage_event_flush_batch_size: Option<u64>,
    pub usage_event_flush_interval_seconds: Option<u64>,
    pub usage_event_flush_max_buffer_bytes: Option<u64>,
}
```

In `backend/src/llm_gateway.rs`, add helpers like:

```rust
fn validate_runtime_refresh_window(
    min_seconds: u64,
    max_seconds: u64,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if !(240..=3600).contains(&min_seconds) || !(240..=3600).contains(&max_seconds) {
        return Err(bad_request("refresh window seconds must be between 240 and 3600"));
    }
    if min_seconds > max_seconds {
        return Err(bad_request("refresh min interval must be less than or equal to max interval"));
    }
    Ok(())
}
```

Use the helper from `update_admin_runtime_config(...)`, persist the new fields, and include them in `get_admin_runtime_config(...)`.

- [ ] **Step 6: Re-run the focused tests**

Run:

```bash
cargo test -p static-flow-shared runtime_config_round_trip_preserves_polling_and_usage_flush_fields -- --nocapture
cargo test -p static-flow-backend update_runtime_config_rejects_invalid_refresh_ranges -- --nocapture
```

Expected: both PASS.

- [ ] **Step 7: Commit**

Run:

```bash
git add shared/src/llm_gateway_store/types.rs \
        shared/src/llm_gateway_store/schema.rs \
        shared/src/llm_gateway_store/codec.rs \
        shared/src/llm_gateway_store/mod.rs \
        backend/src/state.rs \
        backend/src/llm_gateway/types.rs \
        backend/src/llm_gateway.rs
git commit -m "feat: extend llm gateway runtime config"
```

---

### Task 2: Randomize Codex and Kiro status polling

**Files:**
- Modify: `backend/src/llm_gateway/token_refresh.rs`
- Modify: `backend/src/kiro_gateway/status_cache.rs`
- Modify: `backend/src/llm_gateway.rs`
- Modify: `backend/src/kiro_gateway/mod.rs`
- Test: `backend/src/llm_gateway/token_refresh.rs`
- Test: `backend/src/kiro_gateway/status_cache.rs`

- [ ] **Step 1: Write the failing cadence helper tests**

Add these tests to `backend/src/llm_gateway/token_refresh.rs`:

```rust
    #[test]
    fn codex_refresh_interval_draw_uses_configured_bounds() {
        let config = crate::state::LlmGatewayRuntimeConfig {
            codex_status_refresh_min_interval_seconds: 240,
            codex_status_refresh_max_interval_seconds: 300,
            ..crate::state::LlmGatewayRuntimeConfig::default()
        };

        for _ in 0..64 {
            let value = next_codex_refresh_delay(&config).as_secs();
            assert!(value >= 240 && value <= 300);
        }
    }

    #[test]
    fn codex_per_account_jitter_stays_within_configured_limit() {
        let config = crate::state::LlmGatewayRuntimeConfig {
            codex_status_account_jitter_max_seconds: 10,
            ..crate::state::LlmGatewayRuntimeConfig::default()
        };

        for _ in 0..64 {
            let value = next_codex_account_jitter(&config).as_secs();
            assert!(value <= 10);
        }
    }
```

Add the Kiro equivalents to `backend/src/kiro_gateway/status_cache.rs`:

```rust
    #[test]
    fn kiro_refresh_interval_draw_uses_configured_bounds() {
        let config = crate::state::LlmGatewayRuntimeConfig {
            kiro_status_refresh_min_interval_seconds: 240,
            kiro_status_refresh_max_interval_seconds: 300,
            ..crate::state::LlmGatewayRuntimeConfig::default()
        };

        for _ in 0..64 {
            let value = next_kiro_refresh_delay(&config).as_secs();
            assert!(value >= 240 && value <= 300);
        }
    }
```

- [ ] **Step 2: Run the cadence helper test filters and verify they fail**

Run:

```bash
cargo test -p static-flow-backend codex_refresh_interval_draw_uses_configured_bounds -- --nocapture
cargo test -p static-flow-backend kiro_refresh_interval_draw_uses_configured_bounds -- --nocapture
```

Expected: FAIL because the helper functions do not exist yet.

- [ ] **Step 3: Replace fixed tickers with config-driven randomized waits**

In `backend/src/llm_gateway/token_refresh.rs`, replace the fixed ticker loop with config-driven sleep:

```rust
fn next_codex_refresh_delay(config: &LlmGatewayRuntimeConfig) -> Duration {
    let min = config.codex_status_refresh_min_interval_seconds.min(
        config.codex_status_refresh_max_interval_seconds,
    );
    let max = config.codex_status_refresh_min_interval_seconds.max(
        config.codex_status_refresh_max_interval_seconds,
    );
    let secs = if min == max {
        min
    } else {
        rand::thread_rng().gen_range(min..=max)
    };
    Duration::from_secs(secs)
}

fn next_codex_account_jitter(config: &LlmGatewayRuntimeConfig) -> Duration {
    let max = config.codex_status_account_jitter_max_seconds;
    Duration::from_secs(rand::thread_rng().gen_range(0..=max))
}
```

Apply the same structure in `backend/src/kiro_gateway/status_cache.rs`.

Then change the outer loops from `ticker.tick()` to:

```rust
let delay = {
    let config = runtime_config.read().clone();
    next_codex_refresh_delay(&config)
};

tokio::select! {
    _ = shutdown_rx.changed() => {
        if *shutdown_rx.borrow() {
            tracing::info!("Account refresh task shutting down");
            return;
        }
    }
    _ = tokio::time::sleep(delay) => {
        refresh_all_accounts(&pool, &proxy_registry, runtime_config.as_ref()).await?;
    }
}
```

Inside each account loop, insert:

```rust
if index > 0 {
    let jitter = {
        let config = runtime_config.read().clone();
        next_codex_account_jitter(&config)
    };
    if !jitter.is_zero() {
        tokio::time::sleep(jitter).await;
    }
}
```

- [ ] **Step 4: Keep public refresh metadata conservative**

Update:

- `backend/src/llm_gateway.rs`
- `backend/src/kiro_gateway/mod.rs`

so `refresh_interval_seconds` reports the configured max interval rather than the legacy fixed `60`. Use code like:

```rust
let refresh_interval_seconds = runtime
    .runtime_config
    .read()
    .codex_status_refresh_max_interval_seconds;
```

and for Kiro:

```rust
refresh_interval_seconds: state
    .llm_gateway_runtime_config
    .read()
    .kiro_status_refresh_max_interval_seconds,
```

- [ ] **Step 5: Re-run the focused tests**

Run:

```bash
cargo test -p static-flow-backend codex_refresh_interval_draw_uses_configured_bounds -- --nocapture
cargo test -p static-flow-backend codex_per_account_jitter_stays_within_configured_limit -- --nocapture
cargo test -p static-flow-backend kiro_refresh_interval_draw_uses_configured_bounds -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit**

Run:

```bash
git add backend/src/llm_gateway/token_refresh.rs \
        backend/src/kiro_gateway/status_cache.rs \
        backend/src/llm_gateway.rs \
        backend/src/kiro_gateway/mod.rs
git commit -m "feat: randomize codex and kiro status polling"
```

---

### Task 3: Cache exact usage-event totals and remove `count_rows` from usage endpoints

**Files:**
- Modify: `shared/src/llm_gateway_store/mod.rs`
- Modify: `backend/src/llm_gateway/runtime.rs`
- Modify: `backend/src/llm_gateway.rs`
- Modify: `backend/src/kiro_gateway/mod.rs`
- Test: `shared/src/llm_gateway_store/mod.rs`
- Test: `backend/src/llm_gateway/runtime.rs`

- [ ] **Step 1: Write the failing aggregation and runtime-cache tests**

Add a shared-store aggregation test:

```rust
    #[tokio::test]
    async fn aggregate_usage_event_counts_groups_by_provider_and_key() {
        let dir = temp_store_dir("usage-event-counts");
        let store = LlmGatewayStore::connect(&dir.to_string_lossy())
            .await
            .expect("connect llm gateway store");

        let key = sample_key_record("key-count", "Count Key");
        store.create_key(&key).await.expect("create key");

        let now = now_ms();
        let kiro_event = LlmGatewayUsageEventRecord {
            id: "evt-kiro".to_string(),
            key_id: key.id.clone(),
            key_name: key.name.clone(),
            provider_type: LLM_GATEWAY_PROVIDER_KIRO.to_string(),
            account_name: Some("alpha".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/kiro-gateway/v1/messages".to_string(),
            latency_ms: 10,
            endpoint: "/v1/messages".to_string(),
            model: Some("claude-sonnet-4-5".to_string()),
            status_code: 200,
            input_uncached_tokens: 1,
            input_cached_tokens: 0,
            output_tokens: 1,
            billable_tokens: 2,
            usage_missing: false,
            credit_usage: Some(1.0),
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: Some("hello".to_string()),
            created_at: now,
        };
        let codex_event = LlmGatewayUsageEventRecord {
            id: "evt-codex".to_string(),
            key_id: key.id.clone(),
            key_name: key.name.clone(),
            provider_type: LLM_GATEWAY_PROVIDER_CODEX.to_string(),
            account_name: Some("beta".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/llm-gateway/v1/responses".to_string(),
            latency_ms: 12,
            endpoint: "/v1/responses".to_string(),
            model: Some("gpt-5.3-codex".to_string()),
            status_code: 200,
            input_uncached_tokens: 2,
            input_cached_tokens: 0,
            output_tokens: 1,
            billable_tokens: 3,
            usage_missing: false,
            credit_usage: None,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: Some("world".to_string()),
            created_at: now + 1,
        };

        store.append_usage_events(&[kiro_event, codex_event]).await.expect("append usage events");

        let counts = store
            .aggregate_usage_event_counts()
            .await
            .expect("aggregate usage counts");

        assert_eq!(counts.total_event_count, 2);
        assert_eq!(counts.provider_event_counts.get(LLM_GATEWAY_PROVIDER_KIRO), Some(&1));
        assert_eq!(counts.provider_event_counts.get(LLM_GATEWAY_PROVIDER_CODEX), Some(&1));
        assert_eq!(counts.key_event_counts.get(&key.id), Some(&2));

        let _ = fs::remove_dir_all(&dir);
    }
```

Add a runtime update test to `backend/src/llm_gateway/runtime.rs`:

```rust
    #[tokio::test]
    async fn append_usage_event_updates_exact_event_counts_immediately() {
        let dir = temp_dir("llm-gateway-usage-counts");
        let auths_dir = temp_dir("llm-gateway-auths-counts");
        fs::create_dir_all(&auths_dir).expect("create auth dir");

        let store = Arc::new(
            LlmGatewayStore::connect(&dir.to_string_lossy())
                .await
                .expect("connect llm gateway store"),
        );
        let runtime_config = Arc::new(RwLock::new(LlmGatewayRuntimeConfig::default()));
        let account_pool = Arc::new(AccountPool::new(auths_dir.clone()));
        let upstream_proxy_registry = Arc::new(
            UpstreamProxyRegistry::new(store.clone())
                .await
                .expect("create upstream proxy registry"),
        );
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let runtime = LlmGatewayRuntimeState::new(
            store,
            runtime_config,
            account_pool,
            upstream_proxy_registry,
            shutdown_rx,
        )
        .expect("create runtime");
        let key = sample_key();
        let event = LlmGatewayUsageEventRecord {
            id: "evt-1".to_string(),
            key_id: key.id.clone(),
            key_name: key.name.clone(),
            provider_type: key.provider_type.clone(),
            account_name: Some("test-account".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/llm-gateway/v1/responses".to_string(),
            latency_ms: 10,
            endpoint: "/v1/responses".to_string(),
            model: Some("gpt-5".to_string()),
            status_code: 200,
            input_uncached_tokens: 2,
            input_cached_tokens: 0,
            output_tokens: 1,
            billable_tokens: 3,
            usage_missing: false,
            credit_usage: None,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: Some("hello".to_string()),
            created_at: now_ms(),
        };

        runtime.append_usage_event(&key, &event).await.expect("append usage event");

        assert_eq!(runtime.total_usage_event_count(), 1);
        assert_eq!(
            runtime.usage_event_count_for_provider(&key.provider_type),
            1
        );
        assert_eq!(runtime.usage_event_count_for_key(&key.id), 1);

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&auths_dir);
    }
```

- [ ] **Step 2: Run the new test filters and verify they fail**

Run:

```bash
cargo test -p static-flow-shared aggregate_usage_event_counts_groups_by_provider_and_key -- --nocapture
cargo test -p static-flow-backend append_usage_event_updates_exact_event_counts_immediately -- --nocapture
```

Expected: FAIL because the aggregation helper and runtime cache do not exist yet.

- [ ] **Step 3: Add a shared usage-count aggregation helper**

In `shared/src/llm_gateway_store/mod.rs`, add a new dataset SQL helper alongside `aggregate_usage_rollups_from_dataset(...)`:

```rust
pub struct LlmGatewayUsageEventCounts {
    pub total_event_count: usize,
    pub provider_event_counts: HashMap<String, usize>,
    pub key_event_counts: HashMap<String, usize>,
}

pub async fn aggregate_usage_event_counts(&self) -> Result<LlmGatewayUsageEventCounts> {
    let table = self.usage_events_table().await?;
    let dataset = table
        .dataset()
        .context("usage-event counts require a native Lance table")?
        .get()
        .await
        .context("failed to open usage-event dataset for counts")?;
    aggregate_usage_event_counts_from_dataset(&dataset).await
}
```

Use SQL like:

```sql
SELECT
    provider_type,
    key_id,
    CAST(COUNT(*) AS BIGINT) AS event_count
FROM dataset
GROUP BY provider_type, key_id
```

and fold the grouped rows into:

- global total
- `provider_event_counts`
- `key_event_counts`

- [ ] **Step 4: Add the runtime exact-count cache and endpoint accessors**

In `backend/src/llm_gateway/runtime.rs`, add:

```rust
#[derive(Debug, Clone, Default)]
pub(crate) struct UsageEventCountCache {
    pub total_event_count: usize,
    pub provider_event_counts: HashMap<String, usize>,
    pub key_event_counts: HashMap<String, usize>,
}
```

Store it behind `Arc<RwLock<UsageEventCountCache>>`, rebuild it during startup, and update it in `append_usage_event(...)`:

```rust
{
    let mut counts = self.usage_event_counts.write();
    counts.total_event_count = counts.total_event_count.saturating_add(1);
    *counts.provider_event_counts.entry(event.provider_type.clone()).or_default() += 1;
    *counts.key_event_counts.entry(event.key_id.clone()).or_default() += 1;
}
```

Expose helpers:

```rust
pub(crate) fn total_usage_event_count(&self) -> usize {
    self.usage_event_counts.read().total_event_count
}

pub(crate) fn usage_event_count_for_provider(&self, provider_type: &str) -> usize {
    self.usage_event_counts
        .read()
        .provider_event_counts
        .get(provider_type)
        .copied()
        .unwrap_or(0)
}

pub(crate) fn usage_event_count_for_key(&self, key_id: &str) -> usize {
    self.usage_event_counts
        .read()
        .key_event_counts
        .get(key_id)
        .copied()
        .unwrap_or(0)
}
```

Then update:

- `backend/src/llm_gateway.rs::list_admin_usage_events(...)`
- `backend/src/llm_gateway.rs::lookup_public_usage(...)`
- `backend/src/kiro_gateway/mod.rs::list_admin_usage_events(...)`

to use the runtime helpers instead of `store.count_usage_events*`.

- [ ] **Step 5: Re-run the focused tests**

Run:

```bash
cargo test -p static-flow-shared aggregate_usage_event_counts_groups_by_provider_and_key -- --nocapture
cargo test -p static-flow-backend append_usage_event_updates_exact_event_counts_immediately -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit**

Run:

```bash
git add shared/src/llm_gateway_store/mod.rs \
        backend/src/llm_gateway/runtime.rs \
        backend/src/llm_gateway.rs \
        backend/src/kiro_gateway/mod.rs
git commit -m "feat: cache exact llm usage event totals"
```

---

### Task 4: Replace fixed usage and behavior flush constants with threshold-driven batching and raise compaction defaults

**Files:**
- Modify: `backend/src/llm_gateway/runtime.rs`
- Modify: `backend/src/state.rs`
- Modify: `backend/src/handlers.rs`
- Modify: `frontend/src/api.rs`
- Test: `backend/src/llm_gateway/runtime.rs`
- Test: `backend/src/handlers.rs`

- [ ] **Step 1: Write the failing byte-threshold and compaction-default tests**

Add a usage flusher test to `backend/src/llm_gateway/runtime.rs`:

```rust
    #[tokio::test]
    async fn usage_events_flush_when_buffer_bytes_reach_limit() {
        let dir = temp_dir("llm-gateway-usage-byte-flush");
        let auths_dir = temp_dir("llm-gateway-auths-byte-flush");
        fs::create_dir_all(&auths_dir).expect("create auth dir");

        let store = Arc::new(
            LlmGatewayStore::connect(&dir.to_string_lossy())
                .await
                .expect("connect llm gateway store"),
        );
        let runtime_config = Arc::new(RwLock::new(crate::state::LlmGatewayRuntimeConfig {
            usage_event_flush_batch_size: 256,
            usage_event_flush_interval_seconds: 60,
            usage_event_flush_max_buffer_bytes: 64,
            ..crate::state::LlmGatewayRuntimeConfig::default()
        }));
        let account_pool = Arc::new(AccountPool::new(auths_dir.clone()));
        let upstream_proxy_registry = Arc::new(
            UpstreamProxyRegistry::new(store.clone())
                .await
                .expect("create upstream proxy registry"),
        );
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let runtime = LlmGatewayRuntimeState::new(
            store.clone(),
            runtime_config,
            account_pool,
            upstream_proxy_registry,
            shutdown_rx,
        )
        .expect("create runtime");
        let key = sample_key();

        let first = LlmGatewayUsageEventRecord {
            id: "evt-1".to_string(),
            key_id: key.id.clone(),
            key_name: key.name.clone(),
            provider_type: key.provider_type.clone(),
            account_name: Some("test-account".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/llm-gateway/v1/responses".to_string(),
            latency_ms: 10,
            endpoint: "/v1/responses".to_string(),
            model: Some("gpt-5".to_string()),
            status_code: 200,
            input_uncached_tokens: 2,
            input_cached_tokens: 0,
            output_tokens: 1,
            billable_tokens: 3,
            usage_missing: false,
            credit_usage: None,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: Some("1234567890".to_string()),
            created_at: now_ms(),
        };
        let second = LlmGatewayUsageEventRecord {
            id: "evt-2".to_string(),
            key_id: key.id.clone(),
            key_name: key.name.clone(),
            provider_type: key.provider_type.clone(),
            account_name: Some("test-account".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/llm-gateway/v1/responses".to_string(),
            latency_ms: 11,
            endpoint: "/v1/responses".to_string(),
            model: Some("gpt-5".to_string()),
            status_code: 200,
            input_uncached_tokens: 2,
            input_cached_tokens: 0,
            output_tokens: 1,
            billable_tokens: 3,
            usage_missing: false,
            credit_usage: None,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: Some("abcdefghij".to_string()),
            created_at: now_ms() + 1,
        };

        runtime.append_usage_event(&key, &first).await.expect("append first");
        runtime.append_usage_event(&key, &second).await.expect("append second");

        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if store.count_usage_events(Some(&key.id)).await.expect("count") == 2 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("byte threshold should flush buffered events");

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&auths_dir);
    }
```

Update the compaction default expectation in `backend/src/handlers.rs` tests:

```rust
        assert_eq!(CompactionRuntimeConfig::default().scan_interval_seconds, 900);
        assert_eq!(CompactionRuntimeConfig::default().fragment_threshold, 128);
        assert_eq!(CompactionRuntimeConfig::default().prune_older_than_hours, 1);
```

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p static-flow-backend usage_events_flush_when_buffer_bytes_reach_limit -- --nocapture
cargo test -p static-flow-backend update_compaction_runtime_config_applies_partial_update -- --nocapture
```

Expected:

- The flush test fails because the flusher only uses fixed constants
- The compaction expectation fails once the assertion is updated but defaults are still old

- [ ] **Step 3: Make the usage flusher config-driven**

In `backend/src/llm_gateway/runtime.rs`, pass `runtime_config: Arc<RwLock<LlmGatewayRuntimeConfig>>` into `spawn_usage_event_flusher(...)` and replace the fixed constants with per-loop snapshots:

```rust
#[derive(Debug, Clone, Copy)]
struct UsageFlushConfig {
    batch_size: usize,
    flush_interval: Duration,
    max_buffer_bytes: usize,
}

fn usage_flush_config(runtime_config: &LlmGatewayRuntimeConfig) -> UsageFlushConfig {
    UsageFlushConfig {
        batch_size: runtime_config.usage_event_flush_batch_size.max(1) as usize,
        flush_interval: Duration::from_secs(runtime_config.usage_event_flush_interval_seconds.max(1)),
        max_buffer_bytes: runtime_config.usage_event_flush_max_buffer_bytes.max(1) as usize,
    }
}
```

Track buffered bytes with a helper:

```rust
fn estimate_usage_event_bytes(event: &LlmGatewayUsageEventRecord) -> usize {
    event.id.len()
        + event.key_id.len()
        + event.key_name.len()
        + event.provider_type.len()
        + event.account_name.as_deref().map_or(0, str::len)
        + event.request_method.len()
        + event.request_url.len()
        + event.endpoint.len()
        + event.model.as_deref().map_or(0, str::len)
        + event.client_ip.len()
        + event.ip_region.len()
        + event.request_headers_json.len()
        + event.last_message_content.as_deref().map_or(0, str::len)
}
```

- [ ] **Step 4: Upgrade the behavior flusher and compaction defaults**

In `backend/src/state.rs`:

- change compaction defaults to `900 / 128 / 1`
- replace `BEHAVIOR_FLUSH_BATCH_SIZE` and `flush_interval = 5s` with:

```rust
const DEFAULT_BEHAVIOR_FLUSH_BATCH_SIZE: usize = 256;
const DEFAULT_BEHAVIOR_FLUSH_INTERVAL_SECS: u64 = 15;
const DEFAULT_BEHAVIOR_FLUSH_MAX_BUFFER_BYTES: usize = 4 * 1024 * 1024;
```

and a byte estimator:

```rust
fn estimate_behavior_event_bytes(event: &NewApiBehaviorEventInput) -> usize {
    event.client_source.len()
        + event.method.len()
        + event.path.len()
        + event.query.as_deref().map_or(0, str::len)
        + event.page_path.as_deref().map_or(0, str::len)
        + event.referrer.as_deref().map_or(0, str::len)
        + event.client_ip.len()
        + event.ip_region.len()
        + event.ua_raw.len()
        + event.device_type.len()
        + event.os_family.len()
        + event.browser_family.len()
        + event.request_id.as_deref().map_or(0, str::len)
        + event.trace_id.as_deref().map_or(0, str::len)
}
```

Also update the mock compaction config in `frontend/src/api.rs`:

```rust
        Ok(CompactionRuntimeConfig {
            enabled: true,
            scan_interval_seconds: 900,
            fragment_threshold: 128,
            prune_older_than_hours: 1,
        })
```

- [ ] **Step 5: Re-run the focused tests**

Run:

```bash
cargo test -p static-flow-backend usage_events_flush_when_buffer_bytes_reach_limit -- --nocapture
cargo test -p static-flow-backend update_compaction_runtime_config_rejects_invalid_ranges -- --nocapture
cargo test -p static-flow-backend update_compaction_runtime_config_applies_partial_update -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit**

Run:

```bash
git add backend/src/llm_gateway/runtime.rs \
        backend/src/state.rs \
        backend/src/handlers.rs \
        frontend/src/api.rs
git commit -m "feat: batch hot event writes and raise compaction defaults"
```

---

### Task 5: Expose the new runtime-config controls in the admin UI

**Files:**
- Modify: `frontend/src/api.rs`
- Modify: `frontend/src/pages/admin_llm_gateway.rs`
- Test: `frontend/src/api.rs` (serde round-trip if present)

- [ ] **Step 1: Update the frontend runtime-config shape**

In `frontend/src/api.rs`, extend `LlmGatewayRuntimeConfig`:

```rust
pub struct LlmGatewayRuntimeConfig {
    pub auth_cache_ttl_seconds: u64,
    pub max_request_body_bytes: u64,
    pub account_failure_retry_limit: u64,
    pub codex_status_refresh_min_interval_seconds: u64,
    pub codex_status_refresh_max_interval_seconds: u64,
    pub codex_status_account_jitter_max_seconds: u64,
    pub kiro_status_refresh_min_interval_seconds: u64,
    pub kiro_status_refresh_max_interval_seconds: u64,
    pub kiro_status_account_jitter_max_seconds: u64,
    pub usage_event_flush_batch_size: u64,
    pub usage_event_flush_interval_seconds: u64,
    pub usage_event_flush_max_buffer_bytes: u64,
}
```

Update:

- `fetch_admin_llm_gateway_config()`
- `update_admin_llm_gateway_config(...)`

to send/receive the full expanded payload.

- [ ] **Step 2: Add dedicated form state for each new field**

In `frontend/src/pages/admin_llm_gateway.rs`, add state hooks next to the existing TTL/body/retry inputs:

```rust
let codex_refresh_min_input = use_state(|| "240".to_string());
let codex_refresh_max_input = use_state(|| "300".to_string());
let codex_jitter_input = use_state(|| "10".to_string());
let kiro_refresh_min_input = use_state(|| "240".to_string());
let kiro_refresh_max_input = use_state(|| "300".to_string());
let kiro_jitter_input = use_state(|| "10".to_string());
let usage_flush_batch_size_input = use_state(|| "256".to_string());
let usage_flush_interval_input = use_state(|| "15".to_string());
let usage_flush_max_buffer_bytes_input = use_state(|| (8 * 1024 * 1024_u64).to_string());
```

- [ ] **Step 3: Hydrate and save the expanded config**

Update the reload path to set all new inputs from `cfg`, then parse and send them in `on_save_runtime_config`:

```rust
match update_admin_llm_gateway_config(
    ttl,
    max_request_body_bytes,
    account_failure_retry_limit,
    codex_status_refresh_min_interval_seconds,
    codex_status_refresh_max_interval_seconds,
    codex_status_account_jitter_max_seconds,
    kiro_status_refresh_min_interval_seconds,
    kiro_status_refresh_max_interval_seconds,
    kiro_status_account_jitter_max_seconds,
    usage_event_flush_batch_size,
    usage_event_flush_interval_seconds,
    usage_event_flush_max_buffer_bytes,
)
.await
```

Use the same validation style already present:

- `"必须是非负整数"` for integer parse errors
- keep the backend as the source of truth for range validation

- [ ] **Step 4: Add the new controls to the runtime-config form**

Add three UI blocks after the existing body/retry controls:

```rust
<label class={classes!("flex", "flex-col", "gap-2")}>
    <span class={classes!("text-[var(--muted)]")}>{ "codex_status_refresh_window_seconds" }</span>
    <div class={classes!("grid", "gap-2", "md:grid-cols-2")}>
        <input
            value={(*codex_refresh_min_input).clone()}
            oninput={{
                let codex_refresh_min_input = codex_refresh_min_input.clone();
                Callback::from(move |event: InputEvent| {
                    if let Some(target) = event.target_dyn_into::<web_sys::HtmlInputElement>() {
                        codex_refresh_min_input.set(target.value());
                    }
                })
            }}
        />
        <input
            value={(*codex_refresh_max_input).clone()}
            oninput={{
                let codex_refresh_max_input = codex_refresh_max_input.clone();
                Callback::from(move |event: InputEvent| {
                    if let Some(target) = event.target_dyn_into::<web_sys::HtmlInputElement>() {
                        codex_refresh_max_input.set(target.value());
                    }
                })
            }}
        />
    </div>
</label>

<label class={classes!("flex", "flex-col", "gap-2")}>
    <span class={classes!("text-[var(--muted)]")}>{ "kiro_status_refresh_window_seconds" }</span>
    <div class={classes!("grid", "gap-2", "md:grid-cols-2")}>
        <input
            value={(*kiro_refresh_min_input).clone()}
            oninput={{
                let kiro_refresh_min_input = kiro_refresh_min_input.clone();
                Callback::from(move |event: InputEvent| {
                    if let Some(target) = event.target_dyn_into::<web_sys::HtmlInputElement>() {
                        kiro_refresh_min_input.set(target.value());
                    }
                })
            }}
        />
        <input
            value={(*kiro_refresh_max_input).clone()}
            oninput={{
                let kiro_refresh_max_input = kiro_refresh_max_input.clone();
                Callback::from(move |event: InputEvent| {
                    if let Some(target) = event.target_dyn_into::<web_sys::HtmlInputElement>() {
                        kiro_refresh_max_input.set(target.value());
                    }
                })
            }}
        />
    </div>
</label>

<label class={classes!("flex", "flex-col", "gap-2")}>
    <span class={classes!("text-[var(--muted)]")}>{ "usage_event_flush" }</span>
    <div class={classes!("grid", "gap-2", "md:grid-cols-3")}>
        <input
            value={(*usage_flush_batch_size_input).clone()}
            oninput={{
                let usage_flush_batch_size_input = usage_flush_batch_size_input.clone();
                Callback::from(move |event: InputEvent| {
                    if let Some(target) = event.target_dyn_into::<web_sys::HtmlInputElement>() {
                        usage_flush_batch_size_input.set(target.value());
                    }
                })
            }}
        />
        <input
            value={(*usage_flush_interval_input).clone()}
            oninput={{
                let usage_flush_interval_input = usage_flush_interval_input.clone();
                Callback::from(move |event: InputEvent| {
                    if let Some(target) = event.target_dyn_into::<web_sys::HtmlInputElement>() {
                        usage_flush_interval_input.set(target.value());
                    }
                })
            }}
        />
        <input
            value={(*usage_flush_max_buffer_bytes_input).clone()}
            oninput={{
                let usage_flush_max_buffer_bytes_input = usage_flush_max_buffer_bytes_input.clone();
                Callback::from(move |event: InputEvent| {
                    if let Some(target) = event.target_dyn_into::<web_sys::HtmlInputElement>() {
                        usage_flush_max_buffer_bytes_input.set(target.value());
                    }
                })
            }}
        />
    </div>
</label>
```

Add concise helper text under the form:

```rust
{ "默认 Codex/Kiro 状态刷新窗口为 240-300 秒，账号间抖动上限为 10 秒；usage event flush 默认 256 条 / 15 秒 / 8 MiB。" }
```

- [ ] **Step 5: Run the frontend build-level verification**

Run:

```bash
cargo check -p static-flow-frontend
```

If `wasm32-unknown-unknown` is installed, also run:

```bash
cargo clippy -p static-flow-frontend --target wasm32-unknown-unknown -- -D warnings
```

Expected: PASS.

- [ ] **Step 6: Commit**

Run:

```bash
git add frontend/src/api.rs frontend/src/pages/admin_llm_gateway.rs
git commit -m "feat: expose llm gateway polling controls in admin ui"
```

---

### Task 6: Final verification and maintenance prep

**Files:**
- Modify: none
- Verify: repository state and built artifacts

- [ ] **Step 1: Format only the changed Rust files**

Run:

```bash
rustfmt shared/src/llm_gateway_store/types.rs \
        shared/src/llm_gateway_store/schema.rs \
        shared/src/llm_gateway_store/codec.rs \
        shared/src/llm_gateway_store/mod.rs \
        backend/src/state.rs \
        backend/src/llm_gateway/types.rs \
        backend/src/llm_gateway.rs \
        backend/src/llm_gateway/runtime.rs \
        backend/src/llm_gateway/token_refresh.rs \
        backend/src/kiro_gateway/status_cache.rs \
        backend/src/kiro_gateway/mod.rs \
        backend/src/handlers.rs
```

Do not run `cargo fmt` in `deps/lance` or `deps/lancedb`, and do not run workspace-wide formatting.

- [ ] **Step 2: Run the shared/backend test suites most likely to regress**

Run:

```bash
cargo test -p static-flow-shared --lib
cargo test -p static-flow-backend --lib
```

Expected: PASS.

- [ ] **Step 3: Run clippy on the affected Rust crates**

Run:

```bash
cargo clippy -p static-flow-shared -p static-flow-backend --tests -- -D warnings
```

Expected: PASS with zero warnings.

- [ ] **Step 4: Build the CLI and backend artifacts used in maintenance**

Run:

```bash
cargo build -p sf-cli -p static-flow-backend
```

Expected: PASS.

- [ ] **Step 5: Record the maintenance commands for the actual outage**

Run:

```bash
target/debug/sf-cli db audit-storage --db-path /mnt/wsl/data4tb/static-flow-data/lancedb --table llm_gateway_usage_events
target/debug/sf-cli db audit-storage --db-path /mnt/wsl/data4tb/static-flow-data/lancedb --table api_behavior_events
target/debug/sf-cli db rebuild-table-stable --db-path /mnt/wsl/data4tb/static-flow-data/lancedb --table llm_gateway_usage_events --force --batch-size 256
target/debug/sf-cli db rebuild-table-stable --db-path /mnt/wsl/data4tb/static-flow-data/lancedb --table api_behavior_events --force --batch-size 256
target/debug/sf-cli db audit-storage --db-path /mnt/wsl/data4tb/static-flow-data/lancedb --table llm_gateway_usage_events
target/debug/sf-cli db audit-storage --db-path /mnt/wsl/data4tb/static-flow-data/lancedb --table api_behavior_events
```

Expected after rebuild:

- both tables report `stable_row_ids=true`
- fragments are materially reduced
- row counts are unchanged

- [ ] **Step 6: Commit the finished implementation branch state**

Run:

```bash
git status --short
```

Expected: only the intended implementation files are modified.

Then commit with a final integration message, for example:

```bash
git add shared/src/llm_gateway_store/types.rs \
        shared/src/llm_gateway_store/schema.rs \
        shared/src/llm_gateway_store/codec.rs \
        shared/src/llm_gateway_store/mod.rs \
        backend/src/state.rs \
        backend/src/llm_gateway/types.rs \
        backend/src/llm_gateway.rs \
        backend/src/llm_gateway/runtime.rs \
        backend/src/llm_gateway/token_refresh.rs \
        backend/src/kiro_gateway/status_cache.rs \
        backend/src/kiro_gateway/mod.rs \
        backend/src/handlers.rs \
        frontend/src/api.rs \
        frontend/src/pages/admin_llm_gateway.rs
git commit -m "feat: reduce lancedb memory pressure and randomize status polling"
```

## Self-Review

- Spec coverage:
  - hot table rebuild path is covered in Task 6 maintenance commands
  - polling randomization is covered in Task 2
  - usage exact totals are covered in Task 3
  - threshold-driven batching is covered in Task 4
  - admin configurability is covered in Tasks 1 and 5
  - compaction default changes are covered in Task 4
- Placeholder scan:
  - no `TODO`, `TBD`, or “handle appropriately” placeholders remain
- Type consistency:
  - the same runtime-config field names are used across shared, backend, and frontend tasks
