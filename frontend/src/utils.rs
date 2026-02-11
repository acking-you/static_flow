use pulldown_cmark::{html, CowStr, Event, Options, Parser, Tag};

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
    use super::markdown_to_html;

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
}
