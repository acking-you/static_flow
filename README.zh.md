# StaticFlow

[CLI 使用手册](./docs/cli-user-guide.zh.md)

本地优先的动态博客系统：后端在本地运行，通过本地 Nginx + pb-mapper 对外暴露 HTTPS API；文章与图片统一写入 LanceDB，由前端直接请求云端映射端点访问。

## 核心理念

> **"Don't build agents, build skills instead."**
> — [Anthropic Expert Talk](https://www.youtube.com/watch?v=CEvIs9y1uog)

本项目不开发独立 AI Agent。

策略：
- **智能能力**：交给 Claude Code/Codex + skills
- **工程工具**：CLI 只做 LanceDB 读写

## 项目结构

```text
static-flow/
├── frontend/     # Yew WASM 前端
├── backend/      # Axum 后端（LanceDB 查询层）
├── shared/       # 共享类型
├── cli/          # LanceDB CLI 工具
└── content/      # 示例 Markdown 与图片
```

## 推荐部署拓扑

1. 本地运行 `backend`（如 `127.0.0.1:3000`）。
2. 本地 Nginx 前置 backend，提供本地 HTTPS（如 `127.0.0.1:3443`）。
3. 通过 `pb-mapper` 把本地 `127.0.0.1:3443` 映射到云端可访问端点（如 `https://<cloud-host>:8888`）。
4. 前端页面加载后，直接请求该云端 HTTPS 端点作为 API。
5. 可选：云端再加 Nginx（443 + 证书）统一域名，再反代到 pb-mapper local 端口。

主链路（按前端请求视角）：

```text
Frontend(fetch/XHR)
  -> https://<cloud-host>:8888/api
  -> pb-mapper tunnel
  -> Local Nginx https://127.0.0.1:3443
  -> Local backend http://127.0.0.1:3000
```

参考配置：
- 本地 Nginx HTTPS：`deployment-examples/nginx-staticflow-api.conf`
- 云端 Nginx HTTPS（可选）：`deployment-examples/nginx-staticflow-cloud-proxy.conf`

## 快速开始

```bash
# 前置依赖
rustup target add wasm32-unknown-unknown
cargo install trunk

# 编译二进制
make bin-all

# 初始化 LanceDB
cd cli
../target/release/sf-cli init --db-path ../data/lancedb

# 启动后端
cd ../backend
LANCEDB_URI=../data/lancedb ../target/release/static-flow-backend

# 启动前端（另一个终端）
cd ../frontend
trunk serve --open
```

后端: `http://localhost:3000` | 前端: `http://localhost:8080`

## CLI 命令

```bash
cd cli

# 编译 CLI 二进制
make bin-cli

# 一键跑完整 CLI 回归测试（docs + images + CRUD + API）
cd ..
./scripts/test_cli_e2e.sh
# 或：BUILD_PROFILE=release ./scripts/test_cli_e2e.sh
cd cli

# 初始化 LanceDB 表结构
../target/release/sf-cli init --db-path ../data/lancedb

# 手动重跑所有应建索引（适合批量导入后）
# - articles.content（全文索引）
# - articles.vector_en / articles.vector_zh（向量索引）
# - images.vector（向量索引）
# - taxonomies 表用于分类/标签元数据（无向量索引）
../target/release/sf-cli ensure-indexes --db-path ../data/lancedb

# write-article / write-images / sync-notes 默认会自动执行 index-only optimize
# 用于把新写入数据纳入索引覆盖；批量流水线可通过 --no-auto-optimize 关闭

# 写入单篇文章
../target/release/sf-cli write-article \
  --db-path ../data/lancedb \
  --file ../content/post-001.md \
  --summary "文章摘要" \
  --tags "rust,wasm" \
  --category "Tech" \
  --category-description "Engineering notes about Rust + WASM"

# 也可写在 markdown frontmatter 中
# category_description: "Rust 与 WASM 的工程实践"

# 批量写入图片
../target/release/sf-cli write-images \
  --db-path ../data/lancedb \
  --dir ../content/images \
  --recursive \
  --generate-thumbnail

# 缩略图实现细节
# - 仅在 --generate-thumbnail 时生成，尺寸由 --thumbnail-size 控制（默认 256）
# - 缩略图统一存为 PNG 二进制到 images.thumbnail
# - 读取 /api/images/:id-or-filename?thumb=true 时，thumbnail 为空会自动回退原图 data

# 同步本地笔记目录（markdown + 图片）
# - 自动把 markdown 中引用的本地图片写入 images 表
# - 自动把 markdown 图片链接改写为 images/<sha256_id>
# - 自动 upsert 文章到 articles 表
# - 自动 upsert 分类/标签元数据到 taxonomies 表
../target/release/sf-cli sync-notes \
  --db-path ../data/lancedb \
  --dir ../content \
  --recursive \
  --generate-thumbnail

# 查询验证
../target/release/sf-cli query --db-path ../data/lancedb --table articles --limit 10
../target/release/sf-cli query --db-path ../data/lancedb --table articles --limit 1 --format vertical

# 数据库风格管理（增删改查 + 索引）
../target/release/sf-cli db --db-path ../data/lancedb list-tables
../target/release/sf-cli db --db-path ../data/lancedb describe-table articles
../target/release/sf-cli db --db-path ../data/lancedb query-rows articles --where "category='Tech'" --columns id,title,date --limit 5
../target/release/sf-cli db --db-path ../data/lancedb query-rows articles --limit 1 --format vertical
../target/release/sf-cli db --db-path ../data/lancedb count-rows articles --where "vector_en IS NOT NULL"
../target/release/sf-cli db --db-path ../data/lancedb update-rows articles --set "category='Notes'" --where "id='post-001'"
../target/release/sf-cli db --db-path ../data/lancedb delete-rows articles --where "id='draft-001'"
../target/release/sf-cli db --db-path ../data/lancedb list-indexes articles --with-stats
../target/release/sf-cli db --db-path ../data/lancedb ensure-indexes
../target/release/sf-cli db --db-path ../data/lancedb optimize articles
../target/release/sf-cli db --db-path ../data/lancedb optimize images

# 核心表结构
# - articles：文章内容/元数据 + 向量
# - images：图片二进制 + 向量
# - taxonomies：分类/标签元数据（`kind`、`key`、`name`、`description`）

# 与 backend 同款 API 调试命令
../target/release/sf-cli api --db-path ../data/lancedb list-articles --category "Tech"
../target/release/sf-cli api --db-path ../data/lancedb get-article frontend-architecture
../target/release/sf-cli api --db-path ../data/lancedb search --q "staticflow"
../target/release/sf-cli api --db-path ../data/lancedb semantic-search --q "前端 架构"
../target/release/sf-cli api --db-path ../data/lancedb related-articles frontend-architecture
../target/release/sf-cli api --db-path ../data/lancedb list-tags
../target/release/sf-cli api --db-path ../data/lancedb list-categories
../target/release/sf-cli api --db-path ../data/lancedb list-images
../target/release/sf-cli api --db-path ../data/lancedb search-images --id <image_id>
../target/release/sf-cli api --db-path ../data/lancedb get-image <image_id_or_filename> --thumb --out ./tmp-thumb.bin
```

## API 列表

| 端点 | 说明 |
|------|------|
| `GET /api/articles` | 文章列表（支持 tag/category 过滤） |
| `GET /api/articles/:id` | 文章详情 |
| `GET /api/articles/:id/related` | 相关文章（向量相似） |
| `GET /api/search?q=` | 全文搜索 |
| `GET /api/semantic-search?q=` | 语义搜索（向量，含跨语言回退与语义片段高亮） |
| `GET /api/images` | 图片列表 |
| `GET /api/images/:id-or-filename` | 从 LanceDB 读取图片二进制（支持 `?thumb=true`，无缩略图则回退原图） |
| `GET /api/image-search?id=` | 以图搜图 |
| `GET /api/tags` | 标签列表 |
| `GET /api/categories` | 分类列表 |

> 可观测性：每个 backend 响应都会返回 `x-request-id` 与 `x-trace-id`，并且 backend/shared 的请求内日志会带同一组 ID，便于串联排障。

> 检索说明：若你更新了代码但仍看到“英文语义检索无结果”，请重新编译二进制（`cargo build --release -p sf-cli -p static-flow-backend`），旧二进制不会包含向量列回退逻辑。

## 关键环境变量

后端（`backend/.env`）：
- `LANCEDB_URI`（默认 `../data/lancedb`）
- `PORT`（默认 `3000`）
- `BIND_ADDR`（开发建议 `0.0.0.0`，生产建议 `127.0.0.1`）
- `RUST_ENV`（`development` 或 `production`）
- `ALLOWED_ORIGINS`（生产可选，逗号分隔 CORS 白名单）

前端构建时：
- `STATICFLOW_API_BASE`（直连 pb-mapper 端点，例如 `https://<cloud-host>:8888/api`）
- 若使用云端 Nginx 反代，可设为域名（如 `https://api.yourdomain.com/api`）

## 开发命令

```bash
# Workspace
cargo build --workspace
cargo test --workspace
cargo fmt --all
cargo clippy --workspace -- -D warnings

# Frontend
cd frontend && trunk serve
cd frontend && trunk build --release

# Backend
make bin-backend
cd backend && ../target/release/static-flow-backend
cd backend && RUST_ENV=production BIND_ADDR=127.0.0.1 ../target/release/static-flow-backend
```

## License

MIT
