use yew::prelude::*;
use yew_router::prelude::*;

use crate::{
    api::SearchResult,
    components::{pagination::Pagination, scroll_to_top_button::ScrollToTopButton},
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
        .and_then(|loc| loc.query::<SearchPageQuery>().ok())
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
                        },
                        Err(e) => {
                            web_sys::console::error_1(&format!("Search failed: {}", e).into());
                            loading.set(false);
                        },
                    }
                });
            }

            || ()
        });
    }

    html! {
        <main class={classes!(
            "main",
            "min-h-[60vh]",
            "mt-[var(--space-lg)]",
            "pt-10",
            "pb-16"
        )}>
            <div class={classes!("container")}>
                <section class={classes!("page-section", "flex", "flex-col", "items-center", "text-center") }>
                    <p class={classes!("page-kicker")}>{ "搜索" }</p>
                    <h1 class={classes!("page-title", "text-center")}>
                        if keyword.is_empty() {
                            { "搜索文章" }
                        } else {
                            { format!("搜索：{}", keyword) }
                        }
                    </h1>
                    <p class={classes!("page-description", "text-center", "max-w-2xl") }>
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

                <div class={classes!("flex", "flex-col", "gap-6", "mt-8")}>
                    if *loading {
                        <div class={classes!(
                            "flex",
                            "items-center",
                            "justify-center",
                            "gap-3",
                            "py-12",
                            "text-[var(--muted)]",
                            "text-lg"
                        )}>
                            <i class={classes!("fas", "fa-spinner", "fa-spin", "text-2xl", "text-[var(--link)]")}></i>
                            { " 搜索中..." }
                        </div>
                    } else if !results.is_empty() {
                        <>
                            { for visible_results.iter().map(|result| render_search_result(result)) }
                            {
                                if total_pages > 1 {
                                    html! {
                        <div class={classes!("mt-8", "flex", "justify-center")}>
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
                        <div class={classes!(
                            "text-center",
                            "py-16",
                            "text-[var(--muted)]"
                        )}>
                            <p class={classes!("text-xl", "mb-2")}>{ "没有找到匹配的结果" }</p>
                            <p class={classes!("text-base", "opacity-70")}>{ "试试其他关键词？" }</p>
                        </div>
                    }
                </div>
            </div>
            <ScrollToTopButton />
        </main>
    }
}

fn render_search_result(result: &SearchResult) -> Html {
    // 将 HTML 字符串转换为安全的 VNode
    let highlight_html = Html::from_html_unchecked(AttrValue::from(result.highlight.clone()));

    html! {
        <article class={classes!(
            "bg-[var(--surface)]",
            "border",
            "border-[var(--border)]",
            "rounded-[var(--radius)]",
            "p-6",
            "transition-all",
            "duration-300",
            "shadow-[0_1px_3px_rgba(0,0,0,0.05)]",
            "hover:border-[var(--link)]",
            "hover:shadow-[0_4px_12px_rgba(0,0,0,0.08)]",
            "hover:-translate-y-0.5"
        )}>
            <Link<Route> to={Route::ArticleDetail { id: result.id.clone() }} classes={classes!("block", "text-inherit", "no-underline")}>
                <h2 class={classes!(
                    "text-2xl",
                    "font-bold",
                    "text-[var(--text)]",
                    "mb-3",
                    "leading-snug",
                    "transition-colors",
                    "duration-200",
                    "hover:text-[var(--link)]"
                )}>{ &result.title }</h2>
                <div class={classes!(
                    "flex",
                    "items-center",
                    "gap-4",
                    "text-sm",
                    "text-[var(--text-muted)]",
                    "mb-4"
                )}>
                    <span class={classes!(
                        "px-3",
                        "py-1",
                        "bg-[rgba(29,158,216,0.1)]",
                        "text-[var(--link)]",
                        "rounded-full",
                        "font-semibold",
                        "text-xs"
                    )}>{ &result.category }</span>
                    <span class={classes!("flex", "items-center", "gap-1.5")}>
                        <i class={classes!("far", "fa-calendar")}></i>
                        { " " }
                        { &result.date }
                    </span>
                </div>
                <div class={classes!(
                    "text-base",
                    "leading-relaxed",
                    "text-[var(--text)]",
                    "mb-4",
                    "[&_mark]:bg-[rgba(255,235,59,0.4)]",
                    "[&_mark]:text-inherit",
                    "[&_mark]:px-1",
                    "[&_mark]:py-0.5",
                    "[&_mark]:rounded",
                    "[&_mark]:font-semibold"
                )}>
                    { highlight_html }
                </div>
                { if !result.tags.is_empty() {
                    html! {
                        <div class={classes!("flex", "flex-wrap", "gap-2")}>
                            { for result.tags.iter().map(|tag| {
                                html! {
                                    <span class={classes!(
                                        "text-xs",
                                        "px-2.5",
                                        "py-1",
                                        "bg-[rgba(0,0,0,0.05)]",
                                        "text-[var(--text-muted)]",
                                        "rounded-full",
                                        "transition-all",
                                        "duration-200",
                                        "hover:bg-[rgba(29,158,216,0.1)]",
                                        "hover:text-[var(--link)]"
                                    )}>{ format!("#{}", tag) }</span>
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
