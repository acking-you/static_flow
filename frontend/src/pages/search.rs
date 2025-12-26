use yew::prelude::*;
use yew_router::prelude::*;

use crate::{
    api::{fetch_images, search_images_by_id, semantic_search_articles, ImageInfo, SearchResult},
    components::{
        image_with_loading::ImageWithLoading,
        pagination::Pagination,
        scroll_to_top_button::ScrollToTopButton,
    },
    hooks::use_pagination,
    router::Route,
    utils::image_url,
};

#[derive(Properties, Clone, PartialEq)]
pub struct SearchPageProps {
    pub query: Option<String>,
}

#[function_component(SearchPage)]
pub fn search_page() -> Html {
    let location = use_location();
    let query = location.and_then(|loc| loc.query::<SearchPageQuery>().ok());
    let keyword = query.as_ref().and_then(|q| q.q.clone()).unwrap_or_default();
    let mode = query
        .as_ref()
        .and_then(|q| q.mode.clone())
        .unwrap_or_else(|| "keyword".to_string())
        .to_lowercase();
    let mode = if matches!(mode.as_str(), "semantic" | "image") {
        mode
    } else {
        "keyword".to_string()
    };
    let results = use_state(|| Vec::<SearchResult>::new());
    let loading = use_state(|| false);
    let image_catalog = use_state(|| Vec::<ImageInfo>::new());
    let image_results = use_state(|| Vec::<ImageInfo>::new());
    let image_loading = use_state(|| false);
    let selected_image_id = use_state(|| None::<String>);
    let (visible_results, current_page, total_pages, go_to_page) =
        use_pagination((*results).clone(), 15);

    {
        let results = results.clone();
        let loading = loading.clone();
        let keyword = keyword.clone();
        let mode = mode.clone();

        use_effect_with((keyword.clone(), mode.clone()), move |(kw, mode)| {
            if mode == "image" {
                loading.set(false);
                results.set(vec![]);
            } else if kw.trim().is_empty() {
                loading.set(false);
                results.set(vec![]);
            } else {
                loading.set(true);
                let results = results.clone();
                let loading = loading.clone();
                let query_text = kw.clone();
                let use_semantic = mode == "semantic";

                wasm_bindgen_futures::spawn_local(async move {
                    let response = if use_semantic {
                        semantic_search_articles(&query_text).await
                    } else {
                        crate::api::search_articles(&query_text).await
                    };

                    match response {
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

    {
        let image_catalog = image_catalog.clone();
        let image_loading = image_loading.clone();
        let selected_image_id = selected_image_id.clone();
        let image_results = image_results.clone();
        let mode = mode.clone();

        use_effect_with(mode.clone(), move |mode| {
            if mode == "image" {
                image_loading.set(true);
                let image_catalog = image_catalog.clone();
                let image_loading = image_loading.clone();
                let selected_image_id = selected_image_id.clone();
                let image_results = image_results.clone();

                wasm_bindgen_futures::spawn_local(async move {
                    match fetch_images().await {
                        Ok(data) => {
                            image_catalog.set(data);
                            image_loading.set(false);
                            selected_image_id.set(None);
                            image_results.set(vec![]);
                        },
                        Err(e) => {
                            web_sys::console::error_1(
                                &format!("Failed to fetch images: {}", e).into(),
                            );
                            image_loading.set(false);
                        },
                    }
                });
            }

            || ()
        });
    }

    let on_image_select = {
        let image_results = image_results.clone();
        let image_loading = image_loading.clone();
        let selected_image_id = selected_image_id.clone();

        Callback::from(move |id: String| {
            selected_image_id.set(Some(id.clone()));
            image_loading.set(true);

            let image_results = image_results.clone();
            let image_loading = image_loading.clone();

            wasm_bindgen_futures::spawn_local(async move {
                match search_images_by_id(&id).await {
                    Ok(data) => {
                        image_results.set(data);
                        image_loading.set(false);
                    },
                    Err(e) => {
                        web_sys::console::error_1(
                            &format!("Image search failed: {}", e).into(),
                        );
                        image_loading.set(false);
                    },
                }
            });
        })
    };

    let encoded_query = urlencoding::encode(&keyword);
    let keyword_href = crate::config::route_path(&format!("/search?q={}", encoded_query));
    let semantic_href =
        crate::config::route_path(&format!("/search?mode=semantic&q={}", encoded_query));
    let image_href = crate::config::route_path("/search?mode=image");

    let hero_label = if mode == "image" {
        "IMAGE SEARCH".to_string()
    } else if keyword.is_empty() {
        "SEARCH".to_string()
    } else {
        keyword.clone()
    };
    let selected_image = (*selected_image_id).clone();
    let mode_button_base = classes!(
        "px-4",
        "py-2",
        "rounded-full",
        "border",
        "text-sm",
        "font-semibold",
        "transition-all"
    );

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
                        <span>{ hero_label }</span>
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
                        if mode == "image" {
                            { "请选择一张图片开始相似图片搜索" }
                        } else if keyword.is_empty() {
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
                                if mode == "image" {
                                    if *image_loading {
                                        { "SCANNING" }
                                    } else if selected_image_id.is_some() {
                                        { format!("{} RESULTS", image_results.len()) }
                                    } else {
                                        { "READY" }
                                    }
                                } else if keyword.is_empty() {
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

                    // Mode switches
                    <div class={classes!("flex", "items-center", "justify-center", "gap-3", "mt-8")}>
                        <a
                            href={keyword_href}
                            class={classes!(
                                mode_button_base.clone(),
                                if mode == "keyword" { "border-[var(--primary)]" } else { "border-[var(--border)]" },
                                if mode == "keyword" { "text-[var(--primary)]" } else { "text-[var(--muted)]" },
                                if mode == "keyword" { "bg-[var(--primary)]/10" } else { "" },
                                if mode != "keyword" { "hover:text-[var(--primary)]" } else { "" },
                                if mode != "keyword" { "hover:border-[var(--primary)]/60" } else { "" }
                            )}
                        >
                            { "Keyword" }
                        </a>
                        <a
                            href={semantic_href}
                            class={classes!(
                                mode_button_base.clone(),
                                if mode == "semantic" { "border-[var(--primary)]" } else { "border-[var(--border)]" },
                                if mode == "semantic" { "text-[var(--primary)]" } else { "text-[var(--muted)]" },
                                if mode == "semantic" { "bg-[var(--primary)]/10" } else { "" },
                                if mode != "semantic" { "hover:text-[var(--primary)]" } else { "" },
                                if mode != "semantic" { "hover:border-[var(--primary)]/60" } else { "" }
                            )}
                        >
                            { "Semantic" }
                        </a>
                        <a
                            href={image_href}
                            class={classes!(
                                mode_button_base.clone(),
                                if mode == "image" { "border-[var(--primary)]" } else { "border-[var(--border)]" },
                                if mode == "image" { "text-[var(--primary)]" } else { "text-[var(--muted)]" },
                                if mode == "image" { "bg-[var(--primary)]/10" } else { "" },
                                if mode != "image" { "hover:text-[var(--primary)]" } else { "" },
                                if mode != "image" { "hover:border-[var(--primary)]/60" } else { "" }
                            )}
                        >
                            { "Image" }
                        </a>
                    </div>
                </div>

                // Search Results
                <div class={classes!("search-results", "flex", "flex-col", "gap-6", "mt-8")}>
                    if mode == "image" {
                        <>
                            <div class={classes!(
                                "text-sm",
                                "text-[var(--muted)]",
                                "uppercase",
                                "tracking-[0.3em]",
                                "font-semibold"
                            )} style="font-family: 'Space Mono', monospace;">
                                { "IMAGE CATALOG" }
                            </div>

                            if *image_loading && image_catalog.is_empty() {
                                <div class={classes!(
                                    "flex",
                                    "items-center",
                                    "justify-center",
                                    "gap-3",
                                    "py-12",
                                    "text-[var(--muted)]",
                                    "text-lg"
                                )}>
                                    <i class={classes!(
                                        "fas",
                                        "fa-spinner",
                                        "fa-spin",
                                        "text-2xl",
                                        "text-[var(--primary)]"
                                    )}></i>
                                    <span style="font-family: 'Space Mono', monospace;">{ "加载图片中..." }</span>
                                </div>
                            } else if image_catalog.is_empty() {
                                <div class={classes!(
                                    "search-empty",
                                    "text-center",
                                    "py-12",
                                    "px-4",
                                    "bg-[var(--surface)]",
                                    "liquid-glass",
                                    "rounded-2xl",
                                    "border",
                                    "border-[var(--primary)]/30"
                                )}>
                                    <p class={classes!(
                                        "text-base",
                                        "text-[var(--muted)]"
                                    )}>
                                        { "暂无图片，请先运行 sf-cli write-images." }
                                    </p>
                                </div>
                            } else {
                                <div class={classes!(
                                    "grid",
                                    "grid-cols-2",
                                    "md:grid-cols-4",
                                    "gap-4"
                                )}>
                                    { for image_catalog.iter().map(|image| {
                                        let image_id = image.id.clone();
                                        let filename = image.filename.clone();
                                        let selected = selected_image
                                            .as_ref()
                                            .map(|current| current == &image_id)
                                            .unwrap_or(false);
                                        let url = image_url(&format!("images/{}", filename));
                                        let on_image_select = on_image_select.clone();
                                        let card_class = classes!(
                                            "relative",
                                            "overflow-hidden",
                                            "rounded-xl",
                                            "border",
                                            "transition-all",
                                            "duration-200",
                                            "hover:border-[var(--primary)]",
                                            "hover:shadow-[var(--shadow-8)]",
                                            if selected { "border-[var(--primary)]" } else { "border-[var(--border)]" },
                                            if selected { "ring-2" } else { "" },
                                            if selected { "ring-[var(--primary)]/40" } else { "" }
                                        );
                                        html! {
                                            <button
                                                class={card_class}
                                                onclick={Callback::from(move |_| on_image_select.emit(image_id.clone()))}
                                            >
                                                <ImageWithLoading
                                                    src={url}
                                                    alt={filename}
                                                    class={classes!("w-full", "h-32", "object-cover")}
                                                    container_class={classes!("w-full", "h-32")}
                                                />
                                            </button>
                                        }
                                    }) }
                                </div>
                            }

                            if let Some(_) = &*selected_image_id {
                                <div class={classes!(
                                    "mt-8",
                                    "text-sm",
                                    "text-[var(--muted)]",
                                    "uppercase",
                                    "tracking-[0.3em]",
                                    "font-semibold"
                                )} style="font-family: 'Space Mono', monospace;">
                                    { "SIMILAR IMAGES" }
                                </div>

                                if *image_loading {
                                    <div class={classes!(
                                        "flex",
                                        "items-center",
                                        "justify-center",
                                        "gap-3",
                                        "py-8",
                                        "text-[var(--muted)]",
                                        "text-lg"
                                    )}>
                                        <i class={classes!(
                                            "fas",
                                            "fa-spinner",
                                            "fa-spin",
                                            "text-2xl",
                                            "text-[var(--primary)]"
                                        )}></i>
                                        <span style="font-family: 'Space Mono', monospace;">{ "检索相似图片..." }</span>
                                    </div>
                                } else if image_results.is_empty() {
                                    <div class={classes!(
                                        "search-empty",
                                        "text-center",
                                        "py-10",
                                        "px-4",
                                        "bg-[var(--surface)]",
                                        "liquid-glass",
                                        "rounded-2xl",
                                        "border",
                                        "border-[var(--primary)]/30"
                                    )}>
                                        <p class={classes!("text-base", "text-[var(--muted)]")}>
                                            { "暂无相似图片结果" }
                                        </p>
                                    </div>
                                } else {
                                    <div class={classes!(
                                        "grid",
                                        "grid-cols-2",
                                        "md:grid-cols-4",
                                        "gap-4"
                                    )}>
                                        { for image_results.iter().map(|image| {
                                            let filename = image.filename.clone();
                                            let url = image_url(&format!("images/{}", filename));
                                            html! {
                                                <div
                                                    class={classes!(
                                                        "overflow-hidden",
                                                        "rounded-xl",
                                                        "border",
                                                        "border-[var(--border)]"
                                                    )}
                                                >
                                                    <ImageWithLoading
                                                        src={url}
                                                        alt={filename}
                                                        class={classes!("w-full", "h-32", "object-cover")}
                                                        container_class={classes!("w-full", "h-32")}
                                                    />
                                                </div>
                                            }
                                        }) }
                                    </div>
                                }
                            } else {
                                <div class={classes!(
                                    "search-empty",
                                    "text-center",
                                    "py-10",
                                    "px-4",
                                    "bg-[var(--surface)]",
                                    "liquid-glass",
                                    "rounded-2xl",
                                    "border",
                                    "border-[var(--primary)]/30"
                                )}>
                                    <p class={classes!("text-base", "text-[var(--muted)]")}>
                                        { "点击上方图片开始搜索相似图片" }
                                    </p>
                                </div>
                            }
                        </>
                    } else if *loading {
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
                            <i class={classes!(
                                "fas",
                                "fa-spinner",
                                "fa-spin",
                                "text-2xl",
                                "text-[var(--primary)]"
                            )}></i>
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
    mode: Option<String>,
}
