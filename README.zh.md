# StaticFlow

本地优先的动态博客系统。在本地写 Markdown，通过 Claude Code/Codex skill 自动生成摘要和标签，CLI 工具写入 LanceDB，全栈 Rust 实现。

## 设计理念

> **"Don't build agents, build skills instead."**
> — [Anthropic Expert Talk](https://www.youtube.com/watch?v=CEvIs9y1uog)

本项目不开发独立的 AI Agent。

现实是：主流 agent 工具（Claude Code、Codex、Cursor）与其他方案的能力差距不是一星半点。即便在当前 agent 技术快速发展的阶段，各家产品的能力差异都如此悬殊——自己用 Rig/LangChain 造一个专有 agent，无论投入多少精力，都不可能追上主流工具的迭代速度。

正确的做法是：**Build Skills, Not Agents**。

本项目的 AI 自动化策略：
- **智能部分**：交给 Claude Code/Codex，通过 skill 描述工作流
- **工具部分**：只开发朴素的 CLI，负责 LanceDB 读写

好处：
1. 零 agent 开发维护成本
2. 主流 agent 升级时自动受益
3. CLI 保持简单，专注数据操作

## 架构

```
static-flow/
├── frontend/     # Yew WASM 前端
├── backend/      # Axum 后端（LanceDB 查询层）
├── shared/       # 共享类型
├── cli/          # LanceDB CLI 工具（朴素的读写操作）
└── content/      # 本地 Markdown 和图片
```

## 技术栈

| 模块 | 技术 |
|------|------|
| Frontend | Yew 0.21 + WASM + TailwindCSS v4 |
| Backend | Axum 0.7 + LanceDB |
| CLI | clap + LanceDB Rust SDK |
| Shared | serde + 共享数据模型 |

## 快速开始

```bash
# 前置要求
rustup target add wasm32-unknown-unknown
cargo install trunk

# 启动后端
cd backend && cargo run

# 启动前端（另一个终端）
cd frontend && trunk serve --open
```

后端: `http://localhost:3000` | 前端: `http://localhost:8080`

## CLI 工具

```bash
cd cli

# 初始化 LanceDB
cargo run -- init --db-path ./data/lancedb

# 写入文章（配合 Claude Code 生成 metadata）
cargo run -- write-article \
  --file ../content/post.md \
  --summary "文章摘要" \
  --tags "rust,wasm" \
  --category "Tech"

# 批量写入图片
cargo run -- write-images --dir ../content/images

# 查询验证
cargo run -- query --table articles --limit 10
```

## Claude Code 工作流

使用 Claude Code/Codex 处理新文章：

1. 读取 Markdown 文件内容
2. AI 生成摘要（100-200 字）、标签（3-5 个）、分类
3. 调用 CLI 工具写入 LanceDB

示例 prompt：
```
读取 content/new-post.md，生成摘要和标签，然后调用 sf-cli write-article 写入数据库
```

## API

| 端点 | 说明 |
|------|------|
| `GET /api/articles` | 文章列表（支持 tag/category 过滤） |
| `GET /api/articles/:id` | 文章详情 |
| `GET /api/search?q=` | 全文搜索 |
| `GET /api/semantic-search?q=` | 语义搜索（向量） |
| `GET /api/image/:id` | 图片服务 |
| `GET /api/tags` | 标签列表 |
| `GET /api/categories` | 分类列表 |

## 开发路线图

### Phase 1: CLI 工具开发
- [ ] LanceDB schema 设计和初始化
- [ ] write-article 命令实现
- [ ] write-images 命令实现
- [ ] query 命令实现

### Phase 2: Backend LanceDB 集成
- [ ] 移除文件系统实现
- [ ] 集成 LanceDB Rust SDK
- [ ] 重构所有 API 端点
- [ ] 实现向量搜索

### Phase 3: 功能完善
- [ ] 语义搜索 UI
- [ ] 以图搜图功能
- [ ] 相关文章推荐

## 开发命令

```bash
# Workspace 命令
cargo build --workspace      # 构建所有 crate
cargo test --workspace       # 运行测试
cargo fmt --all              # 格式化代码
cargo clippy --workspace     # Lint 检查

# 前端
cd frontend && trunk serve   # 开发模式
cd frontend && trunk build --release  # 生产构建

# 后端
cd backend && cargo run      # 开发模式
cd backend && cargo run --release     # 生产模式
```

## License

MIT
