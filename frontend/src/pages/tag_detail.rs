use yew::prelude::*;

use crate::{
    models::get_mock_articles,
    pages::posts::{group_articles_by_year, render_timeline},
};

#[derive(Properties, Clone, PartialEq)]
pub struct TagDetailProps {
    pub tag: String,
}

#[function_component(TagDetailPage)]
pub fn tag_detail_page(props: &TagDetailProps) -> Html {
    let normalized = props.tag.trim().to_string();
    let filter_value = if normalized.is_empty() { None } else { Some(normalized) };
    let display_tag = filter_value
        .clone()
        .unwrap_or_else(|| "未命名标签".to_string());

    let articles = use_memo((), |_| {
        let mut list = get_mock_articles();
        list.sort_by(|a, b| b.date.cmp(&a.date));
        list
    });

    let mut filtered = (*articles).clone();
    if let Some(tag_value) = filter_value.as_ref() {
        let tag_lower = tag_value.to_lowercase();
        filtered.retain(|article| article.tags.iter().any(|t| t.to_lowercase() == tag_lower));
    } else {
        filtered.clear();
    }

    let total_posts = filtered.len();
    let grouped_by_year = group_articles_by_year(&filtered);

    let description = if let Some(tag_value) = filter_value.as_ref() {
        if total_posts == 0 {
            format!("标签“{}”下暂时没有文章。", tag_value)
        } else {
            format!("该标签共收录 {} 篇文章。", total_posts)
        }
    } else {
        "未提供有效标签，无法展示对应文章。".to_string()
    };

    let empty_message = if let Some(tag_value) = filter_value.as_ref() {
        format!("标签“{}”下暂无文章，换个标签看看？", tag_value)
    } else {
        "请输入有效的标签名称。".to_string()
    };

    html! {
        <main class="main tag-detail-page">
            <div class="container">
                <div class="page archive">
                    <p class="page-kicker">{ "Tags" }</p>
                    <h1 class="single-title">{ format!("标签: {}", display_tag) }</h1>
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
