# Kiro Cache Policy Overrides Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Kiro's hard-coded cache protection thresholds with a global default cache policy plus per-key field-level overrides, expose both in `/admin/kiro-gateway`, and keep default runtime behavior byte-for-byte compatible for keys that inherit the global policy.

**Architecture:** Introduce one shared pure policy model for default values, JSON parsing, merging, validation, and band interpolation; persist only raw JSON in LanceDB; keep the backend runtime as the source of truth by resolving one effective policy per key before Kiro request execution; and let the frontend edit structured forms while sending only global JSON or minimal key override JSON back to the backend. The implementation keeps `Kmodel` global, routes all policy-sensitive Kiro logic through one backend helper module, and preserves existing behavior whenever no new override fields are present.

**Tech Stack:** Rust (Axum backend, Serde, LanceDB-backed shared storage, Yew/WASM frontend), targeted `cargo test`, `cargo clippy`, and per-file `rustfmt`.

---

## File Structure

- Create: `shared/src/llm_gateway_store/kiro_cache_policy.rs`
  - Pure data model for Kiro cache policy/defaults/override merge/validation/interpolation
- Modify: `shared/src/llm_gateway_store/mod.rs`
  - Re-export new shared policy types/helpers
- Modify: `shared/src/llm_gateway_store/types.rs`
  - Persist raw JSON fields on runtime config and Kiro keys
- Modify: `shared/src/llm_gateway_store/schema.rs`
  - Add new LanceDB UTF-8 columns
- Modify: `shared/src/llm_gateway_store/codec.rs`
  - Encode/decode the new JSON columns with correct fallback behavior
- Modify: `shared/src/llm_gateway_store/mod.rs`
  - Add persistence round-trip tests for the new fields
- Modify: `backend/src/state.rs`
  - Add parsed global policy to in-memory runtime config and load it at startup
- Modify: `backend/src/llm_gateway/types.rs`
  - Extend runtime-config request/response DTOs with `kiro_cache_policy_json`
- Modify: `backend/src/llm_gateway.rs`
  - Validate/persist global policy JSON through the admin runtime-config API
- Create: `backend/src/kiro_gateway/cache_policy.rs`
  - Backend-only helpers for resolving effective policy, threshold checks, and policy-driven Kiro cache math
- Modify: `backend/src/kiro_gateway/mod.rs`
  - Store key override JSON, handle three-state patch semantics, and use policy threshold for request-body capture
- Modify: `backend/src/kiro_gateway/types.rs`
  - Add admin Kiro key view fields and patch request field for override JSON
- Modify: `backend/src/kiro_gateway/anthropic/mod.rs`
  - Replace hard-coded cache-protection logic with policy-driven helpers
- Modify: `frontend/src/api.rs`
  - Surface new runtime config and Kiro key fields, including three-state patch support
- Modify: `frontend/src/pages/admin_kiro_gateway.rs`
  - Add structured global-policy editor and per-key override editor/summaries
- Modify: `frontend/src/pages/admin_llm_gateway.rs`
  - Preserve `kiro_cache_policy_json` when that page saves shared runtime config

---

### Task 1: Add the shared Kiro cache policy model and its red tests

**Files:**
- Create: `shared/src/llm_gateway_store/kiro_cache_policy.rs`
- Modify: `shared/src/llm_gateway_store/mod.rs`
- Test: `shared/src/llm_gateway_store/kiro_cache_policy.rs`

- [ ] **Step 1: Write failing unit tests that lock in the current hard-coded behavior**

Create `shared/src/llm_gateway_store/kiro_cache_policy.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_matches_current_hard_coded_thresholds() {
        let policy = default_kiro_cache_policy();

        assert_eq!(policy.small_input_high_credit_boost.target_input_tokens, 100_000);
        assert_eq!(policy.small_input_high_credit_boost.credit_start, 1.0);
        assert_eq!(policy.small_input_high_credit_boost.credit_end, 1.8);
        assert_eq!(policy.prefix_tree_credit_ratio_bands.len(), 2);
        assert_eq!(policy.prefix_tree_credit_ratio_bands[0].credit_start, 0.3);
        assert_eq!(policy.prefix_tree_credit_ratio_bands[0].credit_end, 1.0);
        assert_eq!(policy.prefix_tree_credit_ratio_bands[0].cache_ratio_start, 0.7);
        assert_eq!(policy.prefix_tree_credit_ratio_bands[0].cache_ratio_end, 0.2);
        assert_eq!(policy.prefix_tree_credit_ratio_bands[1].credit_start, 1.0);
        assert_eq!(policy.prefix_tree_credit_ratio_bands[1].credit_end, 2.5);
        assert_eq!(policy.prefix_tree_credit_ratio_bands[1].cache_ratio_start, 0.2);
        assert_eq!(policy.prefix_tree_credit_ratio_bands[1].cache_ratio_end, 0.0);
        assert_eq!(policy.high_credit_diagnostic_threshold, 2.0);
    }

    #[test]
    fn merge_override_keeps_unspecified_fields_from_global_policy() {
        let global = default_kiro_cache_policy();
        let merged = merge_kiro_cache_policy(
            &global,
            Some(&KiroCachePolicyOverride {
                small_input_high_credit_boost: Some(KiroSmallInputHighCreditBoostOverride {
                    target_input_tokens: Some(80_000),
                    credit_start: None,
                    credit_end: None,
                }),
                prefix_tree_credit_ratio_bands: None,
                high_credit_diagnostic_threshold: Some(1.4),
            }),
        )
        .expect("override should merge");

        assert_eq!(merged.small_input_high_credit_boost.target_input_tokens, 80_000);
        assert_eq!(merged.small_input_high_credit_boost.credit_start, 1.0);
        assert_eq!(merged.small_input_high_credit_boost.credit_end, 1.8);
        assert_eq!(merged.prefix_tree_credit_ratio_bands, global.prefix_tree_credit_ratio_bands);
        assert_eq!(merged.high_credit_diagnostic_threshold, 1.4);
    }

    #[test]
    fn validate_policy_rejects_overlapping_credit_bands() {
        let err = validate_kiro_cache_policy(&KiroCachePolicy {
            prefix_tree_credit_ratio_bands: vec![
                KiroCreditRatioBand {
                    credit_start: 0.3,
                    credit_end: 1.0,
                    cache_ratio_start: 0.7,
                    cache_ratio_end: 0.2,
                },
                KiroCreditRatioBand {
                    credit_start: 0.9,
                    credit_end: 2.0,
                    cache_ratio_start: 0.2,
                    cache_ratio_end: 0.0,
                },
            ],
            ..default_kiro_cache_policy()
        })
        .expect_err("overlapping bands must fail");

        assert!(err.to_string().contains("overlap"));
    }

    #[test]
    fn interpolate_prefix_tree_ratio_matches_current_curve_points() {
        let policy = default_kiro_cache_policy();

        assert_eq!(interpolate_prefix_tree_cache_ratio(&policy, Some(0.3)), Some(0.7));
        assert_eq!(interpolate_prefix_tree_cache_ratio(&policy, Some(0.65)), Some(0.45));
        assert_eq!(interpolate_prefix_tree_cache_ratio(&policy, Some(1.0)), Some(0.2));
        assert_eq!(interpolate_prefix_tree_cache_ratio(&policy, Some(1.75)), Some(0.1));
        assert_eq!(interpolate_prefix_tree_cache_ratio(&policy, Some(2.5)), Some(0.0));
    }
}
```

- [ ] **Step 2: Run the shared tests and verify they fail because the module does not exist yet**

Run:

```bash
cargo test -p static-flow-shared default_policy_matches_current_hard_coded_thresholds -- --nocapture
cargo test -p static-flow-shared merge_override_keeps_unspecified_fields_from_global_policy -- --nocapture
cargo test -p static-flow-shared validate_policy_rejects_overlapping_credit_bands -- --nocapture
cargo test -p static-flow-shared interpolate_prefix_tree_ratio_matches_current_curve_points -- --nocapture
```

Expected:

- All four tests fail to compile because `shared/src/llm_gateway_store/kiro_cache_policy.rs` and the named helpers/types do not exist yet

- [ ] **Step 3: Implement the pure shared policy module**

Add the new file with concrete types and helpers:

```rust
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KiroCachePolicy {
    pub small_input_high_credit_boost: KiroSmallInputHighCreditBoostPolicy,
    pub prefix_tree_credit_ratio_bands: Vec<KiroCreditRatioBand>,
    pub high_credit_diagnostic_threshold: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KiroSmallInputHighCreditBoostPolicy {
    pub target_input_tokens: u64,
    pub credit_start: f64,
    pub credit_end: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KiroCreditRatioBand {
    pub credit_start: f64,
    pub credit_end: f64,
    pub cache_ratio_start: f64,
    pub cache_ratio_end: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct KiroCachePolicyOverride {
    #[serde(default)]
    pub small_input_high_credit_boost: Option<KiroSmallInputHighCreditBoostOverride>,
    #[serde(default)]
    pub prefix_tree_credit_ratio_bands: Option<Vec<KiroCreditRatioBand>>,
    #[serde(default)]
    pub high_credit_diagnostic_threshold: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct KiroSmallInputHighCreditBoostOverride {
    #[serde(default)]
    pub target_input_tokens: Option<u64>,
    #[serde(default)]
    pub credit_start: Option<f64>,
    #[serde(default)]
    pub credit_end: Option<f64>,
}
```

Implement the core helpers in the same file:

```rust
pub fn default_kiro_cache_policy() -> KiroCachePolicy {
    KiroCachePolicy {
        small_input_high_credit_boost: KiroSmallInputHighCreditBoostPolicy {
            target_input_tokens: 100_000,
            credit_start: 1.0,
            credit_end: 1.8,
        },
        prefix_tree_credit_ratio_bands: vec![
            KiroCreditRatioBand {
                credit_start: 0.3,
                credit_end: 1.0,
                cache_ratio_start: 0.7,
                cache_ratio_end: 0.2,
            },
            KiroCreditRatioBand {
                credit_start: 1.0,
                credit_end: 2.5,
                cache_ratio_start: 0.2,
                cache_ratio_end: 0.0,
            },
        ],
        high_credit_diagnostic_threshold: 2.0,
    }
}

pub fn default_kiro_cache_policy_json() -> String {
    serde_json::to_string(&default_kiro_cache_policy())
        .expect("default kiro cache policy should serialize")
}

pub fn parse_kiro_cache_policy_json(value: &str) -> Result<KiroCachePolicy> {
    let policy: KiroCachePolicy = serde_json::from_str(value)?;
    validate_kiro_cache_policy(&policy)?;
    Ok(policy)
}

pub fn parse_kiro_cache_policy_override_json(value: &str) -> Result<KiroCachePolicyOverride> {
    let override_policy: KiroCachePolicyOverride = serde_json::from_str(value)?;
    validate_kiro_cache_policy_override(&override_policy)?;
    Ok(override_policy)
}
```

Then add merge/validation/interpolation:

```rust
pub fn merge_kiro_cache_policy(
    base: &KiroCachePolicy,
    override_policy: Option<&KiroCachePolicyOverride>,
) -> Result<KiroCachePolicy> {
    let Some(override_policy) = override_policy else {
        return Ok(base.clone());
    };

    let mut merged = base.clone();
    if let Some(boost) = override_policy.small_input_high_credit_boost.as_ref() {
        if let Some(value) = boost.target_input_tokens {
            merged.small_input_high_credit_boost.target_input_tokens = value;
        }
        if let Some(value) = boost.credit_start {
            merged.small_input_high_credit_boost.credit_start = value;
        }
        if let Some(value) = boost.credit_end {
            merged.small_input_high_credit_boost.credit_end = value;
        }
    }
    if let Some(bands) = override_policy.prefix_tree_credit_ratio_bands.clone() {
        merged.prefix_tree_credit_ratio_bands = bands;
    }
    if let Some(value) = override_policy.high_credit_diagnostic_threshold {
        merged.high_credit_diagnostic_threshold = value;
    }
    validate_kiro_cache_policy(&merged)?;
    Ok(merged)
}

pub fn interpolate_prefix_tree_cache_ratio(
    policy: &KiroCachePolicy,
    credit_usage: Option<f64>,
) -> Option<f64> {
    let observed_credit = credit_usage.filter(|value| value.is_finite())?;
    let first = policy.prefix_tree_credit_ratio_bands.first()?;
    if observed_credit < first.credit_start {
        return None;
    }
    for band in &policy.prefix_tree_credit_ratio_bands {
        if observed_credit <= band.credit_end {
            let progress =
                ((observed_credit - band.credit_start) / (band.credit_end - band.credit_start))
                    .clamp(0.0, 1.0);
            return Some(
                band.cache_ratio_start
                    + (band.cache_ratio_end - band.cache_ratio_start) * progress,
            );
        }
    }
    policy
        .prefix_tree_credit_ratio_bands
        .last()
        .map(|band| band.cache_ratio_end)
}

pub fn validate_kiro_cache_policy_override(
    override_policy: &KiroCachePolicyOverride,
) -> Result<()> {
    if let Some(boost) = override_policy.small_input_high_credit_boost.as_ref() {
        if let Some(value) = boost.target_input_tokens {
            if value == 0 {
                return Err(anyhow!("target_input_tokens must be positive"));
            }
        }
        if let (Some(start), Some(end)) = (boost.credit_start, boost.credit_end) {
            if !start.is_finite() || !end.is_finite() || start >= end {
                return Err(anyhow!("small_input_high_credit_boost credit range is invalid"));
            }
        }
    }
    if let Some(value) = override_policy.high_credit_diagnostic_threshold {
        if !value.is_finite() || value < 0.0 {
            return Err(anyhow!("high_credit_diagnostic_threshold must be finite and >= 0"));
        }
    }
    if let Some(bands) = override_policy.prefix_tree_credit_ratio_bands.as_ref() {
        validate_kiro_cache_policy(&KiroCachePolicy {
            prefix_tree_credit_ratio_bands: bands.clone(),
            ..default_kiro_cache_policy()
        })?;
    }
    Ok(())
}
```

- [ ] **Step 4: Re-export the new shared policy module**

In `shared/src/llm_gateway_store/mod.rs`, add the module and re-exports:

```rust
mod kiro_cache_policy;
mod codec;
mod schema;
mod types;

pub use self::kiro_cache_policy::{
    default_kiro_cache_policy, default_kiro_cache_policy_json,
    interpolate_prefix_tree_cache_ratio, merge_kiro_cache_policy,
    parse_kiro_cache_policy_json, parse_kiro_cache_policy_override_json,
    validate_kiro_cache_policy, KiroCachePolicy, KiroCachePolicyOverride,
    KiroCreditRatioBand, KiroSmallInputHighCreditBoostOverride,
    KiroSmallInputHighCreditBoostPolicy,
};
```

- [ ] **Step 5: Run the shared tests again and verify they pass**

Run:

```bash
cargo test -p static-flow-shared default_policy_matches_current_hard_coded_thresholds -- --nocapture
cargo test -p static-flow-shared merge_override_keeps_unspecified_fields_from_global_policy -- --nocapture
cargo test -p static-flow-shared validate_policy_rejects_overlapping_credit_bands -- --nocapture
cargo test -p static-flow-shared interpolate_prefix_tree_ratio_matches_current_curve_points -- --nocapture
```

Expected:

- All four tests PASS

- [ ] **Step 6: Commit the shared policy model**

```bash
git add shared/src/llm_gateway_store/kiro_cache_policy.rs shared/src/llm_gateway_store/mod.rs
git commit -m "feat: add shared kiro cache policy model"
```

---

### Task 2: Persist global policy JSON and key override JSON through storage and backend APIs

**Files:**
- Modify: `shared/src/llm_gateway_store/types.rs`
- Modify: `shared/src/llm_gateway_store/schema.rs`
- Modify: `shared/src/llm_gateway_store/codec.rs`
- Modify: `shared/src/llm_gateway_store/mod.rs`
- Modify: `backend/src/state.rs`
- Modify: `backend/src/llm_gateway/types.rs`
- Modify: `backend/src/llm_gateway.rs`
- Modify: `backend/src/kiro_gateway/types.rs`
- Modify: `backend/src/kiro_gateway/mod.rs`
- Test: `shared/src/llm_gateway_store/mod.rs`
- Test: `backend/src/llm_gateway.rs`
- Test: `backend/src/kiro_gateway/types.rs`

- [ ] **Step 1: Add failing persistence and DTO tests**

In `shared/src/llm_gateway_store/mod.rs`, add round-trip tests:

```rust
#[tokio::test]
async fn runtime_config_round_trip_preserves_kiro_cache_policy_json() {
    let dir = temp_store_dir("runtime-config-kiro-cache-policy");
    let store = LlmGatewayStore::connect(dir.path().to_str().unwrap()).await.unwrap();
    let config = LlmGatewayRuntimeConfigRecord {
        kiro_cache_policy_json: r#"{"small_input_high_credit_boost":{"target_input_tokens":80000,"credit_start":0.9,"credit_end":1.6},"prefix_tree_credit_ratio_bands":[{"credit_start":0.2,"credit_end":0.8,"cache_ratio_start":0.6,"cache_ratio_end":0.3}],"high_credit_diagnostic_threshold":1.4}"#.to_string(),
        ..LlmGatewayRuntimeConfigRecord::default()
    };

    store.upsert_runtime_config(&config).await.unwrap();
    let loaded = store.get_runtime_config_or_default().await.unwrap();

    assert_eq!(loaded.kiro_cache_policy_json, config.kiro_cache_policy_json);
}

#[tokio::test]
async fn key_round_trip_preserves_kiro_cache_policy_override_json() {
    let dir = temp_store_dir("key-kiro-cache-policy-override");
    let store = LlmGatewayStore::connect(dir.path().to_str().unwrap()).await.unwrap();
    let key = LlmGatewayKeyRecord {
        id: "kiro-key".to_string(),
        name: "Kiro".to_string(),
        secret: "secret".to_string(),
        key_hash: "hash".to_string(),
        status: "active".to_string(),
        provider_type: "kiro".to_string(),
        protocol_family: "anthropic".to_string(),
        public_visible: false,
        quota_billable_limit: 1,
        usage_input_uncached_tokens: 0,
        usage_input_cached_tokens: 0,
        usage_output_tokens: 0,
        usage_billable_tokens: 0,
        usage_credit_total: 0.0,
        usage_credit_missing_events: 0,
        last_used_at: None,
        created_at: 0,
        updated_at: 0,
        route_strategy: None,
        fixed_account_name: None,
        auto_account_names: None,
        account_group_id: None,
        model_name_map: None,
        request_max_concurrency: None,
        request_min_start_interval_ms: None,
        kiro_request_validation_enabled: true,
        kiro_cache_estimation_enabled: true,
        kiro_cache_policy_override_json: Some(r#"{"high_credit_diagnostic_threshold":1.2}"#.to_string()),
    };

    store.create_key(&key).await.unwrap();
    let loaded = store.get_key_by_id_for_provider("kiro-key", "kiro").await.unwrap().unwrap();

    assert_eq!(
        loaded.kiro_cache_policy_override_json.as_deref(),
        Some(r#"{"high_credit_diagnostic_threshold":1.2}"#)
    );
}
```

In `backend/src/llm_gateway.rs`, add validation coverage:

```rust
#[test]
fn update_runtime_config_rejects_invalid_kiro_cache_policy_json() {
    let current = LlmGatewayRuntimeConfig::default();
    let request = UpdateLlmGatewayRuntimeConfigRequest {
        kiro_cache_policy_json: Some(r#"{"prefix_tree_credit_ratio_bands":[{"credit_start":1.0,"credit_end":0.5,"cache_ratio_start":0.2,"cache_ratio_end":0.1}]}"#.to_string()),
        ..UpdateLlmGatewayRuntimeConfigRequest::default()
    };

    let result = apply_runtime_config_update(current, request);
    assert!(result.is_err());
}
```

In `backend/src/kiro_gateway/types.rs`, add the three-state patch test:

```rust
#[test]
fn patch_kiro_key_request_distinguishes_absent_null_and_value_for_policy_override() {
    let absent: PatchKiroKeyRequest = serde_json::from_str("{}").unwrap();
    let clear: PatchKiroKeyRequest =
        serde_json::from_str(r#"{"kiro_cache_policy_override_json":null}"#).unwrap();
    let set: PatchKiroKeyRequest = serde_json::from_str(
        r#"{"kiro_cache_policy_override_json":"{\"high_credit_diagnostic_threshold\":1.6}"}"#,
    )
    .unwrap();

    assert!(absent.kiro_cache_policy_override_json.is_none());
    assert_eq!(clear.kiro_cache_policy_override_json, Some(None));
    assert_eq!(
        set.kiro_cache_policy_override_json,
        Some(Some(r#"{"high_credit_diagnostic_threshold":1.6}"#.to_string()))
    );
}
```

- [ ] **Step 2: Run the tests and verify they fail because the new fields do not exist**

Run:

```bash
cargo test -p static-flow-shared runtime_config_round_trip_preserves_kiro_cache_policy_json -- --nocapture
cargo test -p static-flow-shared key_round_trip_preserves_kiro_cache_policy_override_json -- --nocapture
cargo test -p static-flow-backend update_runtime_config_rejects_invalid_kiro_cache_policy_json -- --nocapture
cargo test -p static-flow-backend patch_kiro_key_request_distinguishes_absent_null_and_value_for_policy_override -- --nocapture
```

Expected:

- Shared tests fail because the persisted fields and schema columns do not exist
- Backend tests fail because runtime config and Kiro patch DTOs do not expose the new field names

- [ ] **Step 3: Add the new persisted JSON fields and LanceDB columns**

Update `shared/src/llm_gateway_store/types.rs`:

```rust
pub struct LlmGatewayKeyRecord {
    // existing fields...
    pub kiro_request_validation_enabled: bool,
    pub kiro_cache_estimation_enabled: bool,
    pub kiro_cache_policy_override_json: Option<String>,
}

pub struct LlmGatewayRuntimeConfigRecord {
    // existing fields...
    pub kiro_cache_kmodels_json: String,
    pub kiro_cache_policy_json: String,
    pub kiro_prefix_cache_mode: String,
    // existing fields...
}

impl Default for LlmGatewayRuntimeConfigRecord {
    fn default() -> Self {
        Self {
            // existing fields...
            kiro_cache_kmodels_json: default_kiro_cache_kmodels_json(),
            kiro_cache_policy_json: default_kiro_cache_policy_json(),
            kiro_prefix_cache_mode: DEFAULT_KIRO_PREFIX_CACHE_MODE.to_string(),
            // existing fields...
        }
    }
}
```

Update `shared/src/llm_gateway_store/schema.rs` and `codec.rs` to add:

```rust
Field::new("kiro_cache_policy_override_json", DataType::Utf8, true),
Field::new("kiro_cache_policy_json", DataType::Utf8, false),
```

and decode with fallback:

```rust
kiro_cache_policy_json: kiro_cache_policy_json
    .and_then(|column| value_string_opt(column, idx))
    .unwrap_or_else(default_kiro_cache_policy_json),
kiro_cache_policy_override_json: kiro_cache_policy_override_json
    .and_then(|column| value_string_opt(column, idx)),
```

- [ ] **Step 4: Thread the new JSON fields through backend runtime config and Kiro admin APIs**

Update `backend/src/state.rs`:

```rust
pub struct LlmGatewayRuntimeConfig {
    // existing fields...
    pub kiro_cache_kmodels_json: String,
    pub kiro_cache_kmodels: BTreeMap<String, f64>,
    pub kiro_cache_policy_json: String,
    pub kiro_cache_policy: KiroCachePolicy,
    pub kiro_prefix_cache_mode: String,
    // existing fields...
}
```

Load the parsed global policy at startup:

```rust
let kiro_cache_policy_json = llm_gateway_runtime_config_record.kiro_cache_policy_json;
let kiro_cache_policy = parse_kiro_cache_policy_json(&kiro_cache_policy_json)
    .unwrap_or_else(|err| {
        tracing::warn!("invalid stored kiro cache policy json, falling back to defaults: {err:#}");
        default_kiro_cache_policy()
    });
```

Extend the runtime-config DTOs in `backend/src/llm_gateway/types.rs`:

```rust
pub struct LlmGatewayRuntimeConfigResponse {
    // existing fields...
    pub kiro_cache_kmodels_json: String,
    pub kiro_cache_policy_json: String,
    pub kiro_prefix_cache_mode: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct UpdateLlmGatewayRuntimeConfigRequest {
    // existing fields...
    pub kiro_cache_kmodels_json: Option<String>,
    pub kiro_cache_policy_json: Option<String>,
    pub kiro_prefix_cache_mode: Option<String>,
}
```

Extend `backend/src/kiro_gateway/types.rs` with three-state patch semantics:

```rust
pub struct AdminKiroKeyView {
    // existing fields...
    pub kiro_cache_policy_override_json: Option<String>,
    pub effective_kiro_cache_policy_json: String,
    pub uses_global_kiro_cache_policy: bool,
}

pub struct PatchKiroKeyRequest {
    // existing fields...
    #[serde(default)]
    pub kiro_cache_policy_override_json: Option<Option<String>>,
}
```

- [ ] **Step 5: Implement runtime-config update validation and Kiro key patch handling**

In `backend/src/llm_gateway.rs`, validate and persist global JSON:

```rust
fn apply_runtime_config_update(
    current: LlmGatewayRuntimeConfig,
    request: UpdateLlmGatewayRuntimeConfigRequest,
) -> Result<LlmGatewayRuntimeConfig, (StatusCode, Json<ErrorResponse>)> {
    let kiro_cache_policy_json = request
        .kiro_cache_policy_json
        .unwrap_or_else(|| current.kiro_cache_policy_json.clone());
    let kiro_cache_policy = parse_kiro_cache_policy_json(&kiro_cache_policy_json)
        .map_err(|_| bad_request("kiro_cache_policy_json is invalid"))?;

    Ok(LlmGatewayRuntimeConfig {
        kiro_cache_policy_json,
        kiro_cache_policy,
        ..current
    })
}

let kiro_cache_policy_json = request
    .kiro_cache_policy_json
    .unwrap_or_else(|| current.kiro_cache_policy_json.clone());
let kiro_cache_policy = parse_kiro_cache_policy_json(&kiro_cache_policy_json)
    .map_err(|_| bad_request("kiro_cache_policy_json is invalid"))?;

let config = LlmGatewayRuntimeConfigRecord {
    // existing fields...
    kiro_cache_policy_json: kiro_cache_policy_json.clone(),
    ..current_record
};

*runtime = LlmGatewayRuntimeConfig {
    // existing fields...
    kiro_cache_policy_json,
    kiro_cache_policy,
    ..runtime.clone()
};
```

In `backend/src/kiro_gateway/mod.rs`, apply the key patch field:

```rust
if let Some(policy_override_json) = request.kiro_cache_policy_override_json {
    key.kiro_cache_policy_override_json = match policy_override_json {
        None => None,
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                None
            } else {
                parse_kiro_cache_policy_override_json(trimmed)
                    .map_err(|err| bad_request(&format!("kiro_cache_policy_override_json is invalid: {err}")))?;
                Some(trimmed.to_string())
            }
        },
    };
}
```

- [ ] **Step 6: Run the tests again and verify they pass**

Run:

```bash
cargo test -p static-flow-shared runtime_config_round_trip_preserves_kiro_cache_policy_json -- --nocapture
cargo test -p static-flow-shared key_round_trip_preserves_kiro_cache_policy_override_json -- --nocapture
cargo test -p static-flow-backend update_runtime_config_rejects_invalid_kiro_cache_policy_json -- --nocapture
cargo test -p static-flow-backend patch_kiro_key_request_distinguishes_absent_null_and_value_for_policy_override -- --nocapture
```

Expected:

- All four tests PASS

- [ ] **Step 7: Commit the persistence and API plumbing**

```bash
git add shared/src/llm_gateway_store/types.rs shared/src/llm_gateway_store/schema.rs shared/src/llm_gateway_store/codec.rs shared/src/llm_gateway_store/mod.rs backend/src/state.rs backend/src/llm_gateway/types.rs backend/src/llm_gateway.rs backend/src/kiro_gateway/types.rs backend/src/kiro_gateway/mod.rs
git commit -m "feat: persist kiro cache policy config"
```

---

### Task 3: Replace hard-coded Kiro cache math with effective per-key policies

**Files:**
- Create: `backend/src/kiro_gateway/cache_policy.rs`
- Modify: `backend/src/kiro_gateway/mod.rs`
- Modify: `backend/src/kiro_gateway/anthropic/mod.rs`
- Test: `backend/src/kiro_gateway/cache_policy.rs`
- Test: `backend/src/kiro_gateway/anthropic/mod.rs`
- Test: `backend/src/kiro_gateway/mod.rs`

- [ ] **Step 1: Write failing backend tests for effective policy resolution and policy-driven math**

Create `backend/src/kiro_gateway/cache_policy.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::LlmGatewayRuntimeConfig;

    fn sample_runtime() -> LlmGatewayRuntimeConfig {
        LlmGatewayRuntimeConfig {
            kiro_cache_policy_json: default_kiro_cache_policy_json(),
            kiro_cache_policy: default_kiro_cache_policy(),
            ..LlmGatewayRuntimeConfig::default()
        }
    }

    fn sample_key(override_json: Option<&str>) -> LlmGatewayKeyRecord {
        LlmGatewayKeyRecord {
            id: "key".to_string(),
            name: "key".to_string(),
            secret: "secret".to_string(),
            key_hash: "hash".to_string(),
            status: "active".to_string(),
            provider_type: "kiro".to_string(),
            protocol_family: "anthropic".to_string(),
            public_visible: false,
            quota_billable_limit: 1,
            usage_input_uncached_tokens: 0,
            usage_input_cached_tokens: 0,
            usage_output_tokens: 0,
            usage_billable_tokens: 0,
            usage_credit_total: 0.0,
            usage_credit_missing_events: 0,
            last_used_at: None,
            created_at: 0,
            updated_at: 0,
            route_strategy: None,
            fixed_account_name: None,
            auto_account_names: None,
            account_group_id: None,
            model_name_map: None,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            kiro_request_validation_enabled: true,
            kiro_cache_estimation_enabled: true,
            kiro_cache_policy_override_json: override_json.map(ToString::to_string),
        }
    }

    #[test]
    fn effective_policy_uses_key_override_for_only_changed_fields() {
        let runtime = sample_runtime();
        let key = sample_key(Some(
            r#"{"small_input_high_credit_boost":{"target_input_tokens":80000},"high_credit_diagnostic_threshold":1.6}"#,
        ));

        let effective = resolve_effective_kiro_cache_policy(&runtime, &key).unwrap();

        assert_eq!(effective.small_input_high_credit_boost.target_input_tokens, 80_000);
        assert_eq!(effective.small_input_high_credit_boost.credit_start, 1.0);
        assert_eq!(effective.small_input_high_credit_boost.credit_end, 1.8);
        assert_eq!(effective.high_credit_diagnostic_threshold, 1.6);
        assert_eq!(effective.prefix_tree_credit_ratio_bands.len(), 2);
    }

    #[test]
    fn should_capture_full_kiro_request_bodies_uses_effective_threshold() {
        let runtime = sample_runtime();
        let key = sample_key(Some(r#"{"high_credit_diagnostic_threshold":1.2}"#));
        let effective = resolve_effective_kiro_cache_policy(&runtime, &key).unwrap();

        assert!(should_capture_full_kiro_request_bodies(&effective, Some(1.3)));
        assert!(!should_capture_full_kiro_request_bodies(&effective, Some(1.1)));
    }
}
```

In `backend/src/kiro_gateway/anthropic/mod.rs`, add policy-driven math tests:

```rust
#[test]
fn build_usage_summary_uses_policy_override_for_boost_target() {
    let mut simulation = sample_simulation(KiroCacheSimulationMode::PrefixTree, u64::MAX);
    simulation.effective_cache_policy = merge_kiro_cache_policy(
        &default_kiro_cache_policy(),
        Some(&KiroCachePolicyOverride {
            small_input_high_credit_boost: Some(KiroSmallInputHighCreditBoostOverride {
                target_input_tokens: Some(80_000),
                credit_start: Some(1.0),
                credit_end: Some(1.8),
            }),
            ..KiroCachePolicyOverride::default()
        }),
    )
    .unwrap();

    let summary = build_kiro_usage_summary(
        "claude-opus-4-6",
        12_000,
        Some(50_000),
        400,
        Some(1.8),
        true,
        &simulation,
    );

    assert_eq!(summary.input_uncached_tokens + summary.input_cached_tokens, 80_000);
}

#[test]
fn build_usage_summary_uses_policy_override_for_prefix_tree_bands() {
    let mut simulation = sample_simulation(KiroCacheSimulationMode::PrefixTree, u64::MAX);
    simulation.effective_cache_policy = merge_kiro_cache_policy(
        &default_kiro_cache_policy(),
        Some(&KiroCachePolicyOverride {
            prefix_tree_credit_ratio_bands: Some(vec![KiroCreditRatioBand {
                credit_start: 0.4,
                credit_end: 1.4,
                cache_ratio_start: 0.5,
                cache_ratio_end: 0.1,
            }]),
            ..KiroCachePolicyOverride::default()
        }),
    )
    .unwrap();

    let summary = build_kiro_usage_summary(
        "claude-opus-4-6",
        12_000,
        Some(100_000),
        400,
        Some(0.9),
        true,
        &simulation,
    );

    assert_eq!(summary.input_cached_tokens, 30_000);
    assert_eq!(summary.input_uncached_tokens, 70_000);
}
```

- [ ] **Step 2: Run the backend tests and verify they fail because no effective-policy helpers exist**

Run:

```bash
cargo test -p static-flow-backend effective_policy_uses_key_override_for_only_changed_fields -- --nocapture
cargo test -p static-flow-backend should_capture_full_kiro_request_bodies_uses_effective_threshold -- --nocapture
cargo test -p static-flow-backend build_usage_summary_uses_policy_override_for_boost_target -- --nocapture
cargo test -p static-flow-backend build_usage_summary_uses_policy_override_for_prefix_tree_bands -- --nocapture
```

Expected:

- All four tests fail to compile because `backend/src/kiro_gateway/cache_policy.rs` does not exist and `KiroSimulationRequestContext` has no effective policy field

- [ ] **Step 3: Implement backend helpers for resolving and using effective policy**

Create `backend/src/kiro_gateway/cache_policy.rs`:

```rust
use anyhow::Result;
use static_flow_shared::llm_gateway_store::{
    interpolate_prefix_tree_cache_ratio, merge_kiro_cache_policy,
    parse_kiro_cache_policy_override_json, KiroCachePolicy, LlmGatewayKeyRecord,
};

use crate::state::LlmGatewayRuntimeConfig;

pub(crate) fn resolve_effective_kiro_cache_policy(
    runtime: &LlmGatewayRuntimeConfig,
    key: &LlmGatewayKeyRecord,
) -> Result<KiroCachePolicy> {
    let override_policy = key
        .kiro_cache_policy_override_json
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(parse_kiro_cache_policy_override_json)
        .transpose()?;
    merge_kiro_cache_policy(&runtime.kiro_cache_policy, override_policy.as_ref())
}

pub(crate) fn uses_global_kiro_cache_policy(key: &LlmGatewayKeyRecord) -> bool {
    key.kiro_cache_policy_override_json
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
}

pub(crate) fn should_capture_full_kiro_request_bodies(
    policy: &KiroCachePolicy,
    credit_usage: Option<f64>,
) -> bool {
    credit_usage.is_some_and(|value| {
        value.is_finite() && value > policy.high_credit_diagnostic_threshold
    })
}
```

Add the policy-driven math helpers in the same file:

```rust
pub(crate) fn adjust_input_tokens_for_cache_creation_cost_with_policy(
    policy: &KiroCachePolicy,
    authoritative_input_tokens: i32,
    credit_usage: Option<f64>,
    cache_estimation_enabled: bool,
) -> i32 {
    let authoritative_input_tokens = authoritative_input_tokens.max(0);
    let boost = &policy.small_input_high_credit_boost;
    if !cache_estimation_enabled
        || authoritative_input_tokens >= boost.target_input_tokens as i32
    {
        return authoritative_input_tokens;
    }
    let Some(observed_credit) = credit_usage.filter(|value| value.is_finite()) else {
        return authoritative_input_tokens;
    };
    if observed_credit <= boost.credit_start {
        return authoritative_input_tokens;
    }
    if observed_credit >= boost.credit_end {
        return boost.target_input_tokens as i32;
    }
    let progress =
        ((observed_credit - boost.credit_start) / (boost.credit_end - boost.credit_start))
            .clamp(0.0, 1.0);
    let boosted = authoritative_input_tokens as f64
        + (boost.target_input_tokens as f64 - authoritative_input_tokens as f64) * progress;
    boosted.round() as i32
}

pub(crate) fn prefix_tree_credit_ratio_cap_basis_points_with_policy(
    policy: &KiroCachePolicy,
    credit_usage: Option<f64>,
) -> Option<u32> {
    interpolate_prefix_tree_cache_ratio(policy, credit_usage)
        .map(|ratio| (ratio.clamp(0.0, 1.0) * 10_000.0).round() as u32)
}
```

- [ ] **Step 4: Thread one effective policy through Kiro request execution and admin key views**

In `backend/src/kiro_gateway/mod.rs`, register the module:

```rust
mod cache_policy;
```

Extend `AdminKiroKeyView` construction with effective policy data:

```rust
impl AdminKiroKeyView {
    pub fn from_key_and_effective_policy(
        value: &LlmGatewayKeyRecord,
        effective_policy: &KiroCachePolicy,
    ) -> Self {
        Self {
            // existing fields...
            kiro_cache_policy_override_json: value.kiro_cache_policy_override_json.clone(),
            effective_kiro_cache_policy_json: serde_json::to_string_pretty(effective_policy)
                .expect("effective kiro cache policy should serialize"),
            uses_global_kiro_cache_policy: uses_global_kiro_cache_policy(value),
        }
    }
}
```

Update all Kiro admin key responses to build the effective policy first:

```rust
let effective_key = state.llm_gateway.overlay_key_usage(&key).await;
let runtime_config = state.llm_gateway_runtime_config.read().clone();
let effective_policy = resolve_effective_kiro_cache_policy(&runtime_config, &effective_key)
    .map_err(|err| internal_error("Failed to resolve effective Kiro cache policy", err))?;
Ok(Json(AdminKiroKeyView::from_key_and_effective_policy(
    &effective_key,
    &effective_policy,
)))
```

Also change request-body capture in `build_kiro_usage_event_record`:

```rust
fn build_kiro_usage_event_record(
    effective_policy: &KiroCachePolicy,
    current: &LlmGatewayKeyRecord,
    event_context: &KiroEventContext,
    latency_ms: i32,
    status_code: i32,
    usage: KiroUsageSummary,
    usage_missing: bool,
    last_message_content: Option<String>,
) -> LlmGatewayUsageEventRecord {
    let capture_full_requests =
        should_capture_full_kiro_request_bodies(effective_policy, usage.credit_usage);
    // existing record construction...
}
```

- [ ] **Step 5: Replace hard-coded Kiro math in `anthropic/mod.rs`**

Add the effective policy to `KiroSimulationRequestContext`:

```rust
#[derive(Clone)]
struct KiroSimulationRequestContext {
    runtime_config: LlmGatewayRuntimeConfig,
    effective_cache_policy: KiroCachePolicy,
    simulation_config: KiroCacheSimulationConfig,
    projection: PromptProjection,
    prefix_cache_match: PrefixCacheMatch,
    conversation_id: String,
}
```

Compute it in `prepare_simulation_request_context` by accepting `key_record`:

```rust
fn prepare_simulation_request_context(
    state: &AppState,
    key_record: &LlmGatewayKeyRecord,
    conversation_state: ConversationState,
    session_tracking: SessionTracking,
    cache_estimation_enabled: bool,
) -> (ConversationState, SessionTracking, KiroSimulationRequestContext) {
    let runtime_config = state.llm_gateway_runtime_config.read().clone();
    let effective_cache_policy =
        resolve_effective_kiro_cache_policy(&runtime_config, key_record)
            .expect("key override should be validated before request execution");
    let simulation_config = KiroCacheSimulationConfig::from(&runtime_config);
    // existing body...
let simulation = KiroSimulationRequestContext {
    runtime_config,
    effective_cache_policy,
    simulation_config,
        projection,
        prefix_cache_match,
        conversation_id: conversation_state.conversation_id.clone(),
};
    // existing return...
}
```

Update the test helper as well so the Kiro Anthropic tests compile with the new field:

```rust
fn sample_simulation(
    mode: KiroCacheSimulationMode,
    matched_tokens: u64,
) -> KiroSimulationRequestContext {
    let runtime_config = LlmGatewayRuntimeConfig {
        kiro_cache_policy_json: default_kiro_cache_policy_json(),
        kiro_cache_policy: default_kiro_cache_policy(),
        ..LlmGatewayRuntimeConfig::default()
    };
    // existing setup...
    KiroSimulationRequestContext {
        runtime_config,
        effective_cache_policy: default_kiro_cache_policy(),
        simulation_config,
        projection,
        prefix_cache_match,
        conversation_id: "conv-1".to_string(),
    }
}
```

Replace the hard-coded helper usage:

```rust
let authoritative_input_tokens = adjust_input_tokens_for_cache_creation_cost_with_policy(
    &simulation.effective_cache_policy,
    resolved_input_tokens,
    credit_usage,
    cache_estimation_enabled,
);
```

and:

```rust
let Some(cap_basis_points) = prefix_tree_credit_ratio_cap_basis_points_with_policy(
    &simulation.effective_cache_policy,
    credit_usage,
) else {
    return prefix_cached_tokens;
};
```

- [ ] **Step 6: Run the backend tests again and verify they pass**

Run:

```bash
cargo test -p static-flow-backend effective_policy_uses_key_override_for_only_changed_fields -- --nocapture
cargo test -p static-flow-backend should_capture_full_kiro_request_bodies_uses_effective_threshold -- --nocapture
cargo test -p static-flow-backend build_usage_summary_uses_policy_override_for_boost_target -- --nocapture
cargo test -p static-flow-backend build_usage_summary_uses_policy_override_for_prefix_tree_bands -- --nocapture
```

Expected:

- All four tests PASS

- [ ] **Step 7: Commit the policy-driven backend logic**

```bash
git add backend/src/kiro_gateway/cache_policy.rs backend/src/kiro_gateway/mod.rs backend/src/kiro_gateway/types.rs backend/src/kiro_gateway/anthropic/mod.rs
git commit -m "feat: apply effective kiro cache policies per key"
```

---

### Task 4: Add frontend global-policy editing and per-key override controls

**Files:**
- Modify: `frontend/src/api.rs`
- Modify: `frontend/src/pages/admin_kiro_gateway.rs`
- Modify: `frontend/src/pages/admin_llm_gateway.rs`
- Test: `frontend/src/pages/admin_kiro_gateway.rs`

- [ ] **Step 1: Add failing frontend helper tests for summaries and override diffing**

At the bottom of `frontend/src/pages/admin_kiro_gateway.rs`, add pure-function tests first:

```rust
#[test]
fn format_kiro_cache_policy_summary_reports_inherit_global() {
    let summary = format_kiro_cache_policy_summary(
        true,
        r#"{"small_input_high_credit_boost":{"target_input_tokens":100000,"credit_start":1.0,"credit_end":1.8},"prefix_tree_credit_ratio_bands":[{"credit_start":0.3,"credit_end":1.0,"cache_ratio_start":0.7,"cache_ratio_end":0.2},{"credit_start":1.0,"credit_end":2.5,"cache_ratio_start":0.2,"cache_ratio_end":0.0}],"high_credit_diagnostic_threshold":2.0}"#,
    )
    .unwrap();

    assert!(summary.contains("inherit global"));
    assert!(summary.contains("boost 1.0 -> 1.8 => 100000"));
    assert!(summary.contains("diag 2.0"));
    assert!(summary.contains("bands 2"));
}

#[test]
fn build_kiro_cache_policy_override_json_only_emits_changed_fields() {
    let global = parse_kiro_cache_policy_form_json(
        r#"{"small_input_high_credit_boost":{"target_input_tokens":100000,"credit_start":1.0,"credit_end":1.8},"prefix_tree_credit_ratio_bands":[{"credit_start":0.3,"credit_end":1.0,"cache_ratio_start":0.7,"cache_ratio_end":0.2},{"credit_start":1.0,"credit_end":2.5,"cache_ratio_start":0.2,"cache_ratio_end":0.0}],"high_credit_diagnostic_threshold":2.0}"#,
    )
    .unwrap();
    let mut edited = global.clone();
    edited.high_credit_diagnostic_threshold = "1.4".to_string();

    let override_json = build_kiro_cache_policy_override_json(&global, &edited).unwrap();

    assert_eq!(
        override_json,
        Some(r#"{"high_credit_diagnostic_threshold":1.4}"#.to_string())
    );
}
```

- [ ] **Step 2: Run the frontend tests and verify they fail because the helper layer does not exist**

Run:

```bash
cargo test -p static-flow-frontend format_kiro_cache_policy_summary_reports_inherit_global -- --nocapture
cargo test -p static-flow-frontend build_kiro_cache_policy_override_json_only_emits_changed_fields -- --nocapture
```

Expected:

- Both tests fail to compile because the policy-form helpers and new API fields do not exist yet

- [ ] **Step 3: Extend frontend API structs and three-state patch support**

In `frontend/src/api.rs`, add the new fields:

```rust
pub struct LlmGatewayRuntimeConfig {
    // existing fields...
    pub kiro_cache_kmodels_json: String,
    pub kiro_cache_policy_json: String,
    pub kiro_prefix_cache_mode: String,
}

pub struct AdminLlmGatewayKeyView {
    // existing fields...
    pub kiro_cache_estimation_enabled: bool,
    pub kiro_cache_policy_override_json: Option<String>,
    pub effective_kiro_cache_policy_json: String,
    pub uses_global_kiro_cache_policy: bool,
}

#[derive(Clone, Debug, Default)]
pub struct PatchAdminLlmGatewayKeyRequest<'a> {
    // existing fields...
    pub kiro_cache_policy_override_json: Option<Option<&'a str>>,
}
```

Update `patch_admin_kiro_key` JSON assembly:

```rust
if let Some(policy_override_json) = request.kiro_cache_policy_override_json {
    body.insert(
        "kiro_cache_policy_override_json".to_string(),
        policy_override_json.map_or(serde_json::Value::Null, |raw| {
            serde_json::Value::String(raw.to_string())
        }),
    );
}
```

- [ ] **Step 4: Add structured policy-form helpers and global editor state**

In `frontend/src/pages/admin_kiro_gateway.rs`, add local form structs:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
struct KiroCachePolicyBandForm {
    credit_start: String,
    credit_end: String,
    cache_ratio_start: String,
    cache_ratio_end: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KiroCachePolicyForm {
    target_input_tokens: String,
    credit_start: String,
    credit_end: String,
    high_credit_diagnostic_threshold: String,
    bands: Vec<KiroCachePolicyBandForm>,
}
```

Add helper functions:

```rust
fn parse_kiro_cache_policy_form_json(raw: &str) -> Result<KiroCachePolicyForm, String> { /* parse JSON into string-backed form */ }

fn serialize_kiro_cache_policy_form_json(form: &KiroCachePolicyForm) -> Result<String, String> { /* validate numbers and serialize full policy */ }

fn build_kiro_cache_policy_override_json(
    global: &KiroCachePolicyForm,
    edited: &KiroCachePolicyForm,
) -> Result<Option<String>, String> { /* emit only changed scalar fields and whole-band replacement */ }

fn format_kiro_cache_policy_summary(
    uses_global: bool,
    effective_json: &str,
) -> Result<String, String> { /* "inherit global · boost ... · diag ... · bands N" */ }
```

Initialize global form state from runtime config:

```rust
let kiro_cache_policy_form = use_state(|| None::<KiroCachePolicyForm>);
```

and on load:

```rust
kiro_cache_policy_form.set(
    parse_kiro_cache_policy_form_json(&config_resp.kiro_cache_policy_json).ok(),
);
```

- [ ] **Step 5: Add the global policy editor and per-key override UI**

Extend the global save path in `admin_kiro_gateway.rs`:

```rust
let Some(policy_form) = (*kiro_cache_policy_form).clone() else {
    let message = "Kiro cache policy form is not loaded yet.".to_string();
    error.set(Some(message.clone()));
    notify.emit((message, true));
    return;
};
next_config.kiro_cache_policy_json = serialize_kiro_cache_policy_form_json(&policy_form)?;
```

In the Kiro key card component, add:

```rust
let policy_override_enabled = use_state(|| !props.key_item.uses_global_kiro_cache_policy);
let key_policy_form = use_state(|| {
    parse_kiro_cache_policy_form_json(&props.key_item.effective_kiro_cache_policy_json)
        .expect("effective policy json from backend should parse")
});
```

When saving the key:

```rust
let policy_override_json = if *policy_override_enabled {
    build_kiro_cache_policy_override_json(&global_policy_form, &key_policy_form_value)?
} else {
    None
};

match patch_admin_kiro_key(&key_id, PatchAdminLlmGatewayKeyRequest {
    // existing fields...
    kiro_cache_policy_override_json: Some(policy_override_json.as_deref()),
})
```

When restoring inheritance:

```rust
match patch_admin_kiro_key(&key_id, PatchAdminLlmGatewayKeyRequest {
    // existing fields...
    kiro_cache_policy_override_json: Some(None),
})
```

Also update `frontend/src/pages/admin_llm_gateway.rs` so its config save payload preserves:

```rust
kiro_cache_policy_json: config
    .as_ref()
    .map(|current| current.kiro_cache_policy_json.clone())
    .unwrap_or_default(),
```

- [ ] **Step 6: Run the frontend tests again and verify they pass**

Run:

```bash
cargo test -p static-flow-frontend format_kiro_cache_policy_summary_reports_inherit_global -- --nocapture
cargo test -p static-flow-frontend build_kiro_cache_policy_override_json_only_emits_changed_fields -- --nocapture
```

Expected:

- Both tests PASS

- [ ] **Step 7: Commit the frontend editor work**

```bash
git add frontend/src/api.rs frontend/src/pages/admin_kiro_gateway.rs frontend/src/pages/admin_llm_gateway.rs
git commit -m "feat: add kiro cache policy admin editors"
```

---

### Task 5: Run focused verification, format changed files, and finish cleanly

**Files:**
- Modify: `shared/src/llm_gateway_store/kiro_cache_policy.rs`
- Modify: `shared/src/llm_gateway_store/mod.rs`
- Modify: `shared/src/llm_gateway_store/types.rs`
- Modify: `shared/src/llm_gateway_store/schema.rs`
- Modify: `shared/src/llm_gateway_store/codec.rs`
- Modify: `backend/src/state.rs`
- Modify: `backend/src/llm_gateway/types.rs`
- Modify: `backend/src/llm_gateway.rs`
- Modify: `backend/src/kiro_gateway/cache_policy.rs`
- Modify: `backend/src/kiro_gateway/mod.rs`
- Modify: `backend/src/kiro_gateway/types.rs`
- Modify: `backend/src/kiro_gateway/anthropic/mod.rs`
- Modify: `frontend/src/api.rs`
- Modify: `frontend/src/pages/admin_kiro_gateway.rs`
- Modify: `frontend/src/pages/admin_llm_gateway.rs`

- [ ] **Step 1: Run rustfmt only on the changed Rust files**

Run:

```bash
rustfmt shared/src/llm_gateway_store/kiro_cache_policy.rs shared/src/llm_gateway_store/mod.rs shared/src/llm_gateway_store/types.rs shared/src/llm_gateway_store/schema.rs shared/src/llm_gateway_store/codec.rs backend/src/state.rs backend/src/llm_gateway/types.rs backend/src/llm_gateway.rs backend/src/kiro_gateway/cache_policy.rs backend/src/kiro_gateway/mod.rs backend/src/kiro_gateway/types.rs backend/src/kiro_gateway/anthropic/mod.rs
cargo fmt -p static-flow-frontend -- src/api.rs src/pages/admin_kiro_gateway.rs src/pages/admin_llm_gateway.rs
```

Expected:

- Formatting completes with no workspace-wide formatting and no edits inside `deps/lance` or `deps/lancedb`

- [ ] **Step 2: Run focused package tests**

Run:

```bash
cargo test -p static-flow-shared kiro_cache_policy -- --nocapture
cargo test -p static-flow-shared runtime_config_round_trip_preserves_kiro_cache_policy_json -- --nocapture
cargo test -p static-flow-shared key_round_trip_preserves_kiro_cache_policy_override_json -- --nocapture
cargo test -p static-flow-backend update_runtime_config_rejects_invalid_kiro_cache_policy_json -- --nocapture
cargo test -p static-flow-backend patch_kiro_key_request_distinguishes_absent_null_and_value_for_policy_override -- --nocapture
cargo test -p static-flow-backend build_usage_summary_uses_policy_override_for_boost_target -- --nocapture
cargo test -p static-flow-backend build_usage_summary_uses_policy_override_for_prefix_tree_bands -- --nocapture
cargo test -p static-flow-backend should_capture_full_kiro_request_bodies_uses_effective_threshold -- --nocapture
cargo test -p static-flow-frontend format_kiro_cache_policy_summary_reports_inherit_global -- --nocapture
cargo test -p static-flow-frontend build_kiro_cache_policy_override_json_only_emits_changed_fields -- --nocapture
```

Expected:

- All listed tests PASS

- [ ] **Step 3: Run clippy on the affected crates and fix everything down to zero**

Run:

```bash
cargo clippy -p static-flow-shared -p static-flow-backend -p static-flow-frontend --tests -- -D warnings
```

Expected:

- Exit code `0`
- No warnings

- [ ] **Step 4: Inspect the final diff and verify spec coverage**

Run:

```bash
git diff --stat
git diff -- shared/src/llm_gateway_store/kiro_cache_policy.rs shared/src/llm_gateway_store/types.rs backend/src/state.rs backend/src/kiro_gateway/cache_policy.rs backend/src/kiro_gateway/anthropic/mod.rs frontend/src/pages/admin_kiro_gateway.rs
```

Check manually that the diff covers:

- shared default policy + override parsing
- persisted global/key JSON fields
- backend effective-policy resolution
- policy-driven boost/band/diagnostic logic
- global editor + key override editor
- `admin_llm_gateway` preserving the new runtime-config field

- [ ] **Step 5: Commit the fully verified feature**

```bash
git add shared/src/llm_gateway_store/kiro_cache_policy.rs shared/src/llm_gateway_store/mod.rs shared/src/llm_gateway_store/types.rs shared/src/llm_gateway_store/schema.rs shared/src/llm_gateway_store/codec.rs backend/src/state.rs backend/src/llm_gateway/types.rs backend/src/llm_gateway.rs backend/src/kiro_gateway/cache_policy.rs backend/src/kiro_gateway/mod.rs backend/src/kiro_gateway/types.rs backend/src/kiro_gateway/anthropic/mod.rs frontend/src/api.rs frontend/src/pages/admin_kiro_gateway.rs frontend/src/pages/admin_llm_gateway.rs
git commit -m "feat: make kiro cache protection policies configurable"
```
