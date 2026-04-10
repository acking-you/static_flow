# Kiro Cache Policy Overrides Design

## Goal

在不破坏当前 Kiro userspace 兼容性的前提下，把 Kiro cache 成本保护相关的硬编码策略，
升级成一套**全局默认 + key 级字段覆盖**的可配置机制，并在 Kiro Admin 后台提供可视化编辑入口。

本次设计同时覆盖两类目前写死在代码里的策略：

1. cache 估算保护策略
   - 小输入但高 credit 时，如何把权威 input token 向更保守的目标值拉升
   - prefix-tree 模式下，随着 credit 上升，cache ratio 上限如何阶梯式收紧
2. 异常高 credit 诊断策略
   - 何时将一次请求视为“异常高积分消耗”
   - 何时为诊断目的持久化完整请求体

本次设计的核心约束：

- 默认行为必须和当前线上行为完全一致
- 配置 source of truth 必须在后端，前端只负责编辑和展示
- key 级策略必须支持“继承全局默认 + 覆盖部分字段”
- 不引入隐式纠错、自动排序、自动裁剪等 heuristic 修补逻辑
- `Kmodel` 继续保持全局配置，不与 key 级 cache policy 混用

## Scope

### In Scope

- 为 Kiro runtime config 增加全局 `kiro_cache_policy`
- 为 Kiro key 增加可空的 `kiro_cache_policy_override`
- 运行时按“内建默认 -> 全局默认 -> key override”解析 effective policy
- 将小输入高 credit 拉升逻辑改成配置驱动
- 将 prefix-tree credit 阶梯 cap 逻辑改成配置驱动
- 将异常高 credit 诊断阈值改成配置驱动
- 在 `/admin/kiro-gateway` 中增加全局策略编辑区
- 在 Kiro key 编辑卡片中增加 override 编辑区
- 增加解析、校验、round-trip、算法和前端交互测试

### Out of Scope

- 修改 Kiro prefix tree 数据结构或 session recovery 机制
- 将 `Kmodel` 下放到 key 级别
- 自动根据 usage 样本在线调参
- 为 account 级别增加同类策略
- 自动帮管理员修复非法 band 配置

## Confirmed Findings

### 1. 当前关键成本保护逻辑确实是硬编码

目前以下逻辑直接写死在后端：

- 小输入高 credit 时，把 `authoritative_input_tokens` 向 `100000` 拉升
- prefix-tree 模式下，credit `0.3 -> 1.0 -> 2.5` 时，cache ratio cap 从 `70% -> 20% -> 0%`
- credit 大于 `2.0` 时视为高 credit 异常，允许持久化完整请求体用于诊断

这意味着 Admin 里虽然已经能配置：

- `kiro_cache_kmodels_json`
- `kiro_prefix_cache_mode`
- prefix cache 容量 / TTL

但真正决定“成本保护强度”的核心策略，仍然无法在后台调整。

### 2. 当前 Kiro key 级别只有开关，没有参数化策略

现在 Kiro key 只支持：

- `kiro_request_validation_enabled`
- `kiro_cache_estimation_enabled`

前者控制请求合法性校验，后者控制是否返回 cache 估算值。
但“如何估算、如何在高 credit 时保守处理”仍然只能走全局硬编码。

### 3. 新策略更适合归属于 key，而不是 account

成本保护的目标是：

- 决定一个 API key 对外暴露的 usage 口径
- 控制该 key 是否更激进或更保守
- 在出现异常 credit 时决定是否加强诊断

这些都是“调用方 contract”层面的行为，不是某个底层账号的行为。
如果把策略挂在 account 上，单个 key 命中不同账号时会得到不稳定的语义，
这与用户确认的“key 级别可调”目标冲突。

### 4. 全局默认仍然必须保留

虽然最终策略归属于 key，但全局默认仍然必需：

- 新 key 创建后不需要手动先填一遍策略
- 老 key 不配置 override 时继续按当前行为运行
- 大多数 key 预期会直接继承默认，不值得维护一份完整副本

因此正确模型不是“全量 key 私有配置”，而是“全局默认 + key 字段级覆盖”。

## Options Considered

### Option A: 只把硬编码常量搬到全局 runtime config

优点：

- 改动最小
- Admin 只需要一处新表单

缺点：

- 无法满足 key 级别差异化成本保护
- 无法支持单个 key 针对异常 credit 场景单独收紧/放宽

### Option B: 全局完整策略 + key 完整策略

优点：

- 模型直接
- 运行时合并简单

缺点：

- 每个 key 都会复制整套配置
- 后续新增字段时漂移风险高
- Admin UI 容易演变成“复制全局 JSON 再手改”

### Option C: 全局默认策略 + key 字段级覆盖

优点：

- 兼容性最好
- 老 key 零迁移成本
- 常用场景只改局部字段
- 便于后续扩展

缺点：

- 需要定义严格的 override 合并语义
- 前后端都要区分 `override` 和 `effective`

### Option D: account 级策略

优点：

- 看起来更接近 credit 来源

缺点：

- 一个 key 可能使用多个账号
- 同一个 key 的 usage contract 会随着路由漂移
- 与用户确认的 key 级控制目标不一致

## Chosen Design

采用 Option C。

系统引入两层配置：

1. 全局 `kiro_cache_policy`
   - 存在于 runtime config
   - 定义全站默认成本保护策略
2. key 级 `kiro_cache_policy_override`
   - 存在于 Kiro key
   - 只记录与全局不同的字段

运行时使用三层解析顺序：

`内建默认值 -> 全局配置 -> key override -> effective policy`

这样既保证了老行为兼容，也允许对单个 key 做差异化控制。

## Detailed Design

### 1. 数据模型

#### 1.1 全局默认策略

后端内存中的强类型结构：

```rust
struct KiroCachePolicy {
    small_input_high_credit_boost: KiroSmallInputHighCreditBoostPolicy,
    prefix_tree_credit_ratio_bands: Vec<KiroCreditRatioBand>,
    high_credit_diagnostic_threshold: f64,
}

struct KiroSmallInputHighCreditBoostPolicy {
    target_input_tokens: u64,
    credit_start: f64,
    credit_end: f64,
}

struct KiroCreditRatioBand {
    credit_start: f64,
    credit_end: f64,
    cache_ratio_start: f64,
    cache_ratio_end: f64,
}
```

#### 1.2 key 级 override

```rust
struct KiroCachePolicyOverride {
    small_input_high_credit_boost: Option<KiroSmallInputHighCreditBoostOverride>,
    prefix_tree_credit_ratio_bands: Option<Vec<KiroCreditRatioBand>>,
    high_credit_diagnostic_threshold: Option<f64>,
}

struct KiroSmallInputHighCreditBoostOverride {
    target_input_tokens: Option<u64>,
    credit_start: Option<f64>,
    credit_end: Option<f64>,
}
```

#### 1.3 存储格式

持久化层不拆成大量新列，而是使用 JSON 字段：

- `llm_gateway_runtime_config.kiro_cache_policy_json`
- `llm_gateway_keys.kiro_cache_policy_override_json`

这样做的原因：

- 与现有 `kiro_cache_kmodels_json` 风格一致
- 不会让 schema 被大量策略列撑爆
- 后续新增字段时无需继续扩张表结构
- 缺字段时容易回落到默认值

### 2. 默认值与兼容性

#### 2.1 内建默认策略

内建默认策略必须精确等价于当前线上行为：

- `small_input_high_credit_boost`
  - `target_input_tokens = 100000`
  - `credit_start = 1.0`
  - `credit_end = 1.8`
- `prefix_tree_credit_ratio_bands`
  - band 1:
    - `credit_start = 0.3`
    - `credit_end = 1.0`
    - `cache_ratio_start = 0.7`
    - `cache_ratio_end = 0.2`
  - band 2:
    - `credit_start = 1.0`
    - `credit_end = 2.5`
    - `cache_ratio_start = 0.2`
    - `cache_ratio_end = 0.0`
- `high_credit_diagnostic_threshold = 2.0`

#### 2.2 旧数据兼容

兼容行为必须满足：

- 旧 runtime config 没有 `kiro_cache_policy_json` 时，自动使用内建默认策略
- 旧 key 没有 `kiro_cache_policy_override_json` 时，视为完全继承全局默认
- 旧 key 的 `kiro_cache_estimation_enabled=false` 时，仍然直接返回 `cache=0`

### 3. 合并规则

effective policy 采用固定解析顺序：

`builtin_default.merge(global_policy).merge(key_override)`

#### 3.1 标量字段

以下字段按字段覆盖：

- `small_input_high_credit_boost.target_input_tokens`
- `small_input_high_credit_boost.credit_start`
- `small_input_high_credit_boost.credit_end`
- `high_credit_diagnostic_threshold`

#### 3.2 band 列表

`prefix_tree_credit_ratio_bands` 使用**整段列表覆盖**：

- key override 未提供 bands：继续继承全局 bands
- key override 提供 bands：整组替换全局 bands

本次设计明确拒绝 band 级 merge。
band 级 merge 会把配置语义搅乱，也会让前端编辑逻辑复杂化。

### 4. 算法语义

#### 4.1 小输入高 credit 拉升策略

当前 `adjust_input_tokens_for_cache_creation_cost` 改为读取 effective policy。

逻辑保持线性插值：

- 如果 `cache_estimation_enabled=false`，不拉升
- 如果 `authoritative_input_tokens >= target_input_tokens`，不拉升
- 如果 `credit_usage <= credit_start`，不拉升
- 如果 `credit_usage >= credit_end`，直接拉到 `target_input_tokens`
- 其余区间按线性比例从原值拉向 `target_input_tokens`

这保留了当前机制本质，只是把阈值和落点从硬编码改成配置。

#### 4.2 prefix-tree credit ratio bands

当前 `prefix_tree_credit_ratio_cap_basis_points` 改为读取 effective bands。

band 的计算语义定义为：

- 当 `credit` 落在某个 band 内时，按
  `cache_ratio_start -> cache_ratio_end` 做线性插值
- 当 `credit` 低于第一段 `credit_start` 时，不额外加 cap
- 当 `credit` 高于最后一段 `credit_end` 时，保持最后一段的 `cache_ratio_end`

这比现有 `if / else` 写法更稳定，也更符合“管理员定义阶梯和落点”的需求。

#### 4.3 高 credit 异常诊断阈值

当前 `is_high_credit_usage` 改为读取 effective policy 中的
`high_credit_diagnostic_threshold`。

它将继续控制：

- 是否记录高 credit anomaly log
- 是否在 usage event 中持久化 `client_request_body_json`
- 是否在 usage event 中持久化 `upstream_request_body_json`

#### 4.4 Kmodel 继续保持全局

公式估算使用的 `kiro_cache_kmodels_json` 继续是全局配置，不参与 key 级 override。

原因是：

- `Kmodel` 属于模型标定参数，不是 key 的成本保护偏好
- 下放到 key 会让同一模型在不同 key 上拥有不同物理价格系数，语义会变差

### 5. 校验规则

后端必须对策略 JSON 做强校验。
任何非法配置都拒绝保存，而不是偷偷修正。

#### 5.1 boost policy 校验

- `target_input_tokens > 0`
- `credit_start` 和 `credit_end` 必须为有限值
- `credit_start < credit_end`

#### 5.2 diagnostic threshold 校验

- 必须为有限值
- `>= 0`

#### 5.3 band 校验

每个 band 必须满足：

- `credit_start` 和 `credit_end` 为有限值
- `credit_start < credit_end`
- `cache_ratio_start` 和 `cache_ratio_end` 为有限值
- `cache_ratio_start` 和 `cache_ratio_end` 都在 `[0, 1]`

band 列表整体必须满足：

- 按 `credit_start` 严格升序
- 不允许重叠
- `cache_ratio` 整体单调不升

这里保留单调不升约束，避免管理员把“credit 越高、允许 cache 越大”这种明显违背成本保护目标的配置写进去。

### 6. 后端接口设计

#### 6.1 runtime config API

在全局 runtime config 请求和响应中增加：

- `kiro_cache_policy_json`

后端职责：

- 解析 JSON
- 校验合法性
- 存储原始 JSON
- 在内存 runtime config 中保存解析后的强类型 `kiro_cache_policy`

#### 6.2 Kiro key admin view

Kiro key admin 响应中增加：

- `kiro_cache_policy_override_json`
- `effective_kiro_cache_policy_json`
- `uses_global_kiro_cache_policy`

这样前端无需自己做 source-of-truth merge，只负责展示：

- 当前 override 是什么
- 当前 effective policy 是什么
- 当前是否在继承全局默认

#### 6.3 Kiro key patch API

Kiro key patch 请求增加：

- `kiro_cache_policy_override_json`

语义：

- 缺失字段：不修改 override
- `null`：清空 override，恢复继承
- 非空字符串：解析、校验并保存为新的 override

### 7. Admin UI Design

#### 7.1 全局默认区

在 `/admin/kiro-gateway` 的 Kiro cache config 区域中保留当前的：

- `Kmodel` JSON 编辑区
- `kiro_prefix_cache_mode`
- prefix cache 容量 / TTL
- conversation anchor 容量 / TTL

并新增一个结构化的 `Kiro Cache Policy` 表单：

- `small input high credit boost`
  - `target_input_tokens`
  - `credit_start`
  - `credit_end`
- `high credit diagnostic threshold`
- `prefix tree credit ratio bands`
  - 可增删行
  - 每行编辑：
    - `credit_start`
    - `credit_end`
    - `cache_ratio_start`
    - `cache_ratio_end`

全局表单保存时：

- 前端将结构化表单序列化成 `kiro_cache_policy_json`
- 与现有 runtime config 一起提交

#### 7.2 key 级 override 区

在每个 Kiro key 卡片中，在现有 `Cache Token 估算` 开关下面增加折叠区。

该区域展示三种状态：

1. `inherit global`
2. `override enabled`
3. `effective policy summary`

交互设计：

- 默认展示简短 summary：
  - `boost 1.0 -> 1.8 => 100000`
  - `diag threshold 2.0`
  - `bands 2`
  - 或 `inherit global`
- 点击 `启用覆盖` 时，将当前 effective policy 复制到本地表单
- 点击 `恢复继承` 时，清空 override 并提交 `null`
- key 级不暴露原始 JSON 文本框，而是使用结构化表单

#### 7.3 前端职责边界

前端只做三件事：

- 编辑结构化表单
- 将 override 表单转换成 JSON
- 展示后端返回的 effective policy 和继承状态

前端不负责：

- 合并全局与 key override
- 自动修正 band 顺序
- 推断非法配置的修复方式

### 8. 存储与迁移

#### 8.1 schema 变化

新增列：

- `llm_gateway_runtime_config.kiro_cache_policy_json`
- `llm_gateway_keys.kiro_cache_policy_override_json`

两列都允许旧表缺列，并在读出时自动回落：

- runtime config 缺列 -> 内建默认 JSON
- key 缺列 -> `None`

#### 8.2 codec 与 round-trip

codec 层需要保证：

- runtime config round-trip 精确保留 `kiro_cache_policy_json`
- key round-trip 精确保留 `kiro_cache_policy_override_json`
- 缺列场景不会 decode 失败

### 9. Testing Strategy

#### 9.1 算法测试

在 Kiro Anthropic 模块中保留现有测试，并新增：

- 默认策略下，现有硬编码行为结果保持不变
- 自定义 boost policy 时，拉升结果按配置变化
- 自定义 bands 时，cap 结果按配置变化
- key override 为空时，effective policy 等于全局
- key override 只覆盖单字段时，其余字段继续继承全局
- 自定义 `high_credit_diagnostic_threshold` 时，高 credit 判定随之变化

#### 9.2 解析与校验测试

需要新增测试覆盖：

- 合法全局 policy JSON 可解析
- 合法 key override JSON 可解析
- `target_input_tokens == 0` 被拒绝
- `credit_start >= credit_end` 被拒绝
- band 重叠被拒绝
- band 未按升序排列被拒绝
- ratio 超出 `[0,1]` 被拒绝
- ratio 整体非单调不升被拒绝

#### 9.3 存储 round-trip 测试

需要新增测试覆盖：

- runtime config 写入/读回保留 `kiro_cache_policy_json`
- key 写入/读回保留 `kiro_cache_policy_override_json`
- 缺列旧表仍能成功读取默认值

#### 9.4 前端轻量测试

前端测试应覆盖：

- `inherit global` 状态展示
- key summary 正确显示 effective policy
- 结构化表单能稳定生成 policy JSON
- “恢复继承” 会清空 override

## Risks

### 1. 前后端字段漂移

如果：

- runtime config response
- key admin view
- key patch request

三者字段名不一致，前端很容易出现“显示的是 effective，提交的是旧 override”的错位。

### 2. 将“最低值”误实现成错误语义

本次需求里口语上的“阶梯最低值”，在算法上真实对应的是：

- 小输入高 credit 拉升目标
- prefix-tree cache ratio cap

如果把它错误实现成“cached token 最低保底值”，会直接破坏现有成本保护方向。

### 3. 前端私自合并配置

如果前端自己做全局 + override 合并，就会出现 source of truth 漂移。
必须以后端返回的 effective policy 为准。

## Rollout

本次功能不需要单独数据迁移脚本。

上线顺序：

1. 部署支持新 JSON 字段和默认回落的后端
2. 确认老数据读取正常
3. 部署 Admin UI
4. 先只使用全局默认配置验证行为不变
5. 再逐步为个别 key 启用 override

## Summary

本次设计把当前写死在 Kiro 网关里的 cache 成本保护逻辑，升级成：

- 全局默认策略
- key 级字段覆盖
- 后端合并后的 effective policy
- Admin 可视化编辑

并明确保持三条边界不变：

- 默认行为不变
- `Kmodel` 仍然全局管理
- source of truth 在后端，不在前端
