# Kiro Prefix Cache Simulation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Kiro's cache-read token estimation with a shared prefix-cache simulator plus exact history-anchor session recovery, while preserving formula mode as a compatibility fallback.

**Architecture:** Build a canonicalized prompt projection from the corrected `ConversationState`, then feed that projection into two shared in-memory structures: a global prefix tree for cache-hit simulation and a global anchor index for session recovery. Keep key-level participation as the guardrail, add provider-level runtime config for mode and capacity, and preserve the current formula path as an explicit compatibility mode.

**Tech Stack:** Rust, Axum, parking_lot, existing Kiro Anthropic conversion pipeline, LanceDB-backed runtime config, Yew admin UI, plus a mature third-party trie/LRU crate if one satisfies the required prefix-match + eviction semantics.

---

## File Structure

- Modify: `backend/src/kiro_gateway/anthropic/mod.rs`
  - Wire session resolution through the new anchor recovery path
  - Replace direct formula-only cache estimation with mode dispatch
  - Emit observability logs for anchor recovery and prefix-cache hits
- Modify: `backend/src/kiro_gateway/anthropic/converter.rs`
  - Keep session metadata compatibility extraction as a secondary source only
  - Reuse corrected `ConversationState` as the source for prompt projection inputs
- Create: `backend/src/kiro_gateway/cache_sim.rs`
  - Own canonical prompt projection, stable-prefix extraction, prefix-tree state, anchor-index state, and eviction logic
- Modify: `backend/src/kiro_gateway/runtime.rs`
  - Hold the shared simulator state inside Kiro runtime startup
- Modify: `backend/src/kiro_gateway/mod.rs`
  - Extend runtime structs / state plumbing used by Anthropic handlers
- Modify: `backend/src/state.rs`
  - Extend `LlmGatewayRuntimeConfig` with Kiro prefix-cache mode/capacity knobs and initialize runtime defaults
- Modify: `backend/src/llm_gateway.rs`
  - Validate and persist the new runtime-config fields via existing admin runtime-config API
- Modify: `backend/src/llm_gateway/types.rs`
  - Expose the new runtime-config fields in backend API structs
- Modify: `shared/src/llm_gateway_store/types.rs`
  - Persist provider-level Kiro prefix-cache config defaults
- Modify: `shared/src/llm_gateway_store/schema.rs`
  - Add runtime-config schema columns for prefix-cache mode and capacities
- Modify: `shared/src/llm_gateway_store/codec.rs`
  - Encode/decode the new runtime-config fields
- Modify: `shared/src/llm_gateway_store/mod.rs`
  - Add round-trip tests for the new runtime-config fields
- Modify: `frontend/src/api.rs`
  - Surface the new provider-level runtime-config fields
- Modify: `frontend/src/pages/admin_kiro_gateway.rs`
  - Add Kiro admin controls for mode and capacities
- Modify: `frontend/src/pages/admin_llm_gateway.rs`
  - Preserve the new runtime-config fields when saving the shared config payload

## Task 1: Lock Down Runtime Config And Public Surface

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

- [ ] **Step 1: Write failing runtime-config persistence tests**

Add tests that describe the exact new fields and defaults:

```rust
#[tokio::test]
async fn runtime_config_round_trip_preserves_kiro_prefix_cache_settings() {
    let store = temp_store().await;
    let config = LlmGatewayRuntimeConfigRecord {
        kiro_prefix_cache_mode: "prefix_tree".to_string(),
        kiro_prefix_cache_max_tokens: 4_000_000,
        kiro_prefix_cache_entry_ttl_seconds: 21_600,
        kiro_conversation_anchor_max_entries: 20_000,
        kiro_conversation_anchor_ttl_seconds: 86_400,
        ..LlmGatewayRuntimeConfigRecord::default()
    };

    store.upsert_runtime_config(&config).await.unwrap();
    let loaded = store.get_runtime_config_or_default().await.unwrap();

    assert_eq!(loaded.kiro_prefix_cache_mode, "prefix_tree");
    assert_eq!(loaded.kiro_prefix_cache_max_tokens, 4_000_000);
    assert_eq!(loaded.kiro_prefix_cache_entry_ttl_seconds, 21_600);
    assert_eq!(loaded.kiro_conversation_anchor_max_entries, 20_000);
    assert_eq!(loaded.kiro_conversation_anchor_ttl_seconds, 86_400);
}

#[test]
fn update_runtime_config_rejects_invalid_kiro_prefix_cache_ranges() {
    let current = LlmGatewayRuntimeConfig::default();
    let request = UpdateLlmGatewayRuntimeConfigRequest {
        kiro_prefix_cache_mode: Some("prefix_tree".to_string()),
        kiro_prefix_cache_max_tokens: Some(0),
        ..Default::default()
    };

    let result = apply_runtime_config_update(current, request);
    assert!(result.is_err());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p static-flow-shared runtime_config_round_trip_preserves_kiro_prefix_cache_settings -- --nocapture
cargo test -p static-flow-backend update_runtime_config_rejects_invalid_kiro_prefix_cache_ranges -- --nocapture
```

Expected:
- first test fails because runtime-config fields do not exist yet
- second test fails because backend request validation does not know these fields

- [ ] **Step 3: Add persisted runtime-config fields and defaults**

Add the following persisted fields to shared/backend runtime config:

```rust
pub const DEFAULT_KIRO_PREFIX_CACHE_MODE: &str = "formula";
pub const DEFAULT_KIRO_PREFIX_CACHE_MAX_TOKENS: u64 = 4_000_000;
pub const DEFAULT_KIRO_PREFIX_CACHE_ENTRY_TTL_SECONDS: u64 = 6 * 60 * 60;
pub const DEFAULT_KIRO_CONVERSATION_ANCHOR_MAX_ENTRIES: u64 = 20_000;
pub const DEFAULT_KIRO_CONVERSATION_ANCHOR_TTL_SECONDS: u64 = 24 * 60 * 60;

pub struct LlmGatewayRuntimeConfigRecord {
    pub kiro_prefix_cache_mode: String,
    pub kiro_prefix_cache_max_tokens: u64,
    pub kiro_prefix_cache_entry_ttl_seconds: u64,
    pub kiro_conversation_anchor_max_entries: u64,
    pub kiro_conversation_anchor_ttl_seconds: u64,
    // existing fields...
}
```

Mirror them through:
- LanceDB schema
- codec encode/decode
- backend runtime config
- admin API request/response structs

- [ ] **Step 4: Implement backend validation and update flow**

Extend runtime-config validation in `backend/src/llm_gateway.rs` with explicit checks:

```rust
match kiro_prefix_cache_mode.as_str() {
    "formula" | "prefix_tree" => {}
    _ => return Err(bad_request("kiro_prefix_cache_mode is invalid")),
}
if kiro_prefix_cache_max_tokens == 0 {
    return Err(bad_request("kiro_prefix_cache_max_tokens must be positive"));
}
if kiro_conversation_anchor_max_entries == 0 {
    return Err(bad_request("kiro_conversation_anchor_max_entries must be positive"));
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run:

```bash
cargo test -p static-flow-shared runtime_config_round_trip_preserves_kiro_prefix_cache_settings -- --nocapture
cargo test -p static-flow-backend update_runtime_config_rejects_invalid_kiro_prefix_cache_ranges -- --nocapture
```

Expected:
- both PASS

- [ ] **Step 6: Commit**

```bash
git add shared/src/llm_gateway_store/types.rs shared/src/llm_gateway_store/schema.rs shared/src/llm_gateway_store/codec.rs shared/src/llm_gateway_store/mod.rs backend/src/state.rs backend/src/llm_gateway/types.rs backend/src/llm_gateway.rs
git commit -m "feat: add kiro prefix cache runtime config"
```

## Task 2: Add Canonical Prompt Projection Tests First

**Files:**
- Create: `backend/src/kiro_gateway/cache_sim.rs`
- Test: `backend/src/kiro_gateway/cache_sim.rs`

- [ ] **Step 1: Write failing canonicalization and anchor tests**

Start the new module with tests that describe the real source-of-truth behavior:

```rust
#[test]
fn canonical_projection_excludes_current_turn_from_lookup_anchor() {
    let state = sample_conversation_state_with_history_and_current();
    let projection = PromptProjection::from_conversation_state(&state);

    assert_eq!(projection.lookup_anchor_hash, hash_history(["u1", "a1"]));
    assert_ne!(projection.lookup_anchor_hash, projection.resume_anchor_hash);
}

#[test]
fn canonical_projection_excludes_current_tool_results_from_stable_prefix_tokens() {
    let state = sample_conversation_state_with_current_tool_result();
    let projection = PromptProjection::from_conversation_state(&state);

    assert!(!projection.stable_prefix_tokens.ends_with(&tool_result_tokens()));
}

#[test]
fn canonical_projection_is_stable_for_equivalent_history() {
    let left = sample_equivalent_state_variant_a();
    let right = sample_equivalent_state_variant_b();

    let left_proj = PromptProjection::from_conversation_state(&left);
    let right_proj = PromptProjection::from_conversation_state(&right);

    assert_eq!(left_proj.lookup_anchor_hash, right_proj.lookup_anchor_hash);
    assert_eq!(left_proj.stable_prefix_tokens, right_proj.stable_prefix_tokens);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p static-flow-backend canonical_projection_ -- --nocapture
```

Expected:
- FAIL because `cache_sim.rs` and `PromptProjection` do not exist yet

- [ ] **Step 3: Add minimal prompt-projection types and canonicalizer**

Create `backend/src/kiro_gateway/cache_sim.rs` with the initial core API:

```rust
pub struct PromptProjection {
    pub lookup_anchor_hash: [u8; 32],
    pub resume_anchor_hash: [u8; 32],
    pub stable_prefix_tokens: Vec<u32>,
}

impl PromptProjection {
    pub fn from_conversation_state(state: &ConversationState) -> Self {
        // build canonical history-before-current
        // build canonical history-after-current-success
        // tokenize stable-prefix region only
    }
}
```

Keep the first implementation small and deterministic:
- canonicalize history/current using explicit helper functions
- no simulator state yet

- [ ] **Step 4: Run tests to verify they pass**

Run:

```bash
cargo test -p static-flow-backend canonical_projection_ -- --nocapture
```

Expected:
- PASS

- [ ] **Step 5: Commit**

```bash
git add backend/src/kiro_gateway/cache_sim.rs
git commit -m "feat: add kiro prompt projection canonicalizer"
```

## Task 3: Evaluate Third-Party Trie/LRU Crates And Add Simulator Skeleton

**Files:**
- Modify: `backend/Cargo.toml`
- Modify: `backend/src/kiro_gateway/cache_sim.rs`
- Test: `backend/src/kiro_gateway/cache_sim.rs`

- [ ] **Step 1: Write failing simulator-state tests**

Add tests that describe the data-structure contract, not the implementation:

```rust
#[test]
fn prefix_cache_simulator_reports_longest_shared_prefix_tokens() {
    let mut sim = PrefixCacheSimulator::new(test_limits());
    sim.record_success(vec![1, 2, 3, 4]);

    let matched = sim.match_prefix_tokens(&[1, 2, 3, 9]);
    assert_eq!(matched, 3);
}

#[test]
fn anchor_index_restores_exact_history_anchor_only() {
    let mut index = ConversationAnchorIndex::new(test_limits());
    index.record_success(hash("a"), "conv-1".to_string(), test_now());

    assert_eq!(index.lookup(&hash("a")), Some("conv-1".to_string()));
    assert_eq!(index.lookup(&hash("ab")), None);
}

#[test]
fn simulator_eviction_drops_oldest_entries_when_capacity_is_exceeded() {
    let mut sim = PrefixCacheSimulator::new(test_limits_with_small_capacity());
    sim.record_success(vec![1, 2, 3]);
    sim.record_success(vec![9, 9, 9]);

    assert!(sim.total_tokens() <= test_limits_with_small_capacity().max_tokens);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p static-flow-backend prefix_cache_simulator_ -- --nocapture
cargo test -p static-flow-backend anchor_index_ -- --nocapture
```

Expected:
- FAIL because simulator/index types do not exist yet

- [ ] **Step 3: Add crate evaluation note and choose implementation dependency**

Evaluate a trie/cache crate before writing custom structures. Document the chosen dependency in code comments or the module header. The target decision should look like:

```rust
// We evaluated mature trie crates first. Chosen implementation uses <crate>
// for prefix storage because it supports <needed capability>. LRU/TTL is
// layered separately because no off-the-shelf crate matched both semantics.
```

If no crate fits longest-prefix token matching cleanly, keep the trie minimal and self-implemented, but use a mature LRU structure (for example an LRU cache crate or equivalent) for eviction bookkeeping.

- [ ] **Step 4: Implement minimal simulator/index skeleton**

Add:

```rust
pub struct PrefixCacheLimits {
    pub max_tokens: usize,
    pub entry_ttl: Duration,
}

pub struct PrefixCacheSimulator { /* trie + leaf bookkeeping */ }
pub struct ConversationAnchorIndex { /* exact hash -> conversation_id */ }
```

Required behavior:
- exact-anchor lookup
- longest-prefix token match
- capacity accounting
- TTL/LRU eviction hooks

- [ ] **Step 5: Run tests to verify they pass**

Run:

```bash
cargo test -p static-flow-backend prefix_cache_simulator_ -- --nocapture
cargo test -p static-flow-backend anchor_index_ -- --nocapture
```

Expected:
- PASS

- [ ] **Step 6: Commit**

```bash
git add backend/Cargo.toml backend/src/kiro_gateway/cache_sim.rs
git commit -m "feat: add kiro prefix cache simulator state"
```

## Task 4: Plumb Shared Simulator State Into Kiro Runtime

**Files:**
- Modify: `backend/src/kiro_gateway/runtime.rs`
- Modify: `backend/src/kiro_gateway/mod.rs`
- Modify: `backend/src/state.rs`
- Test: `backend/src/kiro_gateway/provider.rs` or `backend/src/kiro_gateway/mod.rs`

- [ ] **Step 1: Write failing runtime-state tests**

Add tests that describe initialization and config propagation:

```rust
#[test]
fn kiro_runtime_state_initializes_prefix_cache_simulator_from_runtime_config() {
    let runtime = build_test_kiro_runtime_with_config(LlmGatewayRuntimeConfig {
        kiro_prefix_cache_mode: "prefix_tree".to_string(),
        kiro_prefix_cache_max_tokens: 1234,
        ..LlmGatewayRuntimeConfig::default()
    });

    assert_eq!(runtime.cache_simulator.read().limits().max_tokens, 1234);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test -p static-flow-backend kiro_runtime_state_initializes_prefix_cache_simulator_from_runtime_config -- --nocapture
```

Expected:
- FAIL because runtime state does not hold simulator/index yet

- [ ] **Step 3: Add shared simulator state to Kiro runtime**

Plumb through:
- `KiroGatewayRuntimeState`
- `AppState`
- runtime config snapshot / update readers

The runtime should expose shared handles similar to:

```rust
pub(crate) prefix_cache_simulator: Arc<RwLock<PrefixCacheSimulator>>,
pub(crate) conversation_anchor_index: Arc<RwLock<ConversationAnchorIndex>>,
```

- [ ] **Step 4: Run test to verify it passes**

Run:

```bash
cargo test -p static-flow-backend kiro_runtime_state_initializes_prefix_cache_simulator_from_runtime_config -- --nocapture
```

Expected:
- PASS

- [ ] **Step 5: Commit**

```bash
git add backend/src/kiro_gateway/runtime.rs backend/src/kiro_gateway/mod.rs backend/src/state.rs
git commit -m "feat: wire kiro prefix cache state into runtime"
```

## Task 5: Implement Exact Session Recovery Order

**Files:**
- Modify: `backend/src/kiro_gateway/anthropic/mod.rs`
- Modify: `backend/src/kiro_gateway/cache_sim.rs`
- Test: `backend/src/kiro_gateway/anthropic/mod.rs`

- [ ] **Step 1: Write failing session-recovery tests**

Add tests that describe the required order:

```rust
#[test]
fn resolve_request_session_uses_anchor_index_when_headers_and_metadata_are_missing() {
    let headers = HeaderMap::new();
    let metadata = None;
    let state = sample_state_with_anchor("lookup-hash", "conv-restored");

    let resolved = resolve_request_session(&headers, metadata, &state, &projection);

    assert_eq!(resolved.conversation_id, "conv-restored");
    assert_eq!(resolved.session_tracking.source, SessionIdSource::RecoveredAnchor);
}

#[test]
fn resolve_request_session_does_not_recover_when_anchor_hash_differs() {
    let headers = HeaderMap::new();
    let metadata = None;
    let state = sample_state_with_anchor("other-hash", "conv-restored");

    let resolved = resolve_request_session(&headers, metadata, &state, &projection);

    assert!(matches!(
        resolved.session_tracking.source,
        SessionIdSource::GeneratedFallback(_)
    ));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p static-flow-backend resolve_request_session_uses_anchor_index_when_headers_and_metadata_are_missing -- --nocapture
```

Expected:
- FAIL because anchor-based recovery is not implemented

- [ ] **Step 3: Implement session-recovery order**

Update the resolver so the order is:

```rust
1. explicit request headers
2. metadata compatibility sources
3. exact anchor-index recovery
4. generated fallback UUID
```

Add a new tracking source:

```rust
SessionIdSource::RecoveredAnchor
```

and log fields for:
- `lookup_anchor_hash`
- `recovered_conversation_id`
- `recovery_source=anchor_index`

- [ ] **Step 4: Run tests to verify they pass**

Run:

```bash
cargo test -p static-flow-backend resolve_request_session_ -- --nocapture
```

Expected:
- PASS

- [ ] **Step 5: Commit**

```bash
git add backend/src/kiro_gateway/anthropic/mod.rs backend/src/kiro_gateway/cache_sim.rs
git commit -m "feat: recover kiro sessions from exact history anchors"
```

## Task 6: Replace Formula-Only Cache Estimation With Mode Dispatch

**Files:**
- Modify: `backend/src/kiro_gateway/anthropic/mod.rs`
- Modify: `backend/src/kiro_gateway/cache_sim.rs`
- Test: `backend/src/kiro_gateway/anthropic/mod.rs`

- [ ] **Step 1: Write failing mode-dispatch tests**

Add tests describing the provider-level mode behavior:

```rust
#[test]
fn build_usage_summary_uses_prefix_tree_mode_when_enabled() {
    let summary = build_usage_summary(build_test_usage_input()
        .with_mode("prefix_tree")
        .with_prefix_match_tokens(4096));

    assert_eq!(summary.input_cached_tokens, 4096);
}

#[test]
fn build_usage_summary_keeps_formula_mode_as_compatibility_path() {
    let summary = build_usage_summary(build_test_usage_input()
        .with_mode("formula"));

    assert_eq!(summary.input_cached_tokens, expected_formula_estimate());
}

#[test]
fn build_usage_summary_returns_zero_cache_when_key_toggle_is_off() {
    let summary = build_usage_summary(build_test_usage_input()
        .with_mode("prefix_tree")
        .with_key_cache_enabled(false)
        .with_prefix_match_tokens(4096));

    assert_eq!(summary.input_cached_tokens, 0);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p static-flow-backend build_usage_summary_uses_prefix_tree_mode_when_enabled -- --nocapture
```

Expected:
- FAIL because build path is still formula-only

- [ ] **Step 3: Implement mode dispatch**

Refactor cache estimation into:

```rust
enum KiroCacheMode {
    Formula,
    PrefixTree,
}
```

and branch in usage summary building:
- if key toggle off -> zero cache
- if mode = formula -> existing estimate path
- if mode = prefix_tree -> simulator match path

- [ ] **Step 4: Run tests to verify they pass**

Run:

```bash
cargo test -p static-flow-backend build_usage_summary_uses_prefix_tree_mode_when_enabled -- --nocapture
cargo test -p static-flow-backend build_usage_summary_keeps_formula_mode_as_compatibility_path -- --nocapture
cargo test -p static-flow-backend build_usage_summary_returns_zero_cache_when_key_toggle_is_off -- --nocapture
```

Expected:
- PASS

- [ ] **Step 5: Commit**

```bash
git add backend/src/kiro_gateway/anthropic/mod.rs backend/src/kiro_gateway/cache_sim.rs
git commit -m "feat: add kiro prefix tree cache estimation mode"
```

## Task 7: Record Successful Requests Into Prefix Tree And Anchor Index

**Files:**
- Modify: `backend/src/kiro_gateway/anthropic/mod.rs`
- Modify: `backend/src/kiro_gateway/cache_sim.rs`
- Test: `backend/src/kiro_gateway/anthropic/mod.rs`

- [ ] **Step 1: Write failing persistence/recording tests**

Add tests that describe write timing:

```rust
#[test]
fn successful_request_records_resume_anchor_and_prefix_tokens() {
    let mut harness = test_success_harness();
    harness.finish_success();

    assert!(harness.anchor_index_contains("resume-hash"));
    assert!(harness.prefix_tree_contains(&[1, 2, 3]));
}

#[test]
fn failed_request_does_not_record_prefix_or_anchor() {
    let mut harness = test_failure_harness();
    harness.finish_failure();

    assert!(!harness.anchor_index_contains("resume-hash"));
    assert!(!harness.prefix_tree_contains(&[1, 2, 3]));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p static-flow-backend successful_request_records_resume_anchor_and_prefix_tokens -- --nocapture
```

Expected:
- FAIL because success/failure paths do not update the simulator state

- [ ] **Step 3: Implement success-only recording**

Record to shared state only after a successful request settles:

```rust
if key_record.kiro_cache_estimation_enabled && cache_mode == KiroCacheMode::PrefixTree {
    simulator.record_success(&projection.stable_prefix_tokens, now);
    anchors.record_success(projection.resume_anchor_hash, conversation_id.clone(), model.clone(), now);
}
```

Do not record anything on failure paths.

- [ ] **Step 4: Run tests to verify they pass**

Run:

```bash
cargo test -p static-flow-backend successful_request_records_resume_anchor_and_prefix_tokens -- --nocapture
cargo test -p static-flow-backend failed_request_does_not_record_prefix_or_anchor -- --nocapture
```

Expected:
- PASS

- [ ] **Step 5: Commit**

```bash
git add backend/src/kiro_gateway/anthropic/mod.rs backend/src/kiro_gateway/cache_sim.rs
git commit -m "feat: record successful kiro prefixes and anchors"
```

## Task 8: Add Kiro Admin UI Controls

**Files:**
- Modify: `frontend/src/api.rs`
- Modify: `frontend/src/pages/admin_kiro_gateway.rs`
- Modify: `frontend/src/pages/admin_llm_gateway.rs`
- Test: `frontend/src/pages/admin_kiro_gateway.rs`

- [ ] **Step 1: Write failing admin-UI tests**

Add tests for config serialization and UI rendering:

```rust
#[test]
fn admin_kiro_gateway_serializes_prefix_cache_runtime_fields() {
    let cfg = LlmGatewayRuntimeConfig {
        kiro_prefix_cache_mode: "prefix_tree".to_string(),
        kiro_prefix_cache_max_tokens: 4_000_000,
        kiro_conversation_anchor_max_entries: 20_000,
        ..LlmGatewayRuntimeConfig::default()
    };

    let json = serde_json::to_value(cfg).unwrap();
    assert_eq!(json["kiro_prefix_cache_mode"], "prefix_tree");
}

#[test]
fn admin_kiro_gateway_renders_prefix_cache_controls() {
    // render page
    // assert mode selector and capacity inputs are present
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p static-flow-frontend admin_kiro_gateway_ -- --nocapture
```

Expected:
- FAIL because API/UI fields do not exist

- [ ] **Step 3: Implement admin controls**

Add controls for:
- mode: `formula` / `prefix_tree`
- max tokens
- entry TTL
- anchor max entries
- anchor TTL

Keep the existing per-key boolean toggle as-is.

- [ ] **Step 4: Run tests to verify they pass**

Run:

```bash
cargo test -p static-flow-frontend admin_kiro_gateway_ -- --nocapture
cargo check -p static-flow-frontend --target wasm32-unknown-unknown
```

Expected:
- PASS
- frontend check succeeds

- [ ] **Step 5: Commit**

```bash
git add frontend/src/api.rs frontend/src/pages/admin_kiro_gateway.rs frontend/src/pages/admin_llm_gateway.rs
git commit -m "feat: add kiro prefix cache admin controls"
```

## Task 9: Full Verification

**Files:**
- Verify only

- [ ] **Step 1: Run targeted backend tests**

Run:

```bash
cargo test -p static-flow-backend kiro_gateway::anthropic::tests -- --nocapture
cargo test -p static-flow-backend kiro_gateway::anthropic::converter::tests -- --nocapture
cargo test -p static-flow-backend canonical_projection_ -- --nocapture
cargo test -p static-flow-backend prefix_cache_simulator_ -- --nocapture
cargo test -p static-flow-backend anchor_index_ -- --nocapture
```

Expected:
- PASS

- [ ] **Step 2: Run frontend verification**

Run:

```bash
cargo test -p static-flow-frontend admin_kiro_gateway_ -- --nocapture
cargo check -p static-flow-frontend --target wasm32-unknown-unknown
```

Expected:
- PASS

- [ ] **Step 3: Run clippy**

Run:

```bash
cargo clippy -p static-flow-backend -- -D warnings
cargo clippy -p static-flow-frontend --target wasm32-unknown-unknown -- -D warnings
```

Expected:
- PASS with zero warnings

- [ ] **Step 4: Format changed files**

Run:

```bash
rustfmt backend/src/kiro_gateway/cache_sim.rs backend/src/kiro_gateway/anthropic/mod.rs backend/src/kiro_gateway/anthropic/converter.rs backend/src/kiro_gateway/runtime.rs backend/src/kiro_gateway/mod.rs backend/src/state.rs backend/src/llm_gateway.rs backend/src/llm_gateway/types.rs shared/src/llm_gateway_store/types.rs shared/src/llm_gateway_store/schema.rs shared/src/llm_gateway_store/codec.rs shared/src/llm_gateway_store/mod.rs
cargo fmt -p static-flow-frontend -- frontend/src/api.rs frontend/src/pages/admin_kiro_gateway.rs frontend/src/pages/admin_llm_gateway.rs
```

Expected:
- no formatting diffs remain

- [ ] **Step 5: Final commit**

```bash
git add backend/src/kiro_gateway/cache_sim.rs backend/src/kiro_gateway/anthropic/mod.rs backend/src/kiro_gateway/anthropic/converter.rs backend/src/kiro_gateway/runtime.rs backend/src/kiro_gateway/mod.rs backend/src/state.rs backend/src/llm_gateway.rs backend/src/llm_gateway/types.rs shared/src/llm_gateway_store/types.rs shared/src/llm_gateway_store/schema.rs shared/src/llm_gateway_store/codec.rs shared/src/llm_gateway_store/mod.rs frontend/src/api.rs frontend/src/pages/admin_kiro_gateway.rs frontend/src/pages/admin_llm_gateway.rs
git commit -m "feat: simulate kiro prefix cache and recover sessions"
```

## Self-Review

- **Spec coverage:** covered runtime-config persistence, canonicalization, shared prefix tree, exact anchor recovery, key/provider toggles, admin UI, observability, and verification. No spec section is left without a task.
- **Placeholder scan:** removed generic "handle edge cases" language; every task points to exact files, commands, and expected behavior.
- **Type consistency:** plan consistently uses `PromptProjection`, `PrefixCacheSimulator`, `ConversationAnchorIndex`, `lookup_anchor_hash`, and `resume_anchor_hash`. Provider-level mode remains `formula | prefix_tree` throughout.
