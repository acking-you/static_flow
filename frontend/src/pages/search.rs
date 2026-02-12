use wasm_bindgen::JsCast;
use web_sys::{window, KeyboardEvent};
use yew::prelude::*;
use yew_router::prelude::*;

use crate::{
    api::{
        fetch_images, search_images_by_id, search_images_by_text, semantic_search_articles,
        ImageInfo, SearchResult,
    },
    components::{
        image_with_loading::ImageWithLoading, pagination::Pagination,
        scroll_to_top_button::ScrollToTopButton,
    },
    hooks::use_pagination,
    i18n::{current::search as t, fill_one},
    router::Route,
    utils::image_url,
};

#[allow(dead_code)]
#[derive(Properties, Clone, PartialEq)]
pub struct SearchPageProps {
    pub query: Option<String>,
}

const DEFAULT_TEXT_SEARCH_LIMIT: usize = 50;
const DEFAULT_IMAGE_SEARCH_LIMIT: usize = 24;
const SEARCH_PAGE_SIZE: usize = 15;
const IMAGE_GRID_CHUNK_SIZE: usize = 24;

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
    let mode =
        if matches!(mode.as_str(), "semantic" | "image") { mode } else { "keyword".to_string() };
    let enhanced_highlight = query
        .as_ref()
        .and_then(|q| q.enhanced_highlight)
        .unwrap_or(false);
    let fetch_all = query.as_ref().and_then(|q| q.all).unwrap_or(false);
    let requested_limit = query
        .as_ref()
        .and_then(|q| q.limit)
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_TEXT_SEARCH_LIMIT);
    let active_limit = if fetch_all { None } else { Some(requested_limit) };
    let max_distance = query
        .as_ref()
        .and_then(|q| q.max_distance)
        .filter(|value| value.is_finite() && *value >= 0.0);
    let results = use_state(Vec::<SearchResult>::new);
    let loading = use_state(|| false);
    let image_catalog = use_state(Vec::<ImageInfo>::new);
    let image_results = use_state(Vec::<ImageInfo>::new);
    let image_loading = use_state(|| false);
    let selected_image_id = use_state(|| None::<String>);
    let image_text_results = use_state(Vec::<ImageInfo>::new);
    let image_text_loading = use_state(|| false);
    let image_catalog_visible = use_state(|| IMAGE_GRID_CHUNK_SIZE);
    let image_text_visible = use_state(|| IMAGE_GRID_CHUNK_SIZE);
    let image_similar_visible = use_state(|| IMAGE_GRID_CHUNK_SIZE);
    let image_scroll_loading = use_state(|| false);
    let is_lightbox_open = use_state(|| false);
    let preview_image_url = use_state_eq(|| None::<String>);
    let preview_image_failed = use_state(|| false);
    let image_distance_input = use_state(|| {
        max_distance
            .map(|value| value.to_string())
            .unwrap_or_default()
    });
    let (visible_results, current_page, total_pages, go_to_page) =
        use_pagination((*results).clone(), SEARCH_PAGE_SIZE);

    {
        let results = results.clone();
        let loading = loading.clone();
        let keyword = keyword.clone();
        let mode = mode.clone();
        let active_limit = active_limit;
        let max_distance = max_distance;

        use_effect_with(
            (keyword.clone(), mode.clone(), enhanced_highlight, active_limit, max_distance),
            move |(kw, mode, enhanced_highlight, active_limit, max_distance)| {
                if mode == "image" || kw.trim().is_empty() {
                    loading.set(false);
                    results.set(vec![]);
                } else {
                    loading.set(true);
                    let results = results.clone();
                    let loading = loading.clone();
                    let query_text = kw.clone();
                    let use_semantic = mode == "semantic";
                    let use_enhanced_highlight = *enhanced_highlight;
                    let limit = *active_limit;
                    let max_distance = *max_distance;

                    wasm_bindgen_futures::spawn_local(async move {
                        let response = if use_semantic {
                            semantic_search_articles(
                                &query_text,
                                use_enhanced_highlight,
                                limit,
                                max_distance,
                            )
                            .await
                        } else {
                            crate::api::search_articles(&query_text, limit).await
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
            },
        );
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

    {
        let mode = mode.clone();
        let keyword = keyword.clone();
        let image_text_results = image_text_results.clone();
        let image_text_loading = image_text_loading.clone();
        let max_distance = max_distance;

        use_effect_with(
            (mode.clone(), keyword.clone(), max_distance),
            move |(mode, keyword, max_distance)| {
                if mode != "image" || keyword.trim().is_empty() {
                    image_text_loading.set(false);
                    image_text_results.set(vec![]);
                } else {
                    image_text_loading.set(true);
                    let image_text_results = image_text_results.clone();
                    let image_text_loading = image_text_loading.clone();
                    let query_text = keyword.clone();
                    let query_distance = *max_distance;

                    wasm_bindgen_futures::spawn_local(async move {
                        match search_images_by_text(
                            &query_text,
                            Some(DEFAULT_IMAGE_SEARCH_LIMIT),
                            query_distance,
                        )
                        .await
                        {
                            Ok(data) => {
                                image_text_results.set(data);
                                image_text_loading.set(false);
                            },
                            Err(e) => {
                                web_sys::console::error_1(
                                    &format!("Text image search failed: {}", e).into(),
                                );
                                image_text_loading.set(false);
                            },
                        }
                    });
                }

                || ()
            },
        );
    }

    {
        let mode = mode.clone();
        let image_distance_input = image_distance_input.clone();
        use_effect_with((mode.clone(), max_distance), move |(mode, max_distance)| {
            if mode == "image" {
                image_distance_input.set(
                    max_distance
                        .map(|value| value.to_string())
                        .unwrap_or_default(),
                );
            }
            || ()
        });
    }

    {
        let image_catalog_visible = image_catalog_visible.clone();
        let total = image_catalog.len();
        use_effect_with(total, move |count| {
            image_catalog_visible.set((*count).min(IMAGE_GRID_CHUNK_SIZE));
            || ()
        });
    }

    {
        let image_text_visible = image_text_visible.clone();
        let total = image_text_results.len();
        use_effect_with(total, move |count| {
            image_text_visible.set((*count).min(IMAGE_GRID_CHUNK_SIZE));
            || ()
        });
    }

    {
        let image_similar_visible = image_similar_visible.clone();
        let total = image_results.len();
        use_effect_with(total, move |count| {
            image_similar_visible.set((*count).min(IMAGE_GRID_CHUNK_SIZE));
            || ()
        });
    }

    {
        let mode = mode.clone();
        let image_catalog = image_catalog.clone();
        let image_text_results = image_text_results.clone();
        let image_results = image_results.clone();
        let image_catalog_visible = image_catalog_visible.clone();
        let image_text_visible = image_text_visible.clone();
        let image_similar_visible = image_similar_visible.clone();
        let image_scroll_loading = image_scroll_loading.clone();
        use_effect_with(
            (mode.clone(), image_catalog.len(), image_text_results.len(), image_results.len()),
            move |_| {
                let mut callback: Option<
                    wasm_bindgen::closure::Closure<dyn FnMut(web_sys::Event)>,
                > = None;
                if mode != "image" {
                    image_scroll_loading.set(false);
                } else {
                    let on_scroll = {
                        let image_catalog = image_catalog.clone();
                        let image_text_results = image_text_results.clone();
                        let image_results = image_results.clone();
                        let image_catalog_visible = image_catalog_visible.clone();
                        let image_text_visible = image_text_visible.clone();
                        let image_similar_visible = image_similar_visible.clone();
                        let image_scroll_loading = image_scroll_loading.clone();

                        wasm_bindgen::closure::Closure::wrap(Box::new(
                            move |_event: web_sys::Event| {
                                if *image_scroll_loading {
                                    return;
                                }
                                if !is_near_page_bottom() {
                                    return;
                                }

                                let more_catalog = *image_catalog_visible < image_catalog.len();
                                let more_text = *image_text_visible < image_text_results.len();
                                let more_similar = *image_similar_visible < image_results.len();
                                if !(more_catalog || more_text || more_similar) {
                                    return;
                                }

                                image_scroll_loading.set(true);
                                let image_catalog_visible = image_catalog_visible.clone();
                                let image_text_visible = image_text_visible.clone();
                                let image_similar_visible = image_similar_visible.clone();
                                let image_scroll_loading = image_scroll_loading.clone();
                                let catalog_total = image_catalog.len();
                                let text_total = image_text_results.len();
                                let similar_total = image_results.len();

                                gloo_timers::callback::Timeout::new(180, move || {
                                    if *image_catalog_visible < catalog_total {
                                        image_catalog_visible.set(
                                            (*image_catalog_visible + IMAGE_GRID_CHUNK_SIZE)
                                                .min(catalog_total),
                                        );
                                    }
                                    if *image_text_visible < text_total {
                                        image_text_visible.set(
                                            (*image_text_visible + IMAGE_GRID_CHUNK_SIZE)
                                                .min(text_total),
                                        );
                                    }
                                    if *image_similar_visible < similar_total {
                                        image_similar_visible.set(
                                            (*image_similar_visible + IMAGE_GRID_CHUNK_SIZE)
                                                .min(similar_total),
                                        );
                                    }
                                    image_scroll_loading.set(false);
                                })
                                .forget();
                            },
                        )
                            as Box<dyn FnMut(_)>)
                    };

                    if let Some(win) = window() {
                        let _ = win.add_event_listener_with_callback(
                            "scroll",
                            on_scroll.as_ref().unchecked_ref(),
                        );
                    }
                    callback = Some(on_scroll);
                }
                move || {
                    if let Some(on_scroll) = callback {
                        if let Some(win) = window() {
                            let _ = win.remove_event_listener_with_callback(
                                "scroll",
                                on_scroll.as_ref().unchecked_ref(),
                            );
                        }
                    }
                }
            },
        );
    }

    let open_image_preview = {
        let is_lightbox_open = is_lightbox_open.clone();
        let preview_image_url = preview_image_url.clone();
        let preview_image_failed = preview_image_failed.clone();
        Callback::from(move |src: String| {
            preview_image_failed.set(false);
            preview_image_url.set(Some(src));
            is_lightbox_open.set(true);
        })
    };

    let close_lightbox_click = {
        let is_lightbox_open = is_lightbox_open.clone();
        let preview_image_url = preview_image_url.clone();
        let preview_image_failed = preview_image_failed.clone();
        Callback::from(move |_| {
            is_lightbox_open.set(false);
            preview_image_url.set(None);
            preview_image_failed.set(false);
        })
    };

    {
        let is_lightbox_open = is_lightbox_open.clone();
        let preview_image_url = preview_image_url.clone();
        let preview_image_failed = preview_image_failed.clone();
        use_effect_with(*is_lightbox_open, move |is_open| {
            let keydown_listener_opt = if *is_open {
                let handle = is_lightbox_open.clone();
                let preview_url = preview_image_url.clone();
                let failed = preview_image_failed.clone();
                let listener =
                    wasm_bindgen::closure::Closure::wrap(Box::new(move |event: KeyboardEvent| {
                        if event.key() == "Escape" {
                            handle.set(false);
                            preview_url.set(None);
                            failed.set(false);
                        }
                    })
                        as Box<dyn FnMut(_)>);

                if let Some(win) = window() {
                    let _ = win.add_event_listener_with_callback(
                        "keydown",
                        listener.as_ref().unchecked_ref(),
                    );
                }
                Some(listener)
            } else {
                None
            };

            move || {
                if let Some(listener) = keydown_listener_opt {
                    if let Some(win) = window() {
                        let _ = win.remove_event_listener_with_callback(
                            "keydown",
                            listener.as_ref().unchecked_ref(),
                        );
                    }
                }
            }
        });
    }

    let stop_lightbox_bubble = Callback::from(|event: MouseEvent| event.stop_propagation());
    let mark_preview_failed = {
        let preview_image_failed = preview_image_failed.clone();
        Callback::from(move |_: Event| preview_image_failed.set(true))
    };
    let mark_preview_loaded = {
        let preview_image_failed = preview_image_failed.clone();
        Callback::from(move |_: Event| preview_image_failed.set(false))
    };

    let on_image_select = {
        let image_results = image_results.clone();
        let image_loading = image_loading.clone();
        let selected_image_id = selected_image_id.clone();
        let max_distance = max_distance;

        Callback::from(move |id: String| {
            selected_image_id.set(Some(id.clone()));
            image_loading.set(true);

            let image_results = image_results.clone();
            let image_loading = image_loading.clone();

            wasm_bindgen_futures::spawn_local(async move {
                match search_images_by_id(&id, Some(DEFAULT_IMAGE_SEARCH_LIMIT), max_distance).await
                {
                    Ok(data) => {
                        image_results.set(data);
                        image_loading.set(false);
                    },
                    Err(e) => {
                        web_sys::console::error_1(&format!("Image search failed: {}", e).into());
                        image_loading.set(false);
                    },
                }
            });
        })
    };

    let keyword_href =
        build_search_href(None, &keyword, false, Some(requested_limit), fetch_all, None);
    let semantic_fast_href = build_search_href(
        Some("semantic"),
        &keyword,
        false,
        Some(requested_limit),
        fetch_all,
        max_distance,
    );
    let semantic_precise_href = build_search_href(
        Some("semantic"),
        &keyword,
        true,
        Some(requested_limit),
        fetch_all,
        max_distance,
    );
    let semantic_href =
        if enhanced_highlight { semantic_precise_href.clone() } else { semantic_fast_href.clone() };
    let image_href = build_search_href(Some("image"), &keyword, false, None, false, max_distance);
    let scoped_max_distance = if mode == "semantic" { max_distance } else { None };
    let limited_href = build_search_href(
        Some(mode.as_str()),
        &keyword,
        enhanced_highlight,
        Some(requested_limit),
        false,
        scoped_max_distance,
    );
    let all_results_href = build_search_href(
        Some(mode.as_str()),
        &keyword,
        enhanced_highlight,
        None,
        true,
        scoped_max_distance,
    );
    let semantic_limit_for_mode = if fetch_all { None } else { Some(requested_limit) };
    let semantic_distance_off_href = build_search_href(
        Some("semantic"),
        &keyword,
        enhanced_highlight,
        semantic_limit_for_mode,
        fetch_all,
        None,
    );
    let semantic_distance_strict_href = build_search_href(
        Some("semantic"),
        &keyword,
        enhanced_highlight,
        semantic_limit_for_mode,
        fetch_all,
        Some(0.8),
    );
    let semantic_distance_relaxed_href = build_search_href(
        Some("semantic"),
        &keyword,
        enhanced_highlight,
        semantic_limit_for_mode,
        fetch_all,
        Some(1.2),
    );
    let image_distance_off_href =
        build_search_href(Some("image"), &keyword, false, None, false, None);
    let parsed_image_distance_input = image_distance_input
        .trim()
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0);
    let image_distance_apply_href =
        build_search_href(Some("image"), &keyword, false, None, false, parsed_image_distance_input);
    let on_image_distance_input = {
        let image_distance_input = image_distance_input.clone();
        Callback::from(move |event: InputEvent| {
            if let Some(target) = event.target_dyn_into::<web_sys::HtmlInputElement>() {
                image_distance_input.set(target.value());
            }
        })
    };
    let distance_off_selected = max_distance.is_none();
    let distance_strict_selected = max_distance
        .map(|value| (value - 0.8).abs() < 0.0001)
        .unwrap_or(false);
    let distance_relaxed_selected = max_distance
        .map(|value| (value - 1.2).abs() < 0.0001)
        .unwrap_or(false);

    let hero_label = if mode == "image" && !keyword.is_empty() {
        keyword.clone()
    } else if mode == "image" {
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
                        { t::SEARCH_ENGINE_BADGE }
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
                        if mode == "image" && !keyword.is_empty() {
                            if *image_text_loading {
                                <span class={classes!("search-status-loading")}>
                                    <i class={classes!("fas", "fa-spinner", "fa-spin", "mr-2")}></i>
                                    { t::IMAGE_TEXT_SEARCHING }
                                </span>
                            } else if image_text_results.is_empty() {
                                { fill_one(t::IMAGE_TEXT_MISS_TEMPLATE, &keyword) }
                            } else {
                                <span class={classes!("search-status-found")}>
                                    { fill_one(t::IMAGE_TEXT_FOUND_TEMPLATE, image_text_results.len().to_string()) }
                                </span>
                            }
                        } else if mode == "image" {
                            { t::IMAGE_MODE_HINT }
                        } else if keyword.is_empty() {
                            { t::EMPTY_KEYWORD_HINT }
                        } else if *loading {
                            <span class={classes!("search-status-loading")}>
                                <i class={classes!("fas", "fa-spinner", "fa-spin", "mr-2")}></i>
                                { t::SEARCH_LOADING }
                            </span>
                        } else if mode == "keyword" && results.is_empty() {
                            { fill_one(t::KEYWORD_MISS_TEMPLATE, &keyword) }
                        } else if mode == "keyword" {
                            <span class={classes!("search-status-found")}>
                                { fill_one(
                                    t::KEYWORD_FOUND_TEMPLATE,
                                    results.len().to_string(),
                                ) }
                            </span>
                        } else if results.is_empty() {
                            { fill_one(t::SEMANTIC_MISS_TEMPLATE, &keyword) }
                        } else {
                            <span class={classes!("search-status-found")}>
                                { fill_one(t::SEMANTIC_FOUND_TEMPLATE, results.len().to_string()) }
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
                                    if *image_loading || *image_text_loading {
                                        { t::STATUS_SCANNING }
                                    } else if !keyword.is_empty() {
                                        { format!("{} RESULTS", image_text_results.len()) }
                                    } else if selected_image_id.is_some() {
                                        { format!("{} RESULTS", image_results.len()) }
                                    } else {
                                        { t::STATUS_READY }
                                    }
                                } else if keyword.is_empty() {
                                    { t::STATUS_READY }
                                } else if *loading {
                                    { t::STATUS_SCANNING }
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
                            { t::MODE_KEYWORD }
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
                            { t::MODE_SEMANTIC }
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
                            { t::MODE_IMAGE }
                        </a>
                    </div>

                    if mode != "image" && !keyword.is_empty() {
                        <div class={classes!(
                            "mt-6",
                            "flex",
                            "items-center",
                            "justify-center",
                            "gap-3",
                            "flex-wrap"
                        )}>
                            <span class={classes!(
                                "text-xs",
                                "uppercase",
                                "tracking-[0.2em]",
                                "text-[var(--muted)]",
                                "font-semibold"
                            )}
                            style="font-family: 'Space Mono', monospace;">
                                { t::RESULT_SCOPE }
                            </span>
                            <a
                                href={limited_href}
                                class={classes!(
                                    mode_button_base.clone(),
                                    "text-xs",
                                    if !fetch_all { "border-[var(--primary)]" } else { "border-[var(--border)]" },
                                    if !fetch_all { "text-[var(--primary)]" } else { "text-[var(--muted)]" },
                                    if !fetch_all { "bg-[var(--primary)]/10" } else { "" },
                                    if fetch_all { "hover:text-[var(--primary)]" } else { "" },
                                    if fetch_all { "hover:border-[var(--primary)]/60" } else { "" }
                                )}
                            >
                                { fill_one(t::RESULT_SCOPE_LIMITED_TEMPLATE, requested_limit) }
                            </a>
                            <a
                                href={all_results_href}
                                class={classes!(
                                    mode_button_base.clone(),
                                    "text-xs",
                                    if fetch_all { "border-[var(--primary)]" } else { "border-[var(--border)]" },
                                    if fetch_all { "text-[var(--primary)]" } else { "text-[var(--muted)]" },
                                    if fetch_all { "bg-[var(--primary)]/10" } else { "" },
                                    if !fetch_all { "hover:text-[var(--primary)]" } else { "" },
                                    if !fetch_all { "hover:border-[var(--primary)]/60" } else { "" }
                                )}
                            >
                                { t::RESULT_SCOPE_ALL }
                            </a>
                        </div>
                    }

                    if mode == "semantic" && !keyword.is_empty() {
                        <div class={classes!(
                            "mt-4",
                            "flex",
                            "items-center",
                            "justify-center",
                            "gap-3",
                            "flex-wrap"
                        )}>
                            <span class={classes!(
                                "text-xs",
                                "uppercase",
                                "tracking-[0.2em]",
                                "text-[var(--muted)]",
                                "font-semibold"
                            )}
                            style="font-family: 'Space Mono', monospace;">
                                { t::DISTANCE_FILTER }
                            </span>
                            <a
                                href={semantic_distance_off_href}
                                class={classes!(
                                    mode_button_base.clone(),
                                    "text-xs",
                                    if distance_off_selected { "border-[var(--primary)]" } else { "border-[var(--border)]" },
                                    if distance_off_selected { "text-[var(--primary)]" } else { "text-[var(--muted)]" },
                                    if distance_off_selected { "bg-[var(--primary)]/10" } else { "" },
                                    if !distance_off_selected { "hover:text-[var(--primary)]" } else { "" },
                                    if !distance_off_selected { "hover:border-[var(--primary)]/60" } else { "" }
                                )}
                            >
                                { t::DISTANCE_FILTER_OFF }
                            </a>
                            <a
                                href={semantic_distance_strict_href}
                                class={classes!(
                                    mode_button_base.clone(),
                                    "text-xs",
                                    if distance_strict_selected { "border-[var(--primary)]" } else { "border-[var(--border)]" },
                                    if distance_strict_selected { "text-[var(--primary)]" } else { "text-[var(--muted)]" },
                                    if distance_strict_selected { "bg-[var(--primary)]/10" } else { "" },
                                    if !distance_strict_selected { "hover:text-[var(--primary)]" } else { "" },
                                    if !distance_strict_selected { "hover:border-[var(--primary)]/60" } else { "" }
                                )}
                            >
                                { t::DISTANCE_FILTER_STRICT }
                            </a>
                            <a
                                href={semantic_distance_relaxed_href}
                                class={classes!(
                                    mode_button_base.clone(),
                                    "text-xs",
                                    if distance_relaxed_selected { "border-[var(--primary)]" } else { "border-[var(--border)]" },
                                    if distance_relaxed_selected { "text-[var(--primary)]" } else { "text-[var(--muted)]" },
                                    if distance_relaxed_selected { "bg-[var(--primary)]/10" } else { "" },
                                    if !distance_relaxed_selected { "hover:text-[var(--primary)]" } else { "" },
                                    if !distance_relaxed_selected { "hover:border-[var(--primary)]/60" } else { "" }
                                )}
                            >
                                { t::DISTANCE_FILTER_RELAXED }
                            </a>
                        </div>
                    }

                    if mode == "image" && !keyword.is_empty() {
                        <div class={classes!(
                            "mt-4",
                            "flex",
                            "items-center",
                            "justify-center",
                            "gap-3",
                            "flex-wrap"
                        )}>
                            <span class={classes!(
                                "text-xs",
                                "uppercase",
                                "tracking-[0.2em]",
                                "text-[var(--muted)]",
                                "font-semibold"
                            )}
                            style="font-family: 'Space Mono', monospace;">
                                { t::DISTANCE_FILTER }
                            </span>
                            <input
                                type="number"
                                step="0.01"
                                min="0"
                                value={(*image_distance_input).clone()}
                                placeholder={t::DISTANCE_FILTER_INPUT_PLACEHOLDER}
                                oninput={on_image_distance_input}
                                class={classes!(
                                    "h-10",
                                    "w-40",
                                    "rounded-lg",
                                    "border",
                                    "border-[var(--border)]",
                                    "bg-[var(--surface)]",
                                    "px-3",
                                    "text-sm",
                                    "text-[var(--text)]",
                                    "outline-none",
                                    "focus:border-[var(--primary)]",
                                    "transition-colors"
                                )}
                            />
                            <a
                                href={image_distance_apply_href}
                                class={classes!(
                                    mode_button_base.clone(),
                                    "text-xs",
                                    "border-[var(--border)]",
                                    "text-[var(--muted)]",
                                    "hover:text-[var(--primary)]",
                                    "hover:border-[var(--primary)]/60"
                                )}
                            >
                                { t::DISTANCE_FILTER_APPLY }
                            </a>
                            <a
                                href={image_distance_off_href}
                                class={classes!(
                                    mode_button_base.clone(),
                                    "text-xs",
                                    if distance_off_selected { "border-[var(--primary)]" } else { "border-[var(--border)]" },
                                    if distance_off_selected { "text-[var(--primary)]" } else { "text-[var(--muted)]" },
                                    if distance_off_selected { "bg-[var(--primary)]/10" } else { "" },
                                    if !distance_off_selected { "hover:text-[var(--primary)]" } else { "" },
                                    if !distance_off_selected { "hover:border-[var(--primary)]/60" } else { "" }
                                )}
                            >
                                { t::DISTANCE_FILTER_OFF }
                            </a>
                        </div>
                    }

                    if mode == "semantic" {
                        <div class={classes!(
                            "mt-6",
                            "flex",
                            "items-center",
                            "justify-center",
                            "gap-3",
                            "flex-wrap"
                        )}>
                            <span class={classes!(
                                "text-xs",
                                "uppercase",
                                "tracking-[0.2em]",
                                "text-[var(--muted)]",
                                "font-semibold"
                            )}
                            style="font-family: 'Space Mono', monospace;">
                                { t::HIGHLIGHT_PRECISION }
                            </span>
                            <a
                                href={semantic_fast_href.clone()}
                                class={classes!(
                                    mode_button_base.clone(),
                                    "text-xs",
                                    if !enhanced_highlight { "border-[var(--primary)]" } else { "border-[var(--border)]" },
                                    if !enhanced_highlight { "text-[var(--primary)]" } else { "text-[var(--muted)]" },
                                    if !enhanced_highlight { "bg-[var(--primary)]/10" } else { "" },
                                    if enhanced_highlight { "hover:text-[var(--primary)]" } else { "" },
                                    if enhanced_highlight { "hover:border-[var(--primary)]/60" } else { "" }
                                )}
                            >
                                { t::HIGHLIGHT_FAST }
                            </a>
                            <a
                                href={semantic_precise_href.clone()}
                                class={classes!(
                                    mode_button_base.clone(),
                                    "text-xs",
                                    if enhanced_highlight { "border-[var(--primary)]" } else { "border-[var(--border)]" },
                                    if enhanced_highlight { "text-[var(--primary)]" } else { "text-[var(--muted)]" },
                                    if enhanced_highlight { "bg-[var(--primary)]/10" } else { "" },
                                    if !enhanced_highlight { "hover:text-[var(--primary)]" } else { "" },
                                    if !enhanced_highlight { "hover:border-[var(--primary)]/60" } else { "" }
                                )}
                            >
                                { t::HIGHLIGHT_ENHANCED }
                            </a>
                        </div>
                    }

                    if mode == "keyword" && !keyword.is_empty() {
                        <div class={classes!(
                            "mt-6",
                            "mx-auto",
                            "max-w-3xl",
                            "rounded-xl",
                            "border",
                            "border-[var(--primary)]/30",
                            "bg-[var(--primary)]/5",
                            "px-4",
                            "py-3",
                            "flex",
                            "items-center",
                            "justify-center",
                            "gap-3",
                            "flex-wrap",
                            "text-sm",
                            "text-[var(--muted)]"
                        )}>
                            <i class={classes!("fas", "fa-lightbulb", "text-[var(--primary)]")}></i>
                            <span>
                                { t::KEYWORD_GUIDE_BANNER }
                            </span>
                            <a
                                href={semantic_fast_href.clone()}
                                class={classes!(
                                    "px-3",
                                    "py-1.5",
                                    "rounded-lg",
                                    "border",
                                    "border-[var(--primary)]/60",
                                    "text-[var(--primary)]",
                                    "font-semibold",
                                    "hover:bg-[var(--primary)]/10",
                                    "transition-colors"
                                )}
                            >
                                { t::SWITCH_TO_SEMANTIC }
                            </a>
                        </div>
                    }
                </div>

                // Search Results
                <div class={classes!("search-results", "flex", "flex-col", "gap-6", "mt-8")}>
                    if mode == "image" {
                        <>
                            if !keyword.is_empty() {
                                <div class={classes!(
                                    "text-sm",
                                    "text-[var(--muted)]",
                                    "uppercase",
                                    "tracking-[0.3em]",
                                    "font-semibold"
                                )} style="font-family: 'Space Mono', monospace;">
                                    { t::IMAGE_TEXT_RESULTS }
                                </div>
                                <div class={classes!(
                                    "text-sm",
                                    "text-[var(--muted)]",
                                    "mb-2"
                                )} style="font-family: 'Space Mono', monospace;">
                                    { fill_one(t::IMAGE_TEXT_QUERY_TEMPLATE, &keyword) }
                                </div>

                                if *image_text_loading {
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
                                        <span style="font-family: 'Space Mono', monospace;">{ t::IMAGE_TEXT_SEARCHING }</span>
                                    </div>
                                } else if image_text_results.is_empty() {
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
                                            { t::IMAGE_TEXT_NO_RESULTS }
                                        </p>
                                    </div>
                                } else {
                                    <div class={classes!(
                                        "grid",
                                        "grid-cols-2",
                                        "md:grid-cols-4",
                                        "gap-4"
                                    )}>
                                        { for image_text_results.iter().take(*image_text_visible).map(|image| {
                                            let filename = image.filename.clone();
                                            let url = image_url(&format!("images/{}", filename));
                                            let open_image_preview = open_image_preview.clone();
                                            let preview_url = url.clone();
                                            let on_preview_click = Callback::from(move |_| {
                                                open_image_preview.emit(preview_url.clone());
                                            });
                                            html! {
                                                <div
                                                    class={classes!(
                                                        "overflow-hidden",
                                                        "rounded-xl",
                                                        "border",
                                                        "border-[var(--border)]",
                                                        "cursor-zoom-in"
                                                    )}
                                                    onclick={on_preview_click}
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
                                    if *image_text_visible < image_text_results.len() {
                                        <div class={classes!(
                                            "flex",
                                            "items-center",
                                            "justify-center",
                                            "gap-2",
                                            "py-2",
                                            "text-sm",
                                            "text-[var(--muted)]"
                                        )}>
                                            <i class={classes!(
                                                "fas",
                                                if *image_scroll_loading { "fa-spinner fa-spin" } else { "fa-arrow-down" }
                                            )}></i>
                                            { if *image_scroll_loading { t::IMAGE_SCROLL_LOADING } else { t::IMAGE_SCROLL_HINT } }
                                        </div>
                                    }
                                }
                            }

                            <div class={classes!(
                                "text-sm",
                                "text-[var(--muted)]",
                                "uppercase",
                                "tracking-[0.3em]",
                                "font-semibold"
                            )} style="font-family: 'Space Mono', monospace;">
                                { t::IMAGE_CATALOG }
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
                                    <span style="font-family: 'Space Mono', monospace;">{ t::IMAGE_LOADING }</span>
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
                                        { t::IMAGE_EMPTY_HINT }
                                    </p>
                                </div>
                            } else {
                                <div class={classes!(
                                    "grid",
                                    "grid-cols-2",
                                    "md:grid-cols-4",
                                    "gap-4"
                                )}>
                                    { for image_catalog.iter().take(*image_catalog_visible).map(|image| {
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
                                if *image_catalog_visible < image_catalog.len() {
                                    <div class={classes!(
                                        "flex",
                                        "items-center",
                                        "justify-center",
                                        "gap-2",
                                        "py-2",
                                        "text-sm",
                                        "text-[var(--muted)]"
                                    )}>
                                        <i class={classes!(
                                            "fas",
                                            if *image_scroll_loading { "fa-spinner fa-spin" } else { "fa-arrow-down" }
                                        )}></i>
                                        { if *image_scroll_loading { t::IMAGE_SCROLL_LOADING } else { t::IMAGE_SCROLL_HINT } }
                                    </div>
                                }
                            }

                            if (*selected_image_id).is_some() {
                                <div class={classes!(
                                    "mt-8",
                                    "text-sm",
                                    "text-[var(--muted)]",
                                    "uppercase",
                                    "tracking-[0.3em]",
                                    "font-semibold"
                                )} style="font-family: 'Space Mono', monospace;">
                                    { t::SIMILAR_IMAGES }
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
                                        <span style="font-family: 'Space Mono', monospace;">{ t::IMAGE_SEARCHING }</span>
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
                                            { t::IMAGE_NO_SIMILAR }
                                        </p>
                                    </div>
                                } else {
                                    <div class={classes!(
                                        "grid",
                                        "grid-cols-2",
                                        "md:grid-cols-4",
                                        "gap-4"
                                    )}>
                                        { for image_results.iter().take(*image_similar_visible).map(|image| {
                                            let filename = image.filename.clone();
                                            let url = image_url(&format!("images/{}", filename));
                                            let open_image_preview = open_image_preview.clone();
                                            let preview_url = url.clone();
                                            let on_preview_click = Callback::from(move |_| {
                                                open_image_preview.emit(preview_url.clone());
                                            });
                                            html! {
                                                <div
                                                    class={classes!(
                                                        "overflow-hidden",
                                                        "rounded-xl",
                                                        "border",
                                                        "border-[var(--border)]",
                                                        "cursor-zoom-in"
                                                    )}
                                                    onclick={on_preview_click}
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
                                    if *image_similar_visible < image_results.len() {
                                        <div class={classes!(
                                            "flex",
                                            "items-center",
                                            "justify-center",
                                            "gap-2",
                                            "py-2",
                                            "text-sm",
                                            "text-[var(--muted)]"
                                        )}>
                                            <i class={classes!(
                                                "fas",
                                                if *image_scroll_loading { "fa-spinner fa-spin" } else { "fa-arrow-down" }
                                            )}></i>
                                            { if *image_scroll_loading { t::IMAGE_SCROLL_LOADING } else { t::IMAGE_SCROLL_HINT } }
                                        </div>
                                    }
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
                                        { t::IMAGE_SELECT_HINT }
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
                            <span style="font-family: 'Space Mono', monospace;">{ t::SEARCHING_SHORT }</span>
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
                                { t::NO_RESULTS_TITLE }
                            </p>
                            <p class={classes!("text-base", "text-[var(--muted)]", "opacity-70")}>
                                if mode == "keyword" {
                                    { t::KEYWORD_EMPTY_CARD_DESC }
                                } else {
                                    { t::SEMANTIC_EMPTY_CARD_DESC }
                                }
                            </p>
                            if mode == "keyword" {
                                <div class={classes!("mt-4")}>
                                    <a
                                        href={semantic_fast_href.clone()}
                                        class={classes!(
                                            "inline-flex",
                                            "items-center",
                                            "gap-2",
                                            "px-4",
                                            "py-2",
                                            "rounded-lg",
                                            "border",
                                            "border-[var(--primary)]/60",
                                            "text-[var(--primary)]",
                                            "font-semibold",
                                            "hover:bg-[var(--primary)]/10",
                                            "transition-colors"
                                        )}
                                    >
                                        <i class={classes!("fas", "fa-brain")}></i>
                                        { t::SWITCH_TO_SEMANTIC_CTA }
                                    </a>
                                </div>
                            }
                        </div>
                    }
                </div>
            </div>
            {
                if *is_lightbox_open {
                    html! {
                        <div
                            class={classes!(
                                "fixed",
                                "inset-0",
                                "z-[100]",
                                "flex",
                                "items-center",
                                "justify-center",
                                "bg-black/80",
                                "p-4",
                                "text-white",
                                "backdrop-blur-sm",
                                "transition",
                                "dark:bg-black/80"
                            )}
                            role="dialog"
                            aria-modal="true"
                            onclick={close_lightbox_click.clone()}
                        >
                            <button
                                type="button"
                                class={classes!(
                                    "absolute",
                                    "right-4",
                                    "top-4",
                                    "z-[101]",
                                    "rounded-full",
                                    "bg-black/70",
                                    "px-3",
                                    "py-1",
                                    "text-lg",
                                    "leading-none",
                                    "text-white",
                                    "hover:bg-black"
                                )}
                                aria-label={t::LIGHTBOX_CLOSE_ARIA}
                                onclick={close_lightbox_click.clone()}
                            >
                                { "X" }
                            </button>
                            <div
                                class={classes!(
                                    "absolute",
                                    "left-4",
                                    "top-4",
                                    "z-[101]",
                                    "flex",
                                    "items-center",
                                    "gap-2"
                                )}
                                onclick={stop_lightbox_bubble.clone()}
                            >
                                {
                                    if let Some(src) = (*preview_image_url).clone() {
                                        html! {
                                            <a
                                                href={src.clone()}
                                                download=""
                                                target="_blank"
                                                rel="noopener noreferrer"
                                                class={classes!(
                                                    "inline-flex",
                                                    "items-center",
                                                    "gap-2",
                                                    "rounded-full",
                                                    "bg-black/70",
                                                    "px-3",
                                                    "py-1.5",
                                                    "text-sm",
                                                    "text-white",
                                                    "hover:bg-black"
                                                )}
                                            >
                                                <i class={classes!("fas", "fa-download")}></i>
                                                { t::LIGHTBOX_DOWNLOAD }
                                            </a>
                                        }
                                    } else {
                                        Html::default()
                                    }
                                }
                            </div>
                            <div
                                class={classes!(
                                    "max-h-full",
                                    "max-w-full",
                                    "rounded-[var(--radius)]",
                                    "bg-black/35",
                                    "p-2",
                                    "shadow-[var(--shadow-lg)]"
                                )}
                                onclick={stop_lightbox_bubble.clone()}
                            >
                                {
                                    if let Some(src) = (*preview_image_url).clone() {
                                        html! {
                                            <>
                                                <img
                                                    src={src.clone()}
                                                    alt={t::LIGHTBOX_IMAGE_ALT}
                                                    class={classes!(
                                                        "block",
                                                        "max-h-[90vh]",
                                                        "max-w-[90vw]",
                                                        "h-auto",
                                                        "w-auto",
                                                        "object-contain",
                                                        "cursor-zoom-out"
                                                    )}
                                                    loading="eager"
                                                    decoding="async"
                                                    onerror={mark_preview_failed.clone()}
                                                    onload={mark_preview_loaded.clone()}
                                                    onclick={close_lightbox_click.clone()}
                                                />
                                                {
                                                    if *preview_image_failed {
                                                        html! {
                                                            <div class={classes!(
                                                                "mt-3",
                                                                "max-w-[90vw]",
                                                                "rounded-[var(--radius)]",
                                                                "border",
                                                                "border-red-400/50",
                                                                "bg-black/70",
                                                                "px-3",
                                                                "py-2",
                                                                "text-sm",
                                                                "text-red-100"
                                                            )}>
                                                                { fill_one(t::LIGHTBOX_PREVIEW_FAILED, &src) }
                                                            </div>
                                                        }
                                                    } else {
                                                        html! {}
                                                    }
                                                }
                                            </>
                                        }
                                    } else {
                                        html! {}
                                    }
                                }
                            </div>
                        </div>
                    }
                } else {
                    html! {}
                }
            }
            <ScrollToTopButton />
        </main>
    }
}


fn is_near_page_bottom() -> bool {
    let Some(win) = window() else {
        return false;
    };
    let Some(document) = win.document() else {
        return false;
    };
    let Some(body) = document.body() else {
        return false;
    };
    let viewport_height = win
        .inner_height()
        .ok()
        .and_then(|value| value.as_f64())
        .unwrap_or(0.0);
    let scroll_y = win.scroll_y().unwrap_or(0.0);
    let page_height = body.scroll_height() as f64;
    scroll_y + viewport_height >= page_height - 240.0
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
                    { t::MATCH_BADGE }
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
    enhanced_highlight: Option<bool>,
    limit: Option<usize>,
    all: Option<bool>,
    max_distance: Option<f32>,
}

fn build_search_href(
    mode: Option<&str>,
    keyword: &str,
    enhanced_highlight: bool,
    limit: Option<usize>,
    all: bool,
    max_distance: Option<f32>,
) -> String {
    let has_keyword = !keyword.trim().is_empty();
    let mut params = Vec::new();
    if let Some(mode) = mode {
        if mode != "keyword" {
            params.push(format!("mode={}", urlencoding::encode(mode)));
        }
    }
    if has_keyword {
        params.push(format!("q={}", urlencoding::encode(keyword)));
    }
    if enhanced_highlight {
        params.push("enhanced_highlight=true".to_string());
    }
    if has_keyword {
        if mode != Some("image") {
            if all {
                params.push("all=true".to_string());
            } else if let Some(limit) = limit {
                params.push(format!("limit={limit}"));
            }
        }
        if matches!(mode, Some("semantic") | Some("image")) {
            if let Some(max_distance) = max_distance {
                params.push(format!("max_distance={max_distance}"));
            }
        }
    }

    if params.is_empty() {
        crate::config::route_path("/search")
    } else {
        crate::config::route_path(&format!("/search?{}", params.join("&")))
    }
}
