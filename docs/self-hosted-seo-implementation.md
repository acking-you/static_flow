# 自托管前端 SEO 方案：Axum 动态 HTML 注入 + pb-mapper 公网映射

> 目标：让博客文章能被搜索引擎通过标题直接搜到，同时保留 GitHub Pages 作为备用。

## 1. 问题背景

### 1.1 现状

当前前端部署在 GitHub Pages (`acking-you.github.io`)，是纯 Yew/WASM SPA。
搜索引擎爬虫访问文章页 `/posts/{id}` 时：

1. GitHub Pages 找不到物理文件 → 返回 `404.html`（HTTP 404 状态码）
2. `404.html` 里的 JS 重定向到 `/`，WASM 加载后客户端路由到文章页
3. 爬虫看到的是 **404 + 空壳 HTML**，不会索引文章内容

主页能被搜到是因为 `index.html` 物理存在，返回 200 且包含 meta tags。

### 1.2 目标

- 爬虫访问 `/posts/{id}` 时拿到 **HTTP 200 + 完整 meta tags + 文章正文**
- 文章入库即刻生效，**零额外操作**
- 保留 GitHub Pages 不动，新增 `ackingliu.top` 自托管入口
- 前端本地更新，无需手动部署

## 2. 方案架构

```text
                    ┌─────────────────────────────────────────┐
                    │           Cloud Server (ackingliu.top)   │
                    │                                          │
  用户/爬虫 ──────▶ │  Caddy :443 (SSL, 已配置)                │
                    │       │                                  │
                    │       ▼                                  │
                    │  pb-mapper client (:某端口, 127.0.0.1)   │
                    └───────┬──────────────────────────────────┘
                            │ pb-mapper tunnel
                            ▼
                    ┌───────────────────────────────────────────┐
                    │           Local Machine                    │
                    │                                            │
                    │  Axum :3000                                │
                    │   ├── GET /api/*        → 现有 API handler │
                    │   ├── GET /admin/*      → 现有 admin API   │
                    │   ├── GET /sitemap.xml  → 动态生成         │
                    │   ├── GET /robots.txt   → 静态返回         │
                    │   ├── GET /posts/{id}   → 动态 SEO HTML    │
                    │   └── GET /*            → frontend/dist/   │
                    │                            (SPA fallback)  │
                    │       │                                    │
                    │       ▼                                    │
                    │  LanceDB (articles, images, music)         │
                    └────────────────────────────────────────────┘
```

### 2.1 与 GitHub Pages 的关系

| 入口 | 域名 | SEO | 用途 |
|------|------|-----|------|
| GitHub Pages | `acking-you.github.io` | 仅首页可索引 | 备用/分享链接 |
| 自托管 | `ackingliu.top` | 全站可索引 | 主站，提交给搜索引擎 |

两者共用同一套前端构建产物（`frontend/dist/`），同一个后端 API。

## 3. 实现步骤

### Step 1: 后端添加静态文件服务

**文件**: `backend/src/routes.rs`

在现有 API 路由之后，添加 `tower_http::services::ServeDir` 作为 fallback，
服务 `frontend/dist/` 目录下的静态文件。

```rust
use tower_http::services::{ServeDir, ServeFile};

pub fn create_router(state: AppState) -> Router {
    let api_router = Router::new()
        .route("/api/articles", get(handlers::list_articles))
        // ... 现有所有 /api/* 和 /admin/* 路由 ...
        .with_state(state.clone())
        .layer(/* 现有 middleware */);

    // SEO 路由（需要访问 AppState 查 LanceDB）
    let seo_router = Router::new()
        .route("/posts/:id", get(handlers::seo_article_page))
        .route("/sitemap.xml", get(handlers::sitemap_xml))
        .route("/robots.txt", get(handlers::robots_txt))
        .with_state(state);

    // 静态文件 fallback（SPA：所有未匹配路由返回 index.html）
    let frontend_dir = std::env::var("FRONTEND_DIST_DIR")
        .unwrap_or_else(|_| "../frontend/dist".to_string());
    let spa_fallback = ServeDir::new(&frontend_dir)
        .not_found_service(ServeFile::new(format!("{}/index.html", frontend_dir)));

    api_router
        .merge(seo_router)
        .fallback_service(spa_fallback)
}
```

**关键点**:
- `FRONTEND_DIST_DIR` 环境变量指定前端构建产物目录，默认 `../frontend/dist`
- `ServeDir` + `not_found_service(index.html)` 实现 SPA fallback
- API 路由优先级高于静态文件（Axum 路由匹配顺序）
- SEO 路由 `/posts/:id` 优先于 SPA fallback，确保爬虫拿到注入后的 HTML

**依赖**: `backend/Cargo.toml` 添加

```toml
[dependencies]
tower-http = { version = "0.6", features = ["fs"] }
```

> 注意：检查 `tower-http` 是否已在依赖中，如已有则只需确认 `fs` feature 开启。

### Step 2: 实现动态 SEO HTML handler

**文件**: `backend/src/handlers.rs`（新增函数）

`GET /posts/{id}` 的 handler 逻辑：

1. 从 LanceDB 查询文章（复用现有 `get_article` 的查询逻辑）
2. 读取 `frontend/dist/index.html` 作为模板
3. 替换 `<head>` 中的 meta tags 为文章专属内容
4. 在 `<body>` 开头注入文章纯文本（`<noscript>` 或隐藏 div）
5. 返回修改后的 HTML（Content-Type: text/html, HTTP 200）

```rust
pub async fn seo_article_page(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // 1. 查文章
    let article = state.article_store.get_article(&id).await;

    // 2. 读模板
    let frontend_dir = std::env::var("FRONTEND_DIST_DIR")
        .unwrap_or_else(|_| "../frontend/dist".to_string());
    let template = tokio::fs::read_to_string(format!("{}/index.html", frontend_dir))
        .await
        .unwrap_or_default();

    // 3. 根据是否找到文章，注入不同内容
    let html = match article {
        Ok(Some(article)) => inject_article_seo(&template, &article),
        _ => template, // 文章不存在，返回原始 SPA 页面
    };

    Html(html)
}
```

### Step 3: HTML 注入逻辑

**文件**: `backend/src/seo.rs`（新建模块）

核心函数 `inject_article_seo`，对 `index.html` 模板做字符串替换：

```rust
use static_flow_shared::Article;

const SITE_NAME: &str = "StaticFlow";
const SITE_BASE_URL: &str = "https://ackingliu.top";
const DEFAULT_OG_IMAGE: &str = "/static/android-chrome-512x512.png";

pub fn inject_article_seo(template: &str, article: &Article) -> String {
    let mut html = template.to_string();

    let title = format!("{} - {}", article.title, SITE_NAME);
    let description = extract_description(article);
    let canonical = format!("{}/posts/{}", SITE_BASE_URL, urlencoding::encode(&article.id));
    let og_image = resolve_og_image(article);

    // 替换 <title>
    html = replace_tag_content(&html, "<title>", "</title>", &title);

    // 替换/注入 meta tags（利用 index.html 中已有的 data-sf-seo 标记）
    html = replace_meta(&html, "name", "description", &description);
    html = replace_meta(&html, "property", "og:title", &title);
    html = replace_meta(&html, "property", "og:description", &description);
    html = replace_meta(&html, "property", "og:url", &canonical);
    html = replace_meta(&html, "property", "og:type", "article");
    html = replace_meta(&html, "property", "og:image", &og_image);
    html = replace_meta(&html, "name", "twitter:title", &title);
    html = replace_meta(&html, "name", "twitter:description", &description);
    html = replace_meta(&html, "name", "twitter:image", &og_image);

    // 替换 canonical link
    html = replace_canonical(&html, &canonical);

    // 注入 JSON-LD 结构化数据
    let json_ld = build_article_json_ld(article, &canonical, &og_image);
    html = inject_json_ld(&html, &json_ld);

    // 在 <body> 后注入 SEO 纯文本内容（对用户不可见，WASM 加载后覆盖）
    let seo_content = build_seo_body_content(article);
    html = inject_after_body_tag(&html, &seo_content);

    html
}
```

#### 3.1 描述提取

优先级：`detailed_summary.zh` → `summary` → `content` 前 160 字

```rust
fn extract_description(article: &Article) -> String {
    // 优先用 detailed_summary
    if let Some(ref ds) = article.detailed_summary {
        if let Some(ref zh) = ds.zh {
            if !zh.trim().is_empty() {
                return truncate_text(zh, 160);
            }
        }
    }
    if !article.summary.trim().is_empty() {
        return truncate_text(&article.summary, 160);
    }
    truncate_text(&strip_markdown(&article.content), 160)
}
```

#### 3.2 SEO body 内容

注入到 `<body>` 开头，用 `<div style="display:none">` 包裹，
WASM 加载后自然覆盖整个 DOM：

```rust
fn build_seo_body_content(article: &Article) -> String {
    let tags_html = article.tags.iter()
        .map(|t| format!("<span>{}</span>", html_escape(t)))
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        r#"<div id="seo-content" style="display:none">
<article>
<h1>{title}</h1>
<p>{description}</p>
<p>{content_preview}</p>
<footer>
<span>{author}</span> · <time>{date}</time>
{tags}
</footer>
</article>
</div>"#,
        title = html_escape(&article.title),
        description = html_escape(&extract_description(article)),
        content_preview = html_escape(&truncate_text(&strip_markdown(&article.content), 2000)),
        author = html_escape(&article.author),
        date = html_escape(&article.date),
        tags = tags_html,
    )
}
```

> **为什么用 `display:none` 而不是 `<noscript>`？**
> `<noscript>` 在启用 JS 的浏览器中完全不渲染，但部分爬虫可能忽略 noscript 内容。
> `display:none` 的内容在 DOM 中存在，Google 明确表示会索引 `display:none` 的文本内容。
> WASM 加载后接管整个 `<body>` 渲染，这个 div 自然被替换掉。

#### 3.3 JSON-LD 结构化数据

```rust
fn build_article_json_ld(article: &Article, canonical: &str, og_image: &str) -> String {
    serde_json::json!({
        "@context": "https://schema.org",
        "@type": "BlogPosting",
        "headline": truncate_text(&article.title, 110),
        "description": extract_description(article),
        "url": canonical,
        "image": [og_image],
        "author": {
            "@type": "Person",
            "name": if article.author.trim().is_empty() { "ackingliu" } else { article.author.trim() }
        },
        "publisher": {
            "@type": "Organization",
            "name": SITE_NAME
        },
        "datePublished": &article.date,
        "dateModified": &article.date,
        "keywords": article.tags.join(", "),
        "articleSection": &article.category,
        "inLanguage": "zh-CN",
        "mainEntityOfPage": {
            "@type": "WebPage",
            "@id": canonical
        }
    }).to_string()
}
```

### Step 4: 动态 sitemap.xml

**文件**: `backend/src/handlers.rs`

```rust
pub async fn sitemap_xml(State(state): State<AppState>) -> impl IntoResponse {
    let articles = state.article_store.list_all_articles().await.unwrap_or_default();
    let base = "https://ackingliu.top";

    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">"#
    );

    // 首页
    xml.push_str(&format!(
        "\n  <url><loc>{}</loc><changefreq>daily</changefreq><priority>1.0</priority></url>",
        base
    ));

    // 文章页
    for article in &articles {
        let encoded_id = urlencoding::encode(&article.id);
        xml.push_str(&format!(
            "\n  <url><loc>{}/posts/{}</loc><lastmod>{}</lastmod><changefreq>weekly</changefreq><priority>0.8</priority></url>",
            base, encoded_id, &article.date
        ));
    }

    xml.push_str("\n</urlset>");

    (
        [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
        xml,
    )
}
```

### Step 5: robots.txt

```rust
pub async fn robots_txt() -> impl IntoResponse {
    let body = "User-agent: *\n\
                Allow: /\n\
                Disallow: /admin\n\n\
                Sitemap: https://ackingliu.top/sitemap.xml\n";
    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body)
}
```

### Step 6: CORS 更新

**文件**: `backend/src/routes.rs`

生产环境 `ALLOWED_ORIGINS` 需要加上自托管域名：

```bash
ALLOWED_ORIGINS=https://acking-you.github.io,https://ackingliu.top
```

同源请求（从 ackingliu.top 加载的前端请求 ackingliu.top 的 API）不需要 CORS，
但保留配置以兼容 GitHub Pages 前端跨域请求。

### Step 7: pb-mapper 映射

复用现有 API 的 pb-mapper 链路。由于 Axum 现在同时服务 API 和前端，
只需要一个隧道：

```bash
# 本地：注册服务（Axum :3000 → pb-mapper server）
pb-mapper-server-cli tcp-server \
  --key staticflow-web \
  --addr 127.0.0.1:3000 \
  --pb-mapper-server <cloud-ip>:7666

# 云端：映射到本地端口
pb-mapper-client-cli tcp-server \
  --key staticflow-web \
  --addr 127.0.0.1:<local-port> \
  --pb-mapper-server 127.0.0.1:7666
```

### Step 8: Caddy 配置

云端 Caddy 已有 SSL 配置，新增一个站点块：

```caddyfile
ackingliu.top {
    reverse_proxy 127.0.0.1:<pb-mapper-映射端口>
}
```

Caddy 自动通过 Let's Encrypt 签发 `ackingliu.top` 的证书。

> 如果 API 子域名 `api.ackingliu.top` 也要保留，可以合并：
> ```caddyfile
> ackingliu.top {
>     reverse_proxy 127.0.0.1:<port>
> }
>
> api.ackingliu.top {
>     reverse_proxy 127.0.0.1:<port>
> }
> ```
> 两个域名指向同一个 Axum 实例，API 路由自然匹配。

### Step 9: 前端构建配置

前端需要感知 `SITE_BASE_URL` 的变化。两种方式：

**方式 A（推荐）：环境变量控制 API_BASE**

自托管版本构建时：

```bash
STATICFLOW_API_BASE=/api trunk build --release
```

API_BASE 设为相对路径 `/api`，因为前端和 API 同源（都是 ackingliu.top），
不需要跨域。

GitHub Pages 版本继续用绝对路径：

```bash
STATICFLOW_API_BASE=https://api.ackingliu.top/api trunk build --release
```

**方式 B：运行时检测**

前端 `api.rs` 中根据 `window.location.origin` 动态决定 API_BASE：

```rust
pub fn api_base() -> String {
    if let Some(win) = web_sys::window() {
        let origin = win.location().origin().unwrap_or_default();
        if origin.contains("ackingliu.top") {
            return format!("{}/api", origin); // 同源
        }
    }
    // fallback: 编译时配置
    API_BASE.to_string()
}
```

方式 B 的好处是同一份构建产物可以同时用于 GitHub Pages 和自托管。

### Step 10: 搜索引擎提交

部署完成后：

1. **Google Search Console**: 添加 `ackingliu.top`，提交 sitemap
2. **Bing Webmaster Tools**: 添加站点，提交 sitemap
3. **百度站长平台**: 添加站点，提交 sitemap（百度对 JS 渲染支持差，自托管方案尤其重要）

## 4. 文件改动清单

| 文件 | 改动类型 | 说明 |
|------|----------|------|
| `backend/Cargo.toml` | 修改 | 确认 `tower-http` 的 `fs` feature |
| `backend/src/routes.rs` | 修改 | 添加 SEO 路由 + 静态文件 fallback |
| `backend/src/handlers.rs` | 修改 | 新增 `seo_article_page`, `sitemap_xml`, `robots_txt` |
| `backend/src/seo.rs` | 新建 | HTML 注入逻辑（meta tags, JSON-LD, body content） |
| `backend/src/main.rs` | 修改 | `mod seo;` |

**不需要改动的文件**:
- 前端代码（零改动，或可选方式 B 的 api.rs 小改）
- Caddy 配置（已有 SSL，只加一个站点块）
- pb-mapper（复用现有部署，换个 key）

## 5. 请求处理流程

### 5.1 爬虫访问文章页

```
GET /posts/agent-harness-2026
    ↓
Axum 匹配 /posts/:id 路由
    ↓
seo_article_page handler:
  1. LanceDB 查询 article (id = "agent-harness-2026")
  2. 读取 frontend/dist/index.html
  3. 注入: <title>, meta tags, JSON-LD, <body> 纯文本
  4. 返回 HTTP 200 + 完整 HTML
    ↓
爬虫拿到:
  - HTTP 200 ✓
  - <title>Agent Harness 2026 - StaticFlow</title> ✓
  - <meta name="description" content="..."> ✓
  - JSON-LD BlogPosting ✓
  - 文章正文纯文本 ✓
```

### 5.2 真人用户访问文章页

```
GET /posts/agent-harness-2026
    ↓
同上，返回注入后的 HTML（HTTP 200）
    ↓
浏览器解析 HTML:
  1. <head> 中的 meta tags 已就位（社交分享预览正确）
  2. <body> 中 #seo-content (display:none) 不可见
  3. WASM 加载 → Yew 接管 <body> 渲染
  4. 客户端路由识别 /posts/agent-harness-2026
  5. 调 /api/articles/agent-harness-2026 获取完整数据
  6. 渲染交互式文章页（代码高亮、目录、评论等）
```

**用户体验**: 与当前 GitHub Pages 一致，无闪烁、无重定向。
唯一区别是首屏 HTML 已经包含正确的 `<title>` 和 meta tags。

### 5.3 站内 SPA 导航

```
用户在首页点击文章链接
    ↓
Yew 客户端路由，不发起新的 HTTP 请求
    ↓
正常 SPA 体验（与现在完全一致）
```

## 6. 注意事项

### 6.1 index.html 模板缓存

`seo_article_page` 每次请求都读 `index.html`。可以优化为启动时读一次缓存到内存：

```rust
// AppState 中添加
pub index_html_template: Arc<String>,
```

启动时加载，`trunk build` 后重启 backend 即可刷新。
或用 `notify` crate 监听文件变化自动重载（可选优化）。

### 6.2 SITE_BASE_URL 配置化

`seo.rs` 中的 `SITE_BASE_URL` 应该从环境变量读取：

```rust
fn site_base_url() -> String {
    std::env::var("SITE_BASE_URL").unwrap_or_else(|_| "https://ackingliu.top".to_string())
}
```

### 6.3 前端 seo.rs 中的 SITE_BASE_URL

前端 `frontend/src/seo.rs:8` 硬编码了 `https://acking-you.github.io`。
自托管版本中，客户端 SEO 更新（如 SPA 内导航时的 canonical URL）
应该使用 `ackingliu.top`。可通过编译时环境变量或运行时检测处理。

### 6.4 本地机器关机时的可用性

pb-mapper 隧道依赖本地机器在线。本地关机时 `ackingliu.top` 不可用。
GitHub Pages 作为备用入口不受影响。

如果需要 7×24 可用，可考虑将 backend + LanceDB 迁移到云端 VPS，
但这超出当前方案范围。
