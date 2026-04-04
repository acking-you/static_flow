# Kiro Tool Validation And Normalization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent deterministic Kiro upstream `400 Improperly formed request` failures caused by invalid tool metadata, while preserving existing userspace compatibility for tool schemas that upstream already accepts.

**Architecture:** Extend the existing Kiro request normalization pipeline so `tools` receive the same first-class boundary handling as `messages`. Add a tool normalization/validation pass after `normalize_request(...)`, auto-fill empty tool descriptions with a stable placeholder, reject empty tool names locally, and emit structured diagnostics from the existing Anthropic handler logging path. Keep complex JSON Schema keywords transparent for now; only count and report them.

**Tech Stack:** Rust (Axum backend), Serde JSON, existing Kiro Anthropic converter pipeline, `cargo test`, `cargo clippy`, targeted `cargo fmt -p static-flow-backend`.

---

## File Map

- `backend/src/kiro_gateway/anthropic/converter.rs`
  - Owns request normalization, validation, conversion, and converter-focused tests
  - Add tool normalization/validation data structures and tests here
- `backend/src/kiro_gateway/anthropic/mod.rs`
  - Owns request-entry logging and failure reporting
  - Emit structured logs for tool normalization and tool validation summaries here

---

### Task 1: Add failing converter tests for tool normalization and validation

**Files:**
- Modify: `backend/src/kiro_gateway/anthropic/converter.rs`
- Test: `backend/src/kiro_gateway/anthropic/converter.rs`

- [ ] **Step 1: Add a failing test for empty tool description normalization**

Add a converter test near the existing normalization tests:

```rust
    #[test]
    fn normalize_request_fills_empty_tool_description_with_stable_placeholder() {
        let mut req = base_request(vec![AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Hello"),
            }]);
        req.tools = Some(vec![AnthropicTool {
            tool_type: None,
                name: "demo_tool".to_string(),
                description: "".to_string(),
                input_schema: HashMap::from([
                    ("type".to_string(), serde_json::json!("object")),
                    ("properties".to_string(), serde_json::json!({})),
                    ("required".to_string(), serde_json::json!([])),
                    ("additionalProperties".to_string(), serde_json::json!(true)),
                ]),
                max_uses: None,
            }]);

        let normalized = normalize_request(&req).expect("normalization should succeed");
        let tool = normalized
            .request
            .tools
            .as_ref()
            .and_then(|tools| tools.first())
            .expect("tool should exist after normalization");

        assert_eq!(tool.description, "Client-provided tool 'demo_tool'");
    }
```

- [ ] **Step 2: Add a failing test for empty tool name rejection**

Add a validation-focused test:

```rust
    #[test]
    fn convert_request_rejects_tool_with_empty_name() {
        let mut req = base_request(vec![AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Hello"),
            }]);
        req.tools = Some(vec![AnthropicTool {
            tool_type: None,
                name: "   ".to_string(),
                description: "demo".to_string(),
                input_schema: HashMap::from([
                    ("type".to_string(), serde_json::json!("object")),
                    ("properties".to_string(), serde_json::json!({})),
                    ("required".to_string(), serde_json::json!([])),
                    ("additionalProperties".to_string(), serde_json::json!(true)),
                ]),
                max_uses: None,
            }]);

        let err = convert_request(&req).expect_err("empty tool name should be rejected");
        let message = err.to_string();
        assert!(message.contains("tool 0 has empty name"));
    }
```

- [ ] **Step 3: Add a failing test proving `anyOf` must remain allowed**

Add a regression test so later edits do not over-tighten tool schemas:

```rust
    #[test]
    fn convert_request_keeps_anyof_tool_schema_intact() {
        let mut req = base_request(vec![AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Hello"),
            }]);
        req.tools = Some(vec![AnthropicTool {
            tool_type: None,
                name: "convert_number".to_string(),
                description: "Convert a number".to_string(),
                input_schema: HashMap::from([
                    ("type".to_string(), serde_json::json!("object")),
                    (
                        "properties".to_string(),
                        serde_json::json!({
                            "size": {
                                "anyOf": [{"type": "integer"}, {"type": "null"}]
                            }
                        }),
                    ),
                    ("required".to_string(), serde_json::json!([])),
                    ("additionalProperties".to_string(), serde_json::json!(true)),
                ]),
                max_uses: None,
            }]);

        let result = convert_request(&req).expect("anyOf schema should remain allowed");
        assert_eq!(
            result.conversation_state.current_message.user_input_message
                .user_input_message_context.tools.len(),
            1
        );
    }
```

- [ ] **Step 4: Run the new narrow tests and confirm they fail for the right reasons**

Run:

```bash
cargo test -p static-flow-backend normalize_request_fills_empty_tool_description_with_stable_placeholder -- --nocapture
cargo test -p static-flow-backend convert_request_rejects_tool_with_empty_name -- --nocapture
cargo test -p static-flow-backend convert_request_keeps_anyof_tool_schema_intact -- --nocapture
```

Expected:

- The normalization test fails because descriptions are still passed through unchanged
- The empty-name test fails because there is no tool-level validation yet
- The `anyOf` test should already pass or fail for an unrelated reason; if it fails, stop and record the exact failure before changing code

- [ ] **Step 5: Commit the red test additions**

```bash
git add backend/src/kiro_gateway/anthropic/converter.rs
git commit -m "test: cover kiro tool normalization boundaries"
```

---

### Task 2: Implement tool normalization and validation in the converter pipeline

**Files:**
- Modify: `backend/src/kiro_gateway/anthropic/converter.rs`
- Test: `backend/src/kiro_gateway/anthropic/converter.rs`

- [ ] **Step 1: Add tool normalization event structures**

Near the existing `NormalizationEvent` support, add converter-local tool event types:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolNormalizationEvent {
    pub tool_index: usize,
    pub tool_name: String,
    pub action: &'static str,
    pub reason: &'static str,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ToolValidationSummary {
    pub normalized_tool_description_count: usize,
    pub empty_tool_name_count: usize,
    pub schema_keyword_counts: BTreeMap<String, usize>,
}
```

Extend `NormalizedRequest` with:

```rust
pub(crate) struct NormalizedRequest {
    pub request: MessagesRequest,
    pub tool_use_id_rewrites: Vec<ToolUseIdRewrite>,
    pub normalization_events: Vec<NormalizationEvent>,
    pub tool_normalization_events: Vec<ToolNormalizationEvent>,
    pub tool_validation_summary: ToolValidationSummary,
    message_index_map: Vec<usize>,
}
```

- [ ] **Step 2: Add tool helper functions before validation**

In `converter.rs`, add focused helpers:

```rust
fn normalize_tool_description(name: &str, description: &str) -> Option<String> {
    if description.trim().is_empty() {
        Some(format!("Client-provided tool '{name}'"))
    } else {
        None
    }
}

fn collect_schema_keywords(value: &serde_json::Value, counts: &mut BTreeMap<String, usize>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                match key.as_str() {
                    "anyOf" | "oneOf" | "allOf" | "contains" | "dependentSchemas" => {
                        *counts.entry(key.clone()).or_default() += 1;
                    }
                    _ => {}
                }
                collect_schema_keywords(child, counts);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                collect_schema_keywords(child, counts);
            }
        }
        _ => {}
    }
}
```

- [ ] **Step 3: Add `normalize_tools(...)` and call it from `normalize_request(...)`**

Implement a new helper:

```rust
fn normalize_tools(
    tools: &Option<Vec<super::types::Tool>>,
) -> Result<(Option<Vec<super::types::Tool>>, Vec<ToolNormalizationEvent>, ToolValidationSummary), ConversionError> {
    let Some(tools) = tools else {
        return Ok((None, Vec::new(), ToolValidationSummary::default()));
    };

    let mut normalized = Vec::with_capacity(tools.len());
    let mut events = Vec::new();
    let mut summary = ToolValidationSummary::default();

    for (tool_index, tool) in tools.iter().enumerate() {
        let name = tool.name.trim();
        if name.is_empty() {
            summary.empty_tool_name_count += 1;
            return Err(invalid_request(format!("tool {tool_index} has empty name")));
        }

        let mut normalized_tool = tool.clone();
        normalized_tool.name = name.to_string();

        if let Some(description) = normalize_tool_description(name, &tool.description) {
            normalized_tool.description = description;
            summary.normalized_tool_description_count += 1;
            events.push(ToolNormalizationEvent {
                tool_index,
                tool_name: normalized_tool.name.clone(),
                action: "fill_tool_description",
                reason: "empty_tool_description",
            });
        }

        collect_schema_keywords(
            &serde_json::Value::Object(normalized_tool.input_schema.clone().into_iter().collect()),
            &mut summary.schema_keyword_counts,
        );

        normalized.push(normalized_tool);
    }

    Ok((Some(normalized), events, summary))
}
```

Then update `normalize_request(...)` so the final `MessagesRequest` uses normalized tools instead of `req.tools.clone()`.

- [ ] **Step 4: Keep request validation strict for messages and minimal for tools**

Do not expand `validate_messages_request(...)` into schema validation. Keep the change narrow:

- `normalize_request(...)` rejects empty tool names before validation
- `convert_tools(...)` continues to convert schemas as-is
- no schema keyword is rejected in this task

This preserves the confirmed working `anyOf` behavior.

- [ ] **Step 5: Re-run the converter tests**

Run:

```bash
cargo test -p static-flow-backend normalize_request_fills_empty_tool_description_with_stable_placeholder -- --nocapture
cargo test -p static-flow-backend convert_request_rejects_tool_with_empty_name -- --nocapture
cargo test -p static-flow-backend convert_request_keeps_anyof_tool_schema_intact -- --nocapture
```

Expected:

- All three tests pass

- [ ] **Step 6: Commit the converter implementation**

```bash
git add backend/src/kiro_gateway/anthropic/converter.rs
git commit -m "fix: normalize invalid kiro tool metadata"
```

---

### Task 3: Emit structured tool normalization diagnostics from the request handler

**Files:**
- Modify: `backend/src/kiro_gateway/anthropic/mod.rs`
- Test: `backend/src/kiro_gateway/anthropic/mod.rs`

- [ ] **Step 1: Add a helper that logs tool normalization events**

Near the existing normalization logging helpers in `mod.rs`, add:

```rust
fn log_tool_normalization_events(
    ctx: &RequestLogContext<'_>,
    normalized: &NormalizedRequest,
) {
    for event in &normalized.tool_normalization_events {
        tracing::warn!(
            key_id = %ctx.key_record.id,
            key_name = %ctx.key_record.name,
            route = ctx.route,
            requested_model = ctx.requested_model,
            effective_model = ctx.effective_model,
            stream = ctx.stream,
            buffered_for_cc = ctx.buffered_for_cc,
            request_validation_enabled = ctx.request_validation_enabled,
            tool_index = event.tool_index,
            tool_name = %event.tool_name,
            normalization_action = event.action,
            normalization_reason = event.reason,
            "normalized kiro tool metadata before validation"
        );
    }
}
```

- [ ] **Step 2: Add a helper that logs the summary at debug/info level**

Add:

```rust
fn log_tool_validation_summary(
    ctx: &RequestLogContext<'_>,
    normalized: &NormalizedRequest,
) {
    tracing::info!(
        key_id = %ctx.key_record.id,
        key_name = %ctx.key_record.name,
        route = ctx.route,
        requested_model = ctx.requested_model,
        effective_model = ctx.effective_model,
        stream = ctx.stream,
        buffered_for_cc = ctx.buffered_for_cc,
        request_validation_enabled = ctx.request_validation_enabled,
        normalized_tool_description_count =
            normalized.tool_validation_summary.normalized_tool_description_count,
        empty_tool_name_count = normalized.tool_validation_summary.empty_tool_name_count,
        schema_keyword_counts = ?normalized.tool_validation_summary.schema_keyword_counts,
        "prepared kiro tool validation summary before upstream call"
    );
}
```

- [ ] **Step 3: Call both helpers from the existing normalized-request path**

In the request entry path that already logs message normalization and duplicate `tool_use_id` rewrites, insert:

```rust
    log_tool_normalization_events(&request_ctx, &normalized);
    log_tool_validation_summary(&request_ctx, &normalized);
```

Place them after `normalize_request(payload)` succeeds and before conversion starts, so failures still produce diagnostics.

- [ ] **Step 4: Add a narrow logging-oriented regression test**

Add a test in `mod.rs` that verifies the summary object survives normalization:

```rust
    #[test]
    fn normalize_request_reports_tool_description_fill_summary() {
        let normalized = normalize_request(&base_request_with_empty_tool_description())
            .expect("normalization should succeed");

        assert_eq!(
            normalized.tool_validation_summary.normalized_tool_description_count,
            1
        );
        assert_eq!(normalized.tool_validation_summary.empty_tool_name_count, 0);
        assert_eq!(normalized.tool_normalization_events.len(), 1);
        assert_eq!(normalized.tool_normalization_events[0].reason, "empty_tool_description");
    }
```

- [ ] **Step 5: Run the narrow handler/module tests**

Run:

```bash
cargo test -p static-flow-backend normalize_request_reports_tool_description_fill_summary -- --nocapture
cargo test -p static-flow-backend kiro_gateway::anthropic::converter::tests -- --nocapture
```

Expected:

- The new module test passes
- Existing converter tests continue to pass

- [ ] **Step 6: Commit the logging changes**

```bash
git add backend/src/kiro_gateway/anthropic/mod.rs backend/src/kiro_gateway/anthropic/converter.rs
git commit -m "feat: log kiro tool normalization diagnostics"
```

---

### Task 4: Format, run full targeted verification, and prepare merge-ready output

**Files:**
- Modify: `backend/src/kiro_gateway/anthropic/converter.rs`
- Modify: `backend/src/kiro_gateway/anthropic/mod.rs`

- [ ] **Step 1: Format only the touched backend crate files**

Run:

```bash
cargo fmt -p static-flow-backend -- backend/src/kiro_gateway/anthropic/converter.rs backend/src/kiro_gateway/anthropic/mod.rs
```

Expected:

- The command succeeds without touching unrelated crates or submodules

- [ ] **Step 2: Run the focused backend test suites**

Run:

```bash
cargo test -p static-flow-backend kiro_gateway::anthropic::converter::tests -- --nocapture
cargo test -p static-flow-backend kiro_gateway::anthropic::tests -- --nocapture
```

Expected:

- Both suites pass with no new failures

- [ ] **Step 3: Run clippy for the affected crate**

Run:

```bash
cargo clippy -p static-flow-backend -- -D warnings
```

Expected:

- `Finished` with zero warnings and zero errors

- [ ] **Step 4: Inspect the diff before final commit**

Run:

```bash
git diff -- backend/src/kiro_gateway/anthropic/converter.rs backend/src/kiro_gateway/anthropic/mod.rs
git status --short
```

Expected:

- Only the planned files are changed
- No accidental formatting churn or unrelated edits appear

- [ ] **Step 5: Create the final integration commit**

```bash
git add backend/src/kiro_gateway/anthropic/converter.rs backend/src/kiro_gateway/anthropic/mod.rs
git commit -m "fix: validate and normalize kiro tool metadata"
```
