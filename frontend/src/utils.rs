use pulldown_cmark::{html, CowStr, Event, Options, Parser, Tag};
use url::Url;

use crate::api::API_BASE;


/// Convert image path to API endpoint if it's a relative path
pub fn image_url(path: &str) -> String {
    let normalized = path.trim();

    if normalized.starts_with("http://")
        || normalized.starts_with("https://")
        || normalized.starts_with("data:")
    {
        normalized.to_string()
    } else if normalized.starts_with("images/") {
        let filename = normalized.strip_prefix("images/").unwrap_or(normalized);
        format!("{}/images/{}", API_BASE, filename)
    } else if normalized.starts_with("/api/images/") {
        format!("{}{}", API_BASE.trim_end_matches("/api"), normalized)
    } else {
        normalized.to_string()
    }
}

/// Convert Markdown content into HTML with common extensions enabled.
/// Also transforms relative image paths to API endpoints.
pub fn markdown_to_html(content: &str) -> String {
    if content.trim().is_empty() {
        return String::new();
    }

    // Protect display-math blocks before markdown parsing so `=` lines inside
    // formulas are not interpreted as Setext headings.
    let normalized_content = protect_display_math_blocks(content);
    let normalized_content = normalize_standalone_break_markers(&normalized_content);

    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_SMART_PUNCTUATION);

    let parser = Parser::new_ext(&normalized_content, options);

    // Transform image paths
    let transformed_parser = parser.map(|event| match event {
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) => {
            // Check if image path is relative (starts with "images/")
            let new_url = CowStr::from(image_url(&dest_url));
            Event::Start(Tag::Image {
                link_type,
                dest_url: new_url,
                title,
                id,
            })
        },
        _ => event,
    });

    let mut html_output = String::new();
    html::push_html(&mut html_output, transformed_parser);
    html_output
}

fn normalize_standalone_break_markers(content: &str) -> String {
    let mut result = String::new();
    let mut in_fenced_code = false;
    let mut active_fence = "";

    for line in content.lines() {
        let trimmed = line.trim();

        let fence_marker = if trimmed.starts_with("```") {
            Some("```")
        } else if trimmed.starts_with("~~~") {
            Some("~~~")
        } else {
            None
        };

        if let Some(marker) = fence_marker {
            if in_fenced_code && active_fence == marker {
                in_fenced_code = false;
                active_fence = "";
            } else if !in_fenced_code {
                in_fenced_code = true;
                active_fence = marker;
            }

            result.push_str(line);
            result.push('\n');
            continue;
        }

        if !in_fenced_code && matches!(trimmed, "<br>" | "<br/>" | "<br />") {
            result.push('\n');
            continue;
        }

        result.push_str(line);
        result.push('\n');
    }

    if !content.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }

    result
}

#[derive(Clone, Debug)]
struct MarkdownExportBase {
    origin: String,
    page_url: Url,
    root_url: Url,
}

/// Convert relative markdown resource paths into absolute URLs for external
/// export/copy.
///
/// This is used by "View Raw Markdown" and "Copy/Export Markdown" to keep links
/// and image resources valid after markdown is copied out of the site context.
pub fn markdown_for_external_export(markdown: &str) -> String {
    let Some(window) = web_sys::window() else {
        return markdown.to_string();
    };
    let location = window.location();
    let page_href = match location.href() {
        Ok(value) => value,
        Err(_) => return markdown.to_string(),
    };
    let origin = location.origin().ok();
    markdown_for_external_export_with_base(markdown, &page_href, origin.as_deref())
}

fn markdown_for_external_export_with_base(
    markdown: &str,
    page_href: &str,
    origin_hint: Option<&str>,
) -> String {
    let Some(base) = build_markdown_export_base(page_href, origin_hint) else {
        return markdown.to_string();
    };

    let with_inline = rewrite_inline_markdown_links(markdown, &base);
    let with_references = rewrite_reference_markdown_links(&with_inline, &base);
    rewrite_html_src_href_attributes(&with_references, &base)
}

fn build_markdown_export_base(
    page_href: &str,
    origin_hint: Option<&str>,
) -> Option<MarkdownExportBase> {
    let page_url = Url::parse(page_href).ok()?;
    let origin = origin_hint
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_string())
        .unwrap_or_else(|| page_url.origin().ascii_serialization());
    let root_url = Url::parse(&format!("{origin}/")).ok()?;

    Some(MarkdownExportBase {
        origin,
        page_url,
        root_url,
    })
}

fn rewrite_inline_markdown_links(input: &str, base: &MarkdownExportBase) -> String {
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len() + 64);
    let mut cursor = 0usize;

    while let Some(rel_idx) = input[cursor..].find("](") {
        let marker = cursor + rel_idx;
        output.push_str(&input[cursor..marker + 2]);

        let mut i = marker + 2;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        output.push_str(&input[marker + 2..i]);

        if i >= bytes.len() {
            cursor = i;
            break;
        }

        let (target, after_target, wrapped_in_angles) = if bytes[i] == b'<' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != b'>' {
                j += 1;
            }
            if j >= bytes.len() {
                output.push_str(&input[i..]);
                cursor = bytes.len();
                break;
            }
            (&input[i + 1..j], j + 1, true)
        } else {
            let mut j = i;
            let mut nested_parens = 0i32;
            while j < bytes.len() {
                let b = bytes[j];
                if b == b'\\' {
                    j = (j + 2).min(bytes.len());
                    continue;
                }
                if b == b'(' {
                    nested_parens += 1;
                    j += 1;
                    continue;
                }
                if b == b')' {
                    if nested_parens == 0 {
                        break;
                    }
                    nested_parens -= 1;
                    j += 1;
                    continue;
                }
                if b.is_ascii_whitespace() && nested_parens == 0 {
                    break;
                }
                j += 1;
            }
            (&input[i..j], j, false)
        };

        let resolved = resolve_markdown_target(target, base);
        if wrapped_in_angles {
            output.push('<');
            output.push_str(&resolved);
            output.push('>');
        } else {
            output.push_str(&resolved);
        }

        let suffix_start = after_target;
        let mut j = after_target;
        let mut in_single_quote = false;
        let mut in_double_quote = false;

        while j < bytes.len() {
            let b = bytes[j];
            if b == b'\\' {
                j = (j + 2).min(bytes.len());
                continue;
            }
            if b == b'"' && !in_single_quote {
                in_double_quote = !in_double_quote;
                j += 1;
                continue;
            }
            if b == b'\'' && !in_double_quote {
                in_single_quote = !in_single_quote;
                j += 1;
                continue;
            }
            if b == b')' && !in_single_quote && !in_double_quote {
                break;
            }
            j += 1;
        }

        output.push_str(&input[suffix_start..j]);
        if j < bytes.len() && bytes[j] == b')' {
            output.push(')');
            j += 1;
        }
        cursor = j;
    }

    output.push_str(&input[cursor..]);
    output
}

fn rewrite_reference_markdown_links(input: &str, base: &MarkdownExportBase) -> String {
    let mut output = String::with_capacity(input.len() + 32);

    for segment in input.split_inclusive('\n') {
        let (line, suffix) = if let Some(stripped) = segment.strip_suffix('\n') {
            (stripped, "\n")
        } else {
            (segment, "")
        };
        output.push_str(&rewrite_reference_link_line(line, base));
        output.push_str(suffix);
    }

    output
}

fn rewrite_reference_link_line(line: &str, base: &MarkdownExportBase) -> String {
    let trimmed_start = line.trim_start();
    if !trimmed_start.starts_with('[') {
        return line.to_string();
    }

    let Some(def_idx_rel) = trimmed_start.find("]:") else {
        return line.to_string();
    };

    let leading_ws_len = line.len() - trimmed_start.len();
    let prefix_end = leading_ws_len + def_idx_rel + 2;
    let bytes = line.as_bytes();

    let mut i = prefix_end;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= bytes.len() {
        return line.to_string();
    }

    let (target_start, target_end, wrapped_in_angles) = if bytes[i] == b'<' {
        let mut j = i + 1;
        while j < bytes.len() && bytes[j] != b'>' {
            j += 1;
        }
        if j >= bytes.len() {
            return line.to_string();
        }
        (i + 1, j, true)
    } else {
        let mut j = i;
        while j < bytes.len() && !bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        (i, j, false)
    };

    let raw_target = &line[target_start..target_end];
    let resolved = resolve_markdown_target(raw_target, base);
    let mut rewritten = String::with_capacity(line.len() + 32);
    rewritten.push_str(&line[..target_start]);
    if wrapped_in_angles {
        rewritten.push_str(&resolved);
        rewritten.push('>');
    } else {
        rewritten.push_str(&resolved);
    }
    rewritten.push_str(&line[target_end + usize::from(wrapped_in_angles)..]);
    rewritten
}

fn rewrite_html_src_href_attributes(input: &str, base: &MarkdownExportBase) -> String {
    let step1 = rewrite_html_attribute_urls(input, "href=\"", "\"", base);
    let step2 = rewrite_html_attribute_urls(&step1, "href='", "'", base);
    let step3 = rewrite_html_attribute_urls(&step2, "src=\"", "\"", base);
    rewrite_html_attribute_urls(&step3, "src='", "'", base)
}

fn rewrite_html_attribute_urls(
    input: &str,
    pattern: &str,
    quote: &str,
    base: &MarkdownExportBase,
) -> String {
    let mut output = String::with_capacity(input.len() + 32);
    let mut cursor = 0usize;

    while let Some(rel_idx) = input[cursor..].find(pattern) {
        let start = cursor + rel_idx;
        let value_start = start + pattern.len();
        output.push_str(&input[cursor..value_start]);

        let Some(end_rel) = input[value_start..].find(quote) else {
            output.push_str(&input[value_start..]);
            cursor = input.len();
            break;
        };

        let value_end = value_start + end_rel;
        let raw_target = &input[value_start..value_end];
        output.push_str(&resolve_markdown_target(raw_target, base));
        cursor = value_end;
    }

    output.push_str(&input[cursor..]);
    output
}

fn resolve_markdown_target(raw_target: &str, base: &MarkdownExportBase) -> String {
    let target = raw_target.trim();
    if target.is_empty() || is_non_http_target(target) {
        return raw_target.to_string();
    }

    if target.starts_with("/api/images/") {
        return format!("{}{}", base.origin, target);
    }
    if let Some(filename) = target.strip_prefix("images/") {
        return format!("{}/api/images/{}", base.origin, filename);
    }
    if target.starts_with('/') {
        return format!("{}{}", base.origin, target);
    }

    let join_base = if target.starts_with("./") || target.starts_with("../") {
        &base.page_url
    } else {
        &base.root_url
    };

    match join_base.join(target) {
        Ok(url) => url.to_string(),
        Err(_) => raw_target.to_string(),
    }
}

fn is_non_http_target(target: &str) -> bool {
    target.starts_with('#')
        || target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("mailto:")
        || target.starts_with("tel:")
        || target.starts_with("data:")
        || target.starts_with("javascript:")
        || target.starts_with("ftp://")
        || target.starts_with("//")
}

fn protect_display_math_blocks(content: &str) -> String {
    let mut result = String::new();
    let mut in_fenced_code = false;
    let mut active_fence = "";

    let mut in_math_block = false;
    let mut math_close_delimiter = "";
    let mut math_lines: Vec<String> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        if !in_math_block {
            let fence_marker = if trimmed.starts_with("```") {
                Some("```")
            } else if trimmed.starts_with("~~~") {
                Some("~~~")
            } else {
                None
            };

            if let Some(marker) = fence_marker {
                if in_fenced_code && active_fence == marker {
                    in_fenced_code = false;
                    active_fence = "";
                } else if !in_fenced_code {
                    in_fenced_code = true;
                    active_fence = marker;
                }

                result.push_str(line);
                result.push('\n');
                continue;
            }

            if !in_fenced_code {
                if line.starts_with("$$") {
                    in_math_block = true;
                    math_close_delimiter = "$$";
                    math_lines.push(line.to_string());

                    // Open + close on the same line, e.g. `$$E=mc^2$$`.
                    if line.matches("$$").count() >= 2 {
                        append_math_block(&mut result, &math_lines);
                        math_lines.clear();
                        in_math_block = false;
                        math_close_delimiter = "";
                    }
                    continue;
                }

                if line.starts_with("\\[") {
                    in_math_block = true;
                    math_close_delimiter = "\\]";
                    math_lines.push(line.to_string());

                    if let (Some(start), Some(end)) = (line.find("\\["), line.rfind("\\]")) {
                        if end > start {
                            append_math_block(&mut result, &math_lines);
                            math_lines.clear();
                            in_math_block = false;
                            math_close_delimiter = "";
                        }
                    }
                    continue;
                }
            }

            result.push_str(line);
            result.push('\n');
            continue;
        }

        math_lines.push(line.to_string());
        let should_close = match math_close_delimiter {
            "$$" => line.contains("$$"),
            "\\]" => line.contains("\\]"),
            _ => false,
        };

        if should_close {
            append_math_block(&mut result, &math_lines);
            math_lines.clear();
            in_math_block = false;
            math_close_delimiter = "";
        }
    }

    if in_math_block {
        for line in math_lines {
            result.push_str(&line);
            result.push('\n');
        }
    }

    if !content.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }

    result
}

fn append_math_block(result: &mut String, math_lines: &[String]) {
    if !result.is_empty() && !result.ends_with('\n') {
        result.push('\n');
    }

    let math_text = math_lines.join("\n");
    result.push_str("<div class=\"sf-math-block\">\n");
    result.push_str(&escape_html_text(&math_text));
    result.push_str("\n</div>\n");
}

fn escape_html_text(content: &str) -> String {
    let mut escaped = String::with_capacity(content.len());
    for ch in content.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::{markdown_for_external_export_with_base, markdown_to_html};

    #[test]
    fn display_math_block_with_equals_is_not_promoted_to_heading() {
        let markdown = r#"矩阵乘法：

$$
\begin{bmatrix}
a & b \\
c & d
\end{bmatrix}
\begin{bmatrix}
x \\
y
\end{bmatrix}
=
\begin{bmatrix}
ax + by \\
cx + dy
\end{bmatrix}
$$

欧拉公式（数学中最美的公式之一）。"#;

        let html = markdown_to_html(markdown);

        assert!(html.contains("<div class=\"sf-math-block\">"));
        assert!(html.contains("欧拉公式（数学中最美的公式之一）。"));
        assert!(!html.contains("<h1>$$"));
    }

    #[test]
    fn export_markdown_absolutizes_links_images_and_refs() {
        let markdown = r#"
![img](images/a.png)
[post](/posts/abc)
[doc](docs/spec)
[rel](../x/y)
[ref-link]: article/foo "title"
<a href="posts/hello">x</a>
"#;

        let rewritten = markdown_for_external_export_with_base(
            markdown,
            "https://ackingliu.top/posts/current?x=1",
            Some("https://ackingliu.top"),
        );

        assert!(rewritten.contains("![img](https://ackingliu.top/api/images/a.png)"));
        assert!(rewritten.contains("[post](https://ackingliu.top/posts/abc)"));
        assert!(rewritten.contains("[doc](https://ackingliu.top/docs/spec)"));
        assert!(rewritten.contains("[rel](https://ackingliu.top/x/y)"));
        assert!(rewritten.contains("[ref-link]: https://ackingliu.top/article/foo \"title\""));
        assert!(rewritten.contains("<a href=\"https://ackingliu.top/posts/hello\">x</a>"));
    }

    #[test]
    fn standalone_break_markers_become_paragraph_boundaries() {
        let markdown = "第一段\n<br/>\n第二段";
        let html = markdown_to_html(markdown);
        assert!(html.contains("<p>第一段</p>"));
        assert!(html.contains("<p>第二段</p>"));
        assert!(!html.contains("<br"));
    }

    #[test]
    fn standalone_break_marker_inside_code_fence_is_preserved_as_code_text() {
        let markdown = "```text\nline1\n<br/>\nline2\n```";
        let html = markdown_to_html(markdown);
        assert!(html.contains("&lt;br/&gt;"));
    }
}
