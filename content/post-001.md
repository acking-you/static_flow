---
title: "示例文章 1 - Rust 技术与思考"
summary: "这是一篇关于 Rust 的示例文章，涵盖实践要点与思考。"
tags: ["rust", "wasm", "yew"]
category: "Rust"
author: "Bob"
date: "2024-01-11"
featured_image: "images/wallhaven-5yyyw9.png"
read_time: 4
---

# 用 Rust + Yew 构建本地优先博客

StaticFlow 是一个本地优先（Local-first）、自动化驱动的博客样板项目。

![技术架构示意图](images/wallhaven-5yyyw9.png)

## 亮点

- 无后端依赖，纯静态部署
- 使用 `Yew` 构建前端组件
- 基于 `Trunk` 与 `wasm-pack` 的开发体验

```rust
fn main() {
    println!("Hello StaticFlow!");
}
sdfasf
dsafsda
dfsafasd
fdasfas
dfasf
asdfff
asdfsaf
dsafdasf
asdfasdsd
asdffsdaf
dasfsdaf
asdffsdaf
asdffsdaf
asdffsdf
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


> 小贴士：保持组件小而清晰。
