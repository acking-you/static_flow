# Kiro Conservative Cache Estimation And Kmodel Configuration Design

## Goal

在不破坏现有 Kiro userspace 兼容性的前提下，把当前始终为 `0` 的
Anthropic 风格 `usage.cache_read_input_tokens` 升级成一个**协议级保守下界估计值**，
并把每模型 `Kmodel` 做成 Kiro 管理后台可配置项。

同时，将“基于最近 30 天 Kiro 成功样本重新标定 `Kmodel`”的方法固化成一个可重复执行的 skill，
用于后续重新生成推荐默认值。

本次设计的核心约束是：

- 对外暴露的是 Anthropic 协议字段，不是仅供后台查看的近似展示值
- 估计必须保守，宁可低估 cache，也不能高估
- 不引入复杂回归黑箱或不可解释 heuristic
- 线上生效配置与离线标定流程解耦

## Scope

### In Scope

- 为 Kiro 响应生成保守的 `cache_read_input_tokens`
- 同步更新持久化 usage event 中的 `input_cached_tokens` / `input_uncached_tokens`
- 为 Kiro 运行时配置新增“按模型 `Kmodel` 系数表”
- 在 `/admin/kiro-gateway` 增加该系数表的手工编辑入口
- 提供内置默认值，不要求部署后人工先录入
- 新增一个 skill，固化最近 30 天成功样本的 `Kmodel` 标定流程

### Out of Scope

- 推断 `cache_creation_input_tokens`
- 追求“真实 cache token 精确值”
- 引入自动在线重标定
- 让 skill 直接改写生产运行时配置
- 修改 Kiro upstream 协议或试图从 upstream 强行取真实 cache 值

## Confirmed Findings

### 1. 当前对外协议字段已经预留，但一直返回 0

Kiro Anthropic 兼容响应已经返回：

- `usage.input_tokens`
- `usage.output_tokens`
- `usage.cache_creation_input_tokens`
- `usage.cache_read_input_tokens`

但当前 `cache_read_input_tokens` 直接取 `usage.input_cached_tokens`，
而 Kiro 路径里这个值始终被写成 `0`。

### 2. 当前可观测量不足以唯一反推出真实 cache token

当前本地只能稳定拿到：

- 请求体输入 token 估计
- `ContextUsageEvent` 推导出的上下文使用 token
- 输出 token 估计
- `MeteringEvent` 聚合得到的 `credit_usage`

这组观测量不足以唯一确定真实 cache 读命中量，因此本次只能定义
“保守下界估计”，不能把结果包装成 upstream 真值。

### 3. 最近 30 天成功样本已经足够支撑按模型保守标定

本地内容库中，`provider_type='kiro' AND status_code=200 AND credit_usage_missing=false`
的近 30 天成功样本足够多，且不同模型分布明显不同，因此不能使用单一全局系数。

当前推荐默认值来自按模型计算：

`credit_usage / (Tin + 5 * Tout)` 的 `p80`

对应：

- `claude-opus-4-6 = 8.061927916785985e-06`
- `claude-sonnet-4-6 = 5.055065250835128e-06`
- `claude-haiku-4-5-20251001 = 2.3681034438052206e-06`

并将 `claude-opus-4.6` 视为 `claude-opus-4-6` 的别名。

### 4. 现有输入 token 估计存在双源信息，应取更保守值

Kiro 当前有两种输入 token 近似来源：

- 请求体静态估算
- `ContextUsageEvent` 百分比乘模型上下文窗口换算出的 token

这两个值并不总是一致。为避免将输入 token 高估后进一步把 cache 高估，
运行时估算应采用两者中更小的非零值。

## Options Considered

### Option A: 单一线性公式 + 单一输入估计

直接使用：

`cached = f(credit_usage, request_input_tokens, output_tokens, Kmodel)`

优点：

- 最简单

缺点：

- 对单一输入估计误差敏感
- 容易把 cache 算高

### Option B: 保守下界公式 + 双源输入取最小值

使用固定、可解释的代数公式，但将不确定性压缩到输入侧：

- `Tin_safe = min(request_estimate, context_usage_estimate)` 的非零安全值
- `Ksafe(model)` 为按模型配置的保守系数

优点：

- 易解释
- 容易测试
- 倾向低估，不会系统性夸大 cache

缺点：

- 在一些真实高 cache 命中场景下会显得偏保守

### Option C: 多元回归或复杂分桶拟合

优点：

- 表面上拟合度更高

缺点：

- 黑箱程度高
- 很难解释
- 极易把其他观测误差“误学成 cache”
- 与当前“宁可偏少”的产品要求冲突

## Chosen Design

采用 Option B。

本次只定义一个**协议级保守下界估计机制**，并明确将其与离线标定流程、
后台配置入口分离。

## Detailed Design

### 1. 数据与职责分层

系统拆成三层：

1. **离线标定层**
   - 由新 skill 负责
   - 输入为最近 30 天 Kiro 成功 usage 样本
   - 输出为每模型 `Kmodel` 推荐值

2. **运行时配置层**
   - 存储当前线上生效的每模型 `Kmodel`
   - 在 `/admin/kiro-gateway` 中手工编辑
   - 默认值由内置表提供

3. **协议生成层**
   - 在 Kiro 对外响应时计算 `cache_read_input_tokens`
   - 在 usage event 持久化时同步写入 `input_cached_tokens`
   - 只消费运行时配置，不知道样本分位和标定细节

### 2. 运行时估算公式

定义：

- `Tin_req`：请求体 token 估计
- `Tin_ctx`：`ContextUsageEvent` 推导出的 token，若不存在则为空
- `Tin_safe`：输入 token 的保守安全值
- `Tout`：输出 token 估计
- `Cobs`：观测到的 `credit_usage`
- `Kmodel`：当前模型的运行时配置系数

#### 2.1 安全输入 token

```text
Tin_safe =
  if Tin_req > 0 and Tin_ctx > 0:
      min(Tin_req, Tin_ctx)
  else if Tin_req > 0:
      Tin_req
  else:
      Tin_ctx
```

若最终仍无有效值，则退化为 `0 cache`。

#### 2.2 保守满价成本

Anthropic 风格近似价格结构固定为：

- 输出 token 权重 `5`
- cache read 权重 `0.1`

因此：

```text
Cfull_safe = Kmodel * (Tin_safe + 5 * Tout)
```

#### 2.3 保守 cache read 下界

```text
cache_read_input_tokens =
  max(
    0,
    min(
      Tin_safe,
      floor((Cfull_safe - Cobs) / (0.9 * Kmodel))
    )
  )
```

并同步得到：

```text
input_uncached_tokens = Tin_safe - cache_read_input_tokens
```

#### 2.4 强制归零条件

以下任一条件满足时，直接返回 `0 cache`：

- `credit_usage` 缺失
- `Kmodel <= 0`
- `Tin_safe <= 0`
- `Cfull_safe <= Cobs`
- 任何中间值非有限

### 3. Anthropic 协议字段写回规则

对外响应中的 usage 结构更新为：

- `input_tokens = input_uncached_tokens + cache_read_input_tokens`
- `output_tokens = output_tokens`
- `cache_read_input_tokens = cache_read_input_tokens`
- `cache_creation_input_tokens = 0`

这里 `cache_creation_input_tokens` 明确保持 `0`。
因为当前系统完全没有可用于保守估算“本次新写入 cache 多少 token”的观测源，
任何非零值都属于伪造语义。

### 4. 持久化 usage event 的对齐

当前 Kiro usage event 中：

- `input_uncached_tokens`
- `input_cached_tokens`

也要同步改成同一套估计结果，保证：

- 落库值
- 管理页 usage 列表
- 对外 Anthropic 协议 usage 字段

三者一致。

不得出现“后台展示一套，API 返回另一套”的分裂状态。

### 5. 模型别名与默认表

运行时读取模型系数时，先做稳定别名归并：

- `claude-opus-4.6 -> claude-opus-4-6`

本次仅支持已确认模型的最小别名集合，不引入字符串模糊匹配或启发式推断。

默认表为：

- `claude-opus-4-6 = 8.061927916785985e-06`
- `claude-sonnet-4-6 = 5.055065250835128e-06`
- `claude-haiku-4-5-20251001 = 2.3681034438052206e-06`

这些默认值必须直接内置到运行时默认配置中，部署后无需管理员先手工填写。

### 6. 配置存储

采用现有 `llm_gateway_runtime_config` 链路扩展，不新增 Kiro 专用表。

新增字段保存“每模型 `Kmodel` 映射”，推荐持久化为 JSON 字符串字段，
例如：

- `kiro_cache_kmodel_overrides_json`

原因：

- 当前只知三种主要模型，但未来模型列表仍可能扩展
- JSON 映射比为每个模型单独加列更稳
- 与 runtime config 的“单行全局配置”语义保持一致

运行时解析失败时回退到内置默认表，并记录 warning，但不阻塞启动。

### 7. `/admin/kiro-gateway` 配置入口

在 Kiro 管理页新增独立配置面板，显示：

- 每模型名
- 当前生效 `Kmodel`
- 默认推荐值说明

只支持**手工编辑与保存**，不在页面中加入“在线重算”按钮。

原因：

- 你已明确要求后台只需要手工编辑
- 运行时重算会把离线标定职责污染到生产界面

### 8. 标定 skill

新增一个专门 skill，例如：

- `kiro-kmodel-calibrator`

职责：

- 从最近 30 天 Kiro 成功样本中抽取数据
- 按模型计算 `credit_usage / (Tin + 5 * Tout)` 的分位值
- 以 `p80` 作为推荐 `Kmodel`
- 输出标准化结果报告

约束：

- skill 只输出建议，不自动回写生产配置
- skill 默认使用内容库 `llm_gateway_usage_events`
- skill 必须明确报告样本数、模型分布、过滤条件和最终推荐值

### 9. 日志与可观测性

当运行时完成 cache 估算时，增加轻量诊断字段：

- `estimated_cache_read_input_tokens`
- `estimated_input_uncached_tokens`
- `estimated_input_tokens_source`
- `kmodel`
- `credit_usage`

日志级别以 `debug` 或 `info` 为主，不污染错误日志。

目的是方便后续比对，而不是把算法细节暴露给调用方。

## API And Compatibility

### Backward Compatibility

- 对外响应结构不变，只是 `cache_read_input_tokens` 从恒为 `0` 变成保守估计值
- `cache_creation_input_tokens` 继续为 `0`
- Kiro 管理页新增配置面板，不移除现有字段
- 未配置模型使用内置默认值，不会让现有请求失效

### Userspace Promise

本次不承诺“真实 cache 命中精确还原”，只承诺：

- 协议字段含义稳定
- 算法可解释
- 结果偏保守
- 不会系统性夸大 cache

## Testing Strategy

### Backend

- 单元测试：模型别名归并
- 单元测试：`Tin_req` / `Tin_ctx` 双源输入取值
- 单元测试：`credit_usage` 缺失时强制 `0 cache`
- 单元测试：`Cfull_safe <= Cobs` 时强制 `0 cache`
- 单元测试：正常路径能得到非零保守 cache
- 回归测试：Kiro Anthropic 响应 `usage.cache_read_input_tokens` 与持久化 usage event 一致

### Frontend

- 管理页运行时配置表单可正确加载默认值
- 手工修改 `Kmodel` 后能正确提交和回显

### Skill

- 对固定样本输入可稳定输出推荐值
- 报告中包含模型、样本数、`p50/p80/p90` 与推荐值

## Risks

### 1. 输入 token 估计本身仍是近似值

这会影响 cache 下界的绝对精度，但采用 `Tin_safe=min(...)` 后，
误差方向会明显偏向保守，而不是夸大 cache。

### 2. 不同模型未来计费行为可能变化

因此 `Kmodel` 不能硬编码死在代码里，必须允许后台调整，并由 skill 定期重标定。

### 3. `credit_usage` 缺失请求无法估算

这类请求仍会返回 `0 cache`。这是原则性选择，不应为了表面好看而硬造值。

## Success Criteria

- Kiro 对外返回的 `usage.cache_read_input_tokens` 不再恒为 `0`
- 估算结果在当前样本分布下明显偏保守，不系统性高估
- 默认部署无需手工录入 `Kmodel`
- 管理员可在 `/admin/kiro-gateway` 手工调整各模型系数
- 后续重新标定时，只需运行 skill 即可得到新推荐值
