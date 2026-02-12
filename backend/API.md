# StaticFlow Backend API 文档

## 基础信息

- Base URL（本地开发）: `http://localhost:3000/api`
- Base URL（生产示例，直连 pb-mapper）: `https://<cloud-host>:8888/api`
- Base URL（生产示例，可选云端 Nginx）: `https://api.yourdomain.com/api`
- 协议: HTTP/1.1
- 数据格式: JSON（图片接口返回二进制）
- 请求追踪: backend 会透传/生成 `x-request-id` 与 `x-trace-id`，并在 backend/shared 请求内日志输出同一组 ID
- 查询路径日志: shared 层会输出 `Query path selected` / `Query completed`，字段包含：
  - `query`（逻辑查询名）
  - `path`（当前实际路径，如 `vector_index`、`vector_scan`、`scan_fallback`）
  - `fastest_path`（理论最快路径）
  - `is_fastest`（当前是否走到最快路径）
  - `reason`（为何走该路径，例如缺索引、回退原因）
  - `rows` / `elapsed_ms`（返回行数与耗时）

## CORS 说明

- `RUST_ENV=development`：允许所有来源（便于本地开发）
- `RUST_ENV=production`：默认仅允许 `https://acking-you.github.io`
- `ALLOWED_ORIGINS`：可选，逗号分隔来源白名单，覆盖默认生产来源

示例：

```env
RUST_ENV=production
ALLOWED_ORIGINS=https://acking-you.github.io,https://your-frontend-domain.com
```

---

## API 列表

### 1) 获取文章列表

`GET /api/articles`

查询参数：
- `tag`（可选）按标签过滤（大小写不敏感）
- `category`（可选）按分类过滤（大小写不敏感）

示例：

```bash
curl "http://localhost:3000/api/articles?tag=rust&category=Web"
```

### 2) 获取文章详情

`GET /api/articles/:id`

示例：

```bash
curl http://localhost:3000/api/articles/post-001
```

### 3) 获取相关文章（向量）

`GET /api/articles/:id/related`

示例：

```bash
curl http://localhost:3000/api/articles/post-001/related
```

### 4) 标签与分类

- `GET /api/tags`
- `GET /api/categories`

### 5) 关键词搜索

`GET /api/search?q=关键词`

查询参数：
- `limit`（可选）返回结果上限；不传则不限制，尽可能返回全部召回结果

实现说明：
- 优先使用 LanceDB FTS（BM25）
- 若 FTS 查询失败或返回空结果，自动回退到扫描匹配（保证可用性）

示例：

```bash
curl "http://localhost:3000/api/search?q=rust"
curl "http://localhost:3000/api/search?q=rust&limit=50"
```

### 6) 语义搜索

`GET /api/semantic-search?q=关键词[&enhanced_highlight=true]`

参数：
- `enhanced_highlight`（可选，默认 `false`）：是否启用高精度 highlight 片段重排（更准确但更慢）
- `limit`（可选）：返回结果上限；不传则不限制，尽可能返回全部召回结果
- `max_distance`（可选）：向量距离上界，作用于返回结果中的 `_distance` 字段；越小越严格，不传则不过滤距离

实现说明：
- 默认按 query 语言选择向量列（英文→`vector_en`，中文→`vector_zh`）
- 若主向量列无结果，会自动回退到另一语言向量列再检索一次（例如 `vector_en` 为空时，英文 query 会回退 `vector_zh`）
- `highlight` 为“语义片段”：从正文中分块候选，按语义相似度（余弦）+ 词面重叠加权，选最佳片段
- 若最佳片段存在词面命中，会做 `<mark>` 标注；否则返回最相关语义片段（而非随机摘要）
- 语义检索会记录 `semantic_search.highlight` 阶段耗时；当 `enhanced_highlight=false` 时走 `fast_excerpt`，当 `true` 时走 `semantic_snippet_rerank`

示例：

```bash
curl "http://localhost:3000/api/semantic-search?q=异步编程"
curl "http://localhost:3000/api/semantic-search?q=web"
curl "http://localhost:3000/api/semantic-search?q=web&enhanced_highlight=true"
curl "http://localhost:3000/api/semantic-search?q=web&limit=50"
curl "http://localhost:3000/api/semantic-search?q=web&limit=50&max_distance=0.8"
```

#### `max_distance` 参数原理与示例

作用机制（语义搜索 / 以图搜图一致）：
1. 先把 query 转成向量，执行 `nearest_to(...)` 找最近邻候选。
2. 若传了 `max_distance`，会在 LanceDB 侧应用 `distance_range(None, max_distance)`，即仅保留 `_distance <= max_distance` 的结果。
3. 最后再按 `limit` 截断返回数量。

理解重点：
- `max_distance` 控制“质量门槛”（相似度阈值），`limit` 控制“最多返回多少条”。
- 当语料较集中、阈值较宽松时，即使设置了 `max_distance` 也可能召回很多结果；这属于正常现象。
- 距离数值的尺度取决于索引的距离类型（`distance_type`），不同库/模型间不能直接照搬阈值。

可复现实验（示例）：
1. 先不设阈值：`/api/semantic-search?q=datafusion&limit=200`，观察结果条数和 `_distance` 分布。
2. 设宽松阈值：`/api/semantic-search?q=datafusion&limit=200&max_distance=1.2`，通常条数会减少。
3. 设严格阈值：`/api/semantic-search?q=datafusion&limit=200&max_distance=0.8`，通常条数进一步减少，相关性更高。

如果要查看当前索引的距离类型，可执行：

```bash
./bin/sf-cli db --db-path ./data/lancedb list-indexes articles --with-stats
./bin/sf-cli db --db-path ./data/lancedb list-indexes images --with-stats
```

输出中的 `distance=...` 就是该索引使用的距离度量类型。

### 7) 图片列表

`GET /api/images`

示例：

```bash
curl http://localhost:3000/api/images
```

### 8) 图片读取（从 LanceDB）

`GET /api/images/:id-or-filename`

- 支持通过 `id`（sha256）或 `filename` 查询
- 可选参数 `thumb=true` 读取缩略图

示例：

```bash
curl "http://localhost:3000/api/images/1a31f145e050ecfdd6f6ec2a4dbf4f31f67187f65fcd4f95f5f6c68ca68cfb7b" --output image.bin
curl "http://localhost:3000/api/images/wallhaven-5yyyw9.png?thumb=true" --output thumb.png
```

缩略图实现细节：
- `thumb=true` 时优先返回 `images.thumbnail`，若该字段为空会自动回退 `images.data`。
- `images.thumbnail` 由 CLI 写入时生成（`write-images --generate-thumbnail` 或 `sync-notes --generate-thumbnail`），并统一编码为 PNG。
- 缩略图尺寸由 CLI 参数 `--thumbnail-size` 控制，默认 `256`。
- 当前 `Content-Type` 按 `filename` 后缀推断，因此某些情况下（如原图 jpg 且返回 thumbnail）响应头与字节实际编码可能不一致。

### 9) 以图搜图

`GET /api/image-search?id=<image_id>`

查询参数：
- `limit`（可选）返回结果上限；不传则不限制，尽可能返回全部召回结果
- `max_distance`（可选）向量距离上界，作用于 `_distance` 字段；越小越严格，不传则不过滤距离（见上文“`max_distance` 参数原理与示例”）

示例：

```bash
curl "http://localhost:3000/api/image-search?id=1a31f145e050ecfdd6f6ec2a4dbf4f31f67187f65fcd4f95f5f6c68ca68cfb7b"
curl "http://localhost:3000/api/image-search?id=1a31f145e050ecfdd6f6ec2a4dbf4f31f67187f65fcd4f95f5f6c68ca68cfb7b&limit=24"
curl "http://localhost:3000/api/image-search?id=1a31f145e050ecfdd6f6ec2a4dbf4f31f67187f65fcd4f95f5f6c68ca68cfb7b&limit=24&max_distance=0.8"
```

### 10) 文搜图（Text-to-Image）

`GET /api/image-search-text?q=关键词`

查询参数：
- `limit`（可选）返回结果上限；不传则不限制，尽可能返回全部召回结果
- `max_distance`（可选）向量距离上界，作用于 `_distance` 字段；越小越严格，不传则不过滤距离（见上文“`max_distance` 参数原理与示例”）

实现说明：
- 文本 query 使用 CLIP 文本编码器生成向量，再在 `images.vector` 上执行最近邻检索。
- 为保证图文在同一向量空间，文搜图与图片向量写入使用同一 CLIP 语义空间。

示例：

```bash
curl "http://localhost:3000/api/image-search-text?q=rust mascot"
curl "http://localhost:3000/api/image-search-text?q=database architecture&limit=24"
curl "http://localhost:3000/api/image-search-text?q=clickhouse execution pipeline&limit=24&max_distance=0.8"
```

---

## 错误响应格式

```json
{
  "error": "Error message",
  "code": 500
}
```

---

## 存储模型

后端已基于 LanceDB 运行，不再读取 `content/images` 静态目录。

- `articles` 表：文章元数据、正文、文本向量
- `images` 表：图片二进制、缩略图、视觉向量

图片内容由 API 从 `images.data`（或 `images.thumbnail`）读取并返回。`thumb=true` 时优先 `thumbnail`，为空则回退 `data`。

SVG 写入说明：
- `images.data` 仍保存原始 SVG 字节（原格式不变）。
- 写入时若检测到 SVG，会先光栅化为 PNG 作为 embedding 输入，再写入 `images.vector`（用于向量检索）。

---

## 后端运行

```bash
make bin-all

# 开发环境
LANCEDB_URI=../data/lancedb PORT=3000 ./target/release/static-flow-backend

# 生产环境示例
RUST_ENV=production \
BIND_ADDR=127.0.0.1 \
PORT=9999 \
LANCEDB_URI=/opt/staticflow/data/lancedb \
ALLOWED_ORIGINS=https://acking-you.github.io \
./target/release/static-flow-backend
```

---

## 常见问题

### Q1: 为什么前端图片显示不了？

检查：
1. 文章内图片链接是否是 `images/<image_id>`
2. `images` 表是否有对应记录
3. 前端 `STATICFLOW_API_BASE` 是否指向正确 endpoint（直连 pb-mapper 或云端 Nginx）

### Q2: 如何把本地笔记目录导入？

使用 CLI：

```bash
./target/release/sf-cli sync-notes --db-path ./data/lancedb --dir ./content --recursive --generate-thumbnail
```

默认会自动执行 index-only optimize，把新写入数据纳入索引覆盖。
如使用了 `--no-auto-optimize`（批量场景），请在批次末尾手动执行：

```bash
./target/release/sf-cli db --db-path ./data/lancedb ensure-indexes
./target/release/sf-cli db --db-path ./data/lancedb optimize articles
./target/release/sf-cli db --db-path ./data/lancedb optimize images
```

若需要立刻清理旧版本并回收空间，可直接一键执行：

```bash
./target/release/sf-cli db --db-path ./data/lancedb optimize images --all --prune-now
```

批量处理三张核心表：

```bash
for t in articles images taxonomies; do
  ./target/release/sf-cli db --db-path ./data/lancedb optimize "$t" --all --prune-now
done
```

### Q3: 是否仍需把图片放到后端静态目录？

不需要。当前实现支持图片二进制直接写入 LanceDB，再通过 `/api/images/:id-or-filename` 读取。

### Q3.1: 分类描述来自哪里？

`/api/categories` 的 `description` 来自 `taxonomies` 表（`kind=category`）。
可通过 `sf-cli write-article --category-description ...` 或 `sync-notes`（frontmatter）写入。

### Q3.2: 如何保证文章日期与原文一致？

`write-article` 现已支持 `--date YYYY-MM-DD`：

```bash
./target/release/sf-cli write-article --db-path ./data/lancedb --file ./post.md --date 2026-02-10 ...
```

日期优先级为：`--date` > frontmatter `date` > 当天日期。

### Q4: 如何不用启动 backend，直接调试同款 API 逻辑？

可以使用 `sf-cli api` 子命令（和 backend API 共用同一套 LanceDB 访问代码）：

```bash
./target/release/sf-cli api --db-path ./data/lancedb list-articles --category Tech
./target/release/sf-cli api --db-path ./data/lancedb get-article frontend-architecture
./target/release/sf-cli api --db-path ./data/lancedb search --q "staticflow"
./target/release/sf-cli api --db-path ./data/lancedb semantic-search --q "前端 架构"
./target/release/sf-cli api --db-path ./data/lancedb related-articles frontend-architecture
./target/release/sf-cli api --db-path ./data/lancedb list-tags
./target/release/sf-cli api --db-path ./data/lancedb list-categories
./target/release/sf-cli api --db-path ./data/lancedb list-images
./target/release/sf-cli api --db-path ./data/lancedb search-images --id <image_id>
./target/release/sf-cli api --db-path ./data/lancedb get-image <image_id_or_filename> --thumb --out ./tmp-thumb.bin
```
