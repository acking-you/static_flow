---
title: "LanceDB 图片库事故复盘：manifest 混链导致 /api/images 与搜图 500 的定位与恢复"
summary: "完整复盘一次 images.lance 快照不一致事故：症状、影响范围、证据链、根因、应急回滚、基于本地 LFS 对象的全量恢复流程，以及可执行的预防清单。"
tags:
  - lancedb
  - staticflow
  - incident-response
  - git-lfs
  - xet
  - reliability
  - postmortem
category: "Reliability Engineering"
category_description: "Production reliability notes on root-cause analysis, data recovery, and operational safeguards."
author: "ackingliu"
date: "2026-02-18"
---

# LanceDB 图片库事故复盘：manifest 混链导致 /api/images 与搜图 500 的定位与恢复

> Code Version: StaticFlow `6bd6f64`  
> Data Repo Snapshot During Recovery: `ce4037b` (`/mnt/e/static-flow-data/lancedb`)

## 1. 事故摘要

本次事故表现为：

- `GET /api/images/:id-or-filename` 大面积 `500`
- `GET /api/image-search` / `GET /api/image-search-text` 大面积 `500`
- 前端图片卡片和搜图功能不可用

典型错误日志（后端）：

```text
LanceError(IO): External error: Not found:
.../images.lance/data/<file>.lance
```

核心结论：**`images.lance` 处于“元数据版本链与 data 文件集合不一致”的损坏状态**。

- 运行时读取到了 `421.manifest`（大快照）
- 但 `data/` 目录只剩小集合（最初仅约 91 个）
- `421.manifest` 实际需要约 386 个 data 分片
- 扫描时命中缺失分片，触发 IO Not Found

## 2. 背景与作用域

### 2.1 关键数据路径

- 内容库：`/mnt/e/static-flow-data/lancedb`
- 目标表：`images.lance`
- 版本元数据：`images.lance/_versions/*.manifest`
- 事务元数据：`images.lance/_transactions/*.txn`
- 数据分片：`images.lance/data/*.lance`

### 2.2 与业务接口的对应关系

下列接口都会进入 `images` 表扫描路径：

- `GET /api/images`（分页列表）
- `GET /api/image-search`（以图搜图）
- `GET /api/image-search-text`（文本搜图）
- `GET /api/images/:id-or-filename`（按 ID/文件名取图片）

对应后端入口见：

- `backend/src/handlers.rs:1884` `list_images`
- `backend/src/handlers.rs:1904` `search_images`
- `backend/src/handlers.rs:1930` `search_images_by_text`
- `backend/src/handlers.rs:1968` `serve_image`

对应 LanceDB 扫描路径见：

- `shared/src/lancedb_api.rs:874` `list_images`
- `shared/src/lancedb_api.rs:926` `search_images_by_text`
- `shared/src/lancedb_api.rs:1017` `search_images`
- `shared/src/lancedb_api.rs:1167` 查询路径诊断日志 `Query path selected`

## 3. 时间线（关键节点）

| 时间 | 事件 |
|------|------|
| T0 | 前端反馈图片请求与搜图随机失败，后端日志出现 `LanceError(IO): Not found ...images.lance/data/...` |
| T0+ | 排查确认并非 HTTP/代理问题，而是 LanceDB 数据层缺分片 |
| T0+ | 发现 `images.lance/_versions` 同时存在 `421.manifest` 与 `61.manifest` |
| T0+ | 首次应急：临时移出 `421.manifest` + `420.txn`，服务回落到 `61`，接口恢复但图片总量明显变少 |
| T0+ | 深挖证据链，确认 `421` 是大快照（386 行），`61` 是小快照（30 行） |
| T0+ | 发现 `421` 所需分片大多并未落地，但对应 LFS 对象仍在本地 `.git/lfs/objects` |
| T0+ | 执行全量恢复：从本地 LFS 对象补回 `421` 需要的分片（恢复 295 个，保留 91 个已存在） |
| T0+ | 二次验证：`images` 恢复到 386 行，`/api/images/*` 与 `image-search*` 全部恢复 200 |

## 4. 现象与影响

### 4.1 直接影响

- 首页与文章页部分封面无法加载
- 搜图页功能不可用
- 以图搜图结果无法返回
- 用户体验为“随机可用/不可用”，实质是查询路径触发到缺失分片时才报错

### 4.2 为什么会“有时能用，有时不能用”

`images` 表扫描是按查询路径命中的数据分片来读的：

- 某次查询恰好命中已存在分片：返回 200
- 某次查询命中缺失分片：直接 500

这会造成“随机性”，但本质是数据集合不完整，不是网络抖动。

## 5. 根因分析

### 5.1 不是单纯“421 > 61 就更新”

`421` 数字更大，只说明它在该版本链上更晚；**前提是同一条链的所有依赖文件完整可读**。

本次故障的关键是：

- 元数据链（`421.manifest`）存在
- 但其依赖的数据分片并未完整落地

因此“看起来更新”但“实际上不可读”。

### 5.2 证据链

#### 证据 A：大快照与小快照并存

```bash
ls -1 /mnt/e/static-flow-data/lancedb/images.lance/_versions
# 421.manifest
# 61.manifest
```

#### 证据 B：大快照行数明显更多

恢复 `421` 后：

```bash
./target/release/sf-cli db --db-path /mnt/e/static-flow-data/lancedb count-rows images
# Row count: 386
```

仅回落 `61` 时：

```text
Row count: 30
```

#### 证据 C：工作树跟踪文件数与大快照需求不一致（事故中间态）

```bash
git -C /mnt/e/static-flow-data/lancedb ls-files images.lance/data | wc -l
# 事故排查期间：约 91
```

#### 证据 D：缺失分片可在本地 LFS 缓存找到

例如某缺失分片指针对应：

```text
oid sha256:ec2af2ef1718b083a1c2c209745e1705fffd3b5b519ff98f79c97124da34518a
size 678741
```

该 oid 在本机存在：

```text
/mnt/e/static-flow-data/lancedb/.git/lfs/objects/ec/2a/ec2af2ef...
```

### 5.3 根因归纳

根因是一次数据仓库整理过程中，`images.lance` 被混入了不一致的快照状态：

1. `421.manifest` 被保留并被优先读取
2. 其依赖的 data 分片没有完整落地到 `images.lance/data`
3. 导致运行时读取时连续出现 `Not found`

这属于**快照一致性破坏**，不是单条业务数据删除。

## 6. 处置过程与方案权衡

### 6.1 应急处置（快速止血）

先临时移出：

- `images.lance/_versions/421.manifest`
- `images.lance/_transactions/420-*.txn`

回落到 `61` 链，接口迅速恢复；但图片规模变小（30 行），属于降级运行。

### 6.2 完整恢复（最终方案）

最终选择恢复 `421` 全量链，而不是永久停留在 `61`：

1. 保留/恢复 `421.manifest` 与对应 `420.txn`
2. 枚举 `421` 所有 data 指针
3. 对每个指针从本地 `.git/lfs/objects` 找实体对象
4. 批量复制到 `images.lance/data/`
5. 重新验证 API 和行数

恢复统计：

- 已存在：91
- 新恢复：295
- 总计：386

## 7. 恢复后验证

### 7.1 数据层验证

```bash
./target/release/sf-cli db --db-path /mnt/e/static-flow-data/lancedb count-rows images
# Row count: 386
```

### 7.2 接口层验证

```bash
curl -I 'http://127.0.0.1:39080/api/images/cover-default-wallhaven-mlldj8.jpg'
# HTTP/1.1 200 OK

curl -I 'http://127.0.0.1:39080/api/image-search-text?q=wallhaven&offset=0&limit=8'
# HTTP/1.1 200 OK
```

### 7.3 功能层验证

- 首页封面恢复
- 文章封面恢复
- 搜图分页恢复
- 以图搜图恢复

## 8. 预防与加固

### 8.1 运行流程加固

1. 同步/整理 LanceDB 前先停后端，避免边写边复制
2. 严禁只移动 `manifest/txn` 而不核对 `data` 完整性
3. 每次数据仓库操作后执行“接口冒烟 + 行数核对”

### 8.2 可执行检查清单

```text
[ ] images.lance/_versions 与 _transactions 只保留一条完整可读链
[ ] count-rows images 与预期规模一致
[ ] /api/images/:id-or-filename 返回 200
[ ] /api/image-search-text 返回 200 且 has_more 正常
[ ] git ls-files images.lance/data 与当前活跃 manifest 需求无显著缺口
```

### 8.3 最小化误操作建议

- 不要把“推送前瘦身”直接作用在生产数据目录
- 先在隔离目录做校验，再替换
- 任何 `cleanup` 前先做快照备份

## 9. 可复用的恢复 Runbook

### 9.1 快速止血（回落小快照）

```bash
# 仅示意：把高版本 manifest/txn 临时移出，回落到可读链
mv images.lance/_versions/421.manifest /tmp/
mv images.lance/_transactions/420-*.txn /tmp/
```

### 9.2 全量恢复（基于本地 LFS 对象）

```bash
# 1) 获取目标 manifest 对应的数据文件列表
# 2) 对每个 pointer 提取 oid sha256
# 3) 从 .git/lfs/objects/<aa>/<bb>/<oid> 拷贝到 images.lance/data/<filename>.lance
# 4) 重新验证 count-rows 与 API
```

> 这套流程的关键前提：本机 LFS 缓存仍保有所需对象。

## 10. 结论

这次事故不是 API 层 bug，也不是网络层问题，而是典型的数据快照一致性事故：

- 元数据可见，不代表数据可读
- manifest 版本号更大，不代表可直接上线
- 只要 `manifest/txn/data` 三者不一致，LanceDB 就会在扫描时爆出 IO Not Found

这次恢复的决定性因素是：**缺失分片虽然不在工作树，但仍在本机 LFS 对象库中**。利用这点完成了无外部依赖的全量恢复。

## Code Index

- `backend/src/handlers.rs:1884` `list_images`
- `backend/src/handlers.rs:1904` `search_images`
- `backend/src/handlers.rs:1930` `search_images_by_text`
- `backend/src/handlers.rs:1968` `serve_image`
- `shared/src/lancedb_api.rs:874` `list_images`
- `shared/src/lancedb_api.rs:926` `search_images_by_text`
- `shared/src/lancedb_api.rs:1017` `search_images`
- `shared/src/lancedb_api.rs:1167` query-path diagnostics log
- `scripts/start_backend_from_tmp.sh:7` DB path resolution policy (`DB_PATH` / `COMMENTS_DB_PATH`)

## References

- Lance Table Format: https://lance.org/format/table/
- Vortex File Format: https://docs.vortex.dev/specs/file-format
- Git LFS Spec: https://github.com/git-lfs/git-lfs/blob/main/docs/spec.md
- Hugging Face git-xet docs: https://huggingface.co/docs/hub/xet
