# StaticFlow - 全栈 Rust 博客系统

一个基于 **Rust + WebAssembly** 的现代化博客系统，使用 Yew 前端框架和 Meilisearch 全文搜索引擎。本项目旨在探索 WASM 技术栈和 Meilisearch 搜索能力。

## 📋 项目概述

StaticFlow 是从静态博客演进而来的动态博客系统，**全栈使用 Rust 编写**：
- **Frontend**: Yew (WebAssembly) - 编译为 WASM 在浏览器运行
- **Backend**: Axum + Meilisearch + SQLite - 高性能后端服务
- **CLI Tool**: Rust CLI - 本地内容管理工具

## ✨ 核心特性

- 🦀 **全栈 Rust**: 前后端共享代码，类型安全
- ⚡ **WebAssembly**: 接近原生的性能体验
- 🔍 **Meilisearch**: 快速、相关性高的全文搜索
- 📝 **Markdown 支持**: 实时渲染，样式可定制
- 🎨 **响应式设计**: 移动端适配
- 🔐 **安全设计**: 无需账号系统，基于签名认证
- 🎵 **可扩展**: 支持音乐播放器等扩展功能

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
Meilisearch 1.5+ (全文搜索)
SQLite (元数据存储)
tower-governor (限流)
serde (序列化)
tokio (异步运行时)
```

### CLI Tool
```
Rust 1.75+
clap (命令行解析)
notify (文件监控)
reqwest (HTTP 客户端)
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
│   │   │   └── resources.rs
│   │   ├── models/          # 数据模型
│   │   ├── services/        # 业务逻辑
│   │   │   ├── meilisearch.rs
│   │   │   └── markdown.rs
│   │   ├── middleware/      # 中间件
│   │   │   ├── auth.rs
│   │   │   └── rate_limit.rs
│   │   └── main.rs
│   ├── Cargo.toml
│   └── Dockerfile
│
├── cli-tool/              # 本地管理工具
│   ├── src/
│   │   ├── watcher.rs       # 文件监控
│   │   ├── processor.rs     # 内容处理
│   │   ├── sync.rs          # 同步逻辑
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
- Meilisearch 1.5+

### 1. 安装 Meilisearch

```bash
# macOS
brew install meilisearch

# 或使用 Docker
docker run -d \
  --name meilisearch \
  -p 7700:7700 \
  -v $(pwd)/data/meili_data:/meili_data \
  getmeili/meilisearch:v1.5
```

### 2. 启动后端服务

```bash
cd backend

# 创建配置文件
cp .env.example .env

# 初始化数据库
cargo run --bin init-db

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

### 4. 使用 CLI 工具同步内容

```bash
cd cli-tool

# 构建工具
cargo build --release

# 初始化配置
./target/release/static-flow-cli init

# 同步 Markdown 文件
./target/release/static-flow-cli sync ~/my-blog-posts

# 监控目录（实时同步）
./target/release/static-flow-cli watch ~/my-blog-posts
```

## 📝 配置说明

### Backend `.env`
```env
# 服务配置
SERVER_HOST=0.0.0.0
SERVER_PORT=3000

# Meilisearch 配置
MEILISEARCH_URL=http://localhost:7700
MEILISEARCH_KEY=master_key_here

# 数据库
DATABASE_URL=sqlite://data/blog.db

# 内容目录
CONTENT_DIR=./content
IMAGE_DIR=./content/images

# 限流配置
RATE_LIMIT_PER_MINUTE=60
```

### CLI Tool `config.toml`
```toml
[watch]
content_dir = "/path/to/markdown/files"
image_dir = "/path/to/images"

[backend]
api_url = "http://localhost:3000/api"
# 后续添加认证 token
```

## 🗺️ 开发路线图

### ✅ Week 1: 前端界面复刻（Day 1-7）

**核心目标：使用 Yew + Rust 完全复刻旧博客的界面和样式，所有数据使用 Mock**

**Day 1-2: Yew 项目初始化 + 基础布局**
- [ ] 创建 Yew 项目并配置 Trunk
- [ ] 分析旧博客界面结构（参考 `old/` 目录）
  - 顶部导航栏（桌面端 + 移动端）
  - 首页布局（头像、标题、副标题、社交链接）
  - 底部 Footer
- [ ] 复刻 Header 组件
  - Logo/标题区域
  - 导航菜单（文章、标签、分类）
  - 搜索框
  - 主题切换按钮
- [ ] 复刻响应式导航（移动端汉堡菜单）

**Day 3-4: 首页和文章列表页**
- [ ] 复刻首页布局
  - 个人简介区域（头像、标题、TypeIt 打字效果）
  - 社交链接图标
- [ ] 复刻文章列表卡片
  - 特色图片
  - 文章标题、摘要
  - 发布日期、作者、分类
  - 标签显示
- [ ] 实现分页组件
- [ ] Mock 文章列表数据（10-20 篇）

**Day 5-6: 文章详情页和样式迁移**
- [ ] 复刻文章详情页布局
  - 文章头部信息
  - Markdown 内容渲染（使用 pulldown-cmark）
  - 代码高亮样式
  - 图片展示
- [ ] 迁移 CSS 样式
  - 提取 `old/css/` 中的关键样式
  - 适配到 Yew 组件
  - 暗色主题支持
- [ ] Mock 文章详情数据（3-5 篇完整文章）

**Day 7: 搜索、标签、分类页面**
- [ ] 实现搜索结果展示页面（Mock 数据）
- [ ] 实现标签列表页
- [ ] 实现分类列表页
- [ ] 路由配置和页面跳转
- [ ] 整体样式微调和优化

**里程碑检查点：**
- ✅ 界面和旧博客视觉上高度一致
- ✅ 响应式设计在移动端和桌面端表现良好
- ✅ 所有页面使用 Mock 数据正常展示
- ✅ 暗色/明亮主题切换正常

### 🚧 Week 2: 后端服务 + 数据流打通（Day 8-14）

**Day 8-9: 后端基础框架**
- [ ] Axum 项目初始化
- [ ] Meilisearch 集成和配置
- [ ] SQLite 数据库 schema 设计
- [ ] 核心 API 实现（使用测试数据）
  - `GET /api/articles` - 文章列表
  - `GET /api/articles/:id` - 文章详情
  - `GET /api/search` - 搜索
  - `GET /api/tags` - 标签列表
  - `GET /api/categories` - 分类列表

**Day 10-11: 前后端集成**
- [ ] 创建 `shared` crate（共享数据模型）
- [ ] 前端 API 客户端实现（gloo-net）
- [ ] 替换 Mock 数据为真实 API 调用
- [ ] CORS 配置
- [ ] 错误处理和 Loading 状态

**Day 12-13: CLI 工具（简化版）**
- [ ] 文件监控（notify）
- [ ] Markdown 文件解析
- [ ] 简单的元数据提取（标题、日期、标签）
- [ ] 同步到 Meilisearch 和 SQLite

**Day 14: 测试和优化**
- [ ] 端到端测试完整流程
- [ ] WASM 体积优化
- [ ] 性能优化
- [ ] Bug 修复

### 🔮 Future (Week 3+)

- [ ] 图片处理和 CDN
- [ ] 音乐播放器界面
- [ ] GitHub 评论集成
- [ ] 高级安全认证（签名验证）
- [ ] AI 内容生成（标题、摘要、标签）
- [ ] 性能监控和日志
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

#### 搜索文章
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
      "_formatted": {
        "title": "文章<em>标题</em>"
      }
    }
  ],
  "query": "keyword",
  "processing_time_ms": 2
}
```

### 资源相关

#### 获取图片
```http
GET /api/image/:base64_filename
```

## 🎯 学习目标

### WebAssembly
- [x] Rust 编译到 WASM
- [ ] Yew 组件化开发
- [ ] WASM 与 JavaScript 互操作
- [ ] WASM 性能优化和体积优化

### Meilisearch
- [x] 基础索引和搜索
- [ ] Faceted search（标签筛选）
- [ ] 相关性调优
- [ ] 实时索引更新

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

### Meilisearch 调试

```bash
# 查看索引信息
curl http://localhost:7700/indexes

# 查看文档数量
curl http://localhost:7700/indexes/articles/stats

# 手动搜索测试
curl "http://localhost:7700/indexes/articles/search?q=rust"
```

## 🚢 部署指南

### Docker Compose 部署

```yaml
# docker-compose.yml
version: '3.8'
services:
  meilisearch:
    image: getmeili/meilisearch:v1.5
    ports:
      - "7700:7700"
    volumes:
      - ./data/meili_data:/meili_data
    environment:
      - MEILI_MASTER_KEY=your_master_key

  backend:
    build: ./backend
    ports:
      - "3000:3000"
    depends_on:
      - meilisearch
    environment:
      - MEILISEARCH_URL=http://meilisearch:7700

  frontend:
    build: ./frontend
    ports:
      - "8080:8080"
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

### Meilisearch
- [Meilisearch 官方文档](https://www.meilisearch.com/docs)
- [Rust SDK](https://github.com/meilisearch/meilisearch-rust)

### Axum
- [Axum 官方示例](https://github.com/tokio-rs/axum/tree/main/examples)

---

**当前状态**: 🚧 积极开发中（MVP 阶段）

**下一步**: 完成 Week 1 的 MVP 功能，实现前后端数据流打通
