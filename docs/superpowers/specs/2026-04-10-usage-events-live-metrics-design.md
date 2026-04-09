# Usage Events Live Metrics And Full Request Persistence Design

## Goal

在不引入后台清理任务、不破坏现有 Usage Events userspace 的前提下，
为 `/admin/llm-gateway` 的 `Usage Events` 区域增加两类能力：

1. 基于当前 key 过滤条件的实时指标
   - `RPM`：最近 60 秒内进入网关的请求数
   - `In Flight`：当前仍在处理中的请求数
2. 为 usage event 持久化完整的原始用户 request JSON
   - 不是最后一条消息
   - 包含完整历史、模型、工具等原始请求字段
   - 作为单独字段保存，不混淆现有诊断字段

本次设计的关键约束：

- `RPM` 和 `In Flight` 必须是**实时内存态**指标，而不是每次刷新临时查
  `llm_gateway_usage_events`
- `RPM` 必须按当前选中的 key 过滤生效；未选 key 时返回全部 key 总和
- 实时指标不能依赖后台定时清理任务
- 完整 request 必须作为明确语义的新字段持久化，不能继续滥用现有
  `client_request_body_json`
- 实现应保持简单、可解释、固定内存上界

## Scope

### In Scope

- 后端新增统一的 request activity tracker
- 在 Usage Events 接口返回 `current_rpm` 和 `current_in_flight`
- 前端 Usage Events 页面展示这两个 live 指标
- usage event 表新增 `full_request_json`
- Codex 和 Kiro usage event 写入时都尽量持久化 `full_request_json`
- 保留现有 `client_request_body_json` / `upstream_request_body_json`
  诊断字段

### Out of Scope

- 做按 provider、按模型、按账号的 live 指标维度
- 为 live 指标增加长期时序存储
- 为 Usage Events 页面增加自动轮询
- 对历史旧 usage event 回填 `full_request_json`
- 基于 Usage Events 页面实现新的限流或调度逻辑

## Confirmed Findings

### 1. 当前 Usage Events 接口只返回分页结果，没有 live 指标

当前 `list_admin_usage_events(...)` 只返回：

- `total`
- `offset`
- `limit`
- `has_more`
- `events`
- `generated_at`

因此前端不可能显示当前 RPM 或并发。

### 2. 并发量适合用 RAII，而不是靠 usage event 反推

Codex 和 Kiro 当前都已经在各自调度器里使用 RAII lease 管理 in-flight
资源。

这说明“当前正在处理中的请求数”天然适合做成一个统一的 RAII
activity guard，而不是等请求结束后再从 usage event 反推。

### 3. 从 usage event 表统计 RPM 语义不对

如果每次刷新页面都去查“最近 60 秒内创建了多少 usage event”，
统计到的是**已完成请求速率**，不是**请求进入系统速率**。

对长流式请求来说，这会明显滞后。

### 4. 现有 request body 字段语义不够干净

当前 usage event 已经有：

- `client_request_body_json`
- `upstream_request_body_json`

但这两者当前主要是诊断字段，而且历史语义是“按条件写入”，不是稳定的
canonical source of truth。

因此不能直接把“完整用户 request”继续塞进 `client_request_body_json`
而不改变字段语义。

## Options Considered

### Option A: 每次刷新查 DB 统计最近 60 秒

优点：

- 实现最短

缺点：

- 统计的是 settled usage，不是 live request rate
- 每次刷新都打 DB
- 对长请求明显滞后

### Option B: 每 key 存一串时间戳

例如 `VecDeque<Instant>`，每个请求开始时 push，读取时清理过期项。

优点：

- 语义直观

缺点：

- 内存随请求数线性增长
- 高流量下清理成本更高
- 没必要为 60 秒窗口保留每个请求的单独时间戳

### Option C: 固定 60 个 1 秒桶的 ring buffer + RAII guard

优点：

- O(1) 写入
- O(60) 读取
- 内存固定
- 天然适合“总量 + per-key”双视图
- 不需要后台清理任务

缺点：

- 需要仔细定义桶位复用和惰性清理语义

## Chosen Design

采用 Option C。

引入一个统一的内存组件：

- `RequestActivityTracker`

它负责两件事：

1. 维护实时并发
2. 维护最近 60 秒 RPM 窗口

其输出按 key 过滤：

- `key_id = None`：全部 key 总和
- `key_id = Some(...)`：该 key 当前快照

同时，usage event 存储层新增一个语义明确的新字段：

- `full_request_json`

它表示：

> 原始客户端完整 request JSON

而现有字段继续保留原语义：

- `client_request_body_json`：诊断字段
- `upstream_request_body_json`：诊断字段

## Detailed Design

### 1. RequestActivityTracker

新增统一 tracker，挂到全局应用状态中。

它维护两类状态：

- 全局总量
- 按 `key_id` 分片的状态

每个分片都包含：

- `in_flight: u32`
- `rpm_window: SlidingSecondWindow`

推荐结构：

```rust
struct RequestActivitySnapshot {
    rpm: u32,
    in_flight: u32,
}

struct SlidingSecondBucket {
    tick_sec: u64,
    count: u32,
}

struct SlidingSecondWindow {
    buckets: [SlidingSecondBucket; 60],
}

struct RequestActivityTracker {
    total: ActivityState,
    per_key: HashMap<String, ActivityState>,
}
```

其中 `tick_sec` 必须来自**单调时钟**推导出的秒 tick，而不是 wall-clock
Unix 时间。因为这里是 live telemetry，不是审计时间戳。

### 2. RPM Window Semantics

RPM 定义为：

> 最近 60 秒内进入系统的请求数

更新时机：

- 当请求已经解析出 key，且正式进入处理流程时，记一次“开始”

这意味着 RPM 统计的是 request ingress，而不是 request completion。

### 3. No Background Cleanup: Lazy Slot Reuse

本设计不引入后台任务清理过期桶，而是通过**槽位复用时惰性清理**实现固定
60 秒窗口。

核心写入逻辑：

```text
now_sec = monotonic_elapsed_seconds()
idx = now_sec % 60

if buckets[idx].tick_sec != now_sec:
    buckets[idx].tick_sec = now_sec
    buckets[idx].count = 0

buckets[idx].count += 1
```

也就是说：

- 同一秒内命中同一个槽位，直接累加
- 60 秒后再次绕回同一个槽位时，如果该槽位代表的是旧秒，就先覆盖重置

读取快照时只累加：

```text
now_sec - bucket.tick_sec < 60
```

的桶。

这提供了双重保证：

1. 写入时，旧槽位在复用时被覆盖
2. 读取时，超过 60 秒但尚未被复用的旧槽位也会被忽略

实现代码中必须保留详细英文注释，并附一个 ASCII 可视化示意，解释
“为什么不需要后台清理任务”。

建议注释示意：

```text
tick: 100  -> idx 40 stores second 100
tick: 101  -> idx 41 stores second 101
...
tick: 160  -> idx 40 is reused

Before reuse:
  bucket[40] = { tick_sec: 100, count: 7 }

At tick 160:
  160 % 60 = 40
  bucket[40].tick_sec != 160
  => overwrite old second 100 with new second 160
```

### 4. In-Flight Uses RAII

新增 `RequestActivityGuard`：

- 构造时：
  - `total.in_flight += 1`
  - `per_key[key_id].in_flight += 1`
  - 记录 RPM 当前秒桶
- `Drop` 时：
  - `total.in_flight -= 1`
  - `per_key[key_id].in_flight -= 1`

这样并发计数和 RPM 更新共享同一个生命周期边界。

### 5. Tracker Hook Point

必须在**已经解析出 key 且请求正式进入处理链路**之后创建
`RequestActivityGuard`。

要求：

- 不在认证失败前创建
- 不在 key 未解析成功前创建
- 不等到请求完成才创建

这样可以避免把无效请求算入 live 指标，也避免把 live RPM 退化成
completion RPM。

### 6. API Changes

扩展 `AdminLlmGatewayUsageEventsResponse`，新增：

- `current_rpm: u32`
- `current_in_flight: u32`

接口语义：

- 查询不带 `key_id`：返回全部 key 总和
- 查询带 `key_id`：返回该 key 快照

这两个字段是**当前实时值**，不是与列表页 `events` 同一时间点的精确快照；
它们只要求与本次过滤条件一致。

### 7. Frontend Usage Events UI

在 `Usage Events` 标题区域增加一行 live summary：

- `RPM`
- `In Flight`

行为：

- 点击 `Refresh` 时一起刷新分页数据和 live 指标
- 切换 key filter 时一起跟随变化
- 不增加自动轮询

Usage event 详情弹窗中新增：

- `Full Request`

显示原始完整 request JSON，允许复制。

### 8. Full Request Persistence

为 `LlmGatewayUsageEventRecord` 新增字段：

- `full_request_json: Option<String>`

语义：

- 原始客户端完整 request JSON
- 尽量为每条 usage event 持久化
- 与是否高 credit、是否故障无关

现有字段继续保留：

- `last_message_content`
- `client_request_body_json`
- `upstream_request_body_json`

本次设计不改变这些字段含义。

### 9. Provider-Specific Population

#### 9.1 Kiro

Kiro 已经在请求入口捕获原始 payload。

本次只需要把这份原始 payload 接到新的 `full_request_json` 字段，
而不是仅仅在 selected diagnostic cases 下写入旧字段。

#### 9.2 Codex

Codex usage event 目前没有稳定写入完整原始 request。

需要在构造 usage event 的上下文中补充原始客户端 request JSON，
并写入 `full_request_json`。

### 10. Storage And Compatibility

新增 `full_request_json` 为 nullable UTF-8 字段。

兼容性要求：

- 旧表自动迁移补列
- 旧记录该字段为 `NULL`
- 前端按 `Option<String>` 处理

## Output Contract

完成实现后应满足：

- Usage Events 接口返回：
  - 分页结果
  - `current_rpm`
  - `current_in_flight`
- 页面可根据当前 key filter 显示对应值
- usage event 详情页能看到 `full_request_json`
- Kiro 和 Codex 新事件都尽量持久化 `full_request_json`
- RPM ring buffer 不依赖后台清理任务
- 代码中对 lazy slot reuse 保留详细英文注释和可视化 ASCII 注释

## Files Expected To Change

- `backend/src/state.rs`
- `backend/src/llm_gateway/runtime.rs`
- `backend/src/llm_gateway.rs`
- `backend/src/llm_gateway/types.rs`
- `backend/src/kiro_gateway/mod.rs`
- `backend/src/kiro_gateway/anthropic/mod.rs`
- `shared/src/llm_gateway_store/types.rs`
- `shared/src/llm_gateway_store/schema.rs`
- `shared/src/llm_gateway_store/codec.rs`
- `shared/src/llm_gateway_store/mod.rs`
- `frontend/src/api.rs`
- `frontend/src/pages/admin_llm_gateway.rs`

## Test Plan

至少覆盖：

1. `SlidingSecondWindow`
   - 同秒重复写入会累加
   - 60 秒后槽位复用会覆盖旧秒
   - 读取时忽略超过 60 秒的桶

2. `RequestActivityTracker`
   - `start -> drop` 会正确维护 `in_flight`
   - 总量与 per-key 快照一致

3. Usage Events API
   - `key_id = None` 返回总量 live 指标
   - `key_id = Some(...)` 返回该 key live 指标

4. Store schema / codec
   - `full_request_json` round-trip 正确
   - 旧表缺列时补列成功

5. Provider integration
   - Kiro usage event 写入 `full_request_json`
   - Codex usage event 写入 `full_request_json`

6. Frontend
   - Usage Events 页面显示 `RPM` / `In Flight`
   - 详情弹窗显示 `Full Request`

## Risks

### 1. Per-key tracker map 长时间可能积累空 key

第一版允许 `per_key` 中保留空闲 key 状态。
这比引入复杂清理逻辑更简单，也不会影响 correctness。

### 2. `full_request_json` 会增加 usage event 表体积

这是预期成本。
本次设计明确选择“完整排障能力优先”，不再把完整 request 仅限于高信用点数
诊断场景。

### 3. Live 指标不是审计快照

`current_rpm` 和 `current_in_flight` 是请求时刻的内存快照，不保证和本页
`events` 列表在同一逻辑瞬间完全一致。
这属于实时 dashboard 的正常语义，不是 bug。
