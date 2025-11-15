# StaticFlow 后端 API 文档

## 基础信息

- **Base URL**: `http://localhost:3000/api`
- **协议**: HTTP/1.1
- **数据格式**: JSON
- **字符编码**: UTF-8
- **CORS**: 已启用，允许所有来源

---

## 接口列表

### 1. 获取文章列表

获取所有文章的列表信息（不含正文内容）。

**请求**

```http
GET /api/articles
```

**查询参数**

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| tag | string | 否 | 按标签过滤（大小写不敏感） |
| category | string | 否 | 按分类过滤（大小写不敏感） |

**请求示例**

```bash
# 获取所有文章
curl http://localhost:3000/api/articles

# 按标签过滤
curl "http://localhost:3000/api/articles?tag=rust"

# 按分类过滤
curl "http://localhost:3000/api/articles?category=Web"

# 组合过滤
curl "http://localhost:3000/api/articles?tag=wasm&category=Rust"
```

**响应示例**

```json
{
  "articles": [
    {
      "id": "post-012",
      "title": "示例文章 12 - Web 技术与思考",
      "summary": "这是一篇关于 Web 的示例文章，涵盖实践要点与思考。",
      "tags": ["frontend", "html", "css"],
      "category": "Web",
      "author": "Alice",
      "date": "2024-12-22",
      "featured_image": "/static/hero-3.jpg",
      "read_time": 7
    }
  ],
  "total": 15
}
```

**字段说明**

| 字段 | 类型 | 说明 |
|------|------|------|
| articles | array | 文章列表 |
| articles[].id | string | 文章唯一标识符 |
| articles[].title | string | 文章标题 |
| articles[].summary | string | 文章摘要 |
| articles[].tags | array<string> | 标签列表 |
| articles[].category | string | 分类名称 |
| articles[].author | string | 作者名称 |
| articles[].date | string | 发布日期（YYYY-MM-DD） |
| articles[].featured_image | string\|null | 特色图片URL |
| articles[].read_time | integer | 预计阅读时长（分钟） |
| total | integer | 文章总数 |

---

### 2. 获取文章详情

获取指定文章的完整内容（包含Markdown正文）。

**请求**

```http
GET /api/articles/:id
```

**路径参数**

| 参数 | 类型 | 说明 |
|------|------|------|
| id | string | 文章ID（如：post-001） |

**请求示例**

```bash
curl http://localhost:3000/api/articles/post-001
```

**响应示例（成功）**

```json
{
  "id": "post-001",
  "title": "示例文章 1 - Rust 技术与思考",
  "summary": "这是一篇关于 Rust 的示例文章，涵盖实践要点与思考。",
  "content": "# 用 Rust + Yew 构建本地优先博客\n\nStaticFlow 是...",
  "tags": ["rust", "wasm", "yew"],
  "category": "Rust",
  "author": "Bob",
  "date": "2024-01-11",
  "featured_image": "/static/hero-1.jpg",
  "read_time": 4
}
```

**响应示例（文章不存在）**

```json
{
  "error": "Article not found",
  "code": 404
}
```

**HTTP状态码**

- `200 OK` - 成功获取文章
- `404 Not Found` - 文章不存在
- `500 Internal Server Error` - 服务器内部错误

**字段说明**

在文章列表的基础上，增加：

| 字段 | 类型 | 说明 |
|------|------|------|
| content | string | 文章完整Markdown内容 |

---

### 3. 获取标签列表

获取所有标签及其文章数量统计。

**请求**

```http
GET /api/tags
```

**请求示例**

```bash
curl http://localhost:3000/api/tags
```

**响应示例**

```json
{
  "tags": [
    {
      "name": "rust",
      "count": 3
    },
    {
      "name": "wasm",
      "count": 3
    },
    {
      "name": "frontend",
      "count": 3
    }
  ]
}
```

**字段说明**

| 字段 | 类型 | 说明 |
|------|------|------|
| tags | array | 标签列表（按名称字母排序） |
| tags[].name | string | 标签名称 |
| tags[].count | integer | 使用该标签的文章数量 |

---

### 4. 获取分类列表

获取所有分类及其文章数量、描述信息。

**请求**

```http
GET /api/categories
```

**请求示例**

```bash
curl http://localhost:3000/api/categories
```

**响应示例**

```json
{
  "categories": [
    {
      "name": "Rust",
      "count": 3,
      "description": "静态类型、零成本抽象与 Wasm 生态的实战笔记。"
    },
    {
      "name": "Web",
      "count": 3,
      "description": "现代前端工程化与体验设计相关内容。"
    },
    {
      "name": "DevOps",
      "count": 3,
      "description": "自动化、流水线与交付体验的工程思考。"
    },
    {
      "name": "Productivity",
      "count": 3,
      "description": "效率、写作与自我管理的小实验与道具。"
    },
    {
      "name": "AI",
      "count": 3,
      "description": "Prompt、LLM 与智能体的落地探索。"
    }
  ]
}
```

**字段说明**

| 字段 | 类型 | 说明 |
|------|------|------|
| categories | array | 分类列表（按名称字母排序） |
| categories[].name | string | 分类名称 |
| categories[].count | integer | 该分类下的文章数量 |
| categories[].description | string | 分类描述文字 |

---

## 错误响应格式

所有错误响应遵循统一格式：

```json
{
  "error": "错误描述信息",
  "code": 404
}
```

**字段说明**

| 字段 | 类型 | 说明 |
|------|------|------|
| error | string | 人类可读的错误描述 |
| code | integer | HTTP状态码 |

**常见错误码**

| 状态码 | 说明 |
|--------|------|
| 400 Bad Request | 请求参数错误 |
| 404 Not Found | 资源不存在 |
| 500 Internal Server Error | 服务器内部错误 |

---

## 数据存储

**当前阶段（MVP）**：
- 存储方式：文件系统（Markdown文件）
- 数据目录：`../content/`（相对于backend目录）
- 文件格式：`.md`文件，包含YAML frontmatter

**示例文件结构**：

```markdown
---
title: "文章标题"
summary: "文章摘要"
tags: ["tag1", "tag2"]
category: "分类名"
author: "作者名"
date: "2024-01-01"
featured_image: "/static/image.jpg"
read_time: 5
---

# 文章正文

Markdown内容...
```

---

## 技术栈

- **Web框架**: Axum 0.7
- **异步运行时**: Tokio 1.48
- **序列化**: serde + serde_json
- **Markdown解析**: gray_matter 0.2（YAML frontmatter）
- **CORS**: tower-http 0.5

---

## 开发环境

### 启动后端服务器

```bash
# 使用Makefile（推荐）
make dev-backend

# 或直接使用cargo
cd backend
cargo run
```

### 配置文件

后端配置文件：`backend/.env`

```env
# Content directory (relative to backend/ or absolute path)
CONTENT_DIR=../content

# Server port
PORT=3000

# Log level (trace, debug, info, warn, error)
RUST_LOG=info
```

### 健康检查

```bash
# 检查服务器是否启动
curl http://localhost:3000/api/articles | jq '.total'
```

---

## 未来扩展（Week 2+）

以下功能计划在后续版本实现：

### 1. 全文搜索

```http
GET /api/search?q=关键词
```

**计划使用**: Meilisearch 1.5+

### 2. 图片服务

```http
GET /api/image/:base64_filename
```

**说明**: 根据base64编码的文件名返回图片二进制数据

### 3. 数据库持久化

- SQLite：存储文章元数据
- Meilisearch：全文搜索索引

### 4. 文章管理

```http
POST /api/articles        # 创建文章
PUT /api/articles/:id     # 更新文章
DELETE /api/articles/:id  # 删除文章
```

**认证**: 签名认证（计划实现）

---

## 常见问题

### Q: 如何添加新文章？

A: 在`content/`目录下创建新的`.md`文件，包含YAML frontmatter，重启后端即可。

### Q: 如何修改分类描述？

A: 编辑`backend/src/handlers.rs`中的`CATEGORY_DESCRIPTIONS`常量。

### Q: 支持分页吗？

A: 当前版本不支持，所有文章一次性返回。计划在Week 2实现。

### Q: 日期格式必须是YYYY-MM-DD吗？

A: 是的，前端严格依赖该格式进行解析和分组显示。

---

**文档版本**: v1.0
**最后更新**: 2025-11-15
**维护者**: StaticFlow Team
