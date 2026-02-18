use serde_json::{json, Map, Value};
use static_flow_shared::Article;
use web_sys::{window, Document, Element};

use crate::{config, router::Route, utils::image_url};

const SITE_NAME: &str = "StaticFlow";
const SITE_BASE_URL: &str = "https://acking-you.github.io";
const DEFAULT_AUTHOR: &str = "ackingliu";
const DEFAULT_OG_IMAGE: &str = "/static/android-chrome-512x512.png";
const DEFAULT_DESCRIPTION: &str = "可视化博客 + Skill \
                                   工作流：一键完成创作、分类、标签化、发布与部署；基于 LanceDB \
                                   统一存储文章与图片，支持全文语义、混合检索与 AI 评论自动回复。";

fn document() -> Option<Document> {
    window().and_then(|win| win.document())
}

fn head() -> Option<Element> {
    let doc = document()?;
    doc.query_selector("head").ok().flatten()
}

fn upsert_head_element(selector: &str, tag_name: &str) -> Option<Element> {
    let doc = document()?;
    if let Some(found) = doc.query_selector(selector).ok().flatten() {
        return Some(found);
    }
    let head = head()?;
    let created = doc.create_element(tag_name).ok()?;
    let _ = head.append_child(&created);
    Some(created)
}

fn remove_nodes(selector: &str) {
    let Some(doc) = document() else {
        return;
    };
    let Ok(nodes) = doc.query_selector_all(selector) else {
        return;
    };

    let mut index = 0;
    while index < nodes.length() {
        if let Some(node) = nodes.item(index) {
            if let Some(parent) = node.parent_node() {
                let _ = parent.remove_child(&node);
            }
        }
        index += 1;
    }
}

fn normalize_whitespace(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    for ch in value.chars() {
        if count >= max_chars {
            break;
        }
        out.push(ch);
        count += 1;
    }
    if value.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

fn normalize_meta_text(value: &str, max_chars: usize) -> String {
    let compact = normalize_whitespace(value);
    if compact.is_empty() {
        String::new()
    } else if compact.chars().count() > max_chars {
        truncate_chars(&compact, max_chars)
    } else {
        compact
    }
}

fn set_meta_name(name: &str, content: &str) {
    let selector = format!("meta[name=\"{}\"]", name);
    let Some(element) = upsert_head_element(&selector, "meta") else {
        return;
    };
    let _ = element.set_attribute("name", name);
    let _ = element.set_attribute("content", content);
}

fn set_meta_property(property: &str, content: &str) {
    let selector = format!("meta[property=\"{}\"]", property);
    let Some(element) = upsert_head_element(&selector, "meta") else {
        return;
    };
    let _ = element.set_attribute("property", property);
    let _ = element.set_attribute("content", content);
}

fn set_link_canonical(url: &str) {
    let Some(element) = upsert_head_element("link[rel=\"canonical\"]", "link") else {
        return;
    };
    let _ = element.set_attribute("rel", "canonical");
    let _ = element.set_attribute("href", url);
    let _ = element.set_attribute("data-sf-seo", "canonical");
}

fn set_html_lang(lang: &str) {
    let Some(doc) = document() else {
        return;
    };
    if let Some(root) = doc.document_element() {
        let _ = root.set_attribute("lang", lang);
    }
}

pub fn set_document_title(title: &str) {
    let Some(doc) = document() else {
        return;
    };
    doc.set_title(title);
}

pub fn absolute_url(path_or_url: &str) -> String {
    let trimmed = path_or_url.trim();
    if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
        return trimmed.to_string();
    }
    let normalized_path =
        if trimmed.starts_with('/') { trimmed.to_string() } else { format!("/{}", trimmed) };
    format!("{}{}", SITE_BASE_URL.trim_end_matches('/'), normalized_path)
}

fn default_og_image_url() -> String {
    absolute_url(DEFAULT_OG_IMAGE)
}

fn resolve_social_image_url(raw: Option<&str>) -> String {
    let Some(raw_path) = raw else {
        return default_og_image_url();
    };
    let transformed = image_url(raw_path);
    if transformed.starts_with("http://") || transformed.starts_with("https://") {
        transformed
    } else if transformed.starts_with('/') {
        absolute_url(&transformed)
    } else {
        absolute_url(&format!("/{}", transformed))
    }
}

pub fn set_hreflang_links(links: &[(&str, String)]) {
    remove_nodes("link[rel=\"alternate\"][data-sf-seo=\"hreflang\"]");
    let Some(doc) = document() else {
        return;
    };
    let Some(head) = head() else {
        return;
    };
    for (hreflang, href) in links {
        if href.trim().is_empty() {
            continue;
        }
        let Ok(link) = doc.create_element("link") else {
            continue;
        };
        let _ = link.set_attribute("rel", "alternate");
        let _ = link.set_attribute("hreflang", hreflang);
        let _ = link.set_attribute("href", href);
        let _ = link.set_attribute("data-sf-seo", "hreflang");
        let _ = head.append_child(&link);
    }
}

pub fn set_json_ld(id: &str, payload: &Value, page_scoped: bool) {
    let selector = format!("script[type=\"application/ld+json\"][data-sf-jsonld-id=\"{}\"]", id);
    let Some(element) = upsert_head_element(&selector, "script") else {
        return;
    };
    let _ = element.set_attribute("type", "application/ld+json");
    let _ = element.set_attribute("data-sf-jsonld-id", id);
    if page_scoped {
        let _ = element.set_attribute("data-sf-jsonld-page", "true");
    } else {
        let _ = element.remove_attribute("data-sf-jsonld-page");
    }
    let serialized = serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string());
    element.set_text_content(Some(&serialized));
}

pub fn clear_page_scoped_json_ld() {
    remove_nodes("script[type=\"application/ld+json\"][data-sf-jsonld-page=\"true\"]");
}

fn apply_common_seo(
    title: &str,
    description: &str,
    canonical_url: &str,
    og_type: &str,
    robots: &str,
    html_lang: &str,
    og_image_url: &str,
) {
    let normalized_title = normalize_meta_text(title, 88);
    let normalized_desc = {
        let candidate = normalize_meta_text(description, 180);
        if candidate.is_empty() {
            DEFAULT_DESCRIPTION.to_string()
        } else {
            candidate
        }
    };

    set_document_title(&normalized_title);
    set_html_lang(html_lang);
    set_link_canonical(canonical_url);

    set_meta_name("description", &normalized_desc);
    set_meta_name("robots", robots);
    set_meta_name("googlebot", robots);
    set_meta_name("author", DEFAULT_AUTHOR);
    set_meta_name("twitter:card", "summary_large_image");
    set_meta_name("twitter:title", &normalized_title);
    set_meta_name("twitter:description", &normalized_desc);
    set_meta_name("twitter:image", og_image_url);

    set_meta_property("og:type", og_type);
    set_meta_property("og:site_name", SITE_NAME);
    set_meta_property("og:title", &normalized_title);
    set_meta_property("og:description", &normalized_desc);
    set_meta_property("og:url", canonical_url);
    set_meta_property(
        "og:locale",
        if html_lang.eq_ignore_ascii_case("en") { "en_US" } else { "zh_CN" },
    );
    set_meta_property(
        "og:locale:alternate",
        if html_lang.eq_ignore_ascii_case("en") { "zh_CN" } else { "en_US" },
    );
    set_meta_property("og:image", og_image_url);
}

fn apply_default_hreflang(canonical_url: &str) {
    let entries = vec![
        ("zh-CN", canonical_url.to_string()),
        ("en", canonical_url.to_string()),
        ("x-default", canonical_url.to_string()),
    ];
    set_hreflang_links(&entries);
}

fn route_path_for(route: &Route) -> String {
    match route {
        Route::Home => config::route_path("/"),
        Route::LatestArticles => config::route_path("/latest"),
        Route::Posts => config::route_path("/posts"),
        Route::ArticleDetail {
            id,
        } => config::route_path(&format!("/posts/{}", urlencoding::encode(id))),
        Route::ArticleRaw {
            id,
            lang,
        } => config::route_path(&format!(
            "/posts/{}/raw/{}",
            urlencoding::encode(id),
            urlencoding::encode(lang)
        )),
        Route::Tags => config::route_path("/tags"),
        Route::TagDetail {
            tag,
        } => config::route_path(&format!("/tags/{}", urlencoding::encode(tag))),
        Route::Categories => config::route_path("/categories"),
        Route::CategoryDetail {
            category,
        } => config::route_path(&format!("/categories/{}", urlencoding::encode(category))),
        Route::Search => config::route_path("/search"),
        Route::Admin => config::route_path("/admin"),
        Route::AdminCommentRuns {
            task_id,
        } => config::route_path(&format!("/admin/comments/runs/{}", urlencoding::encode(task_id))),
        Route::NotFound => config::route_path("/404"),
    }
}

pub fn apply_route_seo(route: Option<&Route>) {
    clear_page_scoped_json_ld();

    let fallback_home = Route::Home;
    let active_route = route.unwrap_or(&fallback_home);
    let route_path = route_path_for(active_route);
    let canonical_url = absolute_url(&route_path);
    let og_image = default_og_image_url();

    match active_route {
        Route::Home => {
            apply_common_seo(
                "StaticFlow · AI + Skill 驱动的本地优先技术博客",
                DEFAULT_DESCRIPTION,
                &canonical_url,
                "website",
                "index,follow,max-image-preview:large",
                "zh-CN",
                &og_image,
            );
            apply_default_hreflang(&canonical_url);
            set_json_ld(
                "website",
                &json!({
                    "@context": "https://schema.org",
                    "@type": "WebSite",
                    "name": SITE_NAME,
                    "url": SITE_BASE_URL,
                    "inLanguage": "zh-CN",
                    "potentialAction": {
                        "@type": "SearchAction",
                        "target": format!("{}?q={{search_term_string}}", absolute_url(&config::route_path("/search"))),
                        "query-input": "required name=search_term_string"
                    }
                }),
                true,
            );
        },
        Route::LatestArticles => {
            apply_common_seo(
                "最新文章 · StaticFlow",
                "查看 StaticFlow 最新发布的技术文章与系统实践记录。",
                &canonical_url,
                "website",
                "index,follow,max-image-preview:large",
                "zh-CN",
                &og_image,
            );
            apply_default_hreflang(&canonical_url);
        },
        Route::Posts => {
            apply_common_seo(
                "文章归档 · StaticFlow",
                "按时间浏览 StaticFlow 全部文章，覆盖 Rust、LanceDB、全栈工程与 AI 工作流。",
                &canonical_url,
                "website",
                "index,follow,max-image-preview:large",
                "zh-CN",
                &og_image,
            );
            apply_default_hreflang(&canonical_url);
        },
        Route::Tags => {
            apply_common_seo(
                "标签索引 · StaticFlow",
                "通过标签快速检索 StaticFlow 文章主题。",
                &canonical_url,
                "website",
                "index,follow,max-image-preview:large",
                "zh-CN",
                &og_image,
            );
            apply_default_hreflang(&canonical_url);
        },
        Route::TagDetail {
            tag,
        } => {
            apply_common_seo(
                &format!("标签：{} · StaticFlow", normalize_meta_text(tag, 42)),
                &format!("浏览标签“{}”下的相关文章。", normalize_meta_text(tag, 60)),
                &canonical_url,
                "website",
                "index,follow,max-image-preview:large",
                "zh-CN",
                &og_image,
            );
            apply_default_hreflang(&canonical_url);
        },
        Route::Categories => {
            apply_common_seo(
                "分类导航 · StaticFlow",
                "按分类浏览 StaticFlow 的技术内容。",
                &canonical_url,
                "website",
                "index,follow,max-image-preview:large",
                "zh-CN",
                &og_image,
            );
            apply_default_hreflang(&canonical_url);
        },
        Route::CategoryDetail {
            category,
        } => {
            apply_common_seo(
                &format!("分类：{} · StaticFlow", normalize_meta_text(category, 42)),
                &format!("查看分类“{}”下的文章与实践。", normalize_meta_text(category, 60)),
                &canonical_url,
                "website",
                "index,follow,max-image-preview:large",
                "zh-CN",
                &og_image,
            );
            apply_default_hreflang(&canonical_url);
        },
        Route::Search => {
            apply_common_seo(
                "搜索 · StaticFlow",
                "支持全文、语义与混合检索的 StaticFlow 搜索页面。",
                &canonical_url,
                "website",
                "index,follow,max-image-preview:large",
                "zh-CN",
                &og_image,
            );
            apply_default_hreflang(&canonical_url);
        },
        Route::ArticleDetail {
            id,
        } => {
            apply_common_seo(
                &format!("{} · 文章详情 · StaticFlow", normalize_meta_text(id, 52)),
                "正在加载文章详情与完整正文内容。",
                &canonical_url,
                "article",
                "index,follow,max-image-preview:large",
                "zh-CN",
                &og_image,
            );
            apply_default_hreflang(&canonical_url);
        },
        Route::ArticleRaw {
            id,
            lang,
        } => {
            let normalized_lang = if lang.eq_ignore_ascii_case("en") { "en" } else { "zh" };
            apply_common_seo(
                &format!(
                    "{} · Raw Markdown ({}) · StaticFlow",
                    normalize_meta_text(id, 48),
                    normalized_lang.to_uppercase()
                ),
                "原始 Markdown 查看页面。",
                &canonical_url,
                "article",
                "noindex,follow,max-image-preview:large",
                if normalized_lang == "en" { "en" } else { "zh-CN" },
                &og_image,
            );
            let encoded_id = urlencoding::encode(id);
            let zh_url =
                absolute_url(&config::route_path(&format!("/posts/{}/raw/zh", encoded_id)));
            let en_url =
                absolute_url(&config::route_path(&format!("/posts/{}/raw/en", encoded_id)));
            let entries =
                vec![("zh-CN", zh_url.clone()), ("en", en_url.clone()), ("x-default", zh_url)];
            set_hreflang_links(&entries);
        },
        Route::Admin
        | Route::AdminCommentRuns {
            ..
        } => {
            apply_common_seo(
                "Admin · StaticFlow",
                "StaticFlow 管理界面。",
                &canonical_url,
                "website",
                "noindex,nofollow,noarchive",
                "zh-CN",
                &og_image,
            );
            apply_default_hreflang(&canonical_url);
        },
        Route::NotFound => {
            apply_common_seo(
                "404 · StaticFlow",
                "页面不存在。",
                &canonical_url,
                "website",
                "noindex,nofollow,noarchive",
                "zh-CN",
                &og_image,
            );
            apply_default_hreflang(&canonical_url);
        },
    }
}

fn article_description(article: &Article, preferred_lang: &str) -> String {
    if !article.summary.trim().is_empty() {
        return normalize_meta_text(&article.summary, 180);
    }

    let content = if preferred_lang.eq_ignore_ascii_case("en") {
        article
            .content_en
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(article.content.as_str())
    } else {
        article.content.as_str()
    };

    let mut best_line = String::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed.starts_with("```")
            || trimmed.starts_with('>')
            || trimmed.starts_with("- ")
            || trimmed.starts_with("* ")
            || trimmed.starts_with('|')
            || trimmed.starts_with("![")
        {
            continue;
        }
        best_line = trimmed.to_string();
        if !best_line.is_empty() {
            break;
        }
    }
    if best_line.is_empty() {
        DEFAULT_DESCRIPTION.to_string()
    } else {
        normalize_meta_text(&best_line, 180)
    }
}

pub fn apply_article_seo(article: &Article, article_id: &str, preferred_lang: &str) {
    clear_page_scoped_json_ld();

    let encoded_id = urlencoding::encode(article_id);
    let article_path = config::route_path(&format!("/posts/{}", encoded_id));
    let canonical_url = absolute_url(&article_path);
    let posts_url = absolute_url(&config::route_path("/posts"));
    let home_url = absolute_url(&config::route_path("/"));
    let html_lang = if preferred_lang.eq_ignore_ascii_case("en") { "en" } else { "zh-CN" };
    let title = format!("{} · {}", normalize_meta_text(&article.title, 78), SITE_NAME);
    let description = article_description(article, preferred_lang);
    let og_image = resolve_social_image_url(article.featured_image.as_deref());

    apply_common_seo(
        &title,
        &description,
        &canonical_url,
        "article",
        "index,follow,max-image-preview:large",
        html_lang,
        &og_image,
    );
    apply_default_hreflang(&canonical_url);

    let mut posting = Map::new();
    posting.insert("@context".to_string(), json!("https://schema.org"));
    posting.insert("@type".to_string(), json!("BlogPosting"));
    posting.insert("headline".to_string(), json!(normalize_meta_text(&article.title, 110)));
    posting.insert("description".to_string(), json!(description));
    posting.insert("url".to_string(), json!(canonical_url));
    posting.insert(
        "mainEntityOfPage".to_string(),
        json!({
            "@type": "WebPage",
            "@id": absolute_url(&article_path),
        }),
    );
    posting.insert(
        "author".to_string(),
        json!({
            "@type": "Person",
            "name": if article.author.trim().is_empty() {
                DEFAULT_AUTHOR
            } else {
                article.author.trim()
            }
        }),
    );
    if !article.date.trim().is_empty() {
        posting.insert("datePublished".to_string(), json!(normalize_meta_text(&article.date, 32)));
        posting.insert("dateModified".to_string(), json!(normalize_meta_text(&article.date, 32)));
    }
    posting.insert("inLanguage".to_string(), json!(html_lang));
    posting.insert(
        "publisher".to_string(),
        json!({
            "@type": "Organization",
            "name": SITE_NAME,
        }),
    );
    if !article.tags.is_empty() {
        posting.insert("keywords".to_string(), json!(article.tags.join(", ")));
    }
    if !article.category.trim().is_empty() {
        posting.insert(
            "articleSection".to_string(),
            json!(normalize_meta_text(&article.category, 48)),
        );
    }
    posting.insert("image".to_string(), json!([og_image]));
    set_json_ld("page-blogposting", &Value::Object(posting), true);

    let breadcrumb = json!({
        "@context": "https://schema.org",
        "@type": "BreadcrumbList",
        "itemListElement": [
            {
                "@type": "ListItem",
                "position": 1,
                "name": "Home",
                "item": home_url
            },
            {
                "@type": "ListItem",
                "position": 2,
                "name": "Posts",
                "item": posts_url
            },
            {
                "@type": "ListItem",
                "position": 3,
                "name": normalize_meta_text(&article.title, 92),
                "item": absolute_url(&article_path)
            }
        ]
    });
    set_json_ld("page-breadcrumbs", &breadcrumb, true);
}

pub fn apply_raw_markdown_seo(article_id: &str, lang: &str, raw_page_title: &str) {
    clear_page_scoped_json_ld();
    let normalized_lang = if lang.eq_ignore_ascii_case("en") { "en" } else { "zh" };
    let encoded_id = urlencoding::encode(article_id);
    let route = config::route_path(&format!("/posts/{}/raw/{}", encoded_id, normalized_lang));
    let canonical_url = absolute_url(&route);
    let zh_url = absolute_url(&config::route_path(&format!("/posts/{}/raw/zh", encoded_id)));
    let en_url = absolute_url(&config::route_path(&format!("/posts/{}/raw/en", encoded_id)));
    let title = format!("{} · {}", normalize_meta_text(raw_page_title, 84), SITE_NAME);
    let description = if normalized_lang == "en" {
        "Raw markdown source of this article (English)."
    } else {
        "该文章的原始 Markdown 内容。"
    };

    apply_common_seo(
        &title,
        description,
        &canonical_url,
        "article",
        "noindex,follow,max-image-preview:large",
        if normalized_lang == "en" { "en" } else { "zh-CN" },
        &default_og_image_url(),
    );
    let entries = vec![("zh-CN", zh_url.clone()), ("en", en_url.clone()), ("x-default", zh_url)];
    set_hreflang_links(&entries);
}
