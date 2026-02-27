use std::env;

use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
};
use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use static_flow_shared::Article;

use crate::state::AppState;

// ---------------------------------------------------------------------------
// Environment helpers
// ---------------------------------------------------------------------------

fn site_base_url() -> String {
    env::var("SITE_BASE_URL").unwrap_or_else(|_| "https://ackingliu.top".to_string())
}

// ---------------------------------------------------------------------------
// HTML escaping
// ---------------------------------------------------------------------------

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn html_attr_escape(s: &str) -> String {
    html_escape(s).replace('"', "&quot;")
}

// ---------------------------------------------------------------------------
// Text utilities
// ---------------------------------------------------------------------------

fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_chars).collect();
    format!("{}…", truncated.trim_end())
}

/// Strip Markdown formatting, returning plain text.
fn strip_markdown(md: &str) -> String {
    let parser = Parser::new(md);
    let mut buf = String::with_capacity(md.len());
    let mut in_code_block = false;

    for event in parser {
        match event {
            Event::Text(t) | Event::Code(t) => buf.push_str(&t),
            Event::SoftBreak | Event::HardBreak => buf.push(' '),
            Event::Start(Tag::CodeBlock(_)) => in_code_block = true,
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                buf.push(' ');
            },
            Event::Start(Tag::Paragraph) if !buf.is_empty() && !in_code_block => {
                buf.push(' ');
            },
            _ => {},
        }
    }
    // Collapse whitespace
    buf.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Extract the best description from an article.
fn extract_description(article: &Article) -> String {
    // Priority: detailed_summary.zh → summary → content (first 160 chars)
    if let Some(ref ds) = article.detailed_summary {
        if let Some(ref zh) = ds.zh {
            let text = strip_markdown(zh);
            if !text.is_empty() {
                return truncate_text(&text, 160);
            }
        }
    }
    if !article.summary.is_empty() {
        return truncate_text(&strip_markdown(&article.summary), 160);
    }
    truncate_text(&strip_markdown(&article.content), 160)
}

// ---------------------------------------------------------------------------
// HTML template injection helpers
// ---------------------------------------------------------------------------

fn replace_meta_content(
    html: &str,
    attr_name: &str,
    attr_value: &str,
    new_content: &str,
) -> String {
    // Find <meta ... {attr_name}="{attr_value}" ... content="..."> where whitespace
    // (including newlines) may separate attributes.
    let needle = format!(r#"{attr_name}="{attr_value}""#);

    // Strategy: find the needle, then locate content="..." within the same <meta>
    // tag.
    let mut search_from = 0;
    while let Some(needle_pos) = html[search_from..].find(&needle) {
        let abs_needle = search_from + needle_pos;

        // Find the enclosing <meta tag start
        let tag_start = html[..abs_needle].rfind("<meta").unwrap_or(abs_needle);
        // Find the tag end (> or />)
        let tag_end = match html[abs_needle..].find('>') {
            Some(p) => abs_needle + p,
            None => {
                search_from = abs_needle + needle.len();
                continue;
            },
        };

        let tag_slice = &html[tag_start..=tag_end];

        // Find content="..." within this tag
        if let Some(cpos) = tag_slice.find("content=\"") {
            let content_val_start = tag_start + cpos + "content=\"".len();
            if let Some(end_quote) = html[content_val_start..].find('"') {
                let before = &html[..content_val_start];
                let after = &html[content_val_start + end_quote..];
                return format!("{}{}{}", before, html_attr_escape(new_content), after);
            }
        }

        search_from = tag_end + 1;
    }

    // Try reversed order: content="..." {attr_name}="..." (content appears first)
    let content_needle = "content=\"";
    search_from = 0;
    while let Some(pos) = html[search_from..].find(content_needle) {
        let abs_pos = search_from + pos;
        let content_start = abs_pos + content_needle.len();
        if let Some(end_quote) = html[content_start..].find('"') {
            let tag_end = html[content_start + end_quote..]
                .find('>')
                .map(|p| content_start + end_quote + p);
            if let Some(te) = tag_end {
                if html[abs_pos..=te].contains(&needle) {
                    let before = &html[..content_start];
                    let after = &html[content_start + end_quote..];
                    return format!("{}{}{}", before, html_attr_escape(new_content), after);
                }
            }
            search_from = content_start + end_quote + 1;
        } else {
            break;
        }
    }
    html.to_string()
}

fn replace_title(html: &str, new_title: &str) -> String {
    if let Some(start) = html.find("<title>") {
        if let Some(end) = html[start..].find("</title>") {
            let before = &html[..start + 7]; // after <title>
            let after = &html[start + end..];
            return format!("{}{}{}", before, html_escape(new_title), after);
        }
    }
    html.to_string()
}

fn replace_canonical_href(html: &str, new_href: &str) -> String {
    let needle = r#"rel="canonical" href=""#;
    if let Some(start) = html.find(needle) {
        let href_start = start + needle.len();
        if let Some(end_quote) = html[href_start..].find('"') {
            let before = &html[..href_start];
            let after = &html[href_start + end_quote..];
            return format!("{}{}{}", before, html_attr_escape(new_href), after);
        }
    }
    html.to_string()
}

fn inject_before(html: &str, marker: &str, content: &str) -> String {
    if let Some(pos) = html.find(marker) {
        let before = &html[..pos];
        let after = &html[pos..];
        return format!("{}{}{}", before, content, after);
    }
    html.to_string()
}

// ---------------------------------------------------------------------------
// JSON-LD structured data
// ---------------------------------------------------------------------------

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "")
        .replace('\t', "\\t")
}

fn build_article_json_ld(
    article: &Article,
    canonical: &str,
    og_image: &str,
    description: &str,
) -> String {
    let author = if article.author.is_empty() { "ackingliu" } else { &article.author };
    let mut ld = format!(
        r#"<script type="application/ld+json">
{{
  "@context": "https://schema.org",
  "@type": "Article",
  "headline": "{}",
  "description": "{}",
  "author": {{ "@type": "Person", "name": "{}" }},
  "datePublished": "{}",
  "url": "{}""#,
        json_escape(&article.title),
        json_escape(description),
        json_escape(author),
        json_escape(&article.date),
        json_escape(canonical),
    );
    if !og_image.is_empty() {
        ld.push_str(&format!(
            r#",
  "image": "{}""#,
            json_escape(og_image)
        ));
    }
    if !article.tags.is_empty() {
        let kw: Vec<String> = article
            .tags
            .iter()
            .map(|t| format!("\"{}\"", json_escape(t)))
            .collect();
        ld.push_str(&format!(
            r#",
  "keywords": [{}]"#,
            kw.join(", ")
        ));
    }
    ld.push_str("\n}\n</script>");
    ld
}

// ---------------------------------------------------------------------------
// Main injection: template + article → full HTML
// ---------------------------------------------------------------------------

fn inject_article_seo(template: &str, article: &Article) -> String {
    if template.is_empty() {
        // No template loaded — return a minimal SEO page
        return build_fallback_seo_html(article);
    }

    let base = site_base_url();
    let canonical = format!("{}/posts/{}", base, urlencoding::encode(&article.id));
    let description = extract_description(article);
    let page_title = format!("{} - StaticFlow", article.title);

    let og_image = article
        .featured_image
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|img| {
            if img.starts_with("http") {
                img.to_string()
            } else {
                format!("{}/api/images/{}", base, urlencoding::encode(img))
            }
        })
        .unwrap_or_default();

    let mut html = rewrite_origin_urls(template, &base);

    // <title>
    html = replace_title(&html, &page_title);

    // Standard meta tags
    html = replace_meta_content(&html, "name", "description", &description);

    // Open Graph
    html = replace_meta_content(&html, "property", "og:title", &article.title);
    html = replace_meta_content(&html, "property", "og:description", &description);
    html = replace_meta_content(&html, "property", "og:url", &canonical);
    html = replace_meta_content(&html, "property", "og:type", "article");
    if !og_image.is_empty() {
        html = replace_meta_content(&html, "property", "og:image", &og_image);
    }

    // Twitter Card
    html = replace_meta_content(&html, "name", "twitter:title", &article.title);
    html = replace_meta_content(&html, "name", "twitter:description", &description);
    if !og_image.is_empty() {
        html = replace_meta_content(&html, "name", "twitter:image", &og_image);
    }

    // Canonical
    html = replace_canonical_href(&html, &canonical);

    // JSON-LD before </head>
    let json_ld = build_article_json_ld(article, &canonical, &og_image, &description);
    html = inject_before(&html, "</head>", &format!("\n{}\n", json_ld));

    // Hidden SEO content after <body...>
    let seo_body = build_seo_body_content(article, &description);
    // Find <body or <body ...> tag
    if let Some(body_pos) = html.find("<body") {
        if let Some(gt) = html[body_pos..].find('>') {
            let insert_at = body_pos + gt + 1;
            let before = &html[..insert_at];
            let after = &html[insert_at..];
            html = format!("{}\n{}\n{}", before, seo_body, after);
        }
    }

    html
}

fn build_seo_body_content(article: &Article, description: &str) -> String {
    let plain_content = truncate_text(&strip_markdown(&article.content), 2000);
    // <h1> and <p> must be visible for Bing/Google to index them.
    // Yew replaces <body> on WASM load, so these disappear naturally.
    format!(
        r#"<h1>{title}</h1><p>{desc}</p><div id="seo-content" style="display:none"><article>{content}</article></div>"#,
        title = html_escape(&article.title),
        desc = html_escape(description),
        content = html_escape(&plain_content),
    )
}

fn build_fallback_seo_html(article: &Article) -> String {
    let base = site_base_url();
    let canonical = format!("{}/posts/{}", base, urlencoding::encode(&article.id));
    let description = extract_description(article);
    let page_title = format!("{} - StaticFlow", article.title);
    let og_image = article
        .featured_image
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|img| {
            if img.starts_with("http") {
                img.to_string()
            } else {
                format!("{}/api/images/{}", base, urlencoding::encode(img))
            }
        })
        .unwrap_or_default();

    let json_ld = build_article_json_ld(article, &canonical, &og_image, &description);
    let seo_body = build_seo_body_content(article, &description);

    let mut og_image_tag = String::new();
    if !og_image.is_empty() {
        og_image_tag =
            format!(r#"<meta property="og:image" content="{}" />"#, html_attr_escape(&og_image));
    }

    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8" />
<title>{title}</title>
<meta name="description" content="{desc}" />
<link rel="canonical" href="{canonical}" />
<meta property="og:title" content="{raw_title}" />
<meta property="og:description" content="{desc}" />
<meta property="og:url" content="{canonical}" />
<meta property="og:type" content="article" />
{og_image_tag}
{json_ld}
</head>
<body>
{seo_body}
</body>
</html>"#,
        title = html_escape(&page_title),
        desc = html_attr_escape(&description),
        canonical = html_attr_escape(&canonical),
        raw_title = html_attr_escape(&article.title),
        og_image_tag = og_image_tag,
        json_ld = json_ld,
        seo_body = seo_body,
    )
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /posts/:id — serve SPA HTML with injected SEO meta tags
pub async fn seo_article_page(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let article = match state.store.get_article(&id).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            // Article not found — fall through to SPA (let client-side handle 404)
            if state.index_html_template.is_empty() {
                return (StatusCode::NOT_FOUND, "Not Found").into_response();
            }
            return Html(state.index_html_template.as_ref().clone()).into_response();
        },
        Err(err) => {
            tracing::warn!("SEO page DB error for id={}: {}", id, err);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response();
        },
    };

    let html = inject_article_seo(&state.index_html_template, &article);
    Html(html).into_response()
}

/// GET /sitemap.xml
pub async fn sitemap_xml(State(state): State<AppState>) -> Response {
    let articles = match state.store.list_articles(None, None, None, None).await {
        Ok(resp) => resp.articles,
        Err(err) => {
            tracing::warn!("sitemap: failed to list articles: {}", err);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to generate sitemap")
                .into_response();
        },
    };

    let base = site_base_url();
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
"#,
    );

    // Homepage
    xml.push_str(&format!(
        "  <url>\n    <loc>{}</loc>\n    <changefreq>daily</changefreq>\n    \
         <priority>1.0</priority>\n  </url>\n",
        html_escape(&base)
    ));

    for item in &articles {
        let loc = format!("{}/posts/{}", base, urlencoding::encode(&item.id));
        xml.push_str(&format!(
            "  <url>\n    <loc>{}</loc>\n    <lastmod>{}</lastmod>\n    \
             <changefreq>weekly</changefreq>\n    <priority>0.8</priority>\n  </url>\n",
            html_escape(&loc),
            html_escape(&item.date),
        ));
    }

    xml.push_str("</urlset>\n");

    (StatusCode::OK, [(header::CONTENT_TYPE, "application/xml; charset=utf-8")], xml)
        .into_response()
}

/// GET /robots.txt
pub async fn robots_txt() -> Response {
    let base = site_base_url();
    let body = format!("User-agent: *\nAllow: /\n\nSitemap: {}/sitemap.xml\n", base);
    (StatusCode::OK, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response()
}

// ---------------------------------------------------------------------------
// Homepage SEO: fix canonical/og:url/og:image to match SITE_BASE_URL
// ---------------------------------------------------------------------------

/// Known GitHub Pages origins that should be rewritten to SITE_BASE_URL.
const GITHUB_PAGES_ORIGINS: &[&str] = &["https://acking-you.github.io"];

/// Replace hardcoded GitHub Pages URLs in the template with the configured site
/// URL.
fn rewrite_origin_urls(html: &str, site_base: &str) -> String {
    let mut result = html.to_string();
    for origin in GITHUB_PAGES_ORIGINS {
        result = result.replace(origin, site_base);
    }
    result
}

/// GET / — serve homepage with corrected SEO URLs and visible <h1>
pub async fn seo_homepage(State(state): State<AppState>) -> Response {
    let base = site_base_url();
    let mut html = rewrite_origin_urls(&state.index_html_template, &base);

    // Inject visible <h1> after <body> for search engine indexing.
    // Yew replaces <body> on WASM load, so this disappears naturally.
    if let Some(body_pos) = html.find("<body") {
        if let Some(gt) = html[body_pos..].find('>') {
            let insert_at = body_pos + gt + 1;
            let h1 = "\n<h1>StaticFlow \u{00b7} AI + Skill \
                      \u{9a71}\u{52a8}\u{7684}\u{672c}\u{5730}\u{4f18}\u{5148}\u{6280}\u{672f}\\
                      u{535a}\u{5ba2}</h1>\n";
            html.insert_str(insert_at, h1);
        }
    }

    Html(html).into_response()
}
