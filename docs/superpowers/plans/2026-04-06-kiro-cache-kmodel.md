# Kiro Conservative Cache Estimation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn Kiro's always-zero cache usage into a conservative Anthropic-compatible `cache_read_input_tokens` estimate, persist the same estimate in usage events, expose per-model `Kmodel` in `/admin/kiro-gateway`, and add a reusable skill for re-calibrating defaults from the last 30 days of successful samples.

**Architecture:** Extend the existing global LLM gateway runtime-config row with a JSON-backed Kiro `Kmodel` map, add one pure estimator in the Kiro gateway that derives a conservative cache-read lower bound from `credit_usage`, dual input-token sources, and output tokens, and route both protocol responses and persisted usage events through that same estimator. Keep offline calibration separate as a skill that reports recommended coefficients without mutating production config.

**Tech Stack:** Rust (Axum backend, Serde, LanceDB-backed runtime config, Yew/WASM frontend), markdown skill docs, targeted `cargo test`, `cargo clippy`, `cargo fmt -p static-flow-backend -p static-flow-frontend`.

---

## File Map

- `backend/src/kiro_gateway/anthropic/mod.rs`
  - Current Kiro Anthropic-compatible response generation
  - Replace hard-coded zero cache usage with conservative estimator output
- `backend/src/kiro_gateway/mod.rs`
  - Kiro usage event persistence path
  - Route persisted `input_cached_tokens` / `input_uncached_tokens` through the same estimator
- `backend/src/kiro_gateway/types.rs`
  - Admin Kiro runtime-facing response types if the Kiro page needs a dedicated config view
- `backend/src/kiro_gateway/runtime.rs`
  - Access to shared runtime config inside Kiro runtime
- `backend/src/state.rs`
  - In-memory `LlmGatewayRuntimeConfig`
  - Default values loaded at startup
- `backend/src/llm_gateway.rs`
  - Admin runtime config GET/POST handlers
  - Validation and persistence for new Kiro `Kmodel` field
- `backend/src/llm_gateway/types.rs`
  - Runtime config request/response DTOs
- `shared/src/llm_gateway_store/types.rs`
  - Stored runtime-config row shape
- `shared/src/llm_gateway_store/schema.rs`
  - Runtime-config table schema migration
- `shared/src/llm_gateway_store/codec.rs`
  - Arrow encode/decode of runtime-config rows
- `frontend/src/api.rs`
  - Runtime-config API types and fetch/update helpers
- `frontend/src/pages/admin_kiro_gateway.rs`
  - New Kmodel editor panel on the Kiro admin page
  - Must preserve existing uncommitted card-collapsing work already in this file
- `skills/kiro-kmodel-calibrator/SKILL.md`
  - New skill documenting the reproducible 30-day calibration workflow

---

### Task 1: Add red tests for conservative Kiro cache estimation

**Files:**
- Modify: `backend/src/kiro_gateway/anthropic/mod.rs`
- Modify: `backend/src/kiro_gateway/mod.rs`
- Test: `backend/src/kiro_gateway/anthropic/mod.rs`
- Test: `backend/src/kiro_gateway/mod.rs`

- [ ] **Step 1: Add a failing unit test for dual-source conservative input selection**

Add a test near the existing Kiro Anthropic tests:

```rust
    #[test]
    fn estimate_cache_prefers_smaller_non_zero_input_source() {
        let estimate = estimate_kiro_cache_usage(KiroCacheEstimateInput {
            model: "claude-opus-4-6",
            request_input_tokens: 12_000,
            context_input_tokens: Some(9_000),
            output_tokens: 400,
            credit_usage: Some(0.02),
            kmodels: default_kiro_cache_kmodels(),
        });

        assert_eq!(estimate.input_tokens_total, 9_000);
        assert!(estimate.input_cached_tokens <= 9_000);
        assert_eq!(
            estimate.input_uncached_tokens + estimate.input_cached_tokens,
            estimate.input_tokens_total
        );
    }
```

- [ ] **Step 2: Add a failing unit test for zero-cache fallback when observed credit is too high**

```rust
    #[test]
    fn estimate_cache_returns_zero_when_credit_exceeds_safe_full_cost() {
        let estimate = estimate_kiro_cache_usage(KiroCacheEstimateInput {
            model: "claude-sonnet-4-6",
            request_input_tokens: 6_000,
            context_input_tokens: Some(5_000),
            output_tokens: 200,
            credit_usage: Some(10.0),
            kmodels: default_kiro_cache_kmodels(),
        });

        assert_eq!(estimate.input_cached_tokens, 0);
        assert_eq!(estimate.input_uncached_tokens, 5_000);
    }
```

- [ ] **Step 3: Add a failing persistence-path test proving usage events must carry the same estimate**

In `backend/src/kiro_gateway/mod.rs`, add a test that builds one `KiroUsageSummary`, runs the event builder, and asserts:

```rust
        assert_eq!(record.input_cached_tokens, expected_cached);
        assert_eq!(record.input_uncached_tokens, expected_uncached);
        assert_eq!(
            record.input_cached_tokens + record.input_uncached_tokens,
            expected_total_input
        );
```

- [ ] **Step 4: Run the narrow tests and confirm they fail for the right reason**

Run:

```bash
cargo test -p static-flow-backend estimate_cache_prefers_smaller_non_zero_input_source -- --nocapture
cargo test -p static-flow-backend estimate_cache_returns_zero_when_credit_exceeds_safe_full_cost -- --nocapture
cargo test -p static-flow-backend build_kiro_usage_event_record -- --nocapture
```

Expected:

- The estimator tests fail because no estimator exists yet
- The event-record test fails because cached tokens are still hard-coded to zero

- [ ] **Step 5: Commit the red tests**

```bash
git add backend/src/kiro_gateway/anthropic/mod.rs backend/src/kiro_gateway/mod.rs
git commit -m "test: cover kiro conservative cache estimation"
```

---

### Task 2: Implement backend conservative estimator and wire it into protocol + persistence

**Files:**
- Modify: `backend/src/kiro_gateway/anthropic/mod.rs`
- Modify: `backend/src/kiro_gateway/mod.rs`
- Modify: `backend/src/state.rs`
- Test: `backend/src/kiro_gateway/anthropic/mod.rs`
- Test: `backend/src/kiro_gateway/mod.rs`

- [ ] **Step 1: Add a focused estimator type and helpers in `anthropic/mod.rs`**

Add one pure data model plus helpers near the existing usage helpers:

```rust
#[derive(Debug, Clone)]
struct KiroCacheEstimateInput<'a> {
    model: &'a str,
    request_input_tokens: i32,
    context_input_tokens: Option<i32>,
    output_tokens: i32,
    credit_usage: Option<f64>,
    kmodels: &'a BTreeMap<String, f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KiroCacheEstimate {
    input_tokens_total: i32,
    input_uncached_tokens: i32,
    input_cached_tokens: i32,
}

fn normalize_kiro_kmodel_name(model: &str) -> &str {
    match model {
        "claude-opus-4.6" => "claude-opus-4-6",
        _ => model,
    }
}
```

- [ ] **Step 2: Implement the conservative estimation formula**

Write one pure function:

```rust
fn estimate_kiro_cache_usage(input: KiroCacheEstimateInput<'_>) -> KiroCacheEstimate {
    let request_input = input.request_input_tokens.max(0);
    let context_input = input.context_input_tokens.unwrap_or_default().max(0);
    let safe_input = match (request_input > 0, context_input > 0) {
        (true, true) => request_input.min(context_input),
        (true, false) => request_input,
        (false, true) => context_input,
        (false, false) => 0,
    };
    let output_tokens = input.output_tokens.max(0);
    let Some(observed_credit) = input.credit_usage.filter(|value| value.is_finite() && *value >= 0.0) else {
        return KiroCacheEstimate {
            input_tokens_total: safe_input,
            input_uncached_tokens: safe_input,
            input_cached_tokens: 0,
        };
    };
    let model_name = normalize_kiro_kmodel_name(input.model);
    let Some(kmodel) = input.kmodels.get(model_name).copied().filter(|value| value.is_finite() && *value > 0.0) else {
        return KiroCacheEstimate {
            input_tokens_total: safe_input,
            input_uncached_tokens: safe_input,
            input_cached_tokens: 0,
        };
    };

    let safe_full_cost = kmodel * (safe_input as f64 + 5.0 * output_tokens as f64);
    if !safe_full_cost.is_finite() || safe_full_cost <= observed_credit || safe_input <= 0 {
        return KiroCacheEstimate {
            input_tokens_total: safe_input,
            input_uncached_tokens: safe_input,
            input_cached_tokens: 0,
        };
    }

    let cached = ((safe_full_cost - observed_credit) / (0.9 * kmodel)).floor();
    let cached = cached.max(0.0).min(safe_input as f64) as i32;
    KiroCacheEstimate {
        input_tokens_total: safe_input,
        input_uncached_tokens: safe_input - cached,
        input_cached_tokens: cached,
    }
}
```

- [ ] **Step 3: Route non-streaming and streaming Kiro responses through the estimator**

Replace the current zero-cache writes in all Kiro response builders:

```rust
let estimate = estimate_kiro_cache_usage(KiroCacheEstimateInput {
    model: &request_ctx.model,
    request_input_tokens: request_ctx.input_tokens,
    context_input_tokens,
    output_tokens,
    credit_usage: credit_usage_observed.then_some(credit_usage.max(0.0)),
    kmodels: &state.llm_gateway_runtime_config.read().kiro_cache_kmodels,
});

let usage = KiroUsageSummary {
    input_uncached_tokens: estimate.input_uncached_tokens,
    input_cached_tokens: estimate.input_cached_tokens,
    output_tokens,
    credit_usage: credit_usage_observed.then_some(credit_usage.max(0.0)),
    credit_usage_missing: !credit_usage_observed,
};
```

And keep the protocol response self-consistent:

```rust
"usage": {
    "input_tokens": usage.input_uncached_tokens + usage.input_cached_tokens,
    "output_tokens": usage.output_tokens,
    "cache_creation_input_tokens": 0,
    "cache_read_input_tokens": usage.input_cached_tokens,
}
```

- [ ] **Step 4: Keep persisted usage events aligned**

In `backend/src/kiro_gateway/mod.rs`, do not recompute a different value. Use the already-estimated `KiroUsageSummary` as the source of truth so persisted rows match protocol responses exactly.

- [ ] **Step 5: Run the backend tests and make them pass**

Run:

```bash
cargo test -p static-flow-backend kiro_gateway::anthropic::tests -- --nocapture
cargo test -p static-flow-backend kiro_gateway::tests -- --nocapture
```

Expected: PASS

- [ ] **Step 6: Commit the backend estimator**

```bash
git add backend/src/kiro_gateway/anthropic/mod.rs backend/src/kiro_gateway/mod.rs
git commit -m "feat: estimate conservative kiro cache usage"
```

---

### Task 3: Extend runtime config with per-model Kmodel defaults and admin persistence

**Files:**
- Modify: `shared/src/llm_gateway_store/types.rs`
- Modify: `shared/src/llm_gateway_store/schema.rs`
- Modify: `shared/src/llm_gateway_store/codec.rs`
- Modify: `backend/src/state.rs`
- Modify: `backend/src/llm_gateway/types.rs`
- Modify: `backend/src/llm_gateway.rs`
- Test: `shared/src/llm_gateway_store/mod.rs`
- Test: `backend/src/llm_gateway.rs`

- [ ] **Step 1: Add stored/runtime config fields for Kiro Kmodel JSON plus parsed map**

Extend stored record:

```rust
pub struct LlmGatewayRuntimeConfigRecord {
    // existing fields...
    pub kiro_cache_kmodels_json: String,
    pub updated_at: i64,
}
```

Extend runtime struct:

```rust
pub struct LlmGatewayRuntimeConfig {
    // existing fields...
    pub kiro_cache_kmodels_json: String,
    pub kiro_cache_kmodels: BTreeMap<String, f64>,
}
```

Add one default constructor helper:

```rust
pub fn default_kiro_cache_kmodels() -> BTreeMap<String, f64> {
    BTreeMap::from([
        ("claude-opus-4-6".to_string(), 8.061927916785985e-06),
        ("claude-sonnet-4-6".to_string(), 5.055065250835128e-06),
        ("claude-haiku-4-5-20251001".to_string(), 2.3681034438052206e-06),
    ])
}
```

- [ ] **Step 2: Add JSON encode/decode helpers with strict validation**

Add one parser:

```rust
fn parse_kiro_cache_kmodels_json(value: &str) -> Result<BTreeMap<String, f64>> {
    let map: BTreeMap<String, f64> = serde_json::from_str(value)?;
    if map.is_empty() {
        anyhow::bail!("kiro_cache_kmodels_json must not be empty");
    }
    for (model, coeff) in &map {
        if model.trim().is_empty() || !coeff.is_finite() || *coeff <= 0.0 {
            anyhow::bail!("invalid kiro cache kmodel entry for `{model}`");
        }
    }
    Ok(map)
}
```

Use `serde_json::to_string(&default_kiro_cache_kmodels())` for the built-in default row.

- [ ] **Step 3: Extend admin runtime-config DTOs and validation**

In `backend/src/llm_gateway/types.rs`:

```rust
pub struct LlmGatewayRuntimeConfigResponse {
    // existing fields...
    pub kiro_cache_kmodels_json: String,
}

pub struct UpdateLlmGatewayRuntimeConfigRequest {
    // existing fields...
    pub kiro_cache_kmodels_json: Option<String>,
}
```

In `backend/src/llm_gateway.rs`, validate and persist:

```rust
let kiro_cache_kmodels_json = request
    .kiro_cache_kmodels_json
    .clone()
    .unwrap_or_else(|| current.kiro_cache_kmodels_json.clone());
let kiro_cache_kmodels =
    parse_kiro_cache_kmodels_json(&kiro_cache_kmodels_json)
        .map_err(|_| bad_request("kiro_cache_kmodels_json is invalid"))?;
```

- [ ] **Step 4: Add runtime-config round-trip tests**

Add tests that:

- missing field uses built-in default JSON
- valid JSON round-trips through storage
- invalid JSON returns `400`

Run:

```bash
cargo test -p static-flow-backend update_runtime_config -- --nocapture
cargo test -p static-flow-shared runtime_config -- --nocapture
```

- [ ] **Step 5: Commit the runtime-config changes**

```bash
git add shared/src/llm_gateway_store/types.rs shared/src/llm_gateway_store/schema.rs shared/src/llm_gateway_store/codec.rs backend/src/state.rs backend/src/llm_gateway/types.rs backend/src/llm_gateway.rs
git commit -m "feat: add configurable kiro cache kmodels"
```

---

### Task 4: Expose Kmodel editing on `/admin/kiro-gateway`

**Files:**
- Modify: `frontend/src/api.rs`
- Modify: `frontend/src/pages/admin_kiro_gateway.rs`
- Test: `frontend/src/pages/admin_kiro_gateway.rs`

- [ ] **Step 1: Extend frontend runtime-config type and fetch/update mock data**

In `frontend/src/api.rs`:

```rust
pub struct LlmGatewayRuntimeConfig {
    // existing fields...
    pub kiro_cache_kmodels_json: String,
}
```

Update the mock response to include:

```rust
kiro_cache_kmodels_json: "{\"claude-opus-4-6\":8.061927916785985e-06,\"claude-sonnet-4-6\":5.055065250835128e-06,\"claude-haiku-4-5-20251001\":2.3681034438052206e-06}".to_string(),
```

- [ ] **Step 2: Add one Kmodel config panel to the Kiro admin page**

On `frontend/src/pages/admin_kiro_gateway.rs`, load runtime config alongside the existing account/key payloads and add a compact editor block:

```rust
let runtime_config = use_state(|| None::<LlmGatewayRuntimeConfig>);
let kiro_cache_kmodels_json = use_state(String::new);
```

Render:

```rust
<div class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-4", "py-4", "space-y-3")}>
    <div class={classes!("text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>
        { "Kiro Cache Kmodels" }
    </div>
    <textarea
        class={classes!("min-h-[12rem]", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "font-mono", "text-xs")}
        value={(*kiro_cache_kmodels_json).clone()}
        oninput={...}
    />
    <div class={classes!("text-xs", "text-[var(--muted)]")}>
        { "每个模型一个正数系数；cache_read_input_tokens 会使用这组值做保守下界估计。" }
    </div>
    <button type="button" class={classes!("btn-terminal")} onclick={on_save_kiro_cache_kmodels}>
        { "Save Kmodels" }
    </button>
</div>
```

- [ ] **Step 3: Preserve the current uncommitted key-card collapsing work**

Do not rewrite `frontend/src/pages/admin_kiro_gateway.rs` wholesale. Layer the new config panel around the existing page state and keep the current card-collapsing/candidate-credit diff intact.

- [ ] **Step 4: Add one frontend test for default JSON visibility**

Add a page-level helper test that ensures the Kmodel editor initializes from runtime config and preserves the default JSON string.

- [ ] **Step 5: Run frontend verification**

Run:

```bash
cargo test -p static-flow-frontend pages::admin_kiro_gateway::tests -- --nocapture
cargo check -p static-flow-frontend --target wasm32-unknown-unknown
cargo clippy -p static-flow-frontend --target wasm32-unknown-unknown -- -D warnings
```

- [ ] **Step 6: Commit the frontend UI**

```bash
git add frontend/src/api.rs frontend/src/pages/admin_kiro_gateway.rs
git commit -m "feat: expose kiro cache kmodels in admin"
```

---

### Task 5: Add the calibration skill and end-to-end verification

**Files:**
- Create: `skills/kiro-kmodel-calibrator/SKILL.md`
- Modify: `backend/src/kiro_gateway/anthropic/mod.rs`
- Modify: `backend/src/kiro_gateway/mod.rs`
- Modify: `backend/src/llm_gateway.rs`
- Modify: `shared/src/llm_gateway_store/types.rs`
- Modify: `frontend/src/api.rs`
- Modify: `frontend/src/pages/admin_kiro_gateway.rs`

- [ ] **Step 1: Create the new skill document**

Write `skills/kiro-kmodel-calibrator/SKILL.md` with:

```markdown
---
name: kiro-kmodel-calibrator
description: >-
  Recompute conservative per-model Kiro cache Kmodels from the last 30 days of
  successful Kiro usage events and produce a human-reviewable recommendation report.
---

# Kiro Kmodel Calibrator

## Goal

Derive conservative per-model `Kmodel` defaults from the last 30 days of
successful Kiro usage events.

## Query

provider_type = 'kiro' AND status_code = 200 AND credit_usage_missing = false

## Method

For each model:

1. Treat `input_uncached_tokens` as total input for Kiro historical rows
2. Compute `ratio = credit_usage / (Tin + 5 * Tout)`
3. Filter to `Tin <= 200000`
4. Report sample count, p50, p80, p90
5. Recommend `p80` as `Kmodel`
```

- [ ] **Step 2: Run complete backend verification**

Run:

```bash
cargo test -p static-flow-backend kiro_gateway::anthropic::tests -- --nocapture
cargo test -p static-flow-backend kiro_gateway::tests -- --nocapture
cargo test -p static-flow-backend update_runtime_config -- --nocapture
cargo clippy -p static-flow-backend -- -D warnings
cargo fmt -p static-flow-backend -- backend/src/kiro_gateway/anthropic/mod.rs backend/src/kiro_gateway/mod.rs backend/src/llm_gateway.rs backend/src/llm_gateway/types.rs backend/src/state.rs
```

- [ ] **Step 3: Run shared/frontend verification**

Run:

```bash
cargo test -p static-flow-shared llm_gateway_store -- --nocapture
cargo clippy -p static-flow-shared -- -D warnings
cargo check -p static-flow-frontend --target wasm32-unknown-unknown
cargo clippy -p static-flow-frontend --target wasm32-unknown-unknown -- -D warnings
cargo fmt -p static-flow-frontend -- frontend/src/api.rs frontend/src/pages/admin_kiro_gateway.rs
```

- [ ] **Step 4: Commit the skill and final integration**

```bash
git add skills/kiro-kmodel-calibrator/SKILL.md backend/src/kiro_gateway/anthropic/mod.rs backend/src/kiro_gateway/mod.rs backend/src/llm_gateway.rs backend/src/llm_gateway/types.rs backend/src/state.rs shared/src/llm_gateway_store/types.rs shared/src/llm_gateway_store/schema.rs shared/src/llm_gateway_store/codec.rs frontend/src/api.rs frontend/src/pages/admin_kiro_gateway.rs
git commit -m "feat: add conservative kiro cache kmodel configuration"
```

## Self-Review

- Spec coverage: covers protocol-field estimation, persisted usage alignment, runtime config defaults, admin editing UI, and calibration skill
- Placeholder scan: no TBD/TODO placeholders remain; each task lists file targets, code direction, and commands
- Type consistency: the plan consistently uses `kiro_cache_kmodels_json`, `default_kiro_cache_kmodels`, `estimate_kiro_cache_usage`, and `/admin/kiro-gateway` as the UI surface
