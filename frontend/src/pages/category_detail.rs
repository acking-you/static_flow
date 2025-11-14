use yew::prelude::*;

use crate::{
    models::get_mock_articles,
    pages::posts::{group_articles_by_year, render_timeline},
};

#[derive(Properties, Clone, PartialEq)]
pub struct CategoryDetailProps {
    pub category: String,
}

#[function_component(CategoryDetailPage)]
pub fn category_detail_page(props: &CategoryDetailProps) -> Html {
    let normalized = props.category.trim().to_string();
    let filter_value = if normalized.is_empty() { None } else { Some(normalized) };
    let display_category = filter_value
        .clone()
        .unwrap_or_else(|| "未命名分类".to_string());

    let articles = use_memo((), |_| {
        let mut list = get_mock_articles();
        list.sort_by(|a, b| b.date.cmp(&a.date));
        list
    });

    let mut filtered = (*articles).clone();
    if let Some(category_value) = filter_value.as_ref() {
        let category_lower = category_value.to_lowercase();
        filtered.retain(|article| article.category.to_lowercase() == category_lower);
    } else {
        filtered.clear();
    }

    let total_posts = filtered.len();
    let grouped_by_year = group_articles_by_year(&filtered);

    let description = if let Some(category_value) = filter_value.as_ref() {
        if total_posts == 0 {
            format!("分类“{}”下暂时没有文章。", category_value)
        } else {
            format!("该分类共收录 {} 篇文章。", total_posts)
        }
    } else {
        "未提供有效分类，无法展示对应文章。".to_string()
    };

    let empty_message = if let Some(category_value) = filter_value.as_ref() {
        format!("分类“{}”下暂无文章，换个分类看看？", category_value)
    } else {
        "请输入有效的分类名称。".to_string()
    };

    html! {
        <main class="main category-detail-page">
            <div class="container">
                <div class="page archive">
                    <p class="page-kicker">{ "Categories" }</p>
                    <h1 class="single-title">{ format!("分类: {}", display_category) }</h1>
                    <p class="page-description">{ description }</p>

                    {
                        if grouped_by_year.is_empty() {
                            html! { <p class="timeline-empty">{ empty_message }</p> }
                        } else {
                            render_timeline(&grouped_by_year)
                        }
                    }
                </div>
            </div>
        </main>
    }
}
