# StaticFlow Backend API 文档

## 基础信息

- Base URL（本地开发）: `http://localhost:3000/api`
- Base URL（生产示例，直连 pb-mapper）: `https://<cloud-host>:8888/api`
- Base URL（生产示例，可选云端 Nginx）: `https://api.yourdomain.com/api`
- 协议: HTTP/1.1
- 数据格式: JSON（图片接口返回二进制）

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

示例：

```bash
curl "http://localhost:3000/api/search?q=rust"
```

### 6) 语义搜索

`GET /api/semantic-search?q=关键词`

示例：

```bash
curl "http://localhost:3000/api/semantic-search?q=异步编程"
```

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

示例：

```bash
curl "http://localhost:3000/api/image-search?id=1a31f145e050ecfdd6f6ec2a4dbf4f31f67187f65fcd4f95f5f6c68ca68cfb7b"
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

### Q3: 是否仍需把图片放到后端静态目录？

不需要。当前实现支持图片二进制直接写入 LanceDB，再通过 `/api/images/:id-or-filename` 读取。

### Q3.1: 分类描述来自哪里？

`/api/categories` 的 `description` 来自 `taxonomies` 表（`kind=category`）。
可通过 `sf-cli write-article --category-description ...` 或 `sync-notes`（frontmatter）写入。

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
