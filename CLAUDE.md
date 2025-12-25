# CLAUDE.md

Claude Code 在此项目中的工作指南。

## 核心理念

> **"Don't build agents, build skills instead."**
> — [Anthropic Expert Talk](https://www.youtube.com/watch?v=CEvIs9y1uog)

本项目不开发独立 AI Agent，而是通过 skill 让 Claude Code/Codex 完成智能工作，CLI 只负责朴素的 LanceDB 读写。

## 项目概述

**StaticFlow** 是本地优先的动态博客系统：
- Frontend: Yew 0.21 + WASM + TailwindCSS v4
- Backend: Axum 0.7 + LanceDB
- CLI: LanceDB 读写工具（朴素操作）
- Shared: 前后端共享类型

**当前状态**: Frontend UI 已完成，Backend 使用文件系统（待迁移到 LanceDB），CLI 待开发。

## 架构

```
static-flow/
├── frontend/     # Yew WASM 前端
│   ├── src/components/   # UI 组件
│   ├── src/pages/        # 页面
│   └── src/api.rs        # API 客户端
├── backend/      # Axum 后端
│   ├── src/handlers.rs   # API 处理器
│   ├── src/markdown.rs   # Markdown 解析
│   └── src/state.rs      # 应用状态
├── shared/       # 共享类型
│   └── src/lib.rs        # Article, Tag, Category 等
├── cli/          # LanceDB CLI（待开发）
└── content/      # Markdown 和图片
```

## 开发命令

```bash
# 启动开发环境
cd backend && cargo run                    # 后端 :3000
cd frontend && trunk serve --open          # 前端 :8080

# 构建
cargo build --workspace
trunk build --release                      # 前端生产构建

# 检查
cargo fmt --all && cargo clippy --workspace
```

## LanceDB Schema 设计

### articles 表

| 字段 | 类型 | 说明 |
|------|------|------|
| id | String | 主键，slug 格式 |
| title | String | 标题 |
| content | String | 原始 Markdown |
| summary | String | AI 生成摘要 |
| tags | List\<String\> | AI 生成标签 |
| category | String | AI 生成分类 |
| author | String | 作者 |
| date | String | 发布日期 |
| featured_image | String | 图片 ID 引用 |
| read_time | i32 | 阅读时间（分钟） |
| vector | Vector\<f32, 1536\> | 文本 embedding |
| created_at | Timestamp | 创建时间 |
| updated_at | Timestamp | 更新时间 |

### images 表

| 字段 | 类型 | 说明 |
|------|------|------|
| id | String | 主键，文件 hash |
| filename | String | 原始文件名 |
| data | Binary | 图片二进制 |
| thumbnail | Binary | 缩略图 |
| vector | Vector\<f32, 512\> | CLIP embedding |
| metadata | JSON | 元数据 |
| created_at | Timestamp | 创建时间 |

## API 重构计划

| 端点 | 当前实现 | 目标实现 |
|------|----------|----------|
| GET /api/articles | 文件系统扫描 + 内存缓存 | LanceDB 分页查询 |
| GET /api/articles/:id | 文件读取 | LanceDB 精确查询 |
| GET /api/search | 内存字符串匹配 | LanceDB FTS |
| GET /api/semantic-search | 无 | LanceDB 向量搜索 |
| GET /api/image/:id | content/images/ 文件 | LanceDB 二进制读取 |
| GET /api/tags | 内存聚合 | LanceDB 聚合查询 |
| GET /api/categories | 内存聚合 | LanceDB 聚合查询 |

## CLI 工具设计

### 命令列表

```bash
sf-cli init           # 初始化 LanceDB schema
sf-cli write-article  # 写入单篇文章
sf-cli write-images   # 批量写入图片
sf-cli query          # 查询数据
sf-cli delete         # 删除记录
```

### write-article 参数

```bash
sf-cli write-article \
  --file <path>           # Markdown 文件路径
  --summary <text>        # 摘要（AI 生成）
  --tags <tag1,tag2>      # 标签列表（AI 生成）
  --category <name>       # 分类（AI 生成）
  --vector <json>         # 可选，文本 embedding
```

### write-images 参数

```bash
sf-cli write-images \
  --dir <path>            # 图片目录
  --recursive             # 递归扫描
  --generate-thumbnail    # 生成缩略图
```

## Claude Code 工作流

处理新文章的标准流程：

1. **读取文件**: 读取 Markdown 文件，解析 frontmatter
2. **生成 metadata**: 基于内容生成摘要、标签、分类
3. **写入数据库**: 调用 CLI 工具写入 LanceDB

示例交互：
```
用户: 把 content/new-post.md 同步到数据库

Claude Code:
1. 读取 content/new-post.md
2. 分析内容，生成：
   - 摘要: "本文介绍了 Rust 异步编程的核心概念..."
   - 标签: ["rust", "async", "tokio"]
   - 分类: "Tech"
3. 执行: sf-cli write-article --file content/new-post.md --summary "..." --tags "rust,async,tokio" --category "Tech"
```

## 关键文件

| 文件 | 说明 |
|------|------|
| `backend/src/handlers.rs` | API 处理器，需重构为 LanceDB |
| `backend/src/state.rs` | 应用状态，需改为 LanceDB 连接 |
| `shared/src/lib.rs` | 数据模型定义 |
| `frontend/src/api.rs` | API 客户端 |

## 开发优先级

1. **Phase 1**: CLI 工具开发
   - 实现 init 命令（创建 LanceDB schema）
   - 实现 write-article 命令
   - 实现 write-images 命令

2. **Phase 2**: Backend 重构
   - 添加 lancedb crate 依赖
   - 重构 handlers.rs 使用 LanceDB
   - 实现向量搜索端点

3. **Phase 3**: 功能完善
   - 语义搜索 UI
   - 以图搜图

## 注意事项

- 当前 backend 使用文件系统 + 内存缓存，所有 API 都是真实实现（非 mock）
- content/ 目录包含 15 篇示例文章和图片
- 前端支持 mock feature 用于离线开发
- Tailwind CSS v4 通过 npm 管理，Trunk 构建时自动调用
