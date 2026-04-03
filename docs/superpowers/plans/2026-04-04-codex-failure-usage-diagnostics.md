# Codex Failure Usage Diagnostics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 Codex 网关在所有 post-auth 失败路径里也写入 usage events，并把完整失败诊断 JSON 直接存入 `last_message_content`，同时保持成功请求的现有展示语义不变。

**Architecture:** 复用现有 `llm_gateway_usage_events` 表，不增加 schema。通过在 `PreparedGatewayRequest` 中同时保留原始 client body 和最终 upstream body，再在 `backend/src/llm_gateway.rs` 中抽出成功/失败共用的 usage event builder 与失败诊断 JSON builder，把同步失败和流式失败都收敛到同一套持久化入口。

**Tech Stack:** Rust, Axum, reqwest, serde_json, LanceDB-backed `llm_gateway_usage_events`, existing admin usage modal

---

### Task 1: 保留原始 Client Body 并先写失败测试

**Files:**
- Modify: `backend/src/llm_gateway/types.rs`
- Modify: `backend/src/llm_gateway/request.rs`
- Test: `backend/src/llm_gateway/request.rs`

- [ ] **Step 1: 为 `PreparedGatewayRequest` 加入原始 client body 字段，并写失败测试用例**

```rust
#[derive(Debug, Clone)]
pub(crate) struct PreparedGatewayRequest {
    pub original_path: String,
    pub upstream_path: String,
    pub method: Method,
    pub client_request_body: axum::body::Bytes,
    pub request_body: axum::body::Bytes,
    pub model: Option<String>,
    pub client_visible_model: Option<String>,
    pub wants_stream: bool,
    pub force_upstream_stream: bool,
    pub content_type: String,
    pub response_adapter: GatewayResponseAdapter,
    pub thread_anchor: Option<String>,
    pub tool_name_restore_map: BTreeMap<String, String>,
    pub billable_multiplier: u64,
}

#[tokio::test]
async fn prepare_gateway_request_preserves_raw_body_on_chat_validation_error() {
    let headers = axum::http::HeaderMap::new();
    let body = axum::body::Body::from(
        r#"{"model":"gpt-5.3-codex","messages":[{"role":"user","content":[{"type":"image_url"}]}]}"#,
    );

    let err = prepare_gateway_request(
        "/v1/chat/completions",
        "",
        axum::http::Method::POST,
        &headers,
        body,
        1024 * 1024,
    )
    .await
    .expect_err("unsupported chat payload should fail");

    assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    assert!(err.1 .0.error.contains("content"));
}
```

- [ ] **Step 2: 运行失败测试，确认当前代码没有保留 raw client body 的测试支撑**

Run: `cargo test -p static-flow-backend prepare_gateway_request_preserves_raw_body_on_chat_validation_error -- --nocapture`

Expected: FAIL，原因是测试不存在或 `PreparedGatewayRequest` 尚未携带 `client_request_body`

- [ ] **Step 3: 修改 `prepare_gateway_request()`，始终保留 raw body，再在成功时单独生成 upstream body**

```rust
let raw_body = to_bytes(body, max_request_body_bytes)
    .await
    .map_err(|err| internal_error("Failed to read llm gateway request body", err))?;

let mut json_value = if content_type.starts_with("application/json") && !raw_body.is_empty() {
    serde_json::from_slice::<Value>(&raw_body)
        .map(Some)
        .map_err(|err| bad_request_with_detail("Invalid JSON body", err))?
} else {
    None
};

let request_body = match json_value {
    Some(value) => Bytes::from(
        serde_json::to_vec(&value)
            .map_err(|err| internal_error("Failed to encode gateway request body", err))?,
    ),
    None => raw_body.clone(),
};

Ok(PreparedGatewayRequest {
    original_path,
    upstream_path,
    method,
    client_request_body: raw_body,
    request_body,
    model,
    client_visible_model: None,
    wants_stream: original_wants_stream,
    force_upstream_stream,
    content_type,
    response_adapter,
    thread_anchor,
    tool_name_restore_map,
    billable_multiplier,
})
```

- [ ] **Step 4: 补充一个成功路径测试，确认 upstream body 仍会被归一化**

```rust
#[tokio::test]
async fn prepare_gateway_request_keeps_raw_client_body_and_normalized_upstream_body() {
    let headers = axum::http::HeaderMap::new();
    let body = axum::body::Body::from(
        r#"{"model":"gpt-5.3-codex","input":"hello"}"#,
    );

    let prepared = prepare_gateway_request(
        "/v1/responses",
        "",
        axum::http::Method::POST,
        &headers,
        body,
        1024 * 1024,
    )
    .await
    .expect("responses request should normalize");

    let raw: serde_json::Value =
        serde_json::from_slice(&prepared.client_request_body).expect("raw body json");
    let upstream: serde_json::Value =
        serde_json::from_slice(&prepared.request_body).expect("upstream body json");

    assert_eq!(raw["input"], "hello");
    assert_eq!(upstream["input"], "hello");
    assert_eq!(upstream["stream"], true);
}
```

- [ ] **Step 5: 运行 request 模块测试**

Run: `cargo test -p static-flow-backend prepare_gateway_request_ -- --nocapture`

Expected: PASS，新增的 request 相关测试通过

- [ ] **Step 6: Commit**

```bash
git add backend/src/llm_gateway/types.rs backend/src/llm_gateway/request.rs
git commit -m "test: preserve raw codex client request body"
```

### Task 2: 抽出 Codex 成功/失败共用 usage event builder 和诊断 JSON builder

**Files:**
- Modify: `backend/src/llm_gateway.rs`
- Test: `backend/src/llm_gateway.rs`

- [ ] **Step 1: 先写两个失败测试，锁定成功/失败 `last_message_content` 语义**

```rust
#[test]
fn build_codex_failure_usage_event_preserves_status_and_diagnostic_payload() {
    let key = sample_public_lookup_key();
    let prepared = PreparedGatewayRequest {
        original_path: "/v1/responses".to_string(),
        upstream_path: "/v1/responses".to_string(),
        method: axum::http::Method::POST,
        client_request_body: Bytes::from_static(br#"{"input":"hello"}"#),
        request_body: Bytes::from_static(br#"{"input":"hello","stream":true}"#),
        model: Some("gpt-5.3-codex".to_string()),
        client_visible_model: None,
        wants_stream: false,
        force_upstream_stream: true,
        content_type: "application/json".to_string(),
        response_adapter: GatewayResponseAdapter::Responses,
        thread_anchor: None,
        tool_name_restore_map: BTreeMap::new(),
        billable_multiplier: 1,
    };
    let context = LlmGatewayEventContext {
        request_method: "POST".to_string(),
        request_url: "/api/llm-gateway/v1/responses".to_string(),
        client_ip: "127.0.0.1".to_string(),
        ip_region: "local".to_string(),
        request_headers_json: "{}".to_string(),
        started_at: Instant::now(),
    };

    let diagnostic = build_codex_failure_diagnostic_payload(
        &prepared,
        Some(&context),
        Some("acct-a"),
        "send_upstream",
        502,
        "upstream request failed",
        None,
    );
    let event = build_gateway_usage_event_record(
        &key,
        &prepared,
        &context,
        12,
        502,
        UsageBreakdown {
            usage_missing: true,
            ..UsageBreakdown::default()
        },
        Some(diagnostic.clone()),
        Some("acct-a"),
    );

    assert_eq!(event.status_code, 502);
    assert_eq!(event.last_message_content.as_deref(), Some(diagnostic.as_str()));
}

#[test]
fn codex_failure_diagnostic_payload_contains_client_and_upstream_bodies() {
    let prepared = PreparedGatewayRequest {
        original_path: "/v1/responses".to_string(),
        upstream_path: "/v1/responses".to_string(),
        method: axum::http::Method::POST,
        client_request_body: Bytes::from_static(br#"{"input":"hello"}"#),
        request_body: Bytes::from_static(br#"{"input":"hello","stream":true}"#),
        model: Some("gpt-5.3-codex".to_string()),
        client_visible_model: None,
        wants_stream: false,
        force_upstream_stream: true,
        content_type: "application/json".to_string(),
        response_adapter: GatewayResponseAdapter::Responses,
        thread_anchor: None,
        tool_name_restore_map: BTreeMap::new(),
        billable_multiplier: 1,
    };

    let payload = build_codex_failure_diagnostic_payload(
        &prepared,
        None,
        Some("acct-a"),
        "request_validation",
        400,
        "Invalid JSON body",
        Some(serde_json::json!({ "detail": "bad field" })),
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&payload).expect("diagnostic json");

    assert_eq!(parsed["kind"], "codex_failure_diagnostic");
    assert_eq!(parsed["client_request_body"]["input"], "hello");
    assert_eq!(parsed["upstream_request_body"]["stream"], true);
}
```

- [ ] **Step 2: 运行测试，确认当前代码没有这些 builder**

Run: `cargo test -p static-flow-backend codex_failure_diagnostic_ -- --nocapture`

Expected: FAIL，原因是当前代码还没有实现 `build_gateway_usage_event_record` 和 `build_codex_failure_diagnostic_payload`

- [ ] **Step 3: 在 `backend/src/llm_gateway.rs` 抽出成功/失败共用 usage event builder 和失败诊断 builder**

```rust
fn build_gateway_usage_event_record(
    current: &LlmGatewayKeyRecord,
    prepared: &PreparedGatewayRequest,
    context: &LlmGatewayEventContext,
    latency_ms: i32,
    status_code: u16,
    usage: UsageBreakdown,
    last_message_content: Option<String>,
    selected_account_name: Option<&str>,
) -> LlmGatewayUsageEventRecord {
    LlmGatewayUsageEventRecord {
        id: generate_id("llm-usage"),
        key_id: current.id.clone(),
        key_name: current.name.clone(),
        provider_type: LLM_GATEWAY_PROVIDER_CODEX.to_string(),
        account_name: selected_account_name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        request_method: context.request_method.clone(),
        request_url: context.request_url.clone(),
        latency_ms,
        endpoint: prepared.upstream_path.clone(),
        model: prepared.model.clone(),
        status_code: status_code as i32,
        input_uncached_tokens: usage.input_uncached_tokens,
        input_cached_tokens: usage.input_cached_tokens,
        output_tokens: usage.output_tokens,
        billable_tokens: usage.billable_tokens_with_multiplier(prepared.billable_multiplier),
        usage_missing: usage.usage_missing,
        credit_usage: None,
        credit_usage_missing: false,
        client_ip: context.client_ip.clone(),
        ip_region: context.ip_region.clone(),
        request_headers_json: context.request_headers_json.clone(),
        last_message_content,
        created_at: now_ms(),
    }
}

fn build_codex_failure_diagnostic_payload(
    prepared: &PreparedGatewayRequest,
    event_context: Option<&LlmGatewayEventContext>,
    selected_account_name: Option<&str>,
    failure_stage: &str,
    status_code: u16,
    error: &str,
    details: Option<serde_json::Value>,
) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "kind": "codex_failure_diagnostic",
        "failure_stage": failure_stage,
        "status_code": status_code,
        "request_method": event_context.map(|ctx| ctx.request_method.clone()),
        "request_url": event_context.map(|ctx| ctx.request_url.clone()),
        "endpoint": prepared.upstream_path,
        "model": prepared.model,
        "account_name": selected_account_name,
        "original_last_message_content": extract_last_message_content(&prepared.client_request_body).ok().flatten(),
        "client_request_body": maybe_parse_gateway_json_bytes(&prepared.client_request_body),
        "upstream_request_body": maybe_parse_gateway_json_bytes(&prepared.request_body),
        "error": error,
        "details": details.unwrap_or(serde_json::json!({})),
    }))
    .unwrap_or_else(|serialize_err| {
        format!(
            "{{\"kind\":\"codex_failure_diagnostic\",\"failure_stage\":{:?},\"status_code\":{},\"error\":{:?},\"serialize_error\":{:?}}}",
            failure_stage,
            status_code,
            error,
            serialize_err.to_string()
        )
    })
}
```

- [ ] **Step 4: 把现有 `persist_gateway_usage()` 改为调用共用 builder，成功路径仍然提取最后一条消息**

```rust
let last_message_content = match extract_last_message_content(&prepared.client_request_body) {
    Ok(content) => content,
    Err(err) => {
        tracing::debug!(key_id = %current.id, "Failed to extract last message content: {err}");
        Some(LAST_MESSAGE_CONTENT_EXTRACT_FAILED.to_string())
    }
};

let event = build_gateway_usage_event_record(
    &current,
    prepared,
    &context,
    latency_ms,
    status_code,
    usage,
    last_message_content,
    selected_account_name,
);
```

- [ ] **Step 5: 运行新增 builder 测试**

Run: `cargo test -p static-flow-backend build_codex_failure_usage_event_preserves_status_and_diagnostic_payload codex_failure_diagnostic_payload_contains_client_and_upstream_bodies -- --nocapture`

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add backend/src/llm_gateway.rs
git commit -m "test: add codex failure usage event builders"
```

### Task 3: 覆盖同步失败路径并持久化失败 usage events

**Files:**
- Modify: `backend/src/llm_gateway.rs`
- Test: `backend/src/llm_gateway.rs`

- [ ] **Step 1: 先写失败测试，锁定本地限流错误和失败 event 状态保留**

```rust
#[test]
fn build_codex_failure_usage_event_uses_diagnostic_payload_for_429() {
    let key = sample_public_lookup_key();
    let prepared = PreparedGatewayRequest {
        original_path: "/v1/responses".to_string(),
        upstream_path: "/v1/responses".to_string(),
        method: axum::http::Method::POST,
        client_request_body: Bytes::from_static(br#"{"input":"hello"}"#),
        request_body: Bytes::from_static(br#"{"input":"hello","stream":true}"#),
        model: Some("gpt-5.3-codex".to_string()),
        client_visible_model: None,
        wants_stream: false,
        force_upstream_stream: true,
        content_type: "application/json".to_string(),
        response_adapter: GatewayResponseAdapter::Responses,
        thread_anchor: None,
        tool_name_restore_map: BTreeMap::new(),
        billable_multiplier: 1,
    };
    let context = LlmGatewayEventContext {
        request_method: "POST".to_string(),
        request_url: "/api/llm-gateway/v1/responses".to_string(),
        client_ip: "127.0.0.1".to_string(),
        ip_region: "local".to_string(),
        request_headers_json: "{}".to_string(),
        started_at: Instant::now(),
    };

    let payload = build_codex_failure_diagnostic_payload(
        &prepared,
        Some(&context),
        Some("acct-a"),
        "key_request_limit",
        429,
        "local_start_interval",
        Some(serde_json::json!({ "wait_ms": 123 })),
    );
    let event = build_gateway_usage_event_record(
        &key,
        &prepared,
        &context,
        10,
        429,
        UsageBreakdown {
            usage_missing: true,
            ..UsageBreakdown::default()
        },
        Some(payload.clone()),
        Some("acct-a"),
    );

    assert_eq!(event.status_code, 429);
    assert_eq!(event.last_message_content.as_deref(), Some(payload.as_str()));
}
```

- [ ] **Step 2: 运行测试，确认当前失败路径没有单独持久化入口**

Run: `cargo test -p static-flow-backend build_codex_failure_usage_event_uses_diagnostic_payload_for_429 -- --nocapture`

Expected: FAIL，原因是当前还没有 codex 失败 usage event 的独立持久化 helper

- [ ] **Step 3: 在 `backend/src/llm_gateway.rs` 新增失败持久化 helper，并在 `resolve_auth_for_key`、`request_scheduler.try_acquire`、`prepare_gateway_request`、`send_upstream_with_retry` 失败路径接入**

```rust
async fn persist_gateway_failure_usage(
    gateway: &LlmGatewayRuntimeState,
    cached_key: &CachedKeyLease,
    prepared: &PreparedGatewayRequest,
    status_code: u16,
    usage: UsageBreakdown,
    event_context: Option<&LlmGatewayEventContext>,
    selected_account_name: Option<&str>,
    failure_stage: &str,
    error: &str,
    details: Option<serde_json::Value>,
) -> Result<()> {
    let current = gateway
        .store
        .get_key_by_id(&cached_key.record.id)
        .await?
        .unwrap_or_else(|| cached_key.record.clone());
    let context = event_context.cloned().unwrap_or_else(|| LlmGatewayEventContext {
        request_method: prepared.method.as_str().to_string(),
        request_url: prepared.original_path.clone(),
        client_ip: "unknown".to_string(),
        ip_region: "Unknown".to_string(),
        request_headers_json: "{}".to_string(),
        started_at: Instant::now(),
    });
    let latency_ms = context
        .started_at
        .elapsed()
        .as_millis()
        .min(i32::MAX as u128) as i32;
    let diagnostic_payload = build_codex_failure_diagnostic_payload(
        prepared,
        Some(&context),
        selected_account_name,
        failure_stage,
        status_code,
        error,
        details,
    );
    let event = build_gateway_usage_event_record(
        &current,
        prepared,
        &context,
        latency_ms,
        status_code,
        usage,
        Some(diagnostic_payload),
        selected_account_name,
    );
    let _updated = gateway.append_usage_event(&current, &event).await?;
    Ok(())
}
```

```rust
let request_limit_lease = match state
    .llm_gateway
    .request_scheduler
    .try_acquire(&key_lease.record)
{
    Ok(lease) => lease,
    Err(rejection) => {
        let response = codex_key_request_limit_error(&key_lease.record, rejection.clone());
        let prepared = PreparedGatewayRequest {
            original_path: format!("{gateway_path}{query}"),
            upstream_path: gateway_path.clone(),
            method: parts.method.clone(),
            client_request_body: Bytes::new(),
            request_body: Bytes::new(),
            model: None,
            client_visible_model: None,
            wants_stream: false,
            force_upstream_stream: false,
            content_type: "application/json".to_string(),
            response_adapter: GatewayResponseAdapter::Responses,
            thread_anchor: None,
            tool_name_restore_map: BTreeMap::new(),
            billable_multiplier: 1,
        };
        let _ = persist_gateway_failure_usage(
            state.llm_gateway.as_ref(),
            key_lease.as_ref(),
            &prepared,
            response.0.as_u16(),
            UsageBreakdown {
                usage_missing: true,
                ..UsageBreakdown::default()
            },
            event_context.as_ref(),
            selected_account_name.as_deref(),
            "key_request_limit",
            &response.1.0.error,
            Some(serde_json::json!({
                "reason": rejection.reason,
                "in_flight": rejection.in_flight,
                "max_concurrency": rejection.max_concurrency,
                "min_start_interval_ms": rejection.min_start_interval_ms,
                "wait_ms": rejection.wait.map(|value| value.as_millis() as u64),
                "elapsed_since_last_start_ms": rejection.elapsed_since_last_start_ms,
            })),
        )
        .await;
        return Err(response);
    }
};
```

```rust
let response = match send_upstream_with_retry(
    &state,
    &prepared,
    &parts.headers,
    &auth_snapshot,
    selected_account_name.as_deref(),
)
.await
{
    Ok(response) => response,
    Err(err) => {
        let error_text = err.to_string();
        let _ = persist_gateway_failure_usage(
            state.llm_gateway.as_ref(),
            key_lease.as_ref(),
            &prepared,
            StatusCode::BAD_GATEWAY.as_u16(),
            UsageBreakdown {
                usage_missing: true,
                ..UsageBreakdown::default()
            },
            event_context.as_ref(),
            selected_account_name.as_deref(),
            "send_upstream",
            &error_text,
            None,
        )
        .await;
        return Err(internal_error("Failed to proxy llm gateway request", err));
    }
};
```

- [ ] **Step 4: 对上游非 2xx 响应改用失败诊断 JSON 而不是普通最后一条消息**

```rust
if !status.is_success() {
    let body_text = String::from_utf8_lossy(&body_bytes);
    persist_gateway_failure_usage(
        state.llm_gateway.as_ref(),
        key_lease.as_ref(),
        &prepared,
        status.as_u16(),
        UsageBreakdown {
            usage_missing: true,
            ..UsageBreakdown::default()
        },
        event_context.as_ref(),
        selected_account_name.as_deref(),
        "upstream_non_success",
        &format!("upstream returned non-success status {}", status.as_u16()),
        Some(serde_json::json!({
            "content_type": content_type,
            "upstream_body": body_text.to_string(),
        })),
    )
    .await?;
}
```

- [ ] **Step 5: 运行 llm gateway 定向测试**

Run: `cargo test -p static-flow-backend build_codex_failure_usage_event_ -- --nocapture`

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add backend/src/llm_gateway.rs
git commit -m "feat: persist codex failure usage events"
```

### Task 4: 覆盖 SSE 中途失败并完成全量验证

**Files:**
- Modify: `backend/src/llm_gateway.rs`
- Test: `backend/src/llm_gateway.rs`

- [ ] **Step 1: 写一个聚焦测试，锁定 `599` 失败状态和流失败诊断 JSON**

```rust
#[test]
fn build_codex_failure_diagnostic_payload_preserves_stream_failure_status() {
    let prepared = PreparedGatewayRequest {
        original_path: "/v1/responses".to_string(),
        upstream_path: "/v1/responses".to_string(),
        method: axum::http::Method::POST,
        client_request_body: Bytes::from_static(br#"{"input":"hello"}"#),
        request_body: Bytes::from_static(br#"{"input":"hello","stream":true}"#),
        model: Some("gpt-5.3-codex".to_string()),
        client_visible_model: None,
        wants_stream: true,
        force_upstream_stream: false,
        content_type: "application/json".to_string(),
        response_adapter: GatewayResponseAdapter::Responses,
        thread_anchor: None,
        tool_name_restore_map: BTreeMap::new(),
        billable_multiplier: 1,
    };

    let payload = build_codex_failure_diagnostic_payload(
        &prepared,
        None,
        Some("acct-a"),
        "stream_read",
        599,
        "failed to parse upstream SSE event",
        Some(serde_json::json!({ "stream_kind": "sse" })),
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&payload).expect("diagnostic json");

    assert_eq!(parsed["status_code"], 599);
    assert_eq!(parsed["failure_stage"], "stream_read");
}
```

- [ ] **Step 2: 运行测试，确认当前代码没有对流中途失败单独落 usage**

Run: `cargo test -p static-flow-backend build_codex_failure_diagnostic_payload_preserves_stream_failure_status -- --nocapture`

Expected: FAIL，原因是当前 SSE 中途失败还不会单独落 `599` 诊断事件

- [ ] **Step 3: 修改 `forward_upstream_response()` 的两条 SSE 路径，在聚合流和直通流中把中途失败写为 `599` 诊断事件**

```rust
const CODEX_STREAM_FAILURE_STATUS_CODE: u16 = 599;
```

```rust
let maybe_event = tokio::select! {
    biased;
    _ = shutdown_rx.changed() => {
        if *shutdown_rx.borrow() {
            None
        } else {
            continue;
        }
    }
    event = events.next() => event,
};

match event {
    Ok(event) => {
        collector.observe_event(&event);
        if stream_response_adapter == GatewayResponseAdapter::Responses {
            yield Ok::<Bytes, std::io::Error>(encode_sse_event_with_model_alias(
                &event,
                prepared.model.as_deref(),
                prepared.client_visible_model.as_deref(),
            ));
        } else if let Some(chunk) = convert_response_event_to_chat_chunk(
            &event,
            Some(&prepared.tool_name_restore_map),
            &mut chat_metadata,
            prepared.model.as_deref(),
            prepared.client_visible_model.as_deref(),
        ) {
            yield Ok::<Bytes, std::io::Error>(encode_json_sse_chunk(&chunk));
        }
    }
    Err(err) => {
        let _ = persist_gateway_failure_usage(
            gateway.as_ref(),
            stream_key_lease.as_ref(),
            &prepared,
            CODEX_STREAM_FAILURE_STATUS_CODE,
            UsageBreakdown {
                usage_missing: true,
                ..UsageBreakdown::default()
            },
            event_context.as_ref(),
            selected_account_name.as_deref(),
            "stream_read",
            &format!("failed to parse upstream SSE event: {err}"),
            Some(serde_json::json!({
                "stream_kind": "sse",
            })),
        )
        .await;
        yield Err(std::io::Error::other(format!(
            "failed to parse upstream SSE event: {err}"
        )));
        return;
    }
}
```

- [ ] **Step 4: 运行 llm gateway 测试全量子集**

Run: `cargo test -p static-flow-backend llm_gateway -- --nocapture`

Expected: PASS

- [ ] **Step 5: 运行格式化和静态检查**

Run: `rustfmt backend/src/llm_gateway.rs backend/src/llm_gateway/request.rs backend/src/llm_gateway/types.rs`

Expected: exit code 0

Run: `cargo clippy -p static-flow-backend --tests -- -D warnings`

Expected: PASS with zero warnings

- [ ] **Step 6: Commit**

```bash
git add backend/src/llm_gateway.rs backend/src/llm_gateway/request.rs backend/src/llm_gateway/types.rs
git commit -m "feat: record codex failure diagnostics in usage events"
```

## Self-Review

- Spec coverage:
  - post-auth account routing failure: Task 3
  - key-level scheduler rejection: Task 3
  - request normalization failure with raw body retention: Task 1 + Task 3
  - upstream transport / retry failure: Task 3
  - upstream non-success response diagnostics: Task 3
  - SSE mid-stream failure with `599`: Task 4
- Placeholder scan: no `TODO` / `TBD` / vague “handle appropriately” wording left in executable steps
- Type consistency:
  - `PreparedGatewayRequest.client_request_body`
  - `build_gateway_usage_event_record`
  - `build_codex_failure_diagnostic_payload`
  - `persist_gateway_failure_usage`
  - `CODEX_STREAM_FAILURE_STATUS_CODE`

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-04-04-codex-failure-usage-diagnostics.md`. Two execution options:

**1. Subagent-Driven (recommended)** - I dispatch a fresh subagent per task, review between tasks, fast iteration

**2. Inline Execution** - Execute tasks in this session using executing-plans, batch execution with checkpoints

Which approach?
