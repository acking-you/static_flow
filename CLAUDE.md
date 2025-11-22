# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**StaticFlow** is a local-first, automation-driven blog system built entirely in Rust. It combines a Yew WebAssembly frontend, Axum backend API, LanceDB multimodal database, and an AI Agent tool for automated content synchronization from local Markdown files (Obsidian/Typora).

**Tech Stack:**
- Frontend: Yew 0.21 + WebAssembly (compiled via Trunk)
- Backend: Axum 0.7 + LanceDB (multimodal database)
- AI Agent: Rust + Rig framework + notify (file watching)
- Shared: Common data models between frontend/backend

**Current Status:** Frontend UI completed. Currently developing AI Agent (Phase 1).

**📘 For production deployment guide, see [DEPLOYMENT.md](./DEPLOYMENT.md)**

## Architecture

### Multi-Crate Workspace Structure
The project is designed as a Cargo workspace with 4 main crates:

1. **frontend/** - Yew WASM application
   - Components-based architecture (components/, pages/)
   - Client-side routing via yew-router
   - API calls via gloo-net
   - Markdown rendering via pulldown-cmark
   - TailwindCSS v4 for styling

2. **backend/** - Axum REST API server
   - API routes: articles, search (full-text + vector), images
   - Services: LanceDB integration, Markdown processing
   - Middleware: auth (signature-based), rate limiting
   - Data: LanceDB for all data (metadata + vectors + images)

3. **agent/** - AI Agent for local automation
   - File watcher for Markdown/image changes (notify)
   - AI metadata generation (Rig framework + LLM)
   - Image embedding generation (CLIP model)
   - Direct LanceDB writer (no backend dependency)

4. **shared/** - Shared types and utilities
   - Data models used by both frontend and backend
   - Ensures type safety across the stack

### Key Design Principles

**Local-First Content Creation:**
- Users write in Obsidian/Typora (local Markdown files)
- CLI tool watches directory, auto-syncs changes
- No online editor needed

**AI-Driven Automation:**
- Auto-generate article summaries via LLM (Rig framework)
- Auto-extract tags and categories via LLM
- Auto-generate image embeddings via CLIP model
- Configurable AI provider (OpenAI or local Ollama)

**Image Path Mapping:**
- Local images referenced in Markdown (e.g., `![](images/foo.png)`)
- Agent generates CLIP embeddings and stores full image binary in LanceDB
- Backend serves images via `/api/image/:id`
- Frontend transforms Markdown links to API endpoints
- Support image similarity search (text-to-image, image-to-image)

**Dynamic Serving:**
- Backend renders Markdown to HTML on-demand (not pre-built)
- LanceDB provides full-text search + vector search (semantic)
- API-driven architecture allows flexible frontend updates

## Development Commands

### Prerequisites Setup
```bash
# Install Rust toolchain
rustup install stable

# Add WASM target for frontend
rustup target add wasm32-unknown-unknown

# Install Trunk (WASM bundler)
cargo install trunk

# Optional: Python for CLIP embedding service
pip install lancedb pillow transformers torch
```

### Backend Development
```bash
cd backend

# Copy environment config
cp .env.example .env

# Run development server (will connect to LanceDB)
cargo run

# Run with release optimizations
cargo run --release
```

Backend runs on `http://localhost:3000`

### AI Agent Development
```bash
cd agent

# Copy config
cp config.example.toml config.toml

# Initialize LanceDB schema
cargo run -- init

# One-time sync
cargo run -- sync ~/my-blog-posts

# Watch mode (real-time sync)
cargo run --release -- watch ~/my-blog-posts
```

### Frontend Development
```bash
cd frontend

# Install TailwindCSS (if needed)
npm install -D tailwindcss

# Start dev server with hot reload
trunk serve

# Open browser automatically
trunk serve --open

# Custom port
trunk serve --port 8888

# Production build
trunk build --release
```

Frontend dev server runs on `http://localhost:8080`

### Tailwind CSS v4 Integration

### 跨平台管理（npm 方式）

项目使用 npm 管理 Tailwind CLI，自动支持 Linux/macOS/Windows：

**首次克隆后的安装**：
```bash
cd frontend
npm install      # 安装 Tailwind CLI（自动下载对应平台版本）
# 或使用 pnpm（推荐，更快）
pnpm install
```

**为什么使用 npm？**
- Tailwind CSS v4 官方分发方式
- 自动下载对应平台的二进制文件（linux-x64, macos-arm64, windows-x64 等）
- 版本锁定在 package.json，团队协作一致
- 无需提交大文件到 git（node_modules 已在 .gitignore）

**无 Node.js 环境的备选方案**：
如果需要在无 Node.js 的环境（如 CI/嵌入式设备）使用，可手动下载独立二进制：
https://github.com/tailwindlabs/tailwindcss/releases/latest

下载后放在 frontend/ 目录并修改 Trunk.toml 的 hook：
```toml
[[hooks]]
stage = "pre_build"
command = "./tailwindcss"  # 改回直接调用
command_arguments = ["-i", "input.css", "-o", "static/styles.css", "--minify"]
```
StaticFlow 的前端已经集成 **Tailwind CSS v4.1.17**，通过 npm 管理 Tailwind CLI，由 Trunk 钩子在构建前自动调用。

**混合使用策略**
- 现有的视觉系统依旧全部维护在 `input.css` 的 `@layer components` 区块，确保历史样式不被破坏。
- 新功能或结构简单的组件可以直接在 Yew 模板里写 Tailwind utility classes，加速迭代。
- `src/components/theme_toggle.rs`、`footer.rs` 与 `article_card.rs` 展示了如何在保留组件样式基础上，通过 utility classes 叠加动态状态。

**使用指南**
- `input.css` 结构遵循 `@import "tailwindcss"; → @theme {…} → @layer components {…}`，其中 `@theme` 定义设计令牌，`@layer components` 存放遗留/复杂样式。
- `Trunk.toml` 中的 `pre_build` hook 会在 `trunk build/serve` 前执行 `./tailwindcss -i input.css -o static/styles.css --minify`，因此无需单独运行 npm。
- 在 Rust 组件里使用 Tailwind 时，应通过 `classes!` 宏传入：`classes!("flex", "items-center", conditional_class)`。每个类名必须是单独的字符串参数，避免 `"flex items-center"` 这种写法，以保证 v4 的按需提取准确。
- 可将 `classes!` 返回值直接绑定到 `classes` 属性，或与 `attr!`/`html!` 组合，示例：`html! { <div class={classes!("bg-[var(--surface)]", "rounded-xl", extra)}>...</div> }`。

**常用命令**：
```bash
# 开发模式（带热重载）
trunk serve --open

# 生产构建（Trunk 自动调用 npx）
trunk build --release

# 手动编译 Tailwind（可选）
npm run tailwind         # 单次编译
npm run tailwind:watch   # 监听模式
```

**设计令牌**
- `@theme` 中声明了 `--color-*`、`--spacing-*`、`--shadow`、`--radius` 等 CSS 变量，可在组件样式和 Tailwind utility 中复用。
- 在 Tailwind v4 中可直接书写 `bg-[var(--bg)]`、`text-[var(--text)]`、`shadow-[var(--shadow)]`、`rounded-[var(--radius)]` 等形式，把变量注入到类名里，实现与 `input.css` 设计令牌的一致性。

### CLI Tool
```bash
cd agent

# Build agent
cargo build --release

# Initialize configuration
./target/release/static-flow-agent init

# One-time sync of Markdown directory
./target/release/static-flow-agent sync ~/my-blog-posts

# Watch directory for real-time sync
./target/release/static-flow-agent watch ~/my-blog-posts
```

### Workspace Commands
```bash
# Build all crates
cargo build --workspace

# Run tests across all crates
cargo test --workspace

# Check without building
cargo check --workspace

# Format all code
cargo fmt --all

# Lint all code
cargo clippy --workspace -- -D warnings
```

### WASM Optimization
For production frontend builds, optimize WASM size:
```bash
# Already configured in profile.release:
# opt-level = "z", lto = true, codegen-units = 1, panic = "abort"

# Further optimize with wasm-opt (install via binaryen)
wasm-opt -Oz -o dist/optimized.wasm dist/output.wasm
```

### Meilisearch Debugging
```bash
# LanceDB Python debugging
python
>>> import lancedb
>>> db = lancedb.connect("./data/lancedb")
>>> articles = db.open_table("articles")
>>> articles.count_rows()
>>> articles.head(5)

# Test vector search
>>> query_vector = [0.1] * 512  # dummy vector
>>> results = articles.search(query_vector).limit(10).to_list()
```

## API Design

### RESTful Endpoints
```
GET  /api/articles              # List articles (pagination, filters)
GET  /api/articles/:id          # Get article detail (rendered HTML)
GET  /api/search?q=keyword      # Full-text search (LanceDB FTS)
GET  /api/semantic-search?q=text # Semantic search (vector)
GET  /api/tags                  # List all tags
GET  /api/categories            # List all categories
GET  /api/image/:id             # Serve image by ID
POST /api/image/search          # Image similarity search
```

### Data Models (Shared)
Key types to define in `shared/src/models.rs`:
- `Article` - id, title, content, summary, tags, category, vector (embedding), timestamps
- `ArticleListItem` - lightweight version for list views
- `Image` - id, filename, image_data, thumbnail, vector (CLIP embedding), metadata
- `Tag` - name, count
- `Category` - name, count
- `SearchResult` - article/image hit with similarity scores

## Development Workflow

### Phase 1: AI Agent Core (Current Priority)
1. Design LanceDB schemas (articles + images tables)
2. Integrate Rig framework for LLM agent
3. Implement Markdown file processing pipeline
4. Implement image processing pipeline (CLIP embeddings)
5. LanceDB batch writer
6. File watcher for real-time sync

### Phase 2: Backend Refactor (LanceDB Integration)
1. Remove Meilisearch/SQLite dependencies
2. Integrate LanceDB Rust SDK
3. Refactor API endpoints to query LanceDB
4. Implement full-text search (LanceDB FTS)
5. Implement vector search (semantic + image similarity)

### Phase 3: Frontend Extensions (Multimodal Search)
1. Add semantic search UI
2. Add image-to-image search UI
3. Optimize search result display

### Phase 4: Security & Deployment (Future)
1. Request signature authentication
2. Rate limiting (tower-governor)
3. Docker Compose setup
4. Rathole tunnel for local deployment
5. Nginx reverse proxy

## Important Notes

### Image Handling Strategy
The image handling is integrated into LanceDB:
1. Agent scans image files, generates CLIP embeddings
2. Stores both original image binary + thumbnail + vector in LanceDB
3. Backend serves images via `/api/image/:id` route
4. Frontend Markdown renderer transforms relative paths to API URLs
5. Support multimodal search (text-to-image, image-to-image)

Example transformation:
```markdown
<!-- In local Markdown -->
![screenshot](images/screenshot.png)

<!-- Rendered in frontend -->
<img src="http://localhost:3000/api/image/img_12345">
```

### Workspace Dependencies
When adding dependencies, use workspace-level version management in root `Cargo.toml`:
```toml
[workspace]
members = ["frontend", "backend", "cli-tool", "shared"]

[workspace.dependencies]
serde = { version = "1.0", features = ["derive"] }
tokio = { version = "1.35", features = ["full"] }
```

Then reference in crate `Cargo.toml`:
```toml
[dependencies]
serde = { workspace = true }
```

### Testing Strategy
- Unit tests for `shared` models and utilities
- Integration tests for backend API routes
- Mock API responses for frontend component tests
- E2E test: local file → CLI sync → backend → frontend display

### Configuration Files
- Backend: `.env` (LanceDB path, server config)
- Agent: `config.toml` (watch paths, LanceDB path, AI settings, CLIP service URL)
- Frontend: `Trunk.toml` (build settings, asset copying)

## Migration from Old Blog
The `old/` directory is reserved for static assets from the previous blog system. During Week 1 frontend development:
1. Extract CSS styles from `old/css/`
2. Identify UI components to replicate
3. Migrate theme colors and design tokens
4. Preserve responsive breakpoints

## Debugging Tips

**Frontend (WASM):**
- Use browser DevTools console for panics
- Enable `console_error_panic_hook` for better stack traces
- Check Network tab for API call failures

**Backend:**
- Enable `RUST_LOG=debug` for verbose logging
- Use `tokio-console` for async task inspection
- Test LanceDB queries via Python REPL

**Agent:**
- Test file watcher with: `touch test.md` in watched directory
- Verify LLM API connectivity: `curl https://api.openai.com/v1/models`
- Check LanceDB writes via Python: `db.open_table("articles").count_rows()`

## Performance Considerations

**WASM Bundle Size:**
- Target <500KB gzipped for initial load
- Use `wasm-opt` in CI/CD pipeline
- Consider code splitting for large apps

**LanceDB Performance:**
- Use columnar storage advantages for metadata queries
- Batch vector searches when possible
- Configure ANN index parameters (IVF-PQ for large datasets)
- Monitor query latency with LanceDB built-in stats

**Backend:**
- Use connection pooling for LanceDB clients
- Cache rendered Markdown (optional, measure first)
- Implement ETag headers for static resources

## Future Extensions (Post-MVP)
- RSS feed generation
- GitHub-based commenting system
- Music player component (mentioned in README)
- Article view counters
- Reading time estimation
- Related articles recommendation
- Multi-language support
