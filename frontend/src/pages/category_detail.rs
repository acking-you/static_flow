use yew::prelude::*;
use static_flow_shared::ArticleListItem;

use crate::{
    components::{
        loading_spinner::{LoadingSpinner, SpinnerSize},
        scroll_to_top_button::ScrollToTopButton,
    },
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

    let articles = use_state(|| Vec::<ArticleListItem>::new());
    let loading = use_state(|| true);

    {
        let articles = articles.clone();
        let category = filter_value.clone();
        let loading = loading.clone();
        use_effect_with(category.clone(), move |_| {
            loading.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                let category_ref = category.as_deref();
                match crate::api::fetch_articles(None, category_ref).await {
                    Ok(data) => {
                        articles.set(data);
                        loading.set(false);
                    }
                    Err(e) => {
                        web_sys::console::error_1(&format!("Failed to fetch articles: {}", e).into());
                        loading.set(false);
                    }
                }
            });
            || ()
        });
    }

    let filtered = (*articles).clone();

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
                        if *loading {
                            html! {
                                <div class="flex min-h-[40vh] items-center justify-center">
                                    <LoadingSpinner size={SpinnerSize::Large} />
                                </div>
                            }
                        } else if grouped_by_year.is_empty() {
                            html! { <p class="timeline-empty">{ empty_message }</p> }
                        } else {
                            render_timeline(&grouped_by_year)
                        }
                    }
                </div>
            </div>
            <ScrollToTopButton />
        </main>
    }
}
