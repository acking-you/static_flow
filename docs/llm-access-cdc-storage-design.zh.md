# LLM Access CDC Storage Design

本文记录从 StaticFlow 内置 LLM gateway 迁移到独立 `llm-access`
binary 时的目标存储设计。目标不是把 LanceDB 表一比一搬走，而是把
控制面和分析面重新建模，避免把旧表的历史包袱带进新服务。

## 分层原则

SQLite 负责运行时控制面：

- API key 当前状态
- per-key 路由配置
- runtime config
- account groups
- proxy configs 和 bindings
- key usage rollup
- token/account contribution/sponsor request 队列状态
- CDC outbox、consumer offsets、apply state、近期幂等窗口

DuckDB 负责 append-heavy 历史和分析面：

- `usage_events` 宽事实表
- `usage_event_details` 大字段侧表
- hourly/daily 宽聚合表
- CDC event/audit 历史归档

`cdc_outbox` 和 `cdc_consumer_offsets` 不放到 DuckDB 作为运行时状态。
原因是它们需要小事务、ack、重试和 crash recovery 语义；DuckDB 可以承接
历史审计，但不应该承担消息队列和 offset 协调。

## Source-side outbox

`sf-backend` 的 LLM LanceDB 写入口集中在 `LlmGatewayStore`。因此 CDC
outbox 挂在这个存储边界，而不是分散到 admin handler 或请求 handler：

- `STATICFLOW_LLM_CDC_OUTBOX=/path/to/source-cdc.sqlite3` 开启
- `STATICFLOW_LLM_CDC_SOURCE_INSTANCE=home-staticflow` 可选，用于标识源实例
- 未设置环境变量时完全不写 source outbox

usage event 是 append-only 路径，先写幂等 outbox，再写 LanceDB，避免
flusher 重试时把同一批事件重复 append 到 LanceDB。key/config/request 这类
当前状态变更则在 LanceDB 写入成功后记录 committed event。后续迁移阶段应先
做一次全量 snapshot，再从 `cdc_outbox.seq` 继续 replay；这样历史文件数据和
开启 CDC 后的新变更可以接起来。

## Crate 拓扑

当前拆分为四个独立 crate：

- `llm-access-migrations`：版本化 SQL 文件和 migration runner。
- `llm-access-store`：目标 SQLite/DuckDB bootstrap，不直接持有 DDL 字符串。
- `llm-access-migrator`：迁移工具，先实现 source outbox 到目标 SQLite 的 replay。
- `llm-access`：最终独立 HTTP binary，目前有 storage init、`/healthz`、`/version`
  和 OpenAI/Anthropic 入口占位。

## DuckDB 表形状

`usage_events` 是宽事实表，事件发生时需要用于统计的维度直接冗余进去：

- `key_id`、`key_name`
- `provider_type`、`protocol_family`
- `account_name`
- `account_group_id_at_event`
- `route_strategy_at_event`
- `endpoint`
- `model`、`mapped_model`
- latency、token、credit、status code 等指标

常规报表必须走单表过滤和聚合，不依赖运行时 join。`usage_event_details`
只用于单条 drilldown，不参与普通统计。

允许的 join 只有两类：

- 按 `event_id` 点查 detail
- 迁移校验或离线审计

## 构建策略

`llm-access-store` 默认只编译 SQLite 控制面和 DuckDB schema SQL 输出。
DuckDB Rust runtime 是 feature-gated：

- 默认：不编译 DuckDB C++ 本体
- `duckdb-runtime`：使用系统 `libduckdb`
- `duckdb-bundled`：编译 bundled DuckDB，仅适合构建机或明确允许的环境

这避免在当前生产宿主机上反复编译 DuckDB C++，降低挤占 live backend 内存的风险。

## 当前工具入口

初始化 SQLite 控制面 DB，并输出 DuckDB schema SQL：

```bash
cargo run -p llm-access -- init \
  --sqlite-control /path/to/llm-access.sqlite3 \
  --duckdb-schema-sql /path/to/duckdb-schema.sql
```

启动独立服务空壳：

```bash
cargo run -p llm-access -- serve \
  --bind 127.0.0.1:19080 \
  --sqlite-control /path/to/llm-access.sqlite3 \
  --duckdb-schema-sql /path/to/duckdb-schema.sql
```

当前代码已经把 StaticFlow LLM 写入路径接入 source-side SQLite outbox，并实现
了第一版 source outbox 到目标 SQLite 的 key replay。下一阶段是扩展
`llm-access-migrator` 的 snapshot + 全实体 replay，并把 `llm-access` 的
provider runtime 接到新 store 上。
