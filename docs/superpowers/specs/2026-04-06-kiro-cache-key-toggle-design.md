# Kiro Per-Key Cache Estimation Toggle Design

## Goal

为 Kiro key 增加一个逐 key 的协议级开关，控制该 key 是否启用保守 cache 估算。

这个开关必须真实影响对外 Anthropic 响应里的 `cache_read_input_tokens`，以及
持久化 usage event 里的 `input_cached_tokens` / `input_uncached_tokens`，而不是仅仅
影响后台展示。

## Chosen Design

采用逐 key 布尔开关：`kiro_cache_estimation_enabled`。

- 存储位置：`LlmGatewayKeyRecord`
- 默认值：`true`
- 管理入口：`/admin/kiro-gateway` 的 key 卡片
- 行为：
  - `true`：启用当前保守 cache 估算
  - `false`：禁用估算，协议与 usage event 都回退为 `input_cached_tokens = 0`

## Why This Design

- 这是 key 的对外协议行为，挂在 key 上最符合真实来源。
- 默认值保持 `true`，不破坏现有 userspace。
- 与全局 runtime config 解耦，不会影响其他 key。

## Backend Changes

### Storage

在 `llm_gateway_keys` schema 中新增：

- `kiro_cache_estimation_enabled: bool`

migration 默认补成 `true`。

### Admin DTOs

扩展：

- `AdminKiroKeyView`
- `PatchKiroKeyRequest`
- 前端对应 API 类型

### Request Handling

Kiro 请求路径读取 key 的该字段。

- 非流式响应：决定 `usage.cache_read_input_tokens`
- 流式最终 usage summary：决定落库 cached/uncached split
- buffered stream 回写 usage：同上

关闭时不再调用 cache 估算公式，而是保守地：

- `input_cached_tokens = 0`
- `input_uncached_tokens = safe_input_total`
- `cache_read_input_tokens = 0`
- `cache_creation_input_tokens = 0`

## Frontend Changes

在 `/admin/kiro-gateway` 的 key 卡片上新增一项布尔开关，紧邻现有 Kiro request validation 开关。

文案明确指出：

- 开启：对外回包会暴露保守估算的 cache token
- 关闭：对外 cache token 始终为 0

## Verification

至少覆盖：

- 默认值为 `true`
- 关闭时响应 usage 的 cache 字段为 0
- 关闭时 usage event 的 cached tokens 为 0
- 前端 patch 请求能透传该字段
