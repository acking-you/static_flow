# Kiro Gateway Key Routing Subset Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add per-key account routing controls to `/admin/kiro-gateway` so a Kiro key can use the full account pool by default, bind to one account, or restrict auto-routing to a configured account subset.

**Architecture:** Keep the existing `LlmGatewayKeyRecord` routing fields and wire them through Kiro end to end instead of inventing a new schema. The backend patch path normalizes and persists route settings, the provider filters candidate accounts before applying the existing fairness scheduler, and the admin page exposes the same route model already used by Codex.

**Tech Stack:** Rust (Axum backend, Yew frontend), shared LanceDB key storage via `LlmGatewayKeyRecord`, `cargo test`, `cargo clippy`, targeted `rustfmt`.

---

## File Map

- `backend/src/kiro_gateway/types.rs`
  - Owns request/response payload structs for `/admin/kiro-gateway/*`
  - Add route fields to `PatchKiroKeyRequest`
- `backend/src/kiro_gateway/mod.rs`
  - Owns Kiro admin key CRUD
  - Add normalization helpers and persist route settings during key patch
- `backend/src/kiro_gateway/provider.rs`
  - Owns request-time account selection and failover
  - Filter candidate Kiro auths by key route metadata before fairness ordering
- `backend/src/kiro_gateway/anthropic/mod.rs`
  - Owns authenticated Kiro message handling
  - Pass authenticated `LlmGatewayKeyRecord` into provider calls
- `backend/src/kiro_gateway/anthropic/websearch.rs`
  - Owns the MCP websearch path
  - Pass authenticated `LlmGatewayKeyRecord` into MCP provider calls
- `frontend/src/api.rs`
  - Owns admin Kiro key patch serialization
  - Serialize `route_strategy`, `fixed_account_name`, and `auto_account_names`
- `frontend/src/pages/admin_kiro_gateway.rs`
  - Owns `/admin/kiro-gateway` UI
  - Add route controls, route summary text, and helper sanitization logic

---

### Task 1: Persist Kiro key route settings in the admin patch path

**Files:**
- Modify: `backend/src/kiro_gateway/types.rs`
- Modify: `backend/src/kiro_gateway/mod.rs`
- Test: `backend/src/kiro_gateway/mod.rs`

- [ ] **Step 1: Write the failing normalization tests**

Add these tests to the existing `#[cfg(test)] mod tests` block in `backend/src/kiro_gateway/mod.rs`:

```rust
    use std::collections::{BTreeMap, BTreeSet};

    use super::normalize_key_route_config;

    #[test]
    fn normalize_key_route_config_keeps_auto_without_subset_as_full_pool() {
        let existing = BTreeSet::from(["alpha".to_string(), "beta".to_string()]);

        let normalized = normalize_key_route_config(
            Some("auto"),
            None,
            Some(vec![]),
            &existing,
        )
        .expect("normalize should succeed");

        assert_eq!(
            normalized,
            (Some("auto".to_string()), None, None)
        );
    }

    #[test]
    fn normalize_key_route_config_requires_fixed_account_name() {
        let existing = BTreeSet::from(["alpha".to_string()]);

        let err = normalize_key_route_config(Some("fixed"), None, None, &existing)
            .expect_err("fixed without account should fail");

        assert!(err.to_string().contains("fixed route_strategy requires fixed_account_name"));
    }

    #[test]
    fn normalize_key_route_config_filters_unknown_auto_accounts() {
        let existing = BTreeSet::from(["alpha".to_string(), "beta".to_string()]);

        let normalized = normalize_key_route_config(
            Some("auto"),
            None,
            Some(vec![
                "beta".to_string(),
                "missing".to_string(),
                "alpha".to_string(),
                "beta".to_string(),
            ]),
            &existing,
        )
        .expect("normalize should succeed");

        assert_eq!(
            normalized,
            (
                Some("auto".to_string()),
                None,
                Some(vec!["alpha".to_string(), "beta".to_string()]),
            )
        );
    }

    #[test]
    fn normalize_key_route_config_rejects_auto_subset_when_all_accounts_are_unknown() {
        let existing = BTreeSet::from(["alpha".to_string()]);

        let err = normalize_key_route_config(
            Some("auto"),
            None,
            Some(vec!["missing".to_string()]),
            &existing,
        )
        .expect_err("unknown subset should fail");

        assert!(err
            .to_string()
            .contains("none of the configured auto accounts exist anymore"));
    }
```

- [ ] **Step 2: Run the backend test filter and verify it fails**

Run: `cargo test -p static-flow-backend normalize_key_route_config -- --nocapture`
Expected: FAIL because `normalize_key_route_config` and the new route request fields do not exist yet.

- [ ] **Step 3: Extend the patch request payload**

In `backend/src/kiro_gateway/types.rs`, change `PatchKiroKeyRequest` to:

```rust
#[derive(Debug, Deserialize)]
pub struct PatchKiroKeyRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub quota_billable_limit: Option<u64>,
    #[serde(default)]
    pub route_strategy: Option<String>,
    #[serde(default)]
    pub fixed_account_name: Option<String>,
    #[serde(default)]
    pub auto_account_names: Option<Vec<String>>,
    #[serde(default)]
    pub model_name_map: Option<BTreeMap<String, String>>,
}
```

- [ ] **Step 4: Implement route normalization and persist it during key patch**

In `backend/src/kiro_gateway/mod.rs`, add these helpers near the other normalization helpers:

```rust
fn normalize_route_strategy_input(value: Option<&str>) -> anyhow::Result<Option<String>> {
    let Some(trimmed) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    match trimmed {
        "auto" | "fixed" => Ok(Some(trimmed.to_string())),
        _ => anyhow::bail!("route_strategy must be `auto` or `fixed`"),
    }
}

fn normalize_optional_account_name_input(value: Option<&str>) -> anyhow::Result<Option<String>> {
    let Some(trimmed) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    crate::llm_gateway::accounts::validate_account_name(trimmed)
        .map(Some)
        .map_err(anyhow::Error::msg)
}

fn normalize_auto_account_names_input(
    value: Option<Vec<String>>,
) -> anyhow::Result<Option<Vec<String>>> {
    let Some(values) = value else {
        return Ok(None);
    };
    let mut names = values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(|value| {
            crate::llm_gateway::accounts::validate_account_name(&value)
                .map_err(anyhow::Error::msg)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    names.sort();
    names.dedup();
    if names.is_empty() {
        return Ok(None);
    }
    Ok(Some(names))
}

fn normalize_key_route_config(
    route_strategy: Option<&str>,
    fixed_account_name: Option<&str>,
    auto_account_names: Option<Vec<String>>,
    existing_account_names: &BTreeSet<String>,
) -> anyhow::Result<(Option<String>, Option<String>, Option<Vec<String>>)> {
    let route_strategy = normalize_route_strategy_input(route_strategy)?;
    let fixed_account_name = normalize_optional_account_name_input(fixed_account_name)?;
    let auto_account_names = normalize_auto_account_names_input(auto_account_names)?;

    match route_strategy.as_deref().unwrap_or("auto") {
        "fixed" => {
            let fixed_account_name = fixed_account_name
                .ok_or_else(|| anyhow::anyhow!("fixed route_strategy requires fixed_account_name"))?;
            if !existing_account_names.contains(&fixed_account_name) {
                anyhow::bail!("unknown account `{fixed_account_name}`");
            }
            Ok((
                Some("fixed".to_string()),
                Some(fixed_account_name),
                None,
            ))
        },
        "auto" => {
            let filtered_auto_account_names = auto_account_names.map(|names| {
                names
                    .into_iter()
                    .filter(|name| existing_account_names.contains(name))
                    .collect::<Vec<_>>()
            });
            if filtered_auto_account_names
                .as_ref()
                .is_some_and(|names| names.is_empty())
            {
                anyhow::bail!("none of the configured auto accounts exist anymore");
            }
            Ok((
                Some("auto".to_string()),
                None,
                filtered_auto_account_names.filter(|names| !names.is_empty()),
            ))
        },
        _ => anyhow::bail!("route_strategy must be `auto` or `fixed`"),
    }
}
```

Then update `patch_admin_key(...)` to load existing Kiro auth names and apply the helper before saving:

```rust
    let existing_account_names = state
        .kiro_gateway
        .token_manager
        .list_auths()
        .await
        .map_err(|err| internal_error("Failed to load Kiro accounts", err))?
        .into_iter()
        .map(|auth| auth.name)
        .collect::<BTreeSet<_>>();

    let (route_strategy, fixed_account_name, auto_account_names) =
        normalize_key_route_config(
            request.route_strategy.as_deref(),
            request.fixed_account_name.as_deref(),
            request.auto_account_names,
            &existing_account_names,
        )
        .map_err(|err| bad_request(&err.to_string()))?;

    key.route_strategy = route_strategy;
    key.fixed_account_name = fixed_account_name;
    key.auto_account_names = auto_account_names;
```

- [ ] **Step 5: Run the backend tests again and verify they pass**

Run: `cargo test -p static-flow-backend normalize_key_route_config -- --nocapture`
Expected: PASS for the four new normalization tests.

- [ ] **Step 6: Commit**

```bash
git add backend/src/kiro_gateway/types.rs backend/src/kiro_gateway/mod.rs
git commit -m "feat(kiro-gateway): persist key route settings"
```

---

### Task 2: Apply key route settings during Kiro account selection

**Files:**
- Modify: `backend/src/kiro_gateway/provider.rs`
- Modify: `backend/src/kiro_gateway/anthropic/mod.rs`
- Modify: `backend/src/kiro_gateway/anthropic/websearch.rs`
- Test: `backend/src/kiro_gateway/provider.rs`

- [ ] **Step 1: Write the failing provider routing tests**

In `backend/src/kiro_gateway/provider.rs`, extend the existing test module with:

```rust
    use static_flow_shared::llm_gateway_store::LlmGatewayKeyRecord;

    fn routed_key(
        route_strategy: Option<&str>,
        fixed_account_name: Option<&str>,
        auto_account_names: Option<Vec<&str>>,
    ) -> LlmGatewayKeyRecord {
        LlmGatewayKeyRecord {
            id: "test-key".to_string(),
            name: "test".to_string(),
            secret: "secret".to_string(),
            key_hash: "hash".to_string(),
            status: "active".to_string(),
            provider_type: "kiro".to_string(),
            protocol_family: "anthropic".to_string(),
            public_visible: false,
            quota_billable_limit: 1_000,
            usage_input_uncached_tokens: 0,
            usage_input_cached_tokens: 0,
            usage_output_tokens: 0,
            usage_billable_tokens: 0,
            usage_credit_total: 0.0,
            usage_credit_missing_events: 0,
            last_used_at: None,
            created_at: 0,
            updated_at: 0,
            route_strategy: route_strategy.map(str::to_string),
            fixed_account_name: fixed_account_name.map(str::to_string),
            auto_account_names: auto_account_names
                .map(|names| names.into_iter().map(str::to_string).collect()),
            model_name_map: None,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
        }
    }

    #[test]
    fn filter_auths_for_key_route_keeps_full_pool_for_auto_without_subset() {
        let auths = vec![auth("alpha"), auth("beta"), auth("gamma")];
        let key = routed_key(Some("auto"), None, None);

        let filtered = filter_auths_for_key_route(&auths, &key).expect("filter should succeed");
        let names = filtered.iter().map(|auth| auth.name.as_str()).collect::<Vec<_>>();

        assert_eq!(names, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn filter_auths_for_key_route_keeps_only_fixed_account() {
        let auths = vec![auth("alpha"), auth("beta"), auth("gamma")];
        let key = routed_key(Some("fixed"), Some("beta"), None);

        let filtered = filter_auths_for_key_route(&auths, &key).expect("filter should succeed");
        let names = filtered.iter().map(|auth| auth.name.as_str()).collect::<Vec<_>>();

        assert_eq!(names, vec!["beta"]);
    }

    #[test]
    fn filter_auths_for_key_route_keeps_only_auto_subset() {
        let auths = vec![auth("alpha"), auth("beta"), auth("gamma")];
        let key = routed_key(Some("auto"), None, Some(vec!["gamma", "alpha"]));

        let filtered = filter_auths_for_key_route(&auths, &key).expect("filter should succeed");
        let names = filtered.iter().map(|auth| auth.name.as_str()).collect::<Vec<_>>();

        assert_eq!(names, vec!["alpha", "gamma"]);
    }

    #[test]
    fn filter_auths_for_key_route_rejects_missing_fixed_account() {
        let auths = vec![auth("alpha"), auth("beta")];
        let key = routed_key(Some("fixed"), Some("missing"), None);

        let err = filter_auths_for_key_route(&auths, &key)
            .expect_err("missing fixed account should fail");

        assert!(err.to_string().contains("bound account `missing` is unavailable"));
    }
```

- [ ] **Step 2: Run the provider test filter and verify it fails**

Run: `cargo test -p static-flow-backend filter_auths_for_key_route -- --nocapture`
Expected: FAIL because `filter_auths_for_key_route` does not exist yet.

- [ ] **Step 3: Implement candidate filtering in the provider**

In `backend/src/kiro_gateway/provider.rs`, add the helper and thread the key through the public provider API:

```rust
use static_flow_shared::llm_gateway_store::{LlmGatewayKeyRecord, LLM_GATEWAY_PROVIDER_KIRO};

fn filter_auths_for_key_route(
    auths: &[KiroAuthRecord],
    key: &LlmGatewayKeyRecord,
) -> Result<Vec<KiroAuthRecord>> {
    match key.route_strategy.as_deref().unwrap_or("auto") {
        "fixed" => {
            let name = key.fixed_account_name.as_deref().unwrap_or("");
            if name.is_empty() {
                return Err(anyhow!("fixed route_strategy requires fixed_account_name"));
            }
            let filtered = auths
                .iter()
                .filter(|auth| auth.name == name)
                .cloned()
                .collect::<Vec<_>>();
            if filtered.is_empty() {
                return Err(anyhow!("bound account `{name}` is unavailable"));
            }
            Ok(filtered)
        },
        "auto" => {
            let Some(names) = key.auto_account_names.as_ref() else {
                return Ok(auths.to_vec());
            };
            let filtered = auths
                .iter()
                .filter(|auth| names.iter().any(|name| name == &auth.name))
                .cloned()
                .collect::<Vec<_>>();
            if filtered.is_empty() {
                return Err(anyhow!(
                    "configured auto account subset has no existing accounts: {}",
                    names.join(", ")
                ));
            }
            Ok(filtered)
        },
        other => Err(anyhow!("unsupported route strategy `{other}`")),
    }
}

pub async fn call_api(
    &self,
    key: &LlmGatewayKeyRecord,
    conversation_state: &ConversationState,
) -> Result<ProviderCallResult> {
    self.call_api_inner(key, conversation_state).await
}

pub async fn call_api_stream(
    &self,
    key: &LlmGatewayKeyRecord,
    conversation_state: &ConversationState,
) -> Result<ProviderCallResult> {
    self.call_api_inner(key, conversation_state).await
}

pub async fn call_mcp(
    &self,
    key: &LlmGatewayKeyRecord,
    request_body: &str,
) -> Result<ProviderCallResult> {
    self.call_mcp_inner(key, request_body).await
}
```

Then in both `call_api_inner(...)` and `call_mcp_inner(...)`, filter the fresh auth list before ordering:

```rust
            let auths = self.runtime.token_manager.list_auths().await?;
            let auths = filter_auths_for_key_route(&auths, key)?;
            if auths.is_empty() {
                return Err(anyhow!("no kiro account available for request"));
            }
```

- [ ] **Step 4: Pass the authenticated key record into both request paths**

Update `backend/src/kiro_gateway/anthropic/mod.rs`:

```rust
        let response = match provider.call_api_stream(&key_record, &conversation_state).await {
            Ok(response) => response,
            Err(err) => return map_provider_error(err),
        };

    let response = match provider.call_api(&key_record, &conversation_state).await {
        Ok(response) => response,
        Err(err) => return map_provider_error(err),
    };
```

Update `backend/src/kiro_gateway/anthropic/websearch.rs`:

```rust
    let search_results = match call_mcp_api(provider, &key_record, &mcp_request).await {
        Ok(success) => {
            let McpCallSuccess {
                response,
                account_name,
            } = success;
            event_context.account_name = Some(account_name);
            parse_search_results(&response)
        },
        Err(err) => {
            if should_propagate_mcp_error(&err) {
                return map_provider_error(err);
            }
            tracing::warn!(
                query = %query,
                error = %err,
                "kiro mcp web_search failed; returning empty search results fallback"
            );
            None
        },
    };

async fn call_mcp_api(
    provider: &crate::kiro_gateway::provider::KiroProvider,
    key_record: &LlmGatewayKeyRecord,
    request: &McpRequest,
) -> anyhow::Result<McpCallSuccess> {
    let request_body = serde_json::to_string(request)?;
    let response = provider.call_mcp(key_record, &request_body).await?;
    let account_name = response.account_name;
    let body = response.response.text().await?;
    let mcp_response: McpResponse = serde_json::from_str(&body)?;
    if let Some(error) = &mcp_response.error {
        anyhow::bail!(
            "MCP error: {} - {}",
            error.code.unwrap_or(-1),
            error.message.as_deref().unwrap_or("Unknown error")
        );
    }
    Ok(McpCallSuccess {
        response: mcp_response,
        account_name,
    })
}
```

- [ ] **Step 5: Run the provider tests again and verify they pass**

Run: `cargo test -p static-flow-backend filter_auths_for_key_route -- --nocapture`
Expected: PASS for the new candidate-filtering tests.

- [ ] **Step 6: Commit**

```bash
git add backend/src/kiro_gateway/provider.rs backend/src/kiro_gateway/anthropic/mod.rs backend/src/kiro_gateway/anthropic/websearch.rs
git commit -m "feat(kiro-gateway): honor per-key route settings at runtime"
```

---

### Task 3: Add route controls to the Kiro admin UI and send them in patch requests

**Files:**
- Modify: `frontend/src/api.rs`
- Modify: `frontend/src/pages/admin_kiro_gateway.rs`
- Test: `frontend/src/pages/admin_kiro_gateway.rs`

- [ ] **Step 1: Write the failing frontend helper tests**

At the bottom of `frontend/src/pages/admin_kiro_gateway.rs`, add:

```rust
#[cfg(test)]
mod tests {
    use super::{
        kiro_key_route_summary, sanitize_kiro_auto_account_names,
        sanitize_kiro_fixed_account_name,
    };

    #[test]
    fn sanitize_kiro_auto_account_names_drops_unknown_and_sorts() {
        let available = vec!["beta".to_string(), "alpha".to_string()];
        let configured = vec![
            "beta".to_string(),
            "missing".to_string(),
            "alpha".to_string(),
            "beta".to_string(),
        ];

        assert_eq!(
            sanitize_kiro_auto_account_names(&configured, &available),
            vec!["alpha".to_string(), "beta".to_string()]
        );
    }

    #[test]
    fn sanitize_kiro_fixed_account_name_drops_unknown_value() {
        let available = vec!["alpha".to_string(), "beta".to_string()];

        assert_eq!(
            sanitize_kiro_fixed_account_name(Some("missing"), &available),
            ""
        );
        assert_eq!(
            sanitize_kiro_fixed_account_name(Some(" beta "), &available),
            "beta"
        );
    }

    #[test]
    fn kiro_key_route_summary_uses_full_pool_text_when_subset_is_empty() {
        let summary = kiro_key_route_summary("auto", "", &[]);
        assert!(summary.contains("全账号池自动择优"));
    }
}
```

- [ ] **Step 2: Run the frontend test filter and verify it fails**

Run: `cargo test -p static-flow-frontend sanitize_kiro_auto_account_names -- --nocapture`
Expected: FAIL because the helper functions do not exist yet.

- [ ] **Step 3: Serialize the Kiro route fields in the frontend API layer**

In `frontend/src/api.rs`, extend `patch_admin_kiro_key(...)` so it matches the Codex patch serializer:

```rust
        if let Some(strategy) = request.route_strategy {
            body.insert(
                "route_strategy".to_string(),
                serde_json::Value::String(strategy.to_string()),
            );
        }
        if let Some(account_name) = request.fixed_account_name {
            body.insert(
                "fixed_account_name".to_string(),
                serde_json::Value::String(account_name.to_string()),
            );
        }
        if let Some(account_names) = request.auto_account_names {
            body.insert(
                "auto_account_names".to_string(),
                serde_json::Value::Array(
                    account_names
                        .iter()
                        .map(|value| serde_json::Value::String(value.clone()))
                        .collect(),
                ),
            );
        }

        if let Some(model_name_map) = request.model_name_map {
            let value = serde_json::to_value(model_name_map)
                .map_err(|e| format!("Serialize error: {:?}", e))?;
            body.insert("model_name_map".to_string(), value);
        }
```

- [ ] **Step 4: Implement the Kiro key editor route controls**

In `frontend/src/pages/admin_kiro_gateway.rs`, add the same pure helpers used by the new tests:

```rust
fn sanitize_kiro_auto_account_names(names: &[String], available_names: &[String]) -> Vec<String> {
    let valid_names = available_names.iter().map(|name| name.as_str()).collect::<std::collections::HashSet<_>>();
    let mut sanitized = names
        .iter()
        .filter(|name| valid_names.contains(name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    sanitized.sort();
    sanitized.dedup();
    sanitized
}

fn sanitize_kiro_fixed_account_name(value: Option<&str>, available_names: &[String]) -> String {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return String::new();
    };
    if available_names.iter().any(|name| name == value) {
        value.to_string()
    } else {
        String::new()
    }
}

fn kiro_key_route_summary(
    route_strategy: &str,
    fixed_account_name: &str,
    auto_account_names: &[String],
) -> String {
    if route_strategy == "fixed" {
        if fixed_account_name.is_empty() {
            "绑定: 未选择".to_string()
        } else {
            format!("绑定: {fixed_account_name}")
        }
    } else if auto_account_names.is_empty() {
        "全账号池自动择优；如果某个账号不可用，会继续尝试其他账号。".to_string()
    } else {
        format!(
            "仅在这些账号中自动择优: {}；如果子集里没有可用账号，请求会直接报错。",
            auto_account_names.join(", ")
        )
    }
}
```

Then update `KiroKeyEditorCard` to:

- extend `KiroKeyEditorCardProps` with `accounts: Vec<KiroAccountView>`
- add `route_strategy`, `fixed_account_name`, and `auto_account_names` state
- derive `available_account_names` from `props.accounts`
- sanitize route state in `use_effect_with((props.key_item.clone(), props.accounts.clone()), ...)`
- send route fields in both `on_save` and `on_disable`
- render route controls directly above the existing action buttons

First change the props and call site:

```rust
#[derive(Properties, PartialEq)]
struct KiroKeyEditorCardProps {
    key_item: AdminLlmGatewayKeyView,
    available_models: Vec<KiroModelView>,
    accounts: Vec<KiroAccountView>,
    on_reload: Callback<()>,
    on_copy: Callback<(String, String)>,
    on_flash: Callback<(String, bool)>,
}
```

And in the key inventory render:

```rust
                                for (*keys).iter().map(|key_item| html! {
                                    <KiroKeyEditorCard
                                        key={key_item.id.clone()}
                                        key_item={key_item.clone()}
                                        available_models={(*kiro_models).clone()}
                                        accounts={(*accounts).clone()}
                                        on_reload={on_reload.clone()}
                                        on_copy={on_copy.clone()}
                                        on_flash={notify.clone()}
                                    />
                                })
```

Use this state initialization pattern:

```rust
    let available_account_names = props
        .accounts
        .iter()
        .map(|account| account.name.clone())
        .collect::<Vec<_>>();
    let route_strategy = use_state(|| {
        props
            .key_item
            .route_strategy
            .clone()
            .unwrap_or_else(|| "auto".to_string())
    });
    let fixed_account_name = use_state(|| {
        sanitize_kiro_fixed_account_name(
            props.key_item.fixed_account_name.as_deref(),
            &available_account_names,
        )
    });
    let auto_account_names = use_state(|| {
        sanitize_kiro_auto_account_names(
            props.key_item.auto_account_names.as_deref().unwrap_or(&[]),
            &available_account_names,
        )
    });
```

Use this patch payload in both save paths:

```rust
                match patch_admin_kiro_key(&key_id, PatchAdminLlmGatewayKeyRequest {
                    name: Some(name_value.trim()),
                    status: Some(status_value.trim()),
                    public_visible: None,
                    quota_billable_limit: Some(parsed_quota),
                    route_strategy: Some(route_strategy_value.as_str()),
                    fixed_account_name: Some(fixed_account_name_value.as_str()),
                    auto_account_names: Some(auto_account_names_value.as_slice()),
                    model_name_map: Some(&model_name_map_value),
                    request_max_concurrency: None,
                    request_min_start_interval_ms: None,
                    request_max_concurrency_unlimited: false,
                    request_min_start_interval_ms_unlimited: false,
                })
                .await
```

Render the route section using this concrete block above the existing action buttons:

```rust
            <div class={classes!("mt-4", "space-y-3", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-3")}>
                <div class={classes!("text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Routing" }</div>
                <label class={classes!("text-sm")}>
                    <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Strategy" }</div>
                    <select
                        class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "text-sm")}
                        value={(*route_strategy).clone()}
                        onchange={{
                            let route_strategy = route_strategy.clone();
                            Callback::from(move |event: Event| {
                                let input: HtmlSelectElement = event.target_unchecked_into();
                                route_strategy.set(input.value());
                            })
                        }}
                    >
                        <option value="auto">{ "auto" }</option>
                        <option value="fixed">{ "fixed" }</option>
                    </select>
                </label>
                if *route_strategy == "fixed" {
                    <label class={classes!("text-sm")}>
                        <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Fixed Account" }</div>
                        <select
                            class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "text-sm")}
                            value={(*fixed_account_name).clone()}
                            onchange={{
                                let fixed_account_name = fixed_account_name.clone();
                                Callback::from(move |event: Event| {
                                    let input: HtmlSelectElement = event.target_unchecked_into();
                                    fixed_account_name.set(input.value());
                                })
                            }}
                        >
                            <option value="">{ "-- select --" }</option>
                            { for props.accounts.iter().map(|account| html! {
                                <option value={account.name.clone()}>{ account.name.clone() }</option>
                            }) }
                        </select>
                    </label>
                } else {
                    <div class={classes!("space-y-2")}>
                        <div class={classes!("text-sm", "text-[var(--muted)]")}>{ "Auto Candidate Accounts" }</div>
                        <div class={classes!("grid", "gap-2", "xl:grid-cols-2")}>
                            { for props.accounts.iter().map(|account| {
                                let account_name = account.name.clone();
                                let checked = (*auto_account_names).iter().any(|name| name == &account.name);
                                let auto_account_names = auto_account_names.clone();
                                html! {
                                    <label class={classes!("flex", "items-start", "gap-3", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2.5")}>
                                        <input
                                            type="checkbox"
                                            checked={checked}
                                            onchange={Callback::from(move |_| {
                                                let mut next = (*auto_account_names).clone();
                                                if let Some(index) = next.iter().position(|name| name == &account_name) {
                                                    next.remove(index);
                                                } else {
                                                    next.push(account_name.clone());
                                                    next.sort();
                                                    next.dedup();
                                                }
                                                auto_account_names.set(next);
                                            })}
                                        />
                                        <span class={classes!("font-mono", "text-sm")}>{ account.name.clone() }</span>
                                    </label>
                                }
                            }) }
                        </div>
                    </div>
                }
                <div class={classes!("text-xs", "text-[var(--muted)]")}>
                    { kiro_key_route_summary(&route_strategy, &fixed_account_name, &auto_account_names) }
                </div>
            </div>
```

- [ ] **Step 5: Run the frontend tests and compile checks**

Run: `cargo test -p static-flow-frontend sanitize_kiro_auto_account_names -- --nocapture`
Expected: PASS for the new helper tests.

Then run: `cargo clippy -p static-flow-frontend --tests -- -D warnings`
Expected: PASS with zero warnings/errors.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/api.rs frontend/src/pages/admin_kiro_gateway.rs
git commit -m "feat(kiro-gateway): add admin key routing controls"
```

---

### Task 4: Run full verification and clean up formatting

**Files:**
- Modify if needed: `backend/src/kiro_gateway/types.rs`
- Modify if needed: `backend/src/kiro_gateway/mod.rs`
- Modify if needed: `backend/src/kiro_gateway/provider.rs`
- Modify if needed: `backend/src/kiro_gateway/anthropic/mod.rs`
- Modify if needed: `backend/src/kiro_gateway/anthropic/websearch.rs`
- Modify if needed: `frontend/src/api.rs`
- Modify if needed: `frontend/src/pages/admin_kiro_gateway.rs`

- [ ] **Step 1: Run the Kiro backend test coverage**

Run: `cargo test -p static-flow-backend kiro_gateway::mod::tests -- --nocapture`
Expected: PASS.

Run: `cargo test -p static-flow-backend kiro_gateway::provider::tests -- --nocapture`
Expected: PASS.

- [ ] **Step 2: Run the frontend test coverage**

Run: `cargo test -p static-flow-frontend -- --nocapture`
Expected: PASS.

- [ ] **Step 3: Run clippy for the affected crates**

Run: `cargo clippy -p static-flow-backend -p static-flow-frontend --tests -- -D warnings`
Expected: PASS with zero warnings/errors.

- [ ] **Step 4: Format only the changed Rust files**

Run:

```bash
rustfmt backend/src/kiro_gateway/types.rs \
  backend/src/kiro_gateway/mod.rs \
  backend/src/kiro_gateway/provider.rs \
  backend/src/kiro_gateway/anthropic/mod.rs \
  backend/src/kiro_gateway/anthropic/websearch.rs \
  frontend/src/api.rs \
  frontend/src/pages/admin_kiro_gateway.rs
```

Expected: files are reformatted in place with no workspace-wide formatting.

- [ ] **Step 5: Confirm the worktree is clean and record the final commit**

Run: `git status --short`
Expected: empty output.

If formatting or clippy required fixes after the earlier commits, create one final commit:

```bash
git add backend/src/kiro_gateway/types.rs \
  backend/src/kiro_gateway/mod.rs \
  backend/src/kiro_gateway/provider.rs \
  backend/src/kiro_gateway/anthropic/mod.rs \
  backend/src/kiro_gateway/anthropic/websearch.rs \
  frontend/src/api.rs \
  frontend/src/pages/admin_kiro_gateway.rs
git commit -m "chore(kiro-gateway): finalize key routing subset support"
```
