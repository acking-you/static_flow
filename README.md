# StaticFlow - Local-First Dynamic Blog System

> A local-first, automation-driven blog system built with full-stack Rust. Write in Obsidian, auto-sync with AI, serve dynamically - bridging the gap between static simplicity and dynamic flexibility.

**StaticFlow** 是一个以**本地创作为中心、自动化驱动**的现代博客系统。你可以继续使用 Obsidian 等工具在本地文件夹写 Markdown，本地 AI Agent 自动检测文件变化，通过 LLM 生成摘要和标签，实时同步到 LanceDB 多模态数据库，最终通过 API 暴露给 WASM 前端 - 所有这些都基于全栈 Rust 实现。

**StaticFlow** is a local-first, automation-driven blog system built entirely in Rust. Continue writing in your local folder with Obsidian, let the local AI agent auto-detect changes, generate summaries and tags via LLM, sync to LanceDB multimodal database in real-time, and serve everything through APIs to a WASM frontend - all powered by the Rust ecosystem.

## 📋 核心理念

**写作自由 + 自动化 + 现代技术栈**

传统博客要么是静态生成（每次修改需要重新构建），要么需要在线编辑器（失去本地工具的便利）。StaticFlow 采用第三条路：

1. **本地优先创作**:
   - 使用任何你喜欢的编辑器（Obsidian、Typora、VSCode）
   - Markdown + 本地图片，完全掌控你的内容
   - 无需任何在线操作

2. **智能自动化**:
   - 本地 AI Agent 监控文件夹变化（基于 Rig 框架）
   - LLM 自动生成文章摘要、标签、分类
   - CLIP 模型自动生成图片向量 embedding
   - 实时同步到 LanceDB 多模态数据库

3. **动态服务**:
   - Axum 后端提供 RESTful API
   - Yew WASM 前端提供极致性能
   - LanceDB 提供全文搜索 + 向量搜索 + 图片存储
   - 支持内网穿透，随处访问

## ✨ 核心特性

### 📝 本地创作体验
- ✍️ 使用 Obsidian/Typora 等工具在本地书写
- 🖼️ 图片直接放在本地文件夹
- 📁 基于文件路径的自动索引
- 🔄 文件变化自动检测和同步

### 🤖 AI 驱动自动化
- 🏷️ 自动生成文章标签（基于 LLM）
- 📊 自动生成文章分类（基于 LLM）
- 📄 自动生成文章摘要（基于 LLM）
- 🖼️ 自动生成图片向量 embedding（基于 CLIP）
- 🎯 支持本地（Ollama）或云端（OpenAI）AI 服务

### 🚀 现代技术栈
- 🦀 **全栈 Rust**: 前后端共享代码，类型安全
- ⚡ **WebAssembly**: 接近原生的浏览器性能
- 🔍 **LanceDB**: 多模态数据库（全文 + 向量 + 图片存储）
- 🤖 **Rig Framework**: Rust 原生 AI Agent 框架
- 🎨 **响应式设计**: 移动端和桌面端完美适配
- 🔐 **安全设计**: 基于签名认证，无需账号系统

## 🏗️ 技术栈

### Frontend (Yew WASM)
```
Rust 1.75+
Yew 0.21 (前端框架)
Trunk (构建工具)
yew-router (路由)
gloo-net (HTTP 客户端)
pulldown-cmark (Markdown 渲染)
TailwindCSS (样式)
```

### Backend
```
Rust 1.75+
Axum 0.7 (Web 框架)
LanceDB (多模态数据库)
tower-governor (限流)
serde (序列化)
tokio (异步运行时)
```

### AI Agent
```
Rust 1.75+
Rig (AI Agent 框架)
LanceDB (数据存储)
notify (文件监控)
clap (命令行解析)
reqwest (LLM API 调用)
```

## 📦 项目结构

```
static-flow/
├── frontend/              # Yew WASM 前端
│   ├── src/
│   │   ├── components/      # 可复用组件
│   │   ├── pages/           # 页面组件
│   │   ├── api/             # API 调用封装
│   │   ├── router.rs        # 路由配置
│   │   ├── models.rs        # 数据模型
│   │   └── main.rs
│   ├── static/              # 静态资源
│   ├── index.html
│   ├── Cargo.toml
│   └── Trunk.toml
│
├── backend/               # Axum 后端
│   ├── src/
│   │   ├── api/             # API 路由
│   │   │   ├── articles.rs
│   │   │   ├── search.rs
│   │   │   └── images.rs
│   │   ├── models/          # 数据模型
│   │   ├── services/        # 业务逻辑
│   │   │   ├── lancedb.rs   # LanceDB 客户端
│   │   │   └── markdown.rs
│   │   ├── middleware/      # 中间件
│   │   │   ├── auth.rs
│   │   │   └── rate_limit.rs
│   │   └── main.rs
│   ├── Cargo.toml
│   └── Dockerfile
│
├── agent/                 # AI Agent（本地自动化）
│   ├── src/
│   │   ├── watcher.rs       # 文件监控
│   │   ├── processor.rs     # Markdown 解析
│   │   ├── ai_generator.rs  # LLM 元数据生成
│   │   ├── image_encoder.rs # 图片 embedding（CLIP）
│   │   ├── lancedb_writer.rs # LanceDB 写入
│   │   └── main.rs
│   └── Cargo.toml
│
├── shared/                # 前后端共享代码
│   ├── src/
│   │   ├── models.rs        # 共享数据模型
│   │   └── utils.rs
│   └── Cargo.toml
│
└── README.md
```

## 🚀 快速开始

### 前置要求
- Rust 1.75+ (`rustup install stable`)
- wasm32 target (`rustup target add wasm32-unknown-unknown`)
- Trunk (`cargo install trunk`)
- Python 3.10+（可选，用于运行 CLIP embedding 服务）

### 1. 初始化 LanceDB 数据库

```bash
cd agent

# 创建配置文件
cp config.example.toml config.toml

# 初始化 LanceDB schema
cargo run -- init

# 扫描并同步现有 Markdown 文件
cargo run -- sync ~/my-blog-posts

# 启动文件监控（实时同步）
cargo run --release -- watch ~/my-blog-posts
```

### 2. 启动后端服务

```bash
cd backend

# 创建配置文件
cp .env.example .env

# 启动服务
cargo run --release
```

后端将在 `http://localhost:3000` 运行

### 3. 启动前端（开发模式）

```bash
cd frontend

# 安装 TailwindCSS（如果使用）
npm install -D tailwindcss

# 启动开发服务器
trunk serve --open
```

前端将在 `http://localhost:8080` 运行，支持热重载

## 📝 配置说明

### Backend `.env`
```env
# 服务配置
SERVER_HOST=0.0.0.0
SERVER_PORT=3000

# LanceDB 配置
LANCEDB_PATH=./data/lancedb

# 限流配置
RATE_LIMIT_PER_MINUTE=60
```

### AI Agent `config.toml`
```toml
[watch]
# 本地 Markdown 文件目录（如 Obsidian vault）
content_dir = "/Users/yourname/Documents/MyBlog"
image_dir = "/Users/yourname/Documents/MyBlog/images"

[lancedb]
# LanceDB 数据库路径
db_path = "./data/lancedb"

[ai]
# LLM 服务配置（用于生成摘要、标签、分类）
provider = "openai"  # 或 "ollama"
api_key = "sk-xxx"
model = "gpt-4o-mini"

# 图片 embedding 服务（CLIP）
clip_service_url = "http://localhost:8000/embed"  # 可选，使用本地 Python 服务
```

## 🗺️ 开发路线图

### ✅ Week 1: 前端界面复刻（Day 1-7） - 已完成

前端 UI 已基本完成，使用 Yew + TailwindCSS v4 复刻旧博客界面。

### 🚧 Phase 1: AI Agent 核心开发（优先级最高）

**目标：构建基于 Rig + LanceDB 的本地自动化 Agent**

- [ ] **LanceDB Schema 设计**
  - 定义 Article 表 schema（id, title, content, summary, tags, category, vector, timestamps）
  - 定义 Image 表 schema（id, filename, image_data, thumbnail, vector, metadata）
  - 创建全文索引和向量索引

- [ ] **Rig 框架集成**
  - 初始化 Rig Agent 项目结构
  - 配置 LLM provider（OpenAI/Ollama）
  - 实现基础 prompt 模板（摘要、标签、分类生成）

- [ ] **Markdown 处理流水线**
  - 文件扫描和监控（notify crate）
  - Frontmatter 解析（yaml-rust）
  - Markdown 正文提取
  - 图片链接提取和解析

- [ ] **AI 元数据生成**
  - LLM 调用：根据文章内容生成摘要（100-200 字）
  - LLM 调用：提取 3-5 个关键标签
  - LLM 调用：判断文章分类（Tech/Life/Thoughts 等）
  - 生成文本 embedding（用于语义搜索）

- [ ] **图片处理流水线**
  - 图片文件扫描（支持 jpg/png/webp）
  - 缩略图生成（200x200）
  - 调用 CLIP 模型生成 embedding（512 维向量）
  - 图片二进制存储优化

- [ ] **LanceDB 写入逻辑**
  - 批量插入 Article 记录
  - 批量插入 Image 记录
  - 增量更新机制（检测文件修改时间）
  - 删除处理（文件被删除时同步到 DB）

- [ ] **配置管理**
  - config.toml 解析（watch 路径、AI API key、LanceDB 路径）
  - 环境变量支持
  - 日志系统（tracing + tracing-subscriber）

**里程碑检查点**：
- ✅ 本地 Markdown + 图片 → LanceDB 完整流程打通
- ✅ AI 自动生成摘要、标签、分类
- ✅ 图片 embedding 正确生成并存储
- ✅ 文件变化能实时同步到 LanceDB

### 🔮 Phase 2: Backend 重构（LanceDB 集成）

**目标：Backend 从"处理层"变为"查询层"**

- [ ] **移除旧依赖**
  - 移除 Meilisearch 相关代码
  - 移除 SQLite 相关代码
  - 清理废弃的 API endpoint

- [ ] **LanceDB Rust SDK 集成**
  - 添加 lancedb crate 依赖
  - 实现 LanceDB 连接池
  - 封装查询接口

- [ ] **API Endpoints 重构**
  - `GET /api/articles` - 从 LanceDB 分页查询（支持 tag/category 过滤）
  - `GET /api/articles/:id` - 根据 ID 精确查询
  - `GET /api/search?q=keyword` - 全文搜索（LanceDB FTS）
  - `GET /api/semantic-search?q=text` - 语义搜索（向量搜索）
  - `GET /api/image/:id` - 返回图片二进制
  - `GET /api/image/search?q=text` - 文本搜图（CLIP 向量搜索）
  - `POST /api/image/search` - 以图搜图（上传图片查询相似图）
  - `GET /api/tags` - 标签列表
  - `GET /api/categories` - 分类列表

- [ ] **Markdown 渲染**
  - 保留后端实时渲染 Markdown to HTML
  - 图片链接转换（相对路径 → `/api/image/:id`）
  - 代码高亮（syntect）

- [ ] **性能优化**
  - 结果缓存（moka）
  - 分页查询优化
  - 图片响应 ETag 支持

**里程碑检查点**：
- ✅ 所有 API 从 LanceDB 正确读取数据
- ✅ 全文搜索和向量搜索正常工作
- ✅ 图片服务性能良好（<100ms 响应时间）

### 🎨 Phase 3: Frontend 功能扩展

**目标：支持多模态搜索功能**

- [ ] **语义搜索界面**
  - 添加"智能搜索"模式切换（关键词 vs 语义）
  - 语义搜索结果展示（相似度得分）

- [ ] **以图搜图功能**
  - 图片上传组件
  - 相似图片结果展示（缩略图 + 相似度）
  - 点击查看原图

- [ ] **搜索结果优化**
  - 混合搜索（全文 + 向量结合）
  - 搜索历史记录
  - 热门搜索词

**里程碑检查点**：
- ✅ 用户可以通过语义搜索找到相关文章
- ✅ 用户可以通过上传图片搜索相似内容
- ✅ 搜索体验流畅

### 🔮 Phase 4: 高级功能（Future）

- [ ] 相关文章推荐（基于向量相似度）
- [ ] 文章聚类和主题分析
- [ ] RSS feed 生成
- [ ] 阅读统计和热力图
- [ ] GitHub 评论集成
- [ ] Docker Compose 一键部署
- [ ] Rathole 内网穿透配置

## 📚 API 文档

### 文章相关

#### 获取文章列表
```http
GET /api/articles?page=1&limit=20&tag=rust&category=tech
```

响应：
```json
{
  "articles": [
    {
      "id": "article-slug",
      "title": "文章标题",
      "summary": "文章摘要",
      "tags": ["rust", "wasm"],
      "category": "tech",
      "created_at": "2024-01-01T00:00:00Z",
      "updated_at": "2024-01-02T00:00:00Z"
    }
  ],
  "total": 100,
  "page": 1,
  "limit": 20
}
```

#### 获取文章详情
```http
GET /api/articles/:id
```

响应：
```json
{
  "id": "article-slug",
  "title": "文章标题",
  "content_html": "<h1>渲染后的 HTML</h1>...",
  "tags": ["rust", "wasm"],
  "category": "tech",
  "created_at": "2024-01-01T00:00:00Z"
}
```

#### 搜索文章（全文搜索）
```http
GET /api/search?q=keyword&limit=10
```

响应：
```json
{
  "hits": [
    {
      "id": "article-slug",
      "title": "文章标题",
      "summary": "匹配的摘要内容...",
      "tags": ["rust"],
      "score": 0.95
    }
  ],
  "query": "keyword",
  "total": 42
}
```

#### 语义搜索文章
```http
GET /api/semantic-search?q=Rust编程最佳实践&limit=10
```

响应：
```json
{
  "hits": [
    {
      "id": "article-slug",
      "title": "文章标题",
      "summary": "文章摘要",
      "similarity": 0.87
    }
  ],
  "query": "Rust编程最佳实践",
  "total": 15
}
```

### 图片相关

#### 获取图片
```http
GET /api/image/:id
```

#### 文本搜图
```http
GET /api/image/search?q=sunset&limit=10
```

响应：
```json
{
  "images": [
    {
      "id": "img-001",
      "filename": "sunset.jpg",
      "thumbnail_url": "/api/image/img-001?size=thumbnail",
      "similarity": 0.92
    }
  ]
}
```

#### 以图搜图
```http
POST /api/image/search
Content-Type: multipart/form-data

{
  "image": <binary>
}
```

## 🎯 学习目标

### WebAssembly
- [x] Rust 编译到 WASM
- [ ] Yew 组件化开发
- [ ] WASM 与 JavaScript 互操作
- [ ] WASM 性能优化和体积优化

### LanceDB & Rig
- [ ] LanceDB 多模态存储（向量 + 全文 + 二进制）
- [ ] Rig 框架构建 AI Agent
- [ ] CLIP 图片 embedding 生成
- [ ] 向量相似度搜索优化

### Rust 全栈
- [ ] 前后端代码共享
- [ ] 异步编程（tokio）
- [ ] 错误处理最佳实践

## 🛠️ 开发技巧

### Yew 开发

```bash
# 开发模式（热重载）
trunk serve

# 生产构建
trunk build --release

# 指定端口
trunk serve --port 8888
```

### WASM 优化

```toml
# Cargo.toml
[profile.release]
opt-level = "z"     # 优化体积
lto = true          # Link Time Optimization
codegen-units = 1   # 更好的优化
panic = "abort"     # 减小体积
```

```bash
# 使用 wasm-opt 进一步优化
wasm-opt -Oz -o output_optimized.wasm output.wasm
```

### LanceDB 调试

```bash
# Python 交互式查询（需安装 lancedb）
python
>>> import lancedb
>>> db = lancedb.connect("./data/lancedb")
>>> table = db.open_table("articles")
>>> table.count_rows()
>>> table.head(5)

# 测试向量搜索
>>> results = table.search([0.1] * 512).limit(10).to_list()
```

## 🚢 部署指南

### Docker Compose 部署

```yaml
# docker-compose.yml
version: '3.8'
services:
  backend:
    build: ./backend
    ports:
      - "3000:3000"
    volumes:
      - ./data/lancedb:/app/data/lancedb  # 挂载 LanceDB 数据
    environment:
      - LANCEDB_PATH=/app/data/lancedb

  frontend:
    build: ./frontend
    ports:
      - "8080:8080"

  # 可选：CLIP embedding 服务（Python）
  clip-service:
    image: your-clip-service:latest
    ports:
      - "8000:8000"
```

```bash
docker-compose up -d
```

### Rathole 内网穿透（本地部署）

```toml
# rathole.toml (客户端)
[client]
remote_addr = "your-vps-ip:2333"

[client.services.blog]
local_addr = "127.0.0.1:3000"
token = "your_secret_token"
```

## 🤝 贡献指南

欢迎提交 Issue 和 Pull Request！

## 📄 开源协议

MIT License

## 📚 学习资源

### Yew / WASM
- [Yew 官方文档](https://yew.rs/)
- [Rust and WebAssembly Book](https://rustwasm.github.io/docs/book/)
- [Trunk 文档](https://trunkrs.dev/)

### LanceDB & Rig
- [LanceDB 官方文档](https://lancedb.github.io/lancedb/)
- [LanceDB Rust SDK](https://github.com/lancedb/lancedb/tree/main/rust)
- [Rig 框架文档](https://github.com/0xPlaygrounds/rig)
- [CLIP 模型介绍](https://openai.com/research/clip)

### Axum
- [Axum 官方示例](https://github.com/tokio-rs/axum/tree/main/examples)

---

**当前状态**: 🚧 积极开发中

**架构状态**:
- ✅ Frontend 基础 UI（Yew + TailwindCSS v4）
- 🚧 AI Agent 开发中（Rig + LanceDB）
- ⏳ Backend 待重构（LanceDB 集成）

**下一步**: 完成 Phase 1 - AI Agent 核心开发，实现 Markdown/图片 → LanceDB 的完整流程
