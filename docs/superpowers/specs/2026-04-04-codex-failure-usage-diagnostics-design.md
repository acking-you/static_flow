# Codex Failure Usage Diagnostics Design

## Goal

让 Codex API 代理链路和 Kiro 一样，在“已通过 key 鉴权之后”的失败场景里也把失败样本写入 `llm_gateway_usage_events`，并且在 admin 界面现有的 `last_message_content` 区域直接看到完整诊断信息。

本次设计明确不新增 LanceDB 列，不新增独立诊断表，不改 admin 展示结构。成功请求继续保持当前语义；失败请求把 `last_message_content` 作为诊断 JSON 载体。

## Scope

### In Scope

- Codex 公共代理主链路的失败 usage event 持久化
- 失败诊断 JSON 结构设计与落表
- 成功路径与失败路径共用 usage event 构造逻辑
- 请求体保留策略：
  - 原始 client request body
  - 最终 upstream request body
- 覆盖所有“已通过 key 鉴权后”的失败点

### Out of Scope

- API key 缺失、key hash 无效、key disabled、quota exhausted 这类鉴权前或鉴权阶段失败的 usage event 持久化
- LanceDB schema 变更
- admin 页面新增字段或新弹窗
- 模型页、余额页、models 接口等非请求代理链路的诊断增强

## Existing Problem

Codex 当前只有“已经拿到 upstream 响应并走到 `persist_gateway_usage()`”的路径才会记录 usage event。结果是以下失败样本会直接丢失：

- key 已鉴权，但选不到可用 codex 账号
- key 级本地并发/节流拒绝
- 请求 body 读取/JSON 解析/归一化失败
- upstream transport 失败
- SSE 聚合流或直通流在读取过程中失败

这会导致 admin usage events 只能看到“成功”和“部分非 2xx 上游响应”，看不到真正最难排查的少量异常样本。

## Constraints

- 不破坏 userspace：成功请求的 `last_message_content` 继续表示最后一条消息摘要
- 失败请求允许复用 `last_message_content` 承载诊断 JSON
- 不新增表列，避免 LanceDB schema 变重
- admin 现有详情页必须直接可用，不要求联动前端改造
- 行为边界对齐 Kiro：只覆盖 post-auth failure

## Recommended Approach

采用“Codex 对齐 Kiro”的单表诊断策略：

1. 成功请求继续写普通 usage event
2. 失败请求也写 usage event
3. 成功事件的 `last_message_content` 继续存最后一条消息
4. 失败事件的 `last_message_content` 改存 pretty JSON 诊断包

这保持了 schema 稳定，也让 admin 现有 modal 直接变成失败样本查看器。

## Data Model

### Existing Event Fields Reused

- `status_code`
- `request_method`
- `request_url`
- `endpoint`
- `model`
- `client_ip`
- `ip_region`
- `request_headers_json`
- `last_message_content`

### New In-Memory Diagnostic Inputs

不改表结构，只在运行时补充失败诊断需要的上下文：

- 原始 client request body
- 最终 upstream request body
- 原始最后一条消息摘要
- failure stage
- error text
- optional details JSON

### Failure Diagnostic JSON Shape

失败时写入 `last_message_content` 的 JSON 结构统一为：

```json
{
  "kind": "codex_failure_diagnostic",
  "failure_stage": "send_upstream",
  "status_code": 502,
  "request_method": "POST",
  "request_url": "/api/llm-gateway/v1/responses",
  "endpoint": "/v1/responses",
  "model": "gpt-5.3-codex",
  "account_name": "acct-a",
  "original_last_message_content": "hello",
  "client_request_body": { "...": "..." },
  "upstream_request_body": { "...": "..." },
  "error": "upstream request failed: ...",
  "details": {
    "proxy_url": "http://127.0.0.1:11112",
    "is_timeout": true
  }
}
```

如果 body 不是合法 JSON，就退化成字符串值，而不是丢掉。

## Failure Coverage

### 1. Account Routing Failures

`resolve_auth_for_key()` 在 key 已通过鉴权后仍可能返回：

- fixed account 不可用
- auto subset 没有 existing account
- auto subset 没有 usable account
- route strategy 非法
- legacy auth reload 失败

这些都应写失败 usage event。

### 2. Local Key Scheduler Rejections

`request_scheduler.try_acquire()` 的 key 级并发/节流拒绝属于 post-auth failure，也应写失败 usage event。

这里的诊断重点是：

- rejection reason
- in_flight
- max_concurrency
- min_start_interval_ms
- wait_ms
- elapsed_since_last_start_ms

### 3. Request Normalization Failures

`prepare_gateway_request()` 现在在读取 body、JSON 解析、chat/responses 归一化失败时直接返回错误。需要改造成“失败时仍保留原始 body 用于诊断”。

设计要求：

- 先读出 raw body
- 归一化成功时生成最终 `PreparedGatewayRequest`
- 归一化失败时仍能把 raw client body 写入失败 usage event

### 4. Upstream Transport / Retry Failures

`send_upstream()` / `send_upstream_with_retry()` 出错时，需要把：

- 原始 client body
- 最终 upstream body
- proxy metadata
- selected account
- upstream URL
- reqwest error flags

一起写进失败 usage event。

### 5. Upstream Non-Success Responses

Codex 当前对非 2xx 已经会记录 usage event，但只写普通 `last_message_content`。这里要改成：

- 成功响应：保留原逻辑
- 非 2xx 响应：`last_message_content` 存诊断 JSON
- `details` 里包含 upstream response body preview/full body（可承受文本量时优先完整 body）

### 6. Non-Stream Body Read Failures

`upstream.bytes().await` 失败时，当前直接返回 internal error。应补失败 usage event。

### 7. SSE Aggregate / Direct Stream Failures

两类流都要覆盖：

- force-upstream-stream 聚合路径
- 直通 SSE 路径

如果流已经开始但中途失败，usage event 用合成状态码 `599` 标记，和 Kiro 对齐。对外响应行为保持现状，不为了持久化而改变客户端流协议。

## Implementation Design

### Shared Builder

抽出 Codex 的 usage event builder：

- 成功路径传普通 `last_message_content`
- 失败路径传诊断 JSON

避免在多个失败点重复拼 `LlmGatewayUsageEventRecord`。

### Diagnostic Builder

新增一个 codex failure diagnostic builder，输入：

- `PreparedGatewayRequest` 或 raw client body
- `LlmGatewayEventContext`
- `selected_account_name`
- `failure_stage`
- `status_code`
- `error`
- `details`
- `original_last_message_content`

输出 pretty JSON string。

### Request Representation

为避免请求归一化失败时拿不到原始 body，需要把读取 body 和归一化逻辑解耦：

- raw client body 始终保留
- 最终 upstream body 只在准备成功后生成

`PreparedGatewayRequest` 增加：

- `client_request_body`

现有 `request_body` 继续表示最终 upstream body。

### Persistence Rules

- `status_code == 200/2xx`
  - `last_message_content = extracted last message`
- `status_code != 2xx` 或流中途失败
  - `last_message_content = diagnostic JSON`

### UI Compatibility

前端不需要改，因为 admin usage detail modal 已经直接把 `last_message_content` 以 `<pre>` 展示并支持复制。

## Testing Plan

### Unit Tests

- 成功 usage event 仍保留普通最后一条消息
- 失败 usage event 会把诊断 JSON 写进 `last_message_content`
- 诊断 JSON 同时包含 `client_request_body` 和 `upstream_request_body`
- body 非 JSON 时回退为字符串
- `599` 流失败状态会被保留

### Focused Behavior Tests

- request normalization failure 会落 usage event
- local scheduler rejection 会落 usage event
- upstream transport failure 会落 usage event
- upstream 400/500 会落带详细 body 的 usage event
- SSE 中途失败会落 usage event

### Regression Expectations

- admin usage 列表/详情页无需改动即可看到失败诊断 JSON
- 成功事件展示不变
- usage rollup 继续基于同一张事件表工作

## Risks

### Request Body Volume

失败事件会把完整 request body 放进 `last_message_content`。这是用户明确接受的取舍，但会增大失败事件单条记录体积。由于目标是“几千次里少量失败样本”，这个成本是可接受的。

### Mixed Semantics in One Column

`last_message_content` 在成功和失败事件中语义不同，但这是有意设计，并且可以通过 `status_code` 与 JSON `kind` 一眼识别，不需要额外 schema 负担。

### Stream Failure Semantics

流中途失败未必对应真实 HTTP 非 2xx，因此用合成 `599`。这不是协议语义，而是内部诊断语义，目的是让 admin usage event 能明确标出这类失败样本。

## Acceptance Criteria

- Codex post-auth failure 会落 usage events
- 失败事件在 admin 中可直接看到完整诊断 JSON
- 诊断 JSON 同时包含 client request body 和 upstream request body
- 成功路径展示保持原样
- 不新增 LanceDB 列
