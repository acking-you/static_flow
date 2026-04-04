# Kiro Tool Validation And Normalization Design

## Goal

在不破坏现有 Kiro userspace 兼容性的前提下，修复当前 `kiro-gateway` 对工具定义边界处理不完整的问题，避免请求在本地通过、却被 upstream 以 `400 Improperly formed request` 拒绝。

本次设计只解决已经确认的高置信度边界问题，不引入大范围 schema 改写，也不改变现有对话转换语义。

## Scope

### In Scope

- 为 Kiro Anthropic 请求新增 tool 级别的规范化与校验阶段
- 自动规范化空或纯空白 `tool.description`
- 本地拒绝空或纯空白 `tool.name`
- 增加结构化日志，明确记录 tool 规范化与校验错误
- 增加回归测试，覆盖最小复现请求

### Out of Scope

- 改写或降级复杂 JSON Schema 关键字
- 修改 system prompt 注入的 synthetic history 机制
- 调整当前 message normalization / current-turn 切分逻辑
- 更改对外 API 响应结构
- 处理 Kiro upstream 自身的瞬时 5xx

## Confirmed Findings

### 1. 当前 request validation 只验证 messages，不验证 tools

本地 `request_validation_enabled=true` 时，当前校验逻辑只覆盖 `req.messages`，并不会校验 `req.tools` 的 `name`、`description` 或 `input_schema`。这意味着 tool 定义可以带着明显异常一路传到 upstream。

### 2. 空 tool description 可以稳定复现 upstream 400

使用本机 `~/.static-flow/auths/kiro/*.json` 中的真实 Kiro 账号直接请求 upstream 后，受控实验已经确认：

- 最小合法请求可直接返回 `200`
- 在同一个最小请求里，仅加入一把 `description=""` 的工具，就会稳定返回 `400 Improperly formed request`

这说明“空工具描述”不是猜测，而是已经被 upstream 实证确认为非法输入。

### 3. `anyOf` 不是本轮问题的主因

同样使用真实账号做受控实验后，最小请求里加入带 `anyOf` 的 tool schema 仍可返回 `200`。因此本轮不能把 `anyOf` 或其他复杂 schema 关键字一刀切判成非法，否则会主动破坏现有 userspace。

### 4. 真实坏请求只修空 description 即可从 400 变 200

对这次实际落库的失败请求样本，仅对
`mcp__ida-pro-mcp__patch_address_assembles` 这一把工具补上非空 `description`，其余 payload 不变，upstream 即从 `400 Improperly formed request` 变为 `200`。

这进一步坐实了本轮主因就是空工具描述，而不是请求整体结构或代理层问题。

### 5. 18:08 的 upstream 500 属于另一类问题

同一请求首次返回 `500`、第二次重试原样成功，说明这是 upstream 瞬时内部错误，不应与本轮 deterministic `400` 混为一谈。

## Constraints

- 不破坏现有客户端流量
- 不对 tool schema 做猜测式改写
- 不引入“先发上游失败再从 usage event 倒推”的被动排障模式
- 所有自动规范化都必须是确定性、无语义歧义、可记录的

## Options Considered

### Option A: 严格拒绝所有空 description

本地一旦发现空 `tool.description`，直接返回 `400 invalid_request_error`。

优点：

- 边界最干净
- 行为最容易解释

缺点：

- 会直接打断现有 userspace
- 与当前“优先兼容真实客户端流量”的目标冲突

### Option B: 只修空 description，其他 tools 保持透传

为 `tool.description` 做安全规范化；`tool.name` 做基本合法性校验；复杂 schema 仅做诊断，不做拒绝或改写。

优点：

- 直接命中已经实证确认的主因
- 不会误伤目前已被 upstream 接受的复杂 schema
- 风险最可控

缺点：

- 对 schema 边界的长期定义仍需后续样本继续收敛

### Option C: 同时对空 description 和复杂 schema 做本地强校验

在本轮里直接定义并执行一套保守的 Kiro schema 子集。

优点：

- 边界表面上更完整

缺点：

- 证据不足
- 已经有 `anyOf` 成功样本，贸然拒绝会破坏现有 userspace

## Chosen Design

采用 Option B。

核心原则：

- 只修已经被 upstream 实证确认会失败、且不会引入语义歧义的问题
- 把 tool 边界拉到本地显式处理，不再让上游替我们做校验
- 对未证实非法的 schema 关键字只做诊断，不做猜测式拦截或改写

## Detailed Design

### 1. 在 message normalization 后增加 tool normalization / validation 阶段

现有 `normalize_request` 已经负责 message 级别的安全规范化。此次在同一条前置流水线中新增 tool 级别处理，形成：

1. message normalization
2. tool normalization
3. request validation
4. conversion

这样 `messages` 与 `tools` 都有对称的边界控制，不再出现“message 有 validation、tool 没 validation”的结构性漏洞。

### 2. 空或纯空白 `tool.name` 直接本地拒绝

规则：

- `tool.name` 为空字符串
- `tool.name` 只包含空白字符

都直接返回 `400 invalid_request_error`。

原因：

- 工具名是工具身份本体，不存在“安全自动修复”的空间
- 这类输入语义不完整，继续透传没有价值

错误信息必须明确点出：

- `tool_index`
- `tool_name`（若可打印）
- `validation_reason=empty_tool_name`

### 3. 空或纯空白 `tool.description` 自动规范化

规则：

- 若 `tool.description` 为空或纯空白
- 不修改原始请求审计数据
- 只对工作副本进行规范化

规范化目标值采用稳定占位描述：

`Client-provided tool '<tool_name>'`

若工具名也非法，则不会走到这里，而是按上一条规则直接拒绝。

选择该值的原因：

- 非空
- 语义中性
- 不猜测工具真实用途
- 可稳定复现与测试

### 4. 复杂 schema 关键字本轮不做拦截

对于 `anyOf`、`oneOf`、`allOf`、`contains`、`dependentSchemas` 等关键字：

- 本轮不自动改写
- 本轮不一刀切拒绝
- 仅做诊断统计与日志记录

原因：

- 已有真实 upstream 成功样本证明至少 `anyOf` 可以被接受
- 当前证据不足以定义完整 Kiro schema 子集
- 贸然拦截会破坏 userspace

后续若再抓到新的 deterministic 400 样本，且确认与特定 schema 关键字存在稳定因果关系，再单独做下一轮设计。

### 5. 结构化日志

#### 5.1 自动规范化日志

当空 `description` 被自动补齐时，打 `warn` 日志，字段至少包括：

- `request_id`
- `trace_id`
- `key_id`
- `key_name`
- `tool_index`
- `tool_name`
- `normalization_action=fill_tool_description`
- `normalization_reason=empty_tool_description`

#### 5.2 本地拒绝日志

当工具名非法导致本地 `400` 时，打 `error` 日志，字段至少包括：

- `request_id`
- `trace_id`
- `key_id`
- `key_name`
- `tool_index`
- `validation_error=empty_tool_name`

#### 5.3 诊断摘要日志

每次请求在 validation 完成后附带一份轻量摘要，用于排障而不污染正常日志主体。摘要只需在 debug/info 范围可见，包含：

- `tool_count`
- `normalized_tool_description_count`
- `empty_tool_name_count`
- `schema_keyword_counts`

这份摘要只用于诊断，不参与控制流。

### 6. 审计与 usage event

原始 `client_request_body_json` 保持原样，不写回规范化后的 tool 描述。

原因：

- 审计数据必须保留客户端真实输入
- 排障时要能准确知道用户到底发了什么

若请求因工具名非法而在本地被拒绝，failure usage event 中应继续保留原始请求体，并在已有错误上下文中补充简明摘要，便于后续查询。

### 7. 测试

至少新增以下回归测试：

1. 正常 tool 定义不发生变化
2. 空 `description` 会被规范化为稳定占位描述
3. 空 `name` 会在本地返回 `invalid_request_error`
4. 带 `anyOf` 的 schema 保持透传，不被本地拒绝
5. 使用这次真实最小复现样本构造回归测试，确保不会再次把空描述透传到 upstream payload

## Implementation Notes

- 主要修改文件是 `backend/src/kiro_gateway/anthropic/converter.rs`
- 日志入口仍应落在 `backend/src/kiro_gateway/anthropic/mod.rs`
- 不引入新的配置项
- 不改变现有 public/admin API 结构

## Risks

### 1. 占位描述可能影响极少数模型行为

虽然占位描述是中性的，但任何新增描述文字都可能轻微影响模型对工具的排序偏好。

接受该风险的原因：

- 当前空描述已经被 upstream 明确判为非法
- 相比 deterministic 400，这个风险更低且可接受

### 2. 复杂 schema 的真实边界仍未完全摸清

本轮有意不碰 schema 子集定义，因此未来仍可能出现其他 deterministic 400 样本。

这不是设计缺陷，而是刻意避免在证据不足时做过度收紧。

## Rollout

1. 落地 tool normalization / validation
2. 增加回归测试
3. 观察线上是否还存在 deterministic `400 Improperly formed request`
4. 若仍存在，再基于新的 usage event 样本继续收敛 schema 边界

## Success Criteria

- 空 `tool.description` 不再透传到 Kiro upstream
- 本轮已确认的 deterministic 400 样本不再复现
- 合法且带复杂 schema 的工具请求不被本地误伤
- 日志能明确指出 tool 边界规范化/拒绝发生在何处
