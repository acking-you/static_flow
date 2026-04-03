# LanceDB Memory And Polling Maintenance Design

## Goal

在不削弱现有可观测性、不中断既有 API 语义的前提下，解决当前线上 LanceDB 相关的高内存占用问题，并同时修正 Codex/Kiro 状态轮询的过于激进的默认行为。

本次设计覆盖三条主线：

- 热表重建与存储布局修正
- 热表读写路径重构，减少 version/manifest 膨胀与 `count_rows` 热点
- Codex/Kiro 状态轮询改为随机大轮询 + 账号间随机抖动，并做成可配置项

## Scope

### In Scope

- `llm_gateway_usage_events` 停机重建为 stable-row-id 表
- `api_behavior_events` 停机重建并清理历史 version/fragment 膨胀
- LLM usage event 写入批量化策略升级
- API behavior event 写入批量化策略升级
- LLM gateway usage 计数读路径去 `count_rows`
- Codex/Kiro 后台状态轮询节奏与抖动改造
- 新增并持久化相关 LLM gateway runtime config 字段
- compaction 默认值调整
- 停机维护 runbook 与校验步骤

### Out of Scope

- 改变 admin/public API 的响应结构
- 删除 usage/behavior 事件明细字段
- 把 usage/behavior 分库或按日分表
- 引入近似计数、估算分页或任何弱化精确性的策略
- 修改前端业务语义或隐藏现有可观测性

## Confirmed Findings

### 1. 热点不是“行数少”，而是 version/manifest 膨胀

当前热表的活跃数据量并不算大，但 `_versions/` 明显大于活跃 `data/`。这说明问题核心不是几万行事件本身，而是高频 append 造成的版本膨胀。

### 2. `llm_gateway_usage_events` 是历史遗留的非 stable-row-id 表

新表创建逻辑已经要求 `new_table_enable_stable_row_ids=true`，但现网审计显示这张表仍是 `stable_row_ids=false`。这不是当前 40GB 内存的唯一根因，但会放大 compaction/index remap 的复杂度。

### 3. 读路径里存在真实的 `count_rows` 热点

`llm_gateway_usage_events` 和 `api_behavior_events` 的 admin/public 查询链路都会先做 `count_rows`，而这正是 profiler 中 `Scanner::create_plan`、`ScalarIndexExpr::evaluate` 的直接来源之一。

### 4. 状态轮询当前是固定 60 秒 + 串行直打

Codex 和 Kiro 的后台刷新任务现在都采用固定 `60s` ticker，并在每轮中直接串行请求所有账号的上游状态接口，中间没有任何随机抖动或账号间节流。

### 5. 单条写入批次太小

当前写入默认值：

- `llm_gateway_usage_events`: `64` 条 / `2s`
- `api_behavior_events`: `50` 条 / `5s`

这会持续制造小批次 `.add()`，让热表在低总行数下仍然快速长出大量 version。

## Constraints

- 不破坏 userspace，不改已有 API 字段语义
- 可观测性保持基本不变，admin/public 仍能看到精确 totals 与事件明细
- 不使用“兼容性补丁式”的一次性特殊逻辑
- 停机维护允许表重建与数据目录备份
- 实现后默认行为必须更保守，避免再次打爆上游与本地存储

## Options Considered

### Option A: 只调参数

只放大 compaction 参数与轮询间隔，并重建 `llm_gateway_usage_events`。

优点：

- 风险低
- 上线快

缺点：

- 写入版本生成速率不变
- usage 读路径的 `count_rows` 热点仍在
- 内存问题只能缓解，不能从根因上收住

### Option B: 热表结构性改造，保持 API 不变

重建热表、扩大 flush 批次、去 usage 读路径的 `count_rows`、轮询改为随机区间与账号间 jitter。

优点：

- 直接命中最可能的根因
- 不破坏现有前后端协议
- 适合本次停机维护窗口

缺点：

- 涉及 backend/shared/frontend/cli 多处改动

### Option C: 按天或按月分表

从物理布局上拆 usage/behavior 事件表。

优点：

- 长期扩展性最好

缺点：

- 侵入太大
- 改动已超出本次维护的必要范围

## Chosen Design

采用 Option B。

核心原则：

- 先把热表写入速率降下来
- 再把 usage 读路径从全表 `count_rows` 中拆出来
- 同时把后台状态轮询从“固定节拍直打上游”改成“有节奏、有抖动、可配置”

## Detailed Design

### 1. 热表重建

停机维护时重建以下表：

- `llm_gateway_usage_events`
- `api_behavior_events`

要求：

- 两张表最终都必须 `stable_row_ids=true`
- 保留原表备份目录
- 重建后重新校验 schema、row count、stable row ids、索引与 fragments

执行上不新增一次性迁移器，直接复用现有 CLI 能力：

- `sf-cli db rebuild-table-stable --table llm_gateway_usage_events --force --batch-size 256`
- `sf-cli db rebuild-table-stable --table api_behavior_events --force --batch-size 256`

这样避免新造一套临时工具，也符合当前项目已经存在的重建机制。

### 2. 热表写入从“小批量高频 add”改为“批量 + 字节阈值”

#### `llm_gateway_usage_events`

现状是 `64` 条 / `2s` flush。改为三个边界共同控制：

- `max_batch_events = 256`
- `max_flush_interval_seconds = 15`
- `max_buffer_bytes = 8 MiB`

触发 flush 的条件：

- 达到条数上限
- 达到累计字节上限
- 到达最大等待时间
- 进程 shutdown

设计目标不是单纯“延迟更久”，而是减少 `.add()` 次数，让 version 增长更接近真实流量而不是 flush tick。

#### `api_behavior_events`

同样改为三阈值 flush：

- `max_batch_events = 256`
- `max_flush_interval_seconds = 15`
- `max_buffer_bytes = 4 MiB`

这张表事件结构比 usage event 更窄，所以字节上限更小。

#### Why byte limit matters

只放大 batch size 不够，因为少量超大 usage event 仍然可能把一次 flush 的内存工作集拉得过高。条数阈值和字节阈值必须同时存在。

### 3. Usage 读路径去掉 `count_rows`

`llm_gateway_usage_events` 的 totals 必须继续精确，但不再通过 `count_rows` 现算。

新增一个内存中的精确计数缓存，来源仍然是 usage event 真正的数据，而不是估算值：

- 全局总事件数
- 按 provider 的总事件数
- 按 key_id 的总事件数

构建方式：

- 启动时从 usage event 数据集聚合重建
- 运行时每次 append usage event 增量更新

这个缓存必须直接来源于 usage events 自身，因此即使某个 key 后续被删除，历史 event 总数也仍然正确。

替换范围：

- `admin/llm-gateway/usage`
- `admin/kiro-gateway/usage`
- public usage lookup

它们仍然返回精确 `total`，但不再调用 `count_rows`。

### 4. API behavior 读路径本轮不改响应模型

`api_behavior_events` 的 admin 查询仍然允许复杂过滤条件。要在本轮里为任意过滤组合引入一个精确物化计数系统，复杂度过高，属于过度设计。

本轮对它的处理是：

- 重建表，清掉历史 version/fragment 包袱
- 放大写入 batch，降低后续 version 增长速率
- 调大 compaction 默认值，减少后台维护打扰

也就是说，本轮不会把 `api_behavior_events` 的任意过滤 `count_rows` 完全移除，但会先把其底层表状态修正到健康形态。

### 5. Polling 改为随机大轮询 + 账号间随机抖动

#### 目标行为

Codex 与 Kiro 的后台状态轮询都采用以下策略：

- 每轮结束后，随机等待 `240` 到 `300` 秒再开始下一轮
- 每轮内部对多个账号串行刷新，但请求下一个账号前先随机 sleep `0` 到 `10` 秒
- 手动刷新接口保持立即执行，不受随机轮询影响

这避免多个后台账号在固定整点一起打上游，也避免轮询周期与写入/查询周期形成共振。

#### 可配置项

这些值进入 `llm_gateway_runtime_config` 并持久化：

- `codex_status_refresh_min_interval_seconds`
- `codex_status_refresh_max_interval_seconds`
- `codex_status_account_jitter_max_seconds`
- `kiro_status_refresh_min_interval_seconds`
- `kiro_status_refresh_max_interval_seconds`
- `kiro_status_account_jitter_max_seconds`

默认值统一为：

- 大轮询：`240` 到 `300` 秒
- 账号间 jitter：`10` 秒

#### Runtime update semantics

配置热更新后：

- 新配置在下一轮自动轮询开始时生效
- 不强行中断当前已经开始的一轮刷新

这样语义简单，也避免 watcher 复杂化。

### 6. LLM gateway runtime config 扩展

当前 `llm_gateway_runtime_config` 已持久化以下字段：

- `auth_cache_ttl_seconds`
- `max_request_body_bytes`
- `account_failure_retry_limit`
- `kiro_channel_max_concurrency`
- `kiro_channel_min_start_interval_ms`

本次新增：

- `codex_status_refresh_min_interval_seconds`
- `codex_status_refresh_max_interval_seconds`
- `codex_status_account_jitter_max_seconds`
- `kiro_status_refresh_min_interval_seconds`
- `kiro_status_refresh_max_interval_seconds`
- `kiro_status_account_jitter_max_seconds`
- `usage_event_flush_batch_size`
- `usage_event_flush_interval_seconds`
- `usage_event_flush_max_buffer_bytes`

`api_behavior_events` 的 flush 配置不进入这张表，保持为 backend runtime 常量/环境配置即可。原因很简单：它是站点内部行为分析写路径，不需要和 LLM gateway 管理入口混在一处。

### 7. Compaction 默认值调整

现有默认值过于激进：

- `scan_interval_seconds = 180`
- `fragment_threshold = 10`
- `prune_older_than_hours = 1`

调整为：

- `scan_interval_seconds = 900`
- `fragment_threshold = 128`
- `prune_older_than_hours = 1`

其中 `prune_older_than_hours` 明确保持 `1`，不调大。

原因：

- 增大 `scan_interval_seconds` 可以减少后台扫描频率
- 增大 `fragment_threshold` 可以避免小表过早进入 compact 路径
- 调大 `prune_older_than_hours` 只会保留更多旧 version，方向是错的

### 8. Backward Compatibility

本次设计坚持以下兼容性边界：

- 所有 admin/public API response shape 不变
- usage totals 仍然精确
- event 明细字段不删不减
- 手动刷新行为不变
- compaction runtime admin API 继续保留

允许变化的只有：

- 后台刷新节奏
- 热表物理布局
- 内部读写实现方式

## Data Flow Summary

### Usage write path

1. 请求完成后构造 `LlmGatewayUsageEventRecord`
2. 事件进入内存 buffer
3. buffer 达到条数/字节/时间阈值之一时 flush
4. flush 一次写入一个较大的 `.add()` batch
5. 同时增量更新 usage event count cache 与 key usage rollup

### Usage read path

1. list/query 仍直接查 usage events table
2. `total` 不再调用 `count_rows`
3. `total` 直接来自内存精确计数缓存

### Polling path

1. 后台任务读取 runtime config
2. 为下一轮采样一个 `240-300s` 的随机轮询间隔
3. 轮询多个账号时，在下一个账号请求前采样一个 `0-10s` 的随机 sleep
4. 手动 refresh 跳过上述等待

## Migration And Maintenance Runbook

### Preparation

- 构建新 backend 与 `sf-cli`
- 记录当前审计结果
- 确认数据根路径与备份盘空间

### Maintenance Steps

1. 停服务
2. 运行 `sf-cli db audit-storage --table llm_gateway_usage_events`
3. 运行 `sf-cli db audit-storage --table api_behavior_events`
4. 重建 `llm_gateway_usage_events`
5. 重建 `api_behavior_events`
6. 再次运行 `audit-storage`，确认 `stable_row_ids=true`
7. 启动新服务
8. 观察启动期 usage rollup / usage count cache rebuild 日志
9. 人工验证 admin usage、kiro usage、behavior analytics、public usage lookup

### Rollback

回滚不依赖“反向迁移”。直接使用重建时保留的表目录备份恢复原表目录即可。

## Testing Plan

### Unit Tests

- usage event count cache 启动聚合正确
- usage event append 会增量更新总数、provider 数、key 数
- deleted key 不影响历史 event totals
- Codex 轮询间隔采样始终在 `[240, 300]`
- Kiro 账号间 jitter 始终在 `[0, 10]`
- runtime config 更新后下一轮生效
- usage flush 在条数阈值、时间阈值、字节阈值下都能正确触发

### Integration Tests

- `llm_gateway_usage_events` 重建后 `stable_row_ids=true`
- `api_behavior_events` 重建后 `stable_row_ids=true`
- usage admin/public 查询不再调用 `count_rows`
- 现有 usage 列表分页结果不变
- Kiro/Codex 手动刷新行为不被随机调度影响

### Manual Verification

- 后台启动后 `llm_gateway_usage_events` 总数与重建前一致
- admin usage totals 与事件页数一致
- `api_behavior_events` 页面查询正常
- compactor 周期日志显示默认值已切换到 `900/128/1`
- Codex/Kiro 状态刷新日志显示随机 interval 与 per-account jitter 已生效

## Risks

### 1. 启动期聚合成本

新增 usage event count cache 后，启动期会多一次聚合。但这是单次成本，且远低于线上持续 `count_rows` 的长期代价。

### 2. Flush 放大后的突发内存

batch 变大后，单次 flush 的峰值会升高，所以必须加 `max_buffer_bytes`，不能只有 `batch_size`。

### 3. Runtime config schema migration

新增多个 runtime config 字段后，旧表需要自动补 nullable 列并回填默认值。这个迁移必须保持幂等。

### 4. `api_behavior_events` 复杂过滤计数仍依赖 Lance

本轮不会为任意过滤组合建立精确物化计数系统。这是一个有意识的范围控制，不是遗漏。

## Success Criteria

- `llm_gateway_usage_events` 与 `api_behavior_events` 都完成健康重建
- `llm_gateway_usage_events` 的 totals 查询不再走 `count_rows`
- 状态轮询默认行为变为 `240-300s` 随机大轮询 + `0-10s` 账号间 jitter
- 写入默认 batch 与字节阈值显著降低 version 增长速率
- 默认 compaction 参数切换为 `900/128/1`
- admin/public 现有功能与可观测性不回退
