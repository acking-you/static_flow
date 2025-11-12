# StaticFlow - Local-First Dynamic Blog System

> A local-first, automation-driven blog system built with full-stack Rust. Write in Obsidian, auto-sync with AI, serve dynamically - bridging the gap between static simplicity and dynamic flexibility.

**StaticFlow** 是一个以**本地创作为中心、自动化驱动**的现代博客系统。你可以继续使用 Obsidian 等工具在本地文件夹写 Markdown，本地服务自动检测文件变化，通过 AI 生成摘要和标签，实时同步到搜索引擎和数据库，最终通过 API 暴露给 WASM 前端 - 所有这些都基于全栈 Rust 实现。

**StaticFlow** is a local-first, automation-driven blog system built entirely in Rust. Continue writing in your local folder with Obsidian, let the local service auto-detect changes, generate summaries and tags via LLM, sync to search engine and database in real-time, and serve everything through APIs to a WASM frontend - all powered by the Rust ecosystem.

## 📋 核心理念

**写作自由 + 自动化 + 现代技术栈**

传统博客要么是静态生成（每次修改需要重新构建），要么需要在线编辑器（失去本地工具的便利）。StaticFlow 采用第三条路：

1. **本地优先创作**:
   - 使用任何你喜欢的编辑器（Obsidian、Typora、VSCode）
   - Markdown + 本地图片，完全掌控你的内容
   - 无需任何在线操作

2. **智能自动化**:
   - 本地 CLI 工具监控文件夹变化
   - AI 自动生成文章摘要、标签、分类
   - 图片路径自动映射和转换
   - 实时同步到 Meilisearch 搜索引擎

3. **动态服务**:
   - Axum 后端提供 RESTful API
   - Yew WASM 前端提供极致性能
   - Meilisearch 提供毫秒级全文搜索
   - 支持内网穿透，随处访问

## ✨ 核心特性

### 📝 本地创作体验
- ✍️ 使用 Obsidian/Typora 等工具在本地书写
- 🖼️ 图片直接放在本地文件夹
- 📁 基于文件路径的自动索引
- 🔄 文件变化自动检测和同步

### 🤖 AI 驱动自动化
- 🏷️ 自动生成文章标签
- 📊 自动生成文章分类
- 📄 自动生成文章摘要
- 🎯 基于 LLM（本地或云端）

### 🚀 现代技术栈
- 🦀 **全栈 Rust**: 前后端共享代码，类型安全
- ⚡ **WebAssembly**: 接近原生的浏览器性能
- 🔍 **Meilisearch**: 快速、相关性高的全文搜索
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
# 本地 Markdown 文件目录（如 Obsidian vault）
content_dir = "/Users/yourname/Documents/MyBlog"
image_dir = "/Users/yourname/Documents/MyBlog/images"

[backend]
api_url = "http://localhost:3000/api"
# 后续添加认证 token

[ai]
# AI 服务配置（用于生成摘要、标签、分类）
provider = "openai"  # 或 "local" (ollama)
api_key = "sk-xxx"
model = "gpt-4o-mini"
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

### 🚧 Week 2: 后端服务 + 本地工具基础（Day 8-14）

**Day 8-9: 后端基础框架**
- [ ] Axum 项目初始化
- [ ] Meilisearch 集成和配置
  - 创建 articles 索引
  - 配置搜索字段和排序
- [ ] SQLite 数据库 schema 设计
  - articles 表（id, title, content_path, summary, created_at, updated_at）
  - tags 表
  - categories 表
  - images 表（filename, file_path）
- [ ] 核心 API 实现（使用测试数据）
  - `GET /api/articles` - 文章列表
  - `GET /api/articles/:id` - 文章详情（实时 Markdown 渲染）
  - `GET /api/search?q=keyword` - 搜索
  - `GET /api/tags` - 标签列表
  - `GET /api/categories` - 分类列表
  - `GET /api/image/:base64_filename` - 图片服务

**Day 10-11: 前后端集成**
- [ ] 创建 `shared` crate（共享数据模型）
- [ ] 前端 API 客户端实现（gloo-net）
- [ ] 替换 Mock 数据为真实 API 调用
- [ ] CORS 配置
- [ ] 错误处理和 Loading 状态
- [ ] Markdown 图片链接转换测试
  - 本地相对路径 → HTTP API 路径

**Day 12-13: CLI 工具核心功能**
- [ ] CLI 项目初始化（clap 配置）
- [ ] 文件监控实现（notify crate）
  - 监控 `.md` 文件的创建、修改、删除
  - 监控图片文件的变化
- [ ] Markdown 文件解析
  - 提取 frontmatter（如果有）
  - 基于文件路径生成文章 ID
  - 提取图片引用
- [ ] 图片路径映射
  - 建立 filename → full_path 映射
  - 存储到后端数据库
- [ ] 基础同步到后端
  - 调用后端 API 添加/更新文章
  - 同步到 Meilisearch

**Day 14: 测试完整流程**
- [ ] 端到端测试
  1. 在本地文件夹创建 Markdown 文件
  2. CLI 工具检测并同步
  3. 前端刷新后能看到新文章
  4. 搜索功能正常工作
- [ ] Bug 修复和优化

**里程碑检查点：**
- ✅ 本地文件 → 数据库 → 前端显示的完整流程打通
- ✅ Meilisearch 搜索功能正常
- ✅ 图片链接转换正确
- ✅ 文件变化能实时同步

### 🔮 Week 3+: AI 自动化和高级功能

**AI 内容生成**
- [ ] 集成 LLM API（OpenAI / 本地 Ollama）
- [ ] 实现自动摘要生成
  - 分析文章内容
  - 生成 2-3 句的摘要
- [ ] 实现自动标签生成
  - 基于文章内容提取关键词
  - 生成 3-5 个相关标签
- [ ] 实现自动分类
  - 基于内容判断文章类型
  - 分配到合适的分类

**安全和部署**
- [ ] 请求签名机制
  - 前端公钥加密
  - 后端私钥验证
- [ ] IP + 设备指纹限流
- [ ] 图片处理和优化
  - 缩略图生成
  - 图片压缩
- [ ] Docker Compose 部署
- [ ] Rathole 内网穿透配置
- [ ] Nginx 反向代理

**扩展功能**
- [ ] 音乐播放器界面
- [ ] GitHub 评论集成
- [ ] RSS 订阅支持
- [ ] 文章统计（阅读量、字数）

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
