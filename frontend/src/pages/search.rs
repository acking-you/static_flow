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
            "search-page",
            "min-h-[60vh]",
            "mt-[var(--header-height-mobile)]",
            "md:mt-[var(--header-height-desktop)]",
            "pb-20"
        )}>
            <div class={classes!("container")}>
                // Hero Section with Cyberpunk Tech Style
                <div class={classes!(
                    "search-hero",
                    "text-center",
                    "py-16",
                    "md:py-24",
                    "px-4",
                    "relative",
                    "overflow-hidden"
                )}>
                    // Animated scanline overlay
                    <div class={classes!("search-scanline")}></div>

                    <p class={classes!(
                        "text-sm",
                        "tracking-[0.4em]",
                        "uppercase",
                        "text-[var(--muted)]",
                        "mb-6",
                        "font-semibold",
                        "opacity-50"
                    )}
                    style="font-family: 'Space Mono', monospace;">
                        { "// SEARCH_ENGINE" }
                    </p>

                    <h1 class={classes!(
                        "search-title",
                        "text-5xl",
                        "md:text-7xl",
                        "font-bold",
                        "mb-6",
                        "leading-tight",
                        "opacity-75"
                    )}
                    style="font-family: 'Space Mono', monospace;">
                        if keyword.is_empty() {
                            <span>{ "SEARCH" }</span>
                        } else {
                            <span>{ &keyword }</span>
                        }
                    </h1>

                    <p class={classes!(
                        "text-lg",
                        "md:text-xl",
                        "text-[var(--muted)]",
                        "max-w-2xl",
                        "mx-auto",
                        "leading-relaxed",
                        "mb-8",
                        "opacity-80"
                    )}>
                        if keyword.is_empty() {
                            { "请在上方搜索框输入关键词" }
                        } else if *loading {
                            <span class={classes!("search-status-loading")}>
                                <i class={classes!("fas", "fa-spinner", "fa-spin", "mr-2")}></i>
                                { "正在扫描数据库..." }
                            </span>
                        } else if results.is_empty() {
                            { format!("未找到包含 \"{}\" 的文章", keyword) }
                        } else {
                            <span class={classes!("search-status-found")}>
                                { format!("找到 {} 篇相关文章", results.len()) }
                            </span>
                        }
                    </p>

                    // Decorative tech lines
                    <div class={classes!(
                        "search-tech-lines",
                        "flex",
                        "items-center",
                        "justify-center",
                        "gap-6",
                        "mt-8"
                    )}>
                        <div class={classes!(
                            "search-line-left",
                            "w-24",
                            "h-[2px]",
                            "bg-gradient-to-r",
                            "from-[var(--primary)]/50",
                            "via-sky-500/50",
                            "to-transparent"
                        )}></div>
                        <div class={classes!(
                            "search-badge",
                            "inline-flex",
                            "items-center",
                            "gap-2",
                            "px-6",
                            "py-3",
                            "bg-gradient-to-r",
                            "from-[var(--primary)]/10",
                            "to-sky-500/10",
                            "border-2",
                            "border-[var(--primary)]/30",
                            "rounded-lg",
                            "text-sm",
                            "font-bold",
                            "text-[var(--primary)]"
                        )}>
                            <i class={classes!("fas", "fa-search")}></i>
                            <span style="font-family: 'Space Mono', monospace;">
                                if keyword.is_empty() {
                                    { "READY" }
                                } else if *loading {
                                    { "SCANNING" }
                                } else {
                                    { format!("{} RESULTS", results.len()) }
                                }
                            </span>
                        </div>
                        <div class={classes!(
                            "search-line-right",
                            "w-24",
                            "h-[2px]",
                            "bg-gradient-to-l",
                            "from-[var(--primary)]/50",
                            "via-sky-500/50",
                            "to-transparent"
                        )}></div>
                    </div>
                </div>

                // Search Results
                <div class={classes!("search-results", "flex", "flex-col", "gap-6", "mt-8")}>
                    if *loading {
                        <div class={classes!(
                            "search-loading",
                            "flex",
                            "items-center",
                            "justify-center",
                            "gap-3",
                            "py-12",
                            "text-[var(--muted)]",
                            "text-lg"
                        )}>
                            <i class={classes!("fas", "fa-spinner", "fa-spin", "text-2xl", "text-[var(--primary)]")}></i>
                            <span style="font-family: 'Space Mono', monospace;">{ "正在扫描..." }</span>
                        </div>
                    } else if !results.is_empty() {
                        <>
                            { for visible_results.iter().enumerate().map(|(idx, result)| {
                                let delay_style = format!("animation-delay: {}ms", idx * 80);
                                html! {
                                    <div class={classes!("search-result-wrapper")} style={delay_style}>
                                        { render_search_result(result) }
                                    </div>
                                }
                            }) }
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
                            "search-empty",
                            "text-center",
                            "py-16",
                            "px-4",
                            "bg-[var(--surface)]",
                            "liquid-glass",
                            "rounded-2xl",
                            "border",
                            "border-[var(--primary)]/30"
                        )}>
                            <i class={classes!(
                                "fas",
                                "fa-search",
                                "text-6xl",
                                "text-[var(--primary)]",
                                "mb-6",
                                "opacity-50"
                            )}></i>
                            <p class={classes!("text-xl", "mb-2", "font-bold")} style="font-family: 'Space Mono', monospace;">
                                { "NO RESULTS FOUND" }
                            </p>
                            <p class={classes!("text-base", "text-[var(--muted)]", "opacity-70")}>
                                { "试试其他关键词？" }
                            </p>
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
            "search-result-card",
            "bg-[var(--surface)]",
            "liquid-glass",
            "border-2",
            "border-[var(--primary)]/20",
            "rounded-xl",
            "p-6",
            "transition-all",
            "duration-300",
            "shadow-[0_4px_12px_rgba(var(--primary-rgb),0.1)]",
            "hover:border-[var(--primary)]/50",
            "hover:shadow-[0_8px_24px_rgba(var(--primary-rgb),0.2),0_0_40px_rgba(var(--primary-rgb),0.15)]",
            "hover:-translate-y-1",
            "group",
            "relative"
        )}>
            // Neon glow corner accent
            <div class={classes!("search-result-corner")}></div>

            <Link<Route> to={Route::ArticleDetail { id: result.id.clone() }} classes={classes!("block", "text-inherit", "no-underline")}>
                // Result number badge
                <div class={classes!(
                    "inline-flex",
                    "items-center",
                    "gap-2",
                    "px-3",
                    "py-1",
                    "mb-3",
                    "bg-gradient-to-r",
                    "from-[var(--primary)]/20",
                    "to-sky-500/20",
                    "border",
                    "border-[var(--primary)]/30",
                    "rounded-full",
                    "text-xs",
                    "font-bold",
                    "text-[var(--primary)]"
                )}
                style="font-family: 'Space Mono', monospace;">
                    <i class={classes!("fas", "fa-database")}></i>
                    { "MATCH" }
                </div>

                <h2 class={classes!(
                    "text-2xl",
                    "font-bold",
                    "text-[var(--text)]",
                    "mb-3",
                    "leading-snug",
                    "transition-colors",
                    "duration-200",
                    "group-hover:text-[var(--primary)]"
                )}
                style="font-family: 'Fraunces', serif;">
                    { &result.title }
                </h2>

                // Metadata with tech style
                <div class={classes!(
                    "flex",
                    "items-center",
                    "gap-4",
                    "text-sm",
                    "text-[var(--muted)]",
                    "mb-4",
                    "pb-4",
                    "border-b",
                    "border-[var(--primary)]/20"
                )}>
                    <span class={classes!(
                        "inline-flex",
                        "items-center",
                        "gap-1.5",
                        "px-3",
                        "py-1",
                        "bg-[var(--primary)]/10",
                        "text-[var(--primary)]",
                        "rounded-lg",
                        "font-semibold",
                        "text-xs",
                        "border",
                        "border-[var(--primary)]/30"
                    )}
                    style="font-family: 'Space Mono', monospace;">
                        <i class={classes!("far", "fa-folder")}></i>
                        { &result.category }
                    </span>
                    <span class={classes!("flex", "items-center", "gap-1.5", "opacity-70")}
                    style="font-family: 'Space Mono', monospace;">
                        <i class={classes!("far", "fa-calendar")}></i>
                        { &result.date }
                    </span>
                </div>

                // Highlighted content
                <div class={classes!(
                    "text-base",
                    "leading-relaxed",
                    "text-[var(--text)]",
                    "mb-4",
                    "[&_mark]:bg-gradient-to-r",
                    "[&_mark]:from-[var(--primary)]/20",
                    "[&_mark]:to-sky-400/20",
                    "[&_mark]:text-[var(--primary)]",
                    "[&_mark]:px-2",
                    "[&_mark]:py-1",
                    "[&_mark]:rounded",
                    "[&_mark]:font-semibold",
                    "[&_mark]:border",
                    "[&_mark]:border-[var(--primary)]/30"
                )}>
                    { highlight_html }
                </div>

                // Tags with cyberpunk style
                { if !result.tags.is_empty() {
                    html! {
                        <div class={classes!("flex", "flex-wrap", "gap-2")}>
                            { for result.tags.iter().map(|tag| {
                                html! {
                                    <span class={classes!(
                                        "inline-flex",
                                        "items-center",
                                        "gap-1",
                                        "text-xs",
                                        "px-3",
                                        "py-1.5",
                                        "bg-[var(--primary)]/5",
                                        "text-[var(--muted)]",
                                        "border",
                                        "border-[var(--primary)]/20",
                                        "rounded-lg",
                                        "transition-all",
                                        "duration-200",
                                        "hover:bg-[var(--primary)]/10",
                                        "hover:text-[var(--primary)]",
                                        "hover:border-[var(--primary)]/50"
                                    )}
                                    style="font-family: 'Space Mono', monospace;">
                                        { format!("#{}", tag) }
                                    </span>
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
