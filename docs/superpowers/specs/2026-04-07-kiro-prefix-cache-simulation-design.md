# Kiro Prefix Cache Simulation And Session Recovery Design

## Goal

在不破坏现有 Kiro userspace 的前提下，用一套**机制上更接近真实 prefix cache**
的本地模拟方案，替换当前主要依赖 `credit_usage` 反推的 cache 估算思路。

本次设计同时解决两个互相关联的问题：

1. `cache_read_input_tokens` 需要从“保守公式反推”升级为“基于本地 prefix 匹配的命中下界”
2. 当客户端没有显式 session id 时，系统需要尽量从**已知稳定历史状态**
   恢复正确的 `conversation_id`，而不是直接放弃会话连续性

本次设计的核心约束：

- cache 模拟必须基于 **修正后的 Kiro 请求语义结果**，即 conversion 后的 `ConversationState`
- cache 命中只统计 **stable-prefix 区域**，不能把当前轮新增 user/tool_result 算成可缓存命中
- session 恢复不能靠模糊猜测，只能基于**精确 canonical history anchor** 恢复
- 全站所有 Kiro 请求共享一棵 prefix 树，不按 key 分片
- key 上保留开关，允许按 key 决定是否启用该模拟器
- 数据结构应优先采用成熟第三方 Rust crate；只有在找不到合适库时才自实现核心结构

## Scope

### In Scope

- 引入一套新的 Kiro prefix-cache 模拟机制
- 让 `usage.cache_read_input_tokens` 优先使用 prefix-cache 模拟结果
- 将本地共享 prefix tree 作为 Kiro cache 命中的 source of truth
- 新增会话恢复索引，用于在无 session id 时恢复已知 conversation
- 为 Kiro admin 增加该模拟器的全局配置项
- 复用现有 key 级 cache 开关，控制该 key 是否参与/使用模拟结果
- 将当前公式模式保留为兼容模式

### Out of Scope

- 试图推断 upstream 的真实 KV page/block 内部布局
- 伪造 upstream 真正的 cache telemetry
- 让失败请求参与 prefix tree / conversation anchor 写入
- 通过模糊匹配、相似度匹配或其他 heuristic 恢复 conversation id
- 为不同 key 分别维护独立 prefix tree

## Confirmed Findings

### 1. 当前公式法最多只能给出保守下界，不是机制同类

当前 Kiro cache 估算依赖：

- 输入 token 估计
- 输出 token 估计
- `credit_usage`
- 每模型 `Kmodel`

然后代数反推 `cache_read_input_tokens`。

这套方法只能给出保守下界，不是 upstream prefix cache 的同类机制。
一旦输入 token 估计或 credit 模型偏差变大，cache 估算也会跟着偏。

### 2. 像 SGLang 这类 prefix cache，命中的不是 session id，而是 token prefix / cache span

SGLang 的 RadixAttention / HiCache 机制是：

- 基于 token prefix 的 KV cache span/page
- 通过 radix tree 做最长前缀匹配
- 命中多少前缀，就复用多少缓存

这说明 session id 顶多是“会话连续性锚点”，不是 prefix cache 的直接 key。

### 3. 当前系统中 session 连续性和 cache 命中是两个层次

即使 session id 正确复用：

- tool_result 变化
- system reminder 变化
- tool 定义顺序变化
- 当前轮用户输入变化

都仍然会破坏 prefix cache 的稳定前缀。

因此必须把：

- `conversation_id` 恢复
- prefix cache 命中模拟

建成两套不同的数据结构和算法。

### 4. 修正后的 `ConversationState` 才是唯一可信 source of truth

当前 Kiro 请求在进入 upstream 前会经过：

- transport 噪音清洗
- duplicate tool_use rewrite
- tool normalization
- history/current-turn 切分
- placeholder tool 注入
- orphaned tool_result 清理

因此 cache 与 session 相关计算不能基于原始 client JSON。
必须基于最终 `ConversationState`。

## Options Considered

### Option A: 继续强化公式法

通过更多参数、更复杂回归、更细分模型来改进：

- `credit_usage -> cache`

优点：

- 实现成本低

缺点：

- 机制上仍然不是 prefix cache
- 很难解释
- 无法解决“session 没复用好”造成的结构性问题

### Option B: 全站共享 prefix tree + 精确 history anchor

拆成两套结构：

- prefix tree：做 cache 命中模拟
- anchor index：做 conversation 恢复

并且都基于修正后的 canonical history / stable-prefix tokens。

优点：

- 机制上接近真实 prefix cache
- 可解释
- 可审计
- 能把 session 连续性和 cache 命中问题分开

缺点：

- 实现复杂度高于公式法
- 需要明确 canonicalization 规则与容量控制

### Option C: 每 key 一棵 prefix tree

优点：

- 实现隔离简单

缺点：

- 系统性低估命中
- 不符合 prefix cache 的共享本质
- 与用户确认的“全站共享一棵树”冲突

## Chosen Design

采用 Option B。

本次引入两套共享结构：

1. `KiroPrefixCacheSimulator`
   - 全站共享 prefix tree
   - 负责模拟 `cache_read_input_tokens`

2. `KiroConversationAnchorIndex`
   - 全站共享 anchor 索引
   - 负责在无 session id 时恢复 conversation id

两者共用**同一套 canonicalization 规则**，但输入窗口不同：

- lookup anchor: `canonical(pre-turn history)`
- resume anchor: `canonical(post-turn history)`
- prefix cache match: `canonical(stable-prefix tokens)`

## Detailed Design

### 1. Canonicalization Is The Real Source Of Truth

必须先定义一套统一 canonicalizer，将 `ConversationState` 投影为稳定表示。

输出至少分成三类：

1. `canonical_history_before_current_turn`
2. `canonical_history_after_successful_turn`
3. `canonical_stable_prefix_tokens`

原则：

- 基于修正后的 `ConversationState`
- 不依赖原始 client JSON 字段顺序
- 不包含 transport-only 噪音
- 对工具名、历史结构、消息顺序使用稳定表示

### 2. Stable-Prefix Boundary

第一版中，以下内容进入 `canonical_stable_prefix_tokens`：

- system prompt 的 canonical 表示
- 历史对话 `history`
- 稳定工具定义区

以下内容不进入 `canonical_stable_prefix_tokens`：

- 当前轮新增 user 文本
- 当前轮 `tool_result`
- 当前轮 transport/reminder 类临时内容

这样定义后，cache 命中严格表示：

> “这次请求在进入当前轮新增内容之前，与之前全站 Kiro 请求共享了多少稳定前缀 token”

### 3. Conversation Recovery Uses Anchors, Not Prefix Match

conversation 恢复不能直接用 prefix tree 做模糊推断。

必须拆成两阶段：

#### 3.1 Lookup Anchor

请求到达后，先基于：

`canonical_history_before_current_turn`

计算：

`lookup_anchor_hash`

如果这次请求没有显式 session id，且兼容 metadata 也没有给出有效 session，
则用 `lookup_anchor_hash` 去 `KiroConversationAnchorIndex` 查找是否已有已知 conversation。

只有 **完全相等** 才允许恢复。

#### 3.2 Resume Anchor

请求成功完成后，基于：

`canonical_history_after_successful_turn`

计算：

`resume_anchor_hash`

并把：

- `resume_anchor_hash`
- `conversation_id`
- `model`
- `last_seen_at`

写入 `KiroConversationAnchorIndex`。

这样下一次无 session id 的请求，才能从“上一次成功结束后的状态”恢复 conversation。

### 4. Prefix Cache Matching

cache 模拟不使用 hash 做 lookup，而是用 token 序列做最长前缀匹配。

流程：

1. 从当前请求的 `ConversationState` 提取 `canonical_stable_prefix_tokens`
2. 用该 token 序列去全站共享 prefix tree 中做最长前缀匹配
3. 返回命中的 token 数 `matched_prefix_tokens`
4. 将该值作为 `cache_read_input_tokens`

这一步不看 `credit_usage`。

### 5. Write Timing

为避免污染数据结构：

- 只有请求成功时才写入 prefix tree
- 只有请求成功时才写入 resume anchor
- 失败请求不写入任何共享结构

### 6. Mode And Key-Level Toggle

现有 key 上已经有 Kiro cache estimation 开关。

本次设计将其语义升级为：

- key 关闭时：
  - 不使用 prefix-cache 模拟结果
  - 不把该 key 的请求写入共享 prefix tree
  - 不把该 key 的请求写入 anchor index

- key 开启时：
  - 按 provider 全局 mode 决定具体算法

provider 全局 mode 新增：

- `formula`
- `prefix_tree`

这样保持兼容性：

- 旧逻辑仍可保留为兼容模式
- 新逻辑可逐步切换和验证

### 7. Capacity And Eviction

prefix tree 与 anchor index 都必须有硬上限。

新增全局配置：

- `kiro_prefix_cache_mode`
- `kiro_prefix_cache_max_tokens`
- `kiro_prefix_cache_entry_ttl_seconds`
- `kiro_conversation_anchor_max_entries`
- `kiro_conversation_anchor_ttl_seconds`

第一版淘汰策略：

- `TTL + LRU`

具体规则：

- 节点/anchor 超过 TTL 后可过期删除
- 超过容量上限时，从最久未命中的叶子/anchor 开始裁剪

不做复杂多级缓存，不做 page compaction，不做后台持久化。

### 8. Third-Party Library Policy

本次实现应优先采用成熟第三方库，不优先手搓底层数据结构。

约束如下：

1. prefix tree / trie 结构优先评估社区成熟 crate
   - 可选方向包括 radix trie / qp-trie 一类结构
2. LRU/容量淘汰优先评估成熟 cache crate
3. 只有在以下任一条件满足时，才允许自实现核心结构：
   - 现有 crate 无法表达“最长前缀匹配 + 可裁剪 token 树”
   - crate 维护状态差、质量风险明显、或引入过多无关复杂性
   - crate API 无法满足当前并发/内存边界

如果最终需要自实现，也必须只自实现最小必要层：

- canonical stable-prefix node
- longest-prefix lookup
- leaf-level LRU bookkeeping

不允许为了“感觉更可控”而先默认手搓整套 radix tree。

## Runtime Semantics

### 1. Session Resolution Order

Kiro session 解析顺序更新为：

1. 显式 request headers
2. 兼容 metadata legacy 来源
3. `lookup_anchor_hash` 精确恢复
4. fallback 新 UUID + warn

### 2. Cache Token Fields

在 `prefix_tree` 模式下：

- `cache_read_input_tokens = matched_prefix_tokens`
- `cache_creation_input_tokens = 0`
- `input_tokens = stable_uncached_tokens + cache_read_input_tokens + volatile_current_turn_tokens`

其中第一版只要求：

- `cache_read_input_tokens` 正确反映稳定前缀命中

如果 `input_tokens` 的拆分需要更细口径，可在实现阶段按现有 Anthropic usage 结构再细化。

### 3. Formula Mode Compatibility

在 `formula` 模式下，继续沿用当前 Kmodel 公式逻辑。

这样：

- 现网可以先灰度
- 新方案有明确回退路径

## Observability

新增结构化日志：

- session 恢复命中：
  - `lookup_anchor_hash`
  - `resolved_conversation_id`
  - `recovery_source=anchor_index`
- session 恢复 miss：
  - `lookup_anchor_hash`
  - `fallback_reason`
- prefix cache 命中：
  - `matched_prefix_tokens`
  - `stable_prefix_tokens`
  - `cache_mode`
- prefix tree 淘汰：
  - `evicted_tokens`
  - `evicted_entries`
  - `eviction_reason=ttl|capacity`

## Testing Strategy

至少覆盖以下测试：

1. **Canonicalization stability**
   - 语义等价的 `ConversationState` 生成相同 canonical history / stable-prefix tokens

2. **Lookup / resume anchor separation**
   - `lookup_anchor` 与 `resume_anchor` 输入窗口不同，但 canonicalizer 一致

3. **Session recovery**
   - 无 header / metadata 时，可从精确 anchor 恢复 conversation
   - history 不同一处则不恢复

4. **Prefix tree matching**
   - 最长前缀命中正确
   - 当前轮新增内容不会被算进 cache 命中

5. **Capacity / eviction**
   - TTL 生效
   - LRU 裁剪生效

6. **Compatibility**
   - `formula` 模式行为不回归
   - key 级关闭时完全不参与共享结构

## Risks

### 1. Canonicalization 如果定义不稳，会直接破坏命中率

这是最大风险。
所以 canonicalizer 必须尽量小、尽量可解释，并且严格围绕修正后的 `ConversationState`。

### 2. Prefix tree 若按 token 逐个节点建树，可能有内存压力

因此必须从一开始就带容量上限和淘汰策略。

### 3. 第三方 trie crate 未必完全贴合需求

所以 spec 明确为：

- 优先第三方库
- 但不为了“必须第三方”而把核心语义做歪

### 4. 恢复 conversation 只能精确恢复，不能指望高恢复率

这是有意为之。
本次目标是“稳”，不是“尽可能多恢复”。

## Rollout

推荐 rollout 顺序：

1. 先实现 canonicalizer + prefix tree + anchor index
2. 保留 `formula` 为默认模式
3. 提供 admin 全局模式切换
4. 先在少数 key 上开启 key-level 参与
5. 观察日志与命中数据后，再决定是否把默认模式切到 `prefix_tree`
