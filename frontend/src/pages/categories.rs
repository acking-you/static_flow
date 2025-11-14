use std::collections::BTreeMap;

use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{models::get_mock_articles, router::Route};

const CATEGORY_DESCRIPTIONS: &[(&str, &str)] = &[
    ("Rust", "静态类型、零成本抽象与 Wasm 生态的实战笔记。"),
    ("Web", "现代前端工程化与体验设计相关内容。"),
    ("DevOps", "自动化、流水线与交付体验的工程思考。"),
    ("Productivity", "效率、写作与自我管理的小实验与道具。"),
    ("AI", "Prompt、LLM 与智能体的落地探索。"),
];

#[derive(Clone, PartialEq)]
struct CategoryStat {
    name: String,
    count: usize,
    description: String,
}

#[function_component(CategoriesPage)]
pub fn categories_page() -> Html {
    let categories = use_memo((), |_| {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for article in get_mock_articles() {
            *counts.entry(article.category).or_insert(0) += 1;
        }

        counts
            .into_iter()
            .map(|(name, count)| {
                let description = CATEGORY_DESCRIPTIONS
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, desc)| desc.to_string())
                    .unwrap_or_else(|| "更多分类即将上线。".to_string());

                CategoryStat {
                    name,
                    count,
                    description,
                }
            })
            .collect::<Vec<CategoryStat>>()
    });

    html! {
        <main class="main categories-page">
            <div class="container">
                <section class="page-section">
                    <p class="page-kicker">{ "分类" }</p>
                    <h1 class="page-title">{ "知识图谱" }</h1>
                    <p class="page-description">
                        { format!("当前整理 {} 个分类，持续更新中。点击卡片跳转到分类详情页并查看文章时间线。", categories.len()) }
                    </p>
                </section>

                {
                    if categories.is_empty() {
                        html! {
                            <p class="empty-hint">{ "暂无分类" }</p>
                        }
                    } else {
                        html! {
                            <section class="category-grid" aria-label="分类列表">
                                { for categories.iter().map(|category| {
                                    html! {
                                        <Link<Route>
                                            to={Route::CategoryDetail { category: category.name.clone() }}
                                            classes={classes!("category-card")}
                                        >
                                            <div class="category-card-body">
                                                <p class="category-name">{ &category.name }</p>
                                                <p class="category-description">{ &category.description }</p>
                                            </div>
                                            <span class="category-count">
                                                { format!("{} 篇", category.count) }
                                            </span>
                                        </Link<Route>>
                                    }
                                }) }
                            </section>
                        }
                    }
                }
            </div>
        </main>
    }
}
