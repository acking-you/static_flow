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

### 2.1) 记录文章浏览（自动计数）

`POST /api/articles/:id/view`

用途：
- 前端进入文章详情页时调用，用于浏览计数。
- 后端默认 **60 秒去重**：同一文章 + 同一客户端指纹（IP + User-Agent 哈希）在同一窗口只记 1 次。
- 去重窗口支持运行时配置（见下文 Admin 接口）。
- 统计分桶按 **Asia/Shanghai (UTC+8)** 生成 `day/hour` 维度。
- 响应中的 `counted` 表示本次是否计入新增浏览（`false` 代表命中去重窗口）。

示例：

```bash
curl -X POST "http://localhost:3000/api/articles/post-001/view"
```

响应示例：

```json
{
  "article_id": "post-001",
  "counted": true,
  "total_views": 128,
  "timezone": "Asia/Shanghai",
  "today_views": 12,
  "daily_points": [
    { "key": "2026-02-14", "views": 5 },
    { "key": "2026-02-15", "views": 7 }
  ],
  "server_time_ms": 1760112233445
}
```

返回字段说明：
- `counted`：本次请求是否新增了一条浏览事件
- `total_views`：当前文章累计浏览次数（去重后）
- `today_views`：按 `Asia/Shanghai` 当天累计浏览次数
- `daily_points`：最近一段时间（默认 30 天，上限由 `trend_max_days` 配置）的按天趋势点
- `server_time_ms`：后端记录这次请求时的 Unix 毫秒时间戳

### 2.2) 获取文章浏览趋势

`GET /api/articles/:id/view-trend`

查询参数：
- `granularity`（可选）`day` 或 `hour`，默认 `day`
- `days`（可选）仅 `day` 模式有效，默认值来自运行时配置 `trend_default_days`
- `days` 上限来自运行时配置 `trend_max_days`（默认 `180`）
- `day`（可选）仅 `hour` 模式必填，格式 `YYYY-MM-DD`

示例：

```bash
curl "http://localhost:3000/api/articles/post-001/view-trend"
curl "http://localhost:3000/api/articles/post-001/view-trend?granularity=day&days=14"
curl "http://localhost:3000/api/articles/post-001/view-trend?granularity=hour&day=2026-02-15"
```

说明：
- `day` 模式返回日趋势点（key 为 `YYYY-MM-DD`）。
- `hour` 模式返回 24 小时趋势点（key 为 `00..23`）。
- 趋势返回中的 `total_views` 始终是文章全量累计值，不受 `days/day` 参数影响。

`day` 模式响应示例：

```json
{
  "article_id": "post-001",
  "timezone": "Asia/Shanghai",
  "granularity": "day",
  "day": null,
  "total_views": 128,
  "points": [
    { "key": "2026-02-14", "views": 5 },
    { "key": "2026-02-15", "views": 7 }
  ]
}
```

`hour` 模式响应示例：

```json
{
  "article_id": "post-001",
  "timezone": "Asia/Shanghai",
  "granularity": "hour",
  "day": "2026-02-15",
  "total_views": 128,
  "points": [
    { "key": "00", "views": 0 },
    { "key": "01", "views": 1 },
    { "key": "02", "views": 0 }
  ]
}
```

### 2.3) Admin：浏览统计运行时配置（本地）

> 该接口不在 `/api` 路径下，建议仅通过本地/内网访问，不对公网开放。

`GET /admin/view-analytics-config`

示例：

```bash
curl "http://127.0.0.1:3000/admin/view-analytics-config"
```

响应示例：

```json
{
  "dedupe_window_seconds": 60,
  "trend_default_days": 30,
  "trend_max_days": 180
}
```

`POST /admin/view-analytics-config`

请求体（字段均可选，部分更新）：

```json
{
  "dedupe_window_seconds": 60,
  "trend_default_days": 30,
  "trend_max_days": 180
}
```

参数约束：
- `dedupe_window_seconds`: `1..3600`
- `trend_default_days`: `1..365`
- `trend_max_days`: `1..365`
- 且 `trend_default_days <= trend_max_days`

示例：

```bash
curl -X POST "http://127.0.0.1:3000/admin/view-analytics-config" \
  -H "Content-Type: application/json" \
  -d '{"dedupe_window_seconds":120,"trend_default_days":14,"trend_max_days":180}'
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
- 纯英文 query 会优先使用 `vector_en` 检索
- 非纯英文 query 按语言检测选择主向量列（中文通常走 `vector_zh`）
- 若主向量列无结果，会自动回退到另一语言向量列再检索一次（例如英文 query 在 `vector_en` 0 召回时回退 `vector_zh`）
- 当启用 `hybrid=true` 时，混合检索中的向量召回复用同一套“主列 + 0 召回回退”逻辑，再与词法检索融合
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
- `article_views` 表：文章浏览事件（含去重键、按天/小时分桶字段；默认 60s 去重窗口，可运行时配置）

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

若需要立刻清理未引用/孤儿文件并回收空间，可直接一键执行：

```bash
./target/release/sf-cli db --db-path ./data/lancedb cleanup-orphans --table images
```

批量处理全部清理目标表（`articles`、`images`、`taxonomies`、`article_views`；若 `article_views` 尚未创建会自动跳过）：

```bash
./target/release/sf-cli db --db-path ./data/lancedb cleanup-orphans
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
