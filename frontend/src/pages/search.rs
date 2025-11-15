use yew::prelude::*;
use yew_router::prelude::*;

use crate::{
    api::SearchResult,
    components::pagination::Pagination,
    hooks::use_pagination,
    router::Route,
};

#[derive(Properties, Clone, PartialEq)]
pub struct SearchPageProps {
    pub query: Option<String>,
}

#[function_component(SearchPage)]
pub fn search_page() -> Html {
    let location = use_location();
    let query = location
        .and_then(|loc| {
            loc.query::<SearchPageQuery>().ok()
        })
        .and_then(|q| q.q);

    let keyword = query.clone().unwrap_or_default();
    let results = use_state(|| Vec::<SearchResult>::new());
    let loading = use_state(|| false);
    let (visible_results, current_page, total_pages, go_to_page) =
        use_pagination((*results).clone(), 15);

    {
        let results = results.clone();
        let loading = loading.clone();
        let keyword = keyword.clone();

        use_effect_with(keyword.clone(), move |kw| {
            if kw.trim().is_empty() {
                loading.set(false);
                results.set(vec![]);
            } else {
                loading.set(true);
                let results = results.clone();
                let loading = loading.clone();
                let query_text = kw.clone();

                wasm_bindgen_futures::spawn_local(async move {
                    match crate::api::search_articles(&query_text).await {
                        Ok(data) => {
                            results.set(data);
                            loading.set(false);
                        }
                        Err(e) => {
                            web_sys::console::error_1(&format!("Search failed: {}", e).into());
                            loading.set(false);
                        }
                    }
                });
            }

            || ()
        });
    }

    html! {
        <main class="main search-page">
            <div class="container">
                <section class="page-section">
                    <p class="page-kicker">{ "搜索" }</p>
                    <h1 class="page-title">
                        if keyword.is_empty() {
                            { "搜索文章" }
                        } else {
                            { format!("搜索：{}", keyword) }
                        }
                    </h1>
                    <p class="page-description">
                        if keyword.is_empty() {
                            { "请在上方搜索框输入关键词" }
                        } else if *loading {
                            { "搜索中..." }
                        } else if results.is_empty() {
                            { format!("未找到包含\"{}\"的文章", keyword) }
                        } else {
                            { format!("找到 {} 篇相关文章", results.len()) }
                        }
                    </p>
                </section>

                <div class="search-results">
                    if *loading {
                        <div class="loading-spinner">
                            <i class="fas fa-spinner fa-spin"></i>
                            { " 搜索中..." }
                        </div>
                    } else if !results.is_empty() {
                        <>
                            { for visible_results.iter().map(|result| render_search_result(result)) }
                            {
                                if total_pages > 1 {
                                    html! {
                                        <div class="mt-8 flex justify-center">
                                            <Pagination
                                                current_page={current_page}
                                                total_pages={total_pages}
                                                on_page_change={go_to_page.clone()}
                                            />
                                        </div>
                                    }
                                } else {
                                    Html::default()
                                }
                            }
                        </>
                    } else if !keyword.is_empty() {
                        <div class="empty-hint">
                            <p>{ "没有找到匹配的结果" }</p>
                            <p class="empty-hint-sub">{ "试试其他关键词？" }</p>
                        </div>
                    }
                </div>
            </div>
        </main>
    }
}

fn render_search_result(result: &SearchResult) -> Html {
    // 将 HTML 字符串转换为安全的 VNode
    let highlight_html = Html::from_html_unchecked(AttrValue::from(result.highlight.clone()));

    html! {
        <article class="search-result-item">
            <Link<Route> to={Route::ArticleDetail { id: result.id.clone() }} classes={classes!("search-result-link")}>
                <h2 class="search-result-title">{ &result.title }</h2>
                <div class="search-result-meta">
                    <span class="search-result-category">{ &result.category }</span>
                    <span class="search-result-date">
                        <i class="far fa-calendar"></i>
                        { " " }
                        { &result.date }
                    </span>
                </div>
                <div class="search-result-highlight">
                    { highlight_html }
                </div>
                { if !result.tags.is_empty() {
                    html! {
                        <div class="search-result-tags">
                            { for result.tags.iter().map(|tag| {
                                html! {
                                    <span class="tag-badge">{ format!("#{}", tag) }</span>
                                }
                            }) }
                        </div>
                    }
                } else {
                    html! {}
                }}
            </Link<Route>>
        </article>
    }
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
struct SearchPageQuery {
    q: Option<String>,
}
