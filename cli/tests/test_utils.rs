#[cfg(test)]
mod tests {
    use std::io::Write;

    use sf_cli::utils;
    use tempfile::NamedTempFile;

    #[test]
    fn parse_markdown_reads_frontmatter_and_content() {
        let mut file = NamedTempFile::new().expect("create temp file");
        let markdown = r#"---
title: "Hello"
summary: "Short summary"
content_en: |
  # English Heading
  English body.
detailed_summary:
  zh: "中文测试总结"
  en: "English test summary"
tags:
  - rust
  - wasm
category: "Tech"
author: "Ada"
date: "2024-01-01"
featured_image: "hero.jpg"
read_time: 3
---

# Heading

Body content.
"#;
        file.write_all(markdown.as_bytes()).expect("write markdown");
        file.flush().expect("flush markdown");

        let content = std::fs::read_to_string(file.path()).expect("read markdown");
        let (frontmatter, body) = utils::parse_markdown(&content).expect("parse markdown");

        assert_eq!(frontmatter.title.as_deref(), Some("Hello"));
        assert_eq!(frontmatter.summary.as_deref(), Some("Short summary"));
        assert!(frontmatter
            .content_en
            .as_deref()
            .unwrap_or_default()
            .contains("English Heading"));
        assert_eq!(
            frontmatter
                .detailed_summary
                .as_ref()
                .and_then(|value| value.zh.as_deref()),
            Some("中文测试总结")
        );
        assert_eq!(
            frontmatter
                .detailed_summary
                .as_ref()
                .and_then(|value| value.en.as_deref()),
            Some("English test summary")
        );
        assert_eq!(frontmatter.tags, Some(vec!["rust".to_string(), "wasm".to_string()]));
        assert_eq!(frontmatter.category.as_deref(), Some("Tech"));
        assert_eq!(frontmatter.author.as_deref(), Some("Ada"));
        assert_eq!(frontmatter.date.as_deref(), Some("2024-01-01"));
        assert_eq!(frontmatter.featured_image.as_deref(), Some("hero.jpg"));
        assert_eq!(frontmatter.read_time, Some(3));
        assert!(body.contains("# Heading"));
        assert!(body.contains("Body content."));
    }

    #[test]
    fn parse_tags_trims_and_filters_empty() {
        let tags = utils::parse_tags(" rust, wasm, ,backend ,,");
        assert_eq!(tags, vec!["rust".to_string(), "wasm".to_string(), "backend".to_string()]);
    }

    #[test]
    fn estimate_read_time_uses_minute_rounding() {
        let short = utils::estimate_read_time("word");
        assert_eq!(short, 1);

        let words = std::iter::repeat_n("word", 201)
            .collect::<Vec<_>>()
            .join(" ");
        let rounded = utils::estimate_read_time(&words);
        assert_eq!(rounded, 2);
    }

    #[test]
    fn hash_bytes_matches_sha256() {
        let hash = utils::hash_bytes(b"hello");
        assert_eq!(hash, "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    }
}
