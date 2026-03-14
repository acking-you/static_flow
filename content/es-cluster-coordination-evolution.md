---
title: "Elasticsearch 分布式协调算法演进全景：从 Zen Discovery 到 7.0+ 新协调子系统"
summary: "系统梳理 Elasticsearch 集群协调从 Zen Discovery 到 7.0+ 新协调子系统的完整演进，解释 minimum_master_nodes 的局限、投票配置与 term 的引入、为何 Elastic 选择 Raft-like 而非直接照搬 Raft，以及协调层与副本复制的明确边界。"
detailed_summary_zh: |
  这篇文章按真实实现而不是按概念口号，完整拆解 Elasticsearch 集群协调在 7.0 前后的变化。前半部分复原 Zen Discovery 的发现、选主、等待 join、发布 cluster state 和 minimum_master_nodes 提交流程，指出它为什么把安全性与可用性压在人工配置上，以及为什么在反复网络分区等极端序列下会暴露已知缺陷。

  后半部分转向 7.0 引入的新 cluster coordination 子系统，解释 cluster.initial_master_nodes、voting configuration、pre-vote、term、publish/apply-commit、自动重配置、投票排除 API 等机制如何配合。最后再回答三个核心问题：为什么必须重写、解决了什么、为什么不用标准 Raft，以及这个协调层究竟只负责哪一层、不负责哪一层。
detailed_summary_en: |
  This article reconstructs Elasticsearch cluster coordination from the real implementation path rather than from slogans. It first restores the Zen Discovery workflow before 7.0: peer discovery, master election, waiting for joins, cluster-state publication, and the minimum_master_nodes commit threshold, then explains why safety and availability depended too heavily on operator-managed configuration and why repeated network partitions exposed known weaknesses.

  The second half moves to the 7.0 cluster coordination subsystem: cluster.initial_master_nodes, voting configurations, pre-voting, terms, publish/apply-commit, automatic reconfiguration, and the voting exclusions API. It closes by answering why the rewrite was needed, what it fixed, why Elasticsearch did not adopt textbook Raft, and what this layer is and is not responsible for.
tags: ["elasticsearch", "cluster coordination", "zen discovery", "raft", "distributed systems"]
category: "Distributed Systems"
category_description: "分布式系统中的一致性、集群协调、故障恢复与工程化实现"
author: "ackingliu"
date: "2026-03-12"
---

# Elasticsearch 分布式协调算法演进全景：从 Zen Discovery 到 7.0+ 新协调子系统

这篇文章只讨论 **Elasticsearch 集群协调层**，也就是谁是 master、哪些 master-eligible 节点有投票权、cluster state 如何提交与发布、节点加入和离开时系统如何保持安全与可用。它**不**讨论文档写入如何从 primary 复制到 replica，也**不**讨论 peer recovery、translog、global checkpoint 或 CCR。

如果把 ES 看成两层系统，可以先记住这条总分界线：

```text
协调层（本文主题）
peer discovery -> master election -> voting configuration -> cluster state publish/commit

数据复制层（本文特意区分出去）
primary shard -> replica shard -> seq_no/global checkpoint -> recovery/retention lease
```

很多关于 ES 一致性的误解，都来自把这两层混在一起。7.0 前后被重写的是前者，不是后者。

## 背景与问题边界

Elasticsearch 的 master-eligible 节点需要一起完成两件最基础的事：

1. 选出一个 master。
2. 对新的 cluster state 达成一致，并把它提交出去。

这里的 cluster state 指的是集群元数据和路由元数据，例如：

- 节点成员关系
- 索引和模板元数据
- shard routing table
- voting configuration
- cluster blocks

它不是 Lucene segment，也不是主分片上的文档变更日志。也就是说：

- “谁是当前 primary shard” 这个**结论**在 cluster state 里。
- “某条写请求怎样从 primary 复制到 replica” 这个**执行过程**不在 cluster coordination 里。

这个边界非常关键，因为它直接决定了一个事实：**即使 ES 采用了更接近 Raft 的集群协调算法，也不会自动把文档复制路径变成 Raft 日志复制。**

## 术语与模型

为了避免后文混乱，先固定几个术语。

### 协调层对象

- **master-eligible node**：能参与选主和投票的节点。
- **master**：当前负责发布 cluster state 的节点。
- **voting configuration**：哪些 master-eligible 节点的票数被计算在内。
- **quorum**：投票配置中“超过一半”的响应集合。
- **cluster state publication**：master 把一个新 cluster state 发给其他节点并等待提交。
- **commit / apply commit**：先形成法定提交，再在各节点应用已提交的 cluster state。

### 数据层对象

- **primary shard / replica shard**
- **ReplicationOperation / TransportReplicationAction**
- **ReplicationTracker**
- **peer recovery / retention lease / seq_no / global checkpoint**

它们属于索引写入和副本复制路径，不属于本文所说的 cluster coordination。

## 7.0 之前的 Zen Discovery

7.0 之前，ES 的主协调实现通常被社区称为 **Zen Discovery**，也经常被叫做 **Zen1**。从 6.8.23 的源码可以直接看到它的核心骨架：

- `ZenDiscovery`
- `ElectMasterService`
- `NodeJoinController`
- `PublishClusterStateAction`
- `MasterFaultDetection`
- `NodesFaultDetection`

这套机制不是“没有 quorum”，恰恰相反，它也用 quorum 思想；问题在于 **quorum 依赖 `discovery.zen.minimum_master_nodes` 这个外部人工配置**，而不是让投票成员和提交规则成为集群内部、可提交、可持久化的一部分。

### 发现与选主流程

从 `ZenDiscovery.startInitialJoin()` 到 `innerJoinCluster()`，旧流程可以抽成下面这条链路：

```text
ZenPing 发现节点
-> findMaster()
-> 如果发现活跃 master，则尝试 join 该 master
-> 如果没有活跃 master，则在候选者中选出“最好”的 master
-> 本地当选后等待足够的 master joins
-> 成为 master 并发布新的 cluster state
```

旧算法里有两个特别重要的细节。

第一，**谁更适合当 master** 并不是随机的。`ElectMasterService` 会优先选择 cluster state version 更高的候选节点，再用节点 ID 做 tie-break。这么做的直觉是：如果一个节点知道更多更新，那它更适合继续当 master。

第二，**真正完成“本地成为 master”之前，需要等足够多的 master join 进来**。在 6.8.23 的 `ZenDiscovery.innerJoinCluster()` 里，本地节点当选后会计算：

```text
requiredJoins = max(0, minimum_master_nodes - 1)
```

也就是说，本地节点把自己算作一个 master 票，还需要等待 `minimum_master_nodes - 1` 个额外 master-eligible 节点 join 进来，`NodeJoinController.waitToBeElectedAsMaster()` 才会让它完成选主。

### 发布与提交流程

旧版不是“选出 master 就完事了”。master 还必须把新 cluster state 发出去并提交。

`ZenDiscovery.publish()` 最终会调用 `PublishClusterStateAction.publish(clusterChangedEvent, electMaster.minimumMasterNodes(), ackListener)`。真正的提交门槛在 `PublishClusterStateAction.SendingController` 里：**收到足够多 master-eligible 节点的 ack 之后才 commit**，commit 完再向此前已响应的节点发送 commit 消息。

因此旧版流程更准确地说是：

```text
发现 -> 选出候选 master -> 等待足够 joins
-> 计算新 cluster state
-> 发送 cluster state
-> master acks 达到 minimum_master_nodes
-> commit
-> 各节点应用
```

从“机制是否考虑 quorum”这个角度说，Zen Discovery 并不粗糙；它真正的问题是：**quorum 的正确性严重依赖运维人员是否始终正确维护 `minimum_master_nodes`。**

### `minimum_master_nodes` 的中心地位

旧文档长期要求把 `discovery.zen.minimum_master_nodes` 设置为 master-eligible 节点数的一半以上，也就是大家熟悉的：

```text
(N / 2) + 1
```

例如 3 个 master-eligible 节点，应设为 2。这个设置同时影响：

- 选主时是否“有足够候选者”
- 当选 master 后是否“等到了足够 joins”
- cluster state 是否“拿到了足够 master ack，可以 commit”
- 节点离开后当前 master 是否还“有足够 master 节点继续工作”

这说明旧算法把多个安全条件都折叠成了一个外部阈值。只要这个阈值错了，系统安全性和可用性就一起被拖下水。

### 故障模式与运维代价

旧算法的真正痛点，不在“没有设计”，而在“设计把关键正确性交给了人来维持”。

#### 配置过低时的 split-brain 风险

如果 `minimum_master_nodes` 配得过低，例如 3 个 master-eligible 节点却设成 1，那么网络分区时多个分区都可能认为自己有资格选主并发布 cluster state。这就是经典 split-brain 风险。

旧版 `ElectMasterService.logMinimumMasterNodesWarningIfNecessary()` 甚至会直接打警告：当前值太低，可能导致数据丢失。

#### 配置过高时的可用性风险

反过来，如果阈值设得过高，系统会过度保守。一次正常维护、一次重启、一次故障，都可能让剩余节点无法形成法定人数，集群就卡在无法选主或无法提交 cluster state 的状态。

#### 集群规模变化时的人工负担

更麻烦的是，这个值不是一次配置、永远正确。以下场景都要重新核对它：

- 增加或减少 master-eligible 节点
- 拆分集群
- 滚动升级
- 机器永久下线
- 临时维护窗口

也就是说，**旧算法要求运维人员持续理解当前 master-eligible 节点拓扑，并把正确 quorum 手工写进配置**。算法没把 membership 变化内生化。

#### 已知的极端分区缺陷

Elastic 在 7.0 发布时点名过 Zen Discovery 的一类已知弹性问题：**在某些重复网络分区序列下，cluster state 更新可能丢失**。这不是日常最常见问题，但它说明旧实现的安全性边界不够干净，尤其在复杂故障序列下很难给出像现代共识协议那样直接、可证明的叙事。

#### 选主恢复较慢与日志可解释性不足

Elastic 官方在 7.0 发布文章里还专门提到，旧系统通常要等待几秒才会完成 master election 和故障恢复，而且当无法选主时，日志往往不能立刻给出足够清晰的原因。这不是纯理论缺陷，而是直接影响线上恢复速度和故障诊断。

## 6.x 到 7.0 的过渡阶段

理解演进时，一个常见误区是把时间线说成“6.x 就已经完全是新算法”。更准确的说法是：

- **7.0 是新 cluster coordination 子系统正式成为默认实现的分水岭。**
- **6.x 的末期主要还是旧 Zen Discovery 时代，只是 7.0 为滚动升级保留了兼容路径。**

从 7.0 源码可以直接看出这件事：

- `Coordinator.ZEN1_BWC_TERM = 0`
- `DiscoveryUpgradeService`
- `Coordinator.isZen1Node(...)`
- `PublicationTransportHandler` 里还有对旧 `PublishClusterStateAction` 提交路径的兼容分支

这说明 7.0 的目标并不是“推倒重来然后要求用户停机重建集群”，而是：

1. 引入新协调模型。
2. 在升级过程中兼容旧节点。
3. 等滚动升级完成后，再完全进入新模型。

这也是为什么很多 ES 工程师会口头上把新系统称为 **Zen2**，但官方文档通常更偏向叫它 **new cluster coordination subsystem**。

## 7.0 之后的新协调子系统

7.0 之后，协调子系统的重心从“外部阈值 + 旧发现流程”转向了“持久化 term + voting configuration + quorum-based publication”。

从 7.0 的源码可以看到这批新组件：

- `Coordinator`
- `CoordinationState`
- `CoordinationMetaData` / `VotingConfiguration`
- `JoinHelper`
- `PreVoteCollector`
- `PublicationTransportHandler`
- `ClusterBootstrapService`
- `Reconfigurator`
- `LeaderChecker`
- `FollowersChecker`
- `ClusterFormationFailureHelper`

当前新版本源码又在这个基础上继续细化，例如 `StatefulPreVoteCollector` 等，但 7.0 已经把核心模型定下来了。

### 核心模型：term、voting configuration、committed state

新系统的三个核心支点是：

1. **term**：选主纪元。更高 term 会压过更低 term。
2. **voting configuration**：哪些节点的票算数，不再靠 `minimum_master_nodes` 这种外部数字表达。
3. **last accepted / last committed cluster state**：节点明确持久化自己接受过和提交过的状态。

这三个概念一起把“谁能选主、谁的票有效、什么叫真正提交”从运维经验，变成了集群内部、可持久化、可传播、可验证的状态机逻辑。

### 启动与 bootstrap 机制

新系统第一步不是盲猜有多少节点，而是要求 brand-new cluster 在首次启动时显式设置：

```text
cluster.initial_master_nodes
```

这就是 bootstrap configuration。官方文档明确说明：**集群没有安全的方法自己推断出初始投票集合**。如果这一步做错，最危险的后果不是“选不出 master”，而是**可能意外形成两个彼此独立的新集群，之后无法安全合并**。

这是一个非常关键的设计转向：

- 旧思路更像“告诉每个节点需要多少个 master”
- 新思路更像“明确告诉集群，第一次选举时究竟哪些节点的票应该被计算”

它不是一个语义上的小修，而是从“人数阈值”升级成了“成员集合”。

### 选举流程

新选举流程可以概括成：

```text
peer discovery
-> pre-vote
-> 在更高 term 发起真实选举
-> 收集 join / votes
-> 赢得当前 voting configuration 的 quorum
-> becomeLeader
```

这里最值得注意的是 **pre-vote**。

Elastic 在发布文章里明确说，新系统引入了“Raft-style pre-voting round”。它的作用不是替代正式选举，而是在真正 bump term 之前，先判断这次选举有没有赢面。没有赢面的选举会被压掉，从而避免不必要的 term 抖动和无意义的 leadership churn。

在 `Coordinator` 里，`startInitialJoin()` 会先 `becomeCandidate()`，随后由 `PreVoteCollector` 驱动预投票；一旦具备条件，才进入真正选举并推进 term。

这背后的好处有两个：

1. **压制无望选举**，减少节点在网络抖动时不停自我提名。
2. **把“谁可以推进 term”与“谁真正有机会赢得 quorum”联系起来**。

### 成为 leader 之后的状态发布

新系统依然不是“leader 一宣告，大家就都听它的”。真正关键的是 publication。

在 7.0 中，`MasterService` 负责计算新的 cluster state，而 `Coordinator` 负责把它发布出去。当前源码里也能看到 `Node` 会把 `MasterService` 的 `clusterStatePublisher` 设置成 `Coordinator`。

新 publication 流程大致是：

```text
leader 计算新 cluster state
-> PublicationTransportHandler 发送 full state 或 diff
-> follower handlePublishRequest，持久化 accepted state
-> leader 收到 publish responses
-> 达到 voting configuration 的 quorum
-> 发送 apply-commit
-> follower 应用 committed state
-> leader 最后本地 apply
```

这里有三个非常重要的改进。

#### 提交门槛来自 voting configuration，而不是外部阈值

旧系统问的是：“`minimum_master_nodes` 设成多少？”

新系统问的是：“**当前被提交进 cluster state 的 voting configuration 是谁，它的一半以上是多少？**”

这意味着 quorum 不再依赖节点脑中的 YAML 文件，而来自已经被集群一致接受的元数据。

#### publication 与 commit 被清晰拆开

`PublicationTransportHandler` 先处理 publish，再由 `ApplyCommitRequest` 推动 commit。也就是说：

- follower 先接受某个待提交状态
- leader 在看到法定 publish 响应后，再宣布 commit
- commit 之后各节点才把它作为已提交状态应用

这比“收到就算数”的叙事清晰得多，也更接近现代共识系统的表达方式。

#### full state 与 diff 两种发布路径

新系统没有为了“像教科书 Raft”而把所有东西硬塞进统一日志条目。`PublicationTransportHandler` 会根据节点状态选择发送 full cluster state 还是 diff。这说明 ES 仍然保留了自己“围绕 cluster state publication 优化”的工程风格。

### 自动重配置机制

新系统最能直接降低运维成本的改动，是 **自动维护 voting configuration**。

`Reconfigurator` 的职责就是：当 master-eligible 节点加入或离开时，计算一个更合理的投票集合，并把这件事本身变成 cluster state 更新的一部分。

这解决了旧系统的根本尴尬：

- 旧系统让人手工维护 quorum 阈值
- 新系统让集群自己维护投票成员集合

官方文档里有几个非常重要的行为准则。

#### 奇偶处理

如果 master-eligible 节点数是偶数，ES 通常会把其中一个节点排除出 voting configuration，让投票集合保持奇数。这不是浪费节点，而是为了让网络对半分时，总有一边还能拿到严格多数。

#### 自动 shrink

默认情况下，`cluster.auto_shrink_voting_configuration = true`，系统会在适当时机自动缩小投票配置，减少人工干预成本。但缩容到非常小的配置时，仍然要理解系统对可用性的影响。

#### voting exclusions API

如果你是有意下线节点，尤其是在投票配置很小的时候，官方建议用 **voting configuration exclusions API** 来安全地把节点退休出投票集合。也就是说，成员变化不再靠“改一个 magic number”，而是通过受控的 reconfiguration 过程来完成。

### 故障检测与形成失败诊断

新系统不只是“数学更对”，它还更工程化。

- `LeaderChecker` 负责 follower 观察 leader 是否还活着
- `FollowersChecker` 负责 leader 观察 followers 的跟进情况
- `ClusterFormationFailureHelper` 会在无法形成集群时给出更有解释力的日志

官方发布文章提到，新的 master election 通常可以在 **well under a second** 的范围内完成，而且当无法选主时，日志会定期说明原因。这对线上运维非常实际。

## 新算法出现的根本原因

把前后两代系统放在一起看，7.0 之所以必须重写，不是为了“追赶潮流”，而是旧模型有三个结构性问题已经很难继续修补。

### 外部阈值模型不够稳固

`minimum_master_nodes` 是一个数字，不是一个投票成员集合。它表达不了：

- 哪些节点算票
- 什么时候成员变化已经被集群自己确认
- brand-new cluster 第一次启动时究竟谁有资格参与 bootstrap

它只能表达“至少多少个”。这对于动态 membership 来说天生信息不够。

### 正确性过度依赖运维操作

旧算法不是不能安全运行，而是**安全运行要求运维持续保持配置与拓扑同步**。这在稳定小集群里还能接受，但一旦遇到：

- 自动扩缩容
- 容器编排
- 滚动升级
- 跨可用区故障

就会迅速暴露出“人工维持 quorum 语义”的脆弱性。

### 故障叙事不够清晰

一个现代协调层不仅要“通常可用”，还要在复杂分区、重启、升级、成员变化下给出明确的安全边界。Elastic 7.0 的重写，本质上是在追求：

- 更容易陈述的安全语义
- 更容易证明或建模的状态转换
- 更低的运维误用成本
- 更快的恢复时间

## 新算法具体解决了什么

如果把 7.0+ 的收益按问题归类，可以直接归纳成下面几条。

### 消除了 `minimum_master_nodes` 的人工维护负担

这是最直观的一条。官方 7.0 发布文章明确说，新系统**去掉了这个设置**，由 ES 自己维护 voting configuration。

### 把成员与 quorum 规则纳入 cluster state 本身

旧算法把 quorum 条件放在配置里；新算法把投票成员集合写进 `CoordinationMetaData`，并作为 cluster state 的一部分提交和持久化。这样“谁的票算数”不再是外部常量，而是集群一致状态。

### 改善 bootstrap 安全性

`cluster.initial_master_nodes` 的引入，解决的是 brand-new cluster 第一次形成时的“初始票权集合”问题。它避免了“只知道人数，不知道成员”带来的歧义。

### 压制无望选举与 term 抖动

pre-vote 让节点在真正推进 term 之前先做一次可行性探测。这样网络短抖动时，不会因为每个节点都轻易自提名而造成选主反复重来。

### 降低极端故障序列下的安全风险

Elastic 官方明确把“重复网络分区导致 cluster state 更新丢失”的已知问题列为重写动机之一。新系统更接近现代 quorum/term 模型，就是为了把这类边界做干净。

### 提升恢复速度与诊断体验

新系统选主更快，形成失败时日志更清晰。这一点在分布式系统里不是附属价值，而是直接影响 MTTR。

## 为什么不是标准 Raft

这个问题最容易被过度简化。准确答案不是“ES 不需要共识”，也不是“ES 完全不用 Raft 思想”，而是下面四层意思要一起看。

### 第一层：Elastic 自己承认它与 Raft 很接近

Elastic 工程师 David Turner 在公开讨论里给过非常直接的回答：**新系统与 Raft “quite close”**，并且提到 7.0 发布文章里也明确写了它借鉴了 Raft 和其他分布式一致性工作。

所以如果有人说“ES 完全没用 Raft 思路”，这是不准确的。

### 第二层：ES 协调层不是一个“所有数据都走日志复制”的通用共识层

标准教科书式 Raft 最直观的心智模型是：

```text
leader 追加日志条目
-> 日志复制到多数派
-> commit
-> 状态机应用
```

但 ES 的 cluster coordination 只负责 **cluster state 元数据**。文档写入、副本复制、peer recovery 走的是另一套 replication 路径。因此：

- 如果把 cluster coordination 直接改写成“标准 Raft 日志复制”，也**不会自动覆盖 shard data replication**
- ES 仍然需要保留以 cluster state publication 为中心的现有 master / metadata 工作流

换句话说，ES 没有把整个数据库写入路径都统一收编到一个 Raft log 里，因为它本来也不是这么分层的。

### 第三层：ES 选择了更贴合现有架构的 state publication 模型

从 `PublicationTransportHandler` 能看出，新系统仍然以 **发布 full state / diff** 为中心，而不是硬把所有变更都抽象成教科书式 append-only log entry。这种选择与 ES 原有 `MasterService -> cluster state -> publish` 的架构高度一致。

这说明 Elastic 的设计目标不是“名字上等于 Raft”，而是：

- 保留现有 cluster state 计算与发布模型
- 引入 term、pre-vote、quorum、明确 commit 这些现代共识要素
- 让实现更适合 ES 自己的元数据系统和升级路径

### 第四层：滚动升级兼容本身就是硬约束

7.0 源码里保留了明显的 Zen1 BWC 逻辑，例如 `ZEN1_BWC_TERM`、`DiscoveryUpgradeService`、对旧 commit 动作名的兼容等。这说明新系统必须考虑 **6.x -> 7.x 的渐进升级**。

如果简单回答“为什么不用 Raft”，我会给出这句最短、最准确的版本：

> **因为 ES 需要的是一个与 Raft 很接近、但严格贴合其 cluster state publication 模型、成员变更模型和滚动升级约束的协调子系统，而不是教科书原样照搬。**

## 协调层职责与非职责

这是本文最容易被误读，但也最需要说清楚的部分。

### 协调层负责的内容

ES 的 cluster coordination 负责：

- peer discovery 与 seed hosts 发现
- bootstrap 初始投票集合
- master election
- term 管理
- join validation 与 join vote
- voting configuration 与 reconfiguration
- cluster state 的 publish / commit / apply
- leader / follower 健康检查
- 无法形成集群时的诊断信息

如果一句话概括，就是：

> **它负责让 master-eligible 节点对“元数据控制面”达成一致。**

### 协调层不负责的内容

它**不**负责：

- 主分片写请求复制到副本分片
- `TransportReplicationAction` / `ReplicationOperation`
- `ReplicationTracker` 中的 in-sync set、global checkpoint
- translog replay
- peer recovery
- retention lease 同步
- CCR

这些都属于索引数据路径。

因此下面这句话可以直接当成结论记住：

> **ES 的分布式协调算法不负责副本复制；副本复制走的是独立的 primary-replica replication 机制。**

如果线上出现的是“某个 replica 落后了、global checkpoint 不前进、peer recovery 卡住”，第一反应不该去怀疑 cluster coordination，而应该先看 replication path。

## 关键结论

为了直接回答最常见的四个问题，这里给一个压缩版结论。

### 重写动机

- 旧算法把安全和可用性过度依赖到 `minimum_master_nodes` 的人工维护上。
- brand-new cluster 的 bootstrap 只靠“人数阈值”表达不够。
- 极端分区序列下存在官方承认的已知问题。
- 恢复速度和诊断可解释性都不够理想。

### 新系统解决的问题

- 去掉 `minimum_master_nodes`
- 引入显式 bootstrap configuration
- 把投票成员集合写进 cluster state
- 引入 pre-vote 和更清晰的 term 语义
- 自动维护 voting configuration
- 在不牺牲安全性的前提下提升恢复速度与日志可解释性

### 不直接照搬 Raft 的原因

- 新系统本质上已经是 Raft-like 的
- ES 协调层只覆盖 cluster state，而不是所有数据写入
- ES 延续了自身的 state publication / diff 发布模型
- 需要兼顾从 Zen Discovery 的滚动升级兼容

### 协调层的职责边界

- 它负责 master election、membership、cluster state publish/commit
- 它不负责 shard primary/replica 复制

## 新旧算法的并排对照

| 维度 | 7.0 前 Zen Discovery | 7.0+ 新协调子系统 |
|---|---|---|
| quorum 表达 | `minimum_master_nodes` 外部阈值 | `VotingConfiguration` 成员集合 |
| bootstrap | 主要靠发现与阈值语义 | `cluster.initial_master_nodes` 明确初始投票集合 |
| 选举前探测 | 没有独立 pre-vote 层 | 有 pre-vote，压制无望选举 |
| 选主依据 | 候选者 cluster state version + 节点 ID，外加 join 等待 | term + join votes + 当前 voting configuration quorum |
| 提交门槛 | 足够 master ack 且满足 `minimum_master_nodes` | 当前 voting configuration 的多数派 |
| membership 变化 | 人工维护阈值 | 自动重配置，必要时配合 exclusions API |
| 偶数 master 处理 | 依赖人为理解 quorum | 自动让 voting config 保持奇数更优 |
| 形成失败诊断 | 相对更弱 | 更清晰的周期性形成失败说明 |
| 极端故障叙事 | 边界较难说清 | 更接近现代共识系统表达 |

## 运维与排障场景

### 三主节点滚动重启

#### 旧系统

如果是 3 个 master-eligible 节点，必须确认 `minimum_master_nodes = 2`。设成 1 有 split-brain 风险，设成 3 会让你在下线一个节点后直接失去可用性。

#### 新系统

核心准则变成：**不要同时停掉 voting configuration 中一半或更多节点**。通常三主集群可以安全地一次维护一个节点，但在节点刚加入或刚离开后，要给系统一点时间完成 voting configuration 的调整。

### 四主节点缩到三主节点

#### 旧系统

你要先重新计算 quorum，再确保所有节点配置一致，否则有人按旧值判断，有人按新值判断，风险很高。

#### 新系统

系统会倾向保持奇数大小的 voting configuration。若是有意永久下线节点，先用 voting exclusions API 让它安全退出投票集合，再停机。

### 副本复制异常排查

如果现象是：

- replica 落后
- write ack 卡住
- global checkpoint 不推进
- recovery 一直不结束

那优先排查：

- `TransportReplicationAction`
- `ReplicationOperation`
- `ReplicationTracker`
- `IndexShard`

而不是先排查 cluster coordination。协调层只决定“谁可以发布 shard routing 变化”，不执行文档复制本身。

## 代码索引

如果要从源码继续向下读，建议按下面这组入口看。

### 旧系统入口

- `v6.8.23/server/src/main/java/org/elasticsearch/discovery/zen/ZenDiscovery.java`
- `v6.8.23/server/src/main/java/org/elasticsearch/discovery/zen/ElectMasterService.java`
- `v6.8.23/server/src/main/java/org/elasticsearch/discovery/zen/NodeJoinController.java`
- `v6.8.23/server/src/main/java/org/elasticsearch/discovery/zen/PublishClusterStateAction.java`

### 新系统入口

- `v7.0.0/server/src/main/java/org/elasticsearch/cluster/coordination/Coordinator.java`
- `v7.0.0/server/src/main/java/org/elasticsearch/cluster/coordination/PreVoteCollector.java`
- `v7.0.0/server/src/main/java/org/elasticsearch/cluster/coordination/PublicationTransportHandler.java`
- `v7.0.0/server/src/main/java/org/elasticsearch/cluster/coordination/Reconfigurator.java`
- `v7.0.0/server/src/main/java/org/elasticsearch/cluster/coordination/ClusterBootstrapService.java`
- `current/server/src/main/java/org/elasticsearch/node/Node.java` 中 `MasterService` 与 `Coordinator` 的连线

### 副本复制边界入口

- `server/src/main/java/org/elasticsearch/action/support/replication/TransportReplicationAction.java`
- `server/src/main/java/org/elasticsearch/action/support/replication/ReplicationOperation.java`
- `server/src/main/java/org/elasticsearch/index/seqno/ReplicationTracker.java`
- `server/src/main/java/org/elasticsearch/index/shard/IndexShard.java`

## 参考资料

以下资料是本文的主要依据，优先使用了 Elastic 官方博客、官方文档、Elastic 工程师公开说明和 Elasticsearch 源码：

- Elastic 官方博客，*A new era for cluster coordination in Elasticsearch*  
  https://www.elastic.co/blog/a-new-era-for-cluster-coordination-in-elasticsearch

- Elastic 7.0 文档，*Voting configurations*  
  https://www.elastic.co/guide/en/elasticsearch/reference/7.0/modules-discovery-voting.html

- Elastic 7.0 文档，*Quorum-based decision making*  
  https://www.elastic.co/guide/en/elasticsearch/reference/7.0/modules-discovery-quorums.html

- Elastic 7.0 文档，*Bootstrapping a cluster*  
  https://www.elastic.co/guide/en/elasticsearch/reference/7.0/modules-discovery-bootstrap-cluster.html

- Elastic 工程师 David Turner 在官方论坛对 “是否基于 Raft” 的说明  
  https://discuss.elastic.co/t/what-is-the-algorithm-in-elasticsearch-for-master-election-process/265102

- Elasticsearch v6.8.23 源码：Zen Discovery 相关实现  
  https://github.com/elastic/elasticsearch/blob/v6.8.23/server/src/main/java/org/elasticsearch/discovery/zen/ZenDiscovery.java  
  https://github.com/elastic/elasticsearch/blob/v6.8.23/server/src/main/java/org/elasticsearch/discovery/zen/NodeJoinController.java  
  https://github.com/elastic/elasticsearch/blob/v6.8.23/server/src/main/java/org/elasticsearch/discovery/zen/PublishClusterStateAction.java

- Elasticsearch v7.0.0 源码：新协调子系统相关实现  
  https://github.com/elastic/elasticsearch/blob/v7.0.0/server/src/main/java/org/elasticsearch/cluster/coordination/Coordinator.java  
  https://github.com/elastic/elasticsearch/blob/v7.0.0/server/src/main/java/org/elasticsearch/cluster/coordination/PublicationTransportHandler.java  
  https://github.com/elastic/elasticsearch/blob/v7.0.0/server/src/main/java/org/elasticsearch/cluster/coordination/Reconfigurator.java

