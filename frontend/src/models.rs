// 重新导出shared crate的数据模型
#[cfg(feature = "mock")]
use std::collections::HashMap;

#[cfg_attr(not(feature = "mock"), allow(unused_imports))]
pub use static_flow_shared::{Article, ArticleListItem};

#[cfg(feature = "mock")]
use crate::api::{CategoryInfo, SearchResult, TagInfo};

// =============== Mock 数据 ===============

/// 返回 15 篇文章的列表（ArticleListItem）。
#[cfg(feature = "mock")]
pub fn get_mock_articles() -> Vec<ArticleListItem> {
    mock_articles_full()
        .into_iter()
        .map(ArticleListItem::from)
        .collect()
}

/// 返回完整文章详情。包含 3-5 篇带 Markdown 正文内容的文章。
#[cfg(feature = "mock")]
pub fn get_mock_article_detail(id: &str) -> Option<Article> {
    mock_articles_full().into_iter().find(|a| a.id == id)
}

// 内部函数：构建 15 篇完整文章
#[cfg(feature = "mock")]
fn mock_articles_full() -> Vec<Article> {
    let md_samples = sample_markdowns();

    // 为了真实多样性，混合不同分类/标签/作者
    let categories = [
        ("Rust", "rust"),
        ("Web", "web"),
        ("DevOps", "devops"),
        ("Productivity", "productivity"),
        ("AI", "ai"),
    ];

    let authors = ["Alice", "Bob", "Carol", "Dave"]; // 简单轮换

    let mut items: Vec<Article> = Vec::new();
    for i in 1..=15 {
        let idx = (i as usize) % categories.len();
        let (cat_name, _slug) = categories[idx];

        let tags = match cat_name {
            "Rust" => vec!["rust".to_string(), "wasm".to_string(), "yew".to_string()],
            "Web" => vec!["frontend".to_string(), "html".to_string(), "css".to_string()],
            "DevOps" => vec!["docker".to_string(), "ci".to_string(), "cd".to_string()],
            "Productivity" => {
                vec!["workflow".to_string(), "typing".to_string(), "shortcuts".to_string()]
            },
            "AI" => vec!["llm".to_string(), "prompt".to_string(), "nlp".to_string()],
            _ => vec!["misc".to_string()],
        };

        let author = authors[(i as usize) % authors.len()].to_string();
        let id = format!("post-{:03}", i);
        let title = format!("示例文章 {} - {} 技术与思考", i, cat_name);
        let summary = format!("这是一篇关于 {} 的示例文章，涵盖实践要点与思考。", cat_name);
        let date = format!("2024-{:02}-{:02}", (i % 12).max(1), (10 + i % 18));
        let read_time = 3 + (i as u32 % 8);

        // 为 1..=5 的文章提供更完整的 Markdown 正文，其他给简短正文
        let content = if (1..=5).contains(&i) {
            md_samples[((i - 1) as usize) % md_samples.len()].to_string()
        } else {
            format!(
                "# {}\n\n> 简介\n\n本文简要介绍 {} 的关键概念与实践建议。\n\n- 要点一\n- \
                 要点二\n\n更多内容将在后续更新中补充。",
                title, cat_name
            )
        };

        let featured_image = if i % 3 == 0 {
            Some(crate::config::asset_path(&format!("static/hero-{}.jpg", i % 5 + 1)))
        } else {
            None
        };

        items.push(Article {
            id,
            title,
            summary,
            content,
            tags,
            category: cat_name.to_string(),
            author,
            date,
            featured_image,
            read_time,
        });
    }

    items
}

#[cfg(feature = "mock")]
fn sample_markdowns() -> Vec<&'static str> {
    vec![
        // 1
        r#"# 用 Rust + Yew 构建本地优先博客

StaticFlow 是一个本地优先（Local-first）、自动化驱动的博客样板项目。

## 亮点

- 无后端依赖，纯静态部署
- 使用 `Yew` 构建前端组件
- 基于 `Trunk` 与 `wasm-pack` 的开发体验

```rust
fn main() {
    println!("Hello StaticFlow!");
}
```
```mermaid
graph TD
    A[编写代码] --> B[构建 WASM]
    B --> C[部署静态文件]
    C --> D[用户访问]
    D --> A
```
你好，世界！

$$E = mc^2$$


```mermaid
classDiagram
    class Article {
        +String id
        +String title
        +String content
        +Vec~String~ tags
        +String category
        +DateTime created_at
        +render() Html
        +to_json() String
    }
    
    class ArticleListItem {
        +String id
        +String title
        +String summary
        +Vec~String~ tags
        +from(Article) ArticleListItem
    }
    
    class Tag {
        +String name
        +String slug
        +count() usize
    }
    
    Article "1" --> "*" Tag : has
    ArticleListItem <|-- Article : derives from
```


> 小贴士：保持组件小而清晰。"#,
        // 2
        r#"# Web 前端工程化的三个关键维度

在现代前端中，我们通常从以下维度进行工程化：

1. 构建与打包
2. 质量保障（Lint/Format/Test）
3. 交付与运维

## 开发流程

```mermaid
graph LR
    A[编写代码] --> B[Lint检查]
    B --> C[单元测试]
    C --> D[集成测试]
    D --> E[构建打包]
    E --> F[部署上线]
    F --> G[监控反馈]
    G --> A
```

## 技术栈对比

| 特性 | Webpack | Vite | Trunk |
|------|---------|------|-------|
| 语言 | JavaScript | JavaScript | Rust |
| 启动速度 | 慢 | 快 | 快 |
| HMR | 支持 | 支持 | 支持 |
| 生态 | 成熟 | 快速增长 | 新兴 |

> 实战中，请优先考虑开发者体验（DX）。"#,
        // 3
        r#"# DevOps：让交付流畅与可恢复

持续集成（CI）与持续交付（CD）并非目的，而是代价可控的**反馈回路**：

- 自动化测试覆盖关键路径
- 构建产物可追溯
- 回滚策略与演练

_自动化不是银弹，但值得持续投资。_"#,
        // 4
        r#"# 写作与生产力：短句与列表

短句与列表让阅读更轻：

- 主题先行
- 例子优先
- 结论明确

> 写给读者，也写给未来的自己。"#,
        // 5
        r#"# AI 与提示工程：从问题出发

优秀的 Prompt 往往具备以下特征：

- 明确目标与约束
- 提供上下文与边界
- 指定输出结构

```text
角色：代码审查助手
目标：找出可读性问题并给出例子
约束：不改变行为，仅改善表达
```

> 先把问题描述清楚，AI 的帮助才更稳定。"#,
    ]
}

/// 模拟标签统计
#[cfg(feature = "mock")]
pub fn mock_tags() -> Vec<TagInfo> {
    let mut counts: HashMap<String, usize> = HashMap::new();

    for article in mock_articles_full() {
        for tag in article.tags {
            *counts.entry(tag).or_insert(0) += 1;
        }
    }

    let mut tags: Vec<TagInfo> = counts
        .into_iter()
        .map(|(name, count)| TagInfo {
            name,
            count,
        })
        .collect();
    tags.sort_by(|a, b| a.name.cmp(&b.name));
    tags
}

/// 模拟分类统计，包含描述
#[cfg(feature = "mock")]
pub fn mock_categories() -> Vec<CategoryInfo> {
    // 与后端保持一致的描述
    let descriptions: HashMap<&str, &str> = [
        ("Rust", "静态类型、零成本抽象与 Wasm 生态的实战笔记。"),
        ("Web", "现代前端工程化与体验设计相关内容。"),
        ("DevOps", "自动化、流水线与交付体验的工程思考。"),
        ("Productivity", "效率、写作与自我管理的小实验与道具。"),
        ("AI", "Prompt、LLM 与智能体的落地探索。"),
    ]
    .into_iter()
    .collect();

    let mut counts: HashMap<String, usize> = HashMap::new();
    for article in mock_articles_full() {
        *counts.entry(article.category).or_insert(0) += 1;
    }

    let mut categories: Vec<CategoryInfo> = counts
        .into_iter()
        .map(|(name, count)| {
            let description = descriptions
                .get(name.as_str())
                .copied()
                .unwrap_or("")
                .to_string();
            CategoryInfo {
                name,
                count,
                description,
            }
        })
        .collect();

    categories.sort_by(|a, b| a.name.cmp(&b.name));
    categories
}

/// 模拟搜索结果
#[cfg(feature = "mock")]
pub fn mock_search(keyword: &str) -> Vec<SearchResult> {
    let trimmed = keyword.trim();
    if trimmed.is_empty() {
        return vec![];
    }

    let keyword_lower = trimmed.to_lowercase();

    mock_articles_full()
        .into_iter()
        .filter(|article| {
            article.title.to_lowercase().contains(&keyword_lower)
                || article.summary.to_lowercase().contains(&keyword_lower)
        })
        .map(|article| {
            let highlight_html = highlight_snippet(trimmed, &article.summary);
            SearchResult {
                id: article.id,
                title: article.title,
                summary: article.summary,
                category: article.category,
                date: article.date,
                highlight: highlight_html,
                tags: article.tags,
            }
        })
        .collect()
}

#[cfg(feature = "mock")]
fn highlight_snippet(keyword: &str, summary: &str) -> String {
    if keyword.is_empty() {
        return format!("<p>{}</p>", summary);
    }

    let highlighted = summary.replacen(keyword, &format!("<mark>{}</mark>", keyword), 1);

    if highlighted == summary {
        format!("<p>{} <mark>{}</mark></p>", summary, keyword)
    } else {
        format!("<p>{}</p>", highlighted)
    }
}
