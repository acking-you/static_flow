use std::collections::BTreeMap;

use gloo_timers::callback::Timeout;
use wasm_bindgen::JsCast;
use web_sys::{window, Event, KeyboardEvent};
use yew::prelude::*;
use yew_router::prelude::*;

use crate::{
    api::{
        fetch_images_page, search_images_by_id_page, search_images_by_text_page,
        semantic_search_articles, ImageInfo, SearchResult,
    },
    components::{
        image_with_loading::ImageWithLoading, pagination::Pagination,
        raw_html::RawHtml,
        scroll_to_top_button::ScrollToTopButton,
    },
    hooks::use_pagination,
    i18n::{current::search as t, fill_one},
    music_context::{MusicAction, MusicPlayerContext},
    router::Route,
    utils::image_url,
};

/// SPA-navigate to `href` without a full page reload.
/// Use as `onclick` on `<a>` tags that would otherwise trigger a browser navigation.
fn spa_navigate(href: &str) {
    if let Some(window) = web_sys::window() {
        if let Ok(history) = window.history() {
            let _ = history.push_state_with_url(
                &wasm_bindgen::JsValue::NULL,
                "",
                Some(href),
            );
            if let Ok(event) = Event::new("popstate") {
                let _ = window.dispatch_event(&event);
            }
        }
    }
}

/// Event-delegation handler: intercept clicks on `<a href="/search...">` inside the
/// search page and convert them to SPA pushState navigation so music playback and
/// other in-memory state survive mode switches.
fn intercept_search_links(e: MouseEvent) {
    let search_prefix = crate::config::route_path("/search");
    // Walk up from the click target to find the nearest <a>
    let mut node = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok());
    while let Some(el) = node {
        if el.tag_name().eq_ignore_ascii_case("A") {
            if let Some(href) = el.get_attribute("href") {
                if href.starts_with(&search_prefix) {
                    e.prevent_default();
                    spa_navigate(&href);
                    return;
                }
            }
            break; // found an <a> but not a search link, let browser handle it
        }
        node = el.parent_element();
    }
}

#[allow(dead_code)]
#[derive(Properties, Clone, PartialEq)]
pub struct SearchPageProps {
    pub query: Option<String>,
}

const DEFAULT_TEXT_SEARCH_LIMIT: usize = 50;
const DEFAULT_IMAGE_SEARCH_LIMIT: usize = 8;
const SEARCH_PAGE_SIZE: usize = 15;
const IMAGE_GRID_CHUNK_SIZE: usize = 8;
const LIGHTBOX_MIN_ZOOM: f64 = 0.5;
const LIGHTBOX_MAX_ZOOM: f64 = 3.0;
const LIGHTBOX_ZOOM_STEP: f64 = 0.25;
const MUSIC_SEARCH_RESULT_LIMIT: usize = 50;

#[function_component(SearchPage)]
pub fn search_page() -> Html {
    let location = use_location();
    let query = location
        .as_ref()
        .and_then(|loc| loc.query::<SearchPageQuery>().ok());
    let keyword = query.as_ref().and_then(|q| q.q.clone()).unwrap_or_default();
    let mode = query
        .as_ref()
        .and_then(|q| q.mode.clone())
        .unwrap_or_else(|| "keyword".to_string())
        .to_lowercase();
    let mode = if matches!(mode.as_str(), "semantic" | "image" | "music") {
        mode
    } else {
        "keyword".to_string()
    };
    let music_sub_mode = query
        .as_ref()
        .and_then(|q| q.music_sub_mode.clone())
        .unwrap_or_else(|| "keyword".to_string())
        .to_lowercase();
    let music_sub_mode = if matches!(music_sub_mode.as_str(), "semantic" | "hybrid") {
        music_sub_mode
    } else {
        "keyword".to_string()
    };
    let enhanced_highlight = query
        .as_ref()
        .and_then(|q| q.enhanced_highlight)
        .unwrap_or(false);
    let hybrid = query.as_ref().and_then(|q| q.hybrid).unwrap_or(false);
    let hybrid_rrf_k = query
        .as_ref()
        .and_then(|q| q.hybrid_rrf_k)
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(60.0);
    let hybrid_vector_limit = query
        .as_ref()
        .and_then(|q| q.hybrid_vector_limit)
        .filter(|value| *value > 0);
    let hybrid_fts_limit = query
        .as_ref()
        .and_then(|q| q.hybrid_fts_limit)
        .filter(|value| *value > 0);
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
    let image_catalog_visible = use_state(|| 0usize);
    let image_text_visible = use_state(|| 0usize);
    let image_similar_visible = use_state(|| 0usize);
    let image_catalog_has_more = use_state(|| false);
    let image_text_has_more = use_state(|| false);
    let image_similar_has_more = use_state(|| false);
    let image_scroll_loading = use_state(|| false);
    let is_lightbox_open = use_state(|| false);
    let preview_image_url = use_state_eq(|| None::<String>);
    let preview_image_failed = use_state(|| false);
    let preview_zoom = use_state(|| 1.0_f64);
    let image_distance_input = use_state(|| {
        max_distance
            .map(|value| value.to_string())
            .unwrap_or_default()
    });
    let semantic_advanced_open = use_state(|| hybrid);
    let music_results = use_state(Vec::<crate::api::SongSearchResult>::new);
    let music_loading = use_state(|| false);
    let player_ctx = use_context::<MusicPlayerContext>();
    let hybrid_rrf_k_input = use_state(|| hybrid_rrf_k.to_string());
    let hybrid_vector_limit_input = use_state(|| {
        hybrid_vector_limit
            .map(|value| value.to_string())
            .unwrap_or_default()
    });
    let hybrid_fts_limit_input = use_state(|| {
        hybrid_fts_limit
            .map(|value| value.to_string())
            .unwrap_or_default()
    });
    let (visible_results, current_page, total_pages, go_to_page) =
        use_pagination((*results).clone(), SEARCH_PAGE_SIZE);

    {
        let mode = mode.clone();
        let keyword = keyword.clone();
        let image_catalog_visible = *image_catalog_visible;
        let image_text_visible = *image_text_visible;
        let image_similar_visible = *image_similar_visible;
        let selected_image_id = (*selected_image_id).clone();
        use_effect_with(
            (
                mode.clone(),
                keyword.clone(),
                current_page,
                image_catalog_visible,
                image_text_visible,
                image_similar_visible,
                selected_image_id.clone(),
            ),
            move |_| {
                let persist = move || {
                    if crate::navigation_context::is_return_armed() {
                        return;
                    }
                    let mut state = BTreeMap::new();
                    state.insert("search_page".to_string(), current_page.to_string());
                    state.insert(
                        "image_catalog_visible".to_string(),
                        image_catalog_visible.to_string(),
                    );
                    state.insert("image_text_visible".to_string(), image_text_visible.to_string());
                    state.insert(
                        "image_similar_visible".to_string(),
                        image_similar_visible.to_string(),
                    );
                    state.insert("mode".to_string(), mode.clone());
                    if !keyword.trim().is_empty() {
                        state.insert("keyword".to_string(), keyword.clone());
                    }
                    if let Some(selected) = selected_image_id.as_ref() {
                        state.insert("selected_image_id".to_string(), selected.clone());
                    }
                    crate::navigation_context::save_context_for_current_page(state);
                };

                persist();

                let on_scroll = wasm_bindgen::closure::Closure::wrap(Box::new(move |_: Event| {
                    persist();
                })
                    as Box<dyn FnMut(_)>);

                if let Some(win) = window() {
                    let _ = win.add_event_listener_with_callback(
                        "scroll",
                        on_scroll.as_ref().unchecked_ref(),
                    );
                }

                move || {
                    if let Some(win) = window() {
                        let _ = win.remove_event_listener_with_callback(
                            "scroll",
                            on_scroll.as_ref().unchecked_ref(),
                        );
                    }
                }
            },
        );
    }

    {
        let location = location.clone();
        let mode = mode.clone();
        let keyword = keyword.clone();
        let loading_flag = *loading;
        let image_loading_flag = *image_loading;
        let image_text_loading_flag = *image_text_loading;
        let go_to_page = go_to_page.clone();
        let image_catalog_visible = image_catalog_visible.clone();
        let image_text_visible = image_text_visible.clone();
        let image_similar_visible = image_similar_visible.clone();
        let selected_image_id = selected_image_id.clone();
        let image_results = image_results.clone();
        let image_loading = image_loading.clone();
        let image_similar_has_more = image_similar_has_more.clone();
        let results_len = results.len();
        let image_catalog_len = image_catalog.len();
        let image_text_results_len = image_text_results.len();
        let image_results_len = image_results.len();

        use_effect_with(
            (
                location.clone(),
                mode.clone(),
                keyword.clone(),
                loading_flag,
                image_loading_flag,
                image_text_loading_flag,
                results_len,
                image_catalog_len,
                image_text_results_len,
                image_results_len,
            ),
            move |_| {
                if crate::navigation_context::is_return_armed() {
                    let data_ready = if mode == "image" {
                        if keyword.trim().is_empty() {
                            !image_loading_flag
                        } else {
                            !image_text_loading_flag
                        }
                    } else {
                        !loading_flag
                    };

                    if data_ready {
                        if let Some(context) =
                            crate::navigation_context::pop_context_if_armed_for_current_page()
                        {
                            if let Some(raw) = context.page_state.get("search_page") {
                                if let Ok(page) = raw.parse::<usize>() {
                                    go_to_page.emit(page);
                                }
                            }
                            if let Some(raw) = context.page_state.get("image_catalog_visible") {
                                if let Ok(value) = raw.parse::<usize>() {
                                    image_catalog_visible.set(value.max(IMAGE_GRID_CHUNK_SIZE));
                                }
                            }
                            if let Some(raw) = context.page_state.get("image_text_visible") {
                                if let Ok(value) = raw.parse::<usize>() {
                                    image_text_visible.set(value.max(IMAGE_GRID_CHUNK_SIZE));
                                }
                            }
                            if let Some(raw) = context.page_state.get("image_similar_visible") {
                                if let Ok(value) = raw.parse::<usize>() {
                                    image_similar_visible.set(value.max(IMAGE_GRID_CHUNK_SIZE));
                                }
                            }
                            if let Some(saved_image_id) =
                                context.page_state.get("selected_image_id")
                            {
                                let saved = saved_image_id.trim().to_string();
                                if !saved.is_empty() {
                                    selected_image_id.set(Some(saved.clone()));
                                    image_loading.set(true);
                                    let image_results = image_results.clone();
                                    let image_loading = image_loading.clone();
                                    let image_similar_has_more = image_similar_has_more.clone();
                                    wasm_bindgen_futures::spawn_local(async move {
                                        match search_images_by_id_page(
                                            &saved,
                                            Some(DEFAULT_IMAGE_SEARCH_LIMIT),
                                            Some(0),
                                            max_distance,
                                        )
                                        .await
                                        {
                                            Ok(data) => {
                                                image_results.set(data.images);
                                                image_similar_has_more.set(data.has_more);
                                                image_loading.set(false);
                                            },
                                            Err(e) => {
                                                web_sys::console::error_1(
                                                    &format!("Image search restore failed: {}", e)
                                                        .into(),
                                                );
                                                image_similar_has_more.set(false);
                                                image_loading.set(false);
                                            },
                                        }
                                    });
                                }
                            }

                            let scroll_y = context.scroll_y.max(0.0);
                            Timeout::new(220, move || {
                                if let Some(win) = window() {
                                    win.scroll_to_with_x_and_y(0.0, scroll_y);
                                }
                            })
                            .forget();
                        }
                    }
                }

                || ()
            },
        );
    }

    {
        let results = results.clone();
        let loading = loading.clone();
        let keyword = keyword.clone();
        let mode = mode.clone();

        use_effect_with(
            (
                keyword.clone(),
                mode.clone(),
                enhanced_highlight,
                active_limit,
                max_distance,
                hybrid,
                hybrid_rrf_k,
                hybrid_vector_limit,
                hybrid_fts_limit,
            ),
            move |(
                kw,
                mode,
                enhanced_highlight,
                active_limit,
                max_distance,
                hybrid,
                hybrid_rrf_k,
                hybrid_vector_limit,
                hybrid_fts_limit,
            )| {
                if mode == "image" || mode == "music" || kw.trim().is_empty() {
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
                    let hybrid_enabled = *hybrid;
                    let hybrid_rrf_k = *hybrid_rrf_k;
                    let hybrid_vector_limit = *hybrid_vector_limit;
                    let hybrid_fts_limit = *hybrid_fts_limit;

                    wasm_bindgen_futures::spawn_local(async move {
                        let response = if use_semantic {
                            semantic_search_articles(
                                &query_text,
                                use_enhanced_highlight,
                                limit,
                                max_distance,
                                hybrid_enabled,
                                if hybrid_enabled { Some(hybrid_rrf_k) } else { None },
                                if hybrid_enabled { hybrid_vector_limit } else { None },
                                if hybrid_enabled { hybrid_fts_limit } else { None },
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

    // Music search effect
    {
        let music_results = music_results.clone();
        let music_loading = music_loading.clone();
        let keyword = keyword.clone();
        let mode = mode.clone();
        let music_sub_mode = music_sub_mode.clone();

        use_effect_with(
            (keyword.clone(), mode.clone(), music_sub_mode.clone()),
            move |(kw, mode, sub_mode)| {
                if mode != "music" || kw.trim().is_empty() {
                    music_loading.set(false);
                    music_results.set(vec![]);
                } else {
                    music_loading.set(true);
                    let music_results = music_results.clone();
                    let music_loading = music_loading.clone();
                    let q = kw.clone();
                    let api_mode = match sub_mode.as_str() {
                        "semantic" => Some("semantic"),
                        "hybrid" => Some("hybrid"),
                        _ => None,
                    };

                    wasm_bindgen_futures::spawn_local(async move {
                        match crate::api::search_songs(
                            &q,
                            Some(MUSIC_SEARCH_RESULT_LIMIT),
                            api_mode,
                        )
                        .await
                        {
                            Ok(data) => music_results.set(data),
                            Err(_) => music_results.set(vec![]),
                        }
                        music_loading.set(false);
                    });
                }
                || ()
            },
        );
    }

    // Keep global playlist synced with music-search results on this page.
    {
        let player_ctx = player_ctx.clone();
        let keyword = keyword.clone();
        let mode = mode.clone();
        let music_sub_mode = music_sub_mode.clone();
        let music_results_snapshot = (*music_results).clone();
        use_effect_with(
            (mode.clone(), keyword.clone(), music_sub_mode.clone(), music_results_snapshot.clone()),
            move |(mode, keyword, music_sub_mode, results)| {
                if let Some(ctx) = player_ctx.as_ref() {
                    if mode == "music" {
                        let ids = if keyword.trim().is_empty() {
                            vec![]
                        } else {
                            results
                                .iter()
                                .map(|item| item.id.clone())
                                .collect::<Vec<_>>()
                        };
                        let source = format!("search-music:{}:{}", music_sub_mode, keyword);
                        ctx.dispatch(MusicAction::SetPlaylist {
                            source,
                            ids,
                        });
                    }
                }
                || ()
            },
        );
    }

    {
        let image_catalog = image_catalog.clone();
        let image_catalog_visible = image_catalog_visible.clone();
        let image_catalog_has_more = image_catalog_has_more.clone();
        let image_loading = image_loading.clone();
        let selected_image_id = selected_image_id.clone();
        let image_results = image_results.clone();
        let image_similar_visible = image_similar_visible.clone();
        let image_similar_has_more = image_similar_has_more.clone();
        let mode = mode.clone();

        use_effect_with(mode.clone(), move |mode| {
            if mode == "image" {
                image_loading.set(true);
                let image_catalog = image_catalog.clone();
                let image_catalog_visible = image_catalog_visible.clone();
                let image_catalog_has_more = image_catalog_has_more.clone();
                let image_loading = image_loading.clone();
                let selected_image_id = selected_image_id.clone();
                let image_results = image_results.clone();
                let image_similar_visible = image_similar_visible.clone();
                let image_similar_has_more = image_similar_has_more.clone();

                wasm_bindgen_futures::spawn_local(async move {
                    match fetch_images_page(Some(IMAGE_GRID_CHUNK_SIZE), Some(0)).await {
                        Ok(data) => {
                            image_catalog_visible.set(data.images.len());
                            image_catalog_has_more.set(data.has_more);
                            image_catalog.set(data.images);
                            image_loading.set(false);
                            selected_image_id.set(None);
                            image_results.set(vec![]);
                            image_similar_visible.set(0);
                            image_similar_has_more.set(false);
                        },
                        Err(e) => {
                            web_sys::console::error_1(
                                &format!("Failed to fetch images: {}", e).into(),
                            );
                            image_catalog_has_more.set(false);
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
        let image_text_visible = image_text_visible.clone();
        let image_text_has_more = image_text_has_more.clone();
        let image_text_loading = image_text_loading.clone();

        use_effect_with(
            (mode.clone(), keyword.clone(), max_distance),
            move |(mode, keyword, max_distance)| {
                if mode != "image" || keyword.trim().is_empty() {
                    image_text_loading.set(false);
                    image_text_results.set(vec![]);
                    image_text_visible.set(0);
                    image_text_has_more.set(false);
                } else {
                    image_text_loading.set(true);
                    let image_text_results = image_text_results.clone();
                    let image_text_visible = image_text_visible.clone();
                    let image_text_has_more = image_text_has_more.clone();
                    let image_text_loading = image_text_loading.clone();
                    let query_text = keyword.clone();
                    let query_distance = *max_distance;

                    wasm_bindgen_futures::spawn_local(async move {
                        match search_images_by_text_page(
                            &query_text,
                            Some(IMAGE_GRID_CHUNK_SIZE),
                            Some(0),
                            query_distance,
                        )
                        .await
                        {
                            Ok(data) => {
                                image_text_visible.set(data.images.len());
                                image_text_has_more.set(data.has_more);
                                image_text_results.set(data.images);
                                image_text_loading.set(false);
                            },
                            Err(e) => {
                                web_sys::console::error_1(
                                    &format!("Text image search failed: {}", e).into(),
                                );
                                image_text_has_more.set(false);
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
        let mode = mode.clone();
        let semantic_advanced_open = semantic_advanced_open.clone();
        let hybrid_rrf_k_input = hybrid_rrf_k_input.clone();
        let hybrid_vector_limit_input = hybrid_vector_limit_input.clone();
        let hybrid_fts_limit_input = hybrid_fts_limit_input.clone();
        use_effect_with(
            (mode.clone(), hybrid, hybrid_rrf_k, hybrid_vector_limit, hybrid_fts_limit),
            move |(mode, hybrid, hybrid_rrf_k, hybrid_vector_limit, hybrid_fts_limit)| {
                if mode == "semantic" {
                    semantic_advanced_open.set(*hybrid);
                    hybrid_rrf_k_input.set(hybrid_rrf_k.to_string());
                    hybrid_vector_limit_input.set(
                        hybrid_vector_limit
                            .map(|value| value.to_string())
                            .unwrap_or_default(),
                    );
                    hybrid_fts_limit_input.set(
                        hybrid_fts_limit
                            .map(|value| value.to_string())
                            .unwrap_or_default(),
                    );
                }
                || ()
            },
        );
    }

    {
        let mode = mode.clone();
        let image_scroll_loading = image_scroll_loading.clone();
        use_effect_with(mode.clone(), move |mode| {
            if mode != "image" {
                image_scroll_loading.set(false);
            }
            || ()
        });
    }

    let open_image_preview = {
        let is_lightbox_open = is_lightbox_open.clone();
        let preview_image_url = preview_image_url.clone();
        let preview_image_failed = preview_image_failed.clone();
        let preview_zoom = preview_zoom.clone();
        Callback::from(move |src: String| {
            preview_image_failed.set(false);
            preview_image_url.set(Some(src));
            preview_zoom.set(1.0);
            is_lightbox_open.set(true);
        })
    };

    let close_lightbox_click = {
        let is_lightbox_open = is_lightbox_open.clone();
        let preview_image_url = preview_image_url.clone();
        let preview_image_failed = preview_image_failed.clone();
        let preview_zoom = preview_zoom.clone();
        Callback::from(move |_| {
            is_lightbox_open.set(false);
            preview_image_url.set(None);
            preview_image_failed.set(false);
            preview_zoom.set(1.0);
        })
    };

    {
        let is_lightbox_open = is_lightbox_open.clone();
        let preview_image_url = preview_image_url.clone();
        let preview_image_failed = preview_image_failed.clone();
        let preview_zoom = preview_zoom.clone();
        use_effect_with(*is_lightbox_open, move |is_open| {
            let keydown_listener_opt = if *is_open {
                let handle = is_lightbox_open.clone();
                let preview_url = preview_image_url.clone();
                let failed = preview_image_failed.clone();
                let zoom = preview_zoom.clone();
                let listener =
                    wasm_bindgen::closure::Closure::wrap(Box::new(move |event: KeyboardEvent| {
                        match event.key().as_str() {
                            "Escape" => {
                                handle.set(false);
                                preview_url.set(None);
                                failed.set(false);
                                zoom.set(1.0);
                            },
                            "+" | "=" => {
                                zoom.set((*zoom + LIGHTBOX_ZOOM_STEP).min(LIGHTBOX_MAX_ZOOM));
                            },
                            "-" | "_" => {
                                zoom.set((*zoom - LIGHTBOX_ZOOM_STEP).max(LIGHTBOX_MIN_ZOOM));
                            },
                            "0" => {
                                zoom.set(1.0);
                            },
                            _ => {},
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
    let zoom_in_click = {
        let preview_zoom = preview_zoom.clone();
        Callback::from(move |event: MouseEvent| {
            event.stop_propagation();
            preview_zoom.set((*preview_zoom + LIGHTBOX_ZOOM_STEP).min(LIGHTBOX_MAX_ZOOM));
        })
    };
    let zoom_out_click = {
        let preview_zoom = preview_zoom.clone();
        Callback::from(move |event: MouseEvent| {
            event.stop_propagation();
            preview_zoom.set((*preview_zoom - LIGHTBOX_ZOOM_STEP).max(LIGHTBOX_MIN_ZOOM));
        })
    };
    let zoom_reset_click = {
        let preview_zoom = preview_zoom.clone();
        Callback::from(move |event: MouseEvent| {
            event.stop_propagation();
            preview_zoom.set(1.0);
        })
    };

    let on_image_select = {
        let image_results = image_results.clone();
        let image_similar_visible = image_similar_visible.clone();
        let image_similar_has_more = image_similar_has_more.clone();
        let image_loading = image_loading.clone();
        let selected_image_id = selected_image_id.clone();

        Callback::from(move |id: String| {
            selected_image_id.set(Some(id.clone()));
            image_loading.set(true);

            let image_results = image_results.clone();
            let image_similar_visible = image_similar_visible.clone();
            let image_similar_has_more = image_similar_has_more.clone();
            let image_loading = image_loading.clone();

            wasm_bindgen_futures::spawn_local(async move {
                match search_images_by_id_page(
                    &id,
                    Some(IMAGE_GRID_CHUNK_SIZE),
                    Some(0),
                    max_distance,
                )
                .await
                {
                    Ok(data) => {
                        image_similar_visible.set(data.images.len());
                        image_similar_has_more.set(data.has_more);
                        image_results.set(data.images);
                        image_loading.set(false);
                    },
                    Err(e) => {
                        web_sys::console::error_1(&format!("Image search failed: {}", e).into());
                        image_similar_has_more.set(false);
                        image_loading.set(false);
                    },
                }
            });
        })
    };

    let load_more_image_text = {
        let keyword = keyword.clone();
        let image_text_results = image_text_results.clone();
        let image_text_visible = image_text_visible.clone();
        let image_text_has_more = image_text_has_more.clone();
        let image_scroll_loading = image_scroll_loading.clone();
        Callback::from(move |event: MouseEvent| {
            event.prevent_default();
            if *image_scroll_loading || !*image_text_has_more || keyword.trim().is_empty() {
                return;
            }
            image_scroll_loading.set(true);
            let keyword = keyword.clone();
            let image_text_results = image_text_results.clone();
            let image_text_visible = image_text_visible.clone();
            let image_text_has_more = image_text_has_more.clone();
            let image_scroll_loading = image_scroll_loading.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let offset = image_text_results.len();
                match search_images_by_text_page(
                    &keyword,
                    Some(IMAGE_GRID_CHUNK_SIZE),
                    Some(offset),
                    max_distance,
                )
                .await
                {
                    Ok(page) => {
                        let mut next = (*image_text_results).clone();
                        next.extend(page.images);
                        let next_len = next.len();
                        image_text_results.set(next);
                        image_text_visible.set(next_len);
                        image_text_has_more.set(page.has_more);
                    },
                    Err(e) => {
                        web_sys::console::error_1(
                            &format!("Text image paging failed: {}", e).into(),
                        );
                        image_text_has_more.set(false);
                    },
                }
                image_scroll_loading.set(false);
            });
        })
    };

    let load_more_image_catalog = {
        let image_catalog = image_catalog.clone();
        let image_catalog_visible = image_catalog_visible.clone();
        let image_catalog_has_more = image_catalog_has_more.clone();
        let image_scroll_loading = image_scroll_loading.clone();
        Callback::from(move |event: MouseEvent| {
            event.prevent_default();
            if *image_scroll_loading || !*image_catalog_has_more {
                return;
            }
            image_scroll_loading.set(true);
            let image_catalog = image_catalog.clone();
            let image_catalog_visible = image_catalog_visible.clone();
            let image_catalog_has_more = image_catalog_has_more.clone();
            let image_scroll_loading = image_scroll_loading.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let offset = image_catalog.len();
                match fetch_images_page(Some(IMAGE_GRID_CHUNK_SIZE), Some(offset)).await {
                    Ok(page) => {
                        let mut next = (*image_catalog).clone();
                        next.extend(page.images);
                        let next_len = next.len();
                        image_catalog.set(next);
                        image_catalog_visible.set(next_len);
                        image_catalog_has_more.set(page.has_more);
                    },
                    Err(e) => {
                        web_sys::console::error_1(
                            &format!("Image catalog paging failed: {}", e).into(),
                        );
                        image_catalog_has_more.set(false);
                    },
                }
                image_scroll_loading.set(false);
            });
        })
    };

    let load_more_similar_images = {
        let selected_image_id = selected_image_id.clone();
        let image_results = image_results.clone();
        let image_similar_visible = image_similar_visible.clone();
        let image_similar_has_more = image_similar_has_more.clone();
        let image_scroll_loading = image_scroll_loading.clone();
        Callback::from(move |event: MouseEvent| {
            event.prevent_default();
            if *image_scroll_loading || !*image_similar_has_more {
                return;
            }
            let Some(selected_id) = (*selected_image_id).clone() else {
                return;
            };
            image_scroll_loading.set(true);
            let image_results = image_results.clone();
            let image_similar_visible = image_similar_visible.clone();
            let image_similar_has_more = image_similar_has_more.clone();
            let image_scroll_loading = image_scroll_loading.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let offset = image_results.len();
                match search_images_by_id_page(
                    &selected_id,
                    Some(IMAGE_GRID_CHUNK_SIZE),
                    Some(offset),
                    max_distance,
                )
                .await
                {
                    Ok(page) => {
                        let mut next = (*image_results).clone();
                        next.extend(page.images);
                        let next_len = next.len();
                        image_results.set(next);
                        image_similar_visible.set(next_len);
                        image_similar_has_more.set(page.has_more);
                    },
                    Err(e) => {
                        web_sys::console::error_1(
                            &format!("Similar image paging failed: {}", e).into(),
                        );
                        image_similar_has_more.set(false);
                    },
                }
                image_scroll_loading.set(false);
            });
        })
    };

    let keyword_href = build_search_href(
        None,
        &keyword,
        false,
        Some(requested_limit),
        fetch_all,
        None,
        false,
        None,
        None,
        None,
        None,
    );
    let semantic_fast_href = build_search_href(
        Some("semantic"),
        &keyword,
        false,
        Some(requested_limit),
        fetch_all,
        max_distance,
        hybrid,
        Some(hybrid_rrf_k),
        hybrid_vector_limit,
        hybrid_fts_limit,
        None,
    );
    let semantic_precise_href = build_search_href(
        Some("semantic"),
        &keyword,
        true,
        Some(requested_limit),
        fetch_all,
        max_distance,
        hybrid,
        Some(hybrid_rrf_k),
        hybrid_vector_limit,
        hybrid_fts_limit,
        None,
    );
    let semantic_href =
        if enhanced_highlight { semantic_precise_href.clone() } else { semantic_fast_href.clone() };
    let image_href = build_search_href(
        Some("image"),
        &keyword,
        false,
        None,
        false,
        max_distance,
        false,
        None,
        None,
        None,
        None,
    );
    let music_href = build_search_href(
        Some("music"),
        &keyword,
        false,
        None,
        false,
        None,
        false,
        None,
        None,
        None,
        Some(music_sub_mode.as_str()),
    );
    let music_sub_keyword_href = build_search_href(
        Some("music"),
        &keyword,
        false,
        None,
        false,
        None,
        false,
        None,
        None,
        None,
        Some("keyword"),
    );
    let music_sub_semantic_href = build_search_href(
        Some("music"),
        &keyword,
        false,
        None,
        false,
        None,
        false,
        None,
        None,
        None,
        Some("semantic"),
    );
    let music_sub_hybrid_href = build_search_href(
        Some("music"),
        &keyword,
        false,
        None,
        false,
        None,
        false,
        None,
        None,
        None,
        Some("hybrid"),
    );
    let scoped_max_distance = if mode == "semantic" { max_distance } else { None };
    let limited_href = build_search_href(
        Some(mode.as_str()),
        &keyword,
        enhanced_highlight,
        Some(requested_limit),
        false,
        scoped_max_distance,
        hybrid,
        Some(hybrid_rrf_k),
        hybrid_vector_limit,
        hybrid_fts_limit,
        None,
    );
    let all_results_href = build_search_href(
        Some(mode.as_str()),
        &keyword,
        enhanced_highlight,
        None,
        true,
        scoped_max_distance,
        hybrid,
        Some(hybrid_rrf_k),
        hybrid_vector_limit,
        hybrid_fts_limit,
        None,
    );
    let hybrid_default_scope_hint = if fetch_all {
        t::HYBRID_DEFAULT_SCOPE_ALL.to_string()
    } else {
        fill_one(t::HYBRID_DEFAULT_SCOPE_LIMIT_TEMPLATE, requested_limit)
    };
    let hybrid_vector_limit_placeholder = if fetch_all {
        t::HYBRID_VECTOR_LIMIT_ALL.to_string()
    } else {
        fill_one(t::HYBRID_VECTOR_LIMIT_SCOPE_TEMPLATE, requested_limit)
    };
    let hybrid_fts_limit_placeholder = if fetch_all {
        t::HYBRID_FTS_LIMIT_ALL.to_string()
    } else {
        fill_one(t::HYBRID_FTS_LIMIT_SCOPE_TEMPLATE, requested_limit)
    };
    let semantic_limit_for_mode = if fetch_all { None } else { Some(requested_limit) };
    let semantic_distance_off_href = build_search_href(
        Some("semantic"),
        &keyword,
        enhanced_highlight,
        semantic_limit_for_mode,
        fetch_all,
        None,
        hybrid,
        Some(hybrid_rrf_k),
        hybrid_vector_limit,
        hybrid_fts_limit,
        None,
    );
    let semantic_distance_strict_href = build_search_href(
        Some("semantic"),
        &keyword,
        enhanced_highlight,
        semantic_limit_for_mode,
        fetch_all,
        Some(0.8),
        hybrid,
        Some(hybrid_rrf_k),
        hybrid_vector_limit,
        hybrid_fts_limit,
        None,
    );
    let semantic_distance_relaxed_href = build_search_href(
        Some("semantic"),
        &keyword,
        enhanced_highlight,
        semantic_limit_for_mode,
        fetch_all,
        Some(1.2),
        hybrid,
        Some(hybrid_rrf_k),
        hybrid_vector_limit,
        hybrid_fts_limit,
        None,
    );
    let image_distance_off_href = build_search_href(
        Some("image"),
        &keyword,
        false,
        None,
        false,
        None,
        false,
        None,
        None,
        None,
        None,
    );
    let parsed_image_distance_input = image_distance_input
        .trim()
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0);
    let image_distance_apply_href = build_search_href(
        Some("image"),
        &keyword,
        false,
        None,
        false,
        parsed_image_distance_input,
        false,
        None,
        None,
        None,
        None,
    );

    let on_hybrid_rrf_k_input = {
        let hybrid_rrf_k_input = hybrid_rrf_k_input.clone();
        Callback::from(move |event: InputEvent| {
            if let Some(target) = event.target_dyn_into::<web_sys::HtmlInputElement>() {
                hybrid_rrf_k_input.set(target.value());
            }
        })
    };
    let on_hybrid_vector_limit_input = {
        let hybrid_vector_limit_input = hybrid_vector_limit_input.clone();
        Callback::from(move |event: InputEvent| {
            if let Some(target) = event.target_dyn_into::<web_sys::HtmlInputElement>() {
                hybrid_vector_limit_input.set(target.value());
            }
        })
    };
    let on_hybrid_fts_limit_input = {
        let hybrid_fts_limit_input = hybrid_fts_limit_input.clone();
        Callback::from(move |event: InputEvent| {
            if let Some(target) = event.target_dyn_into::<web_sys::HtmlInputElement>() {
                hybrid_fts_limit_input.set(target.value());
            }
        })
    };
    let parsed_hybrid_rrf_k_input = hybrid_rrf_k_input
        .trim()
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite() && *value > 0.0);
    let parsed_hybrid_vector_limit_input = hybrid_vector_limit_input
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0);
    let parsed_hybrid_fts_limit_input = hybrid_fts_limit_input
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0);
    let semantic_hybrid_off_href = build_search_href(
        Some("semantic"),
        &keyword,
        enhanced_highlight,
        semantic_limit_for_mode,
        fetch_all,
        max_distance,
        false,
        None,
        None,
        None,
        None,
    );
    let semantic_hybrid_on_href = build_search_href(
        Some("semantic"),
        &keyword,
        enhanced_highlight,
        semantic_limit_for_mode,
        fetch_all,
        max_distance,
        true,
        Some(hybrid_rrf_k),
        hybrid_vector_limit,
        hybrid_fts_limit,
        None,
    );
    let semantic_hybrid_apply_href = build_search_href(
        Some("semantic"),
        &keyword,
        enhanced_highlight,
        semantic_limit_for_mode,
        fetch_all,
        max_distance,
        hybrid,
        parsed_hybrid_rrf_k_input.or(Some(hybrid_rrf_k)),
        parsed_hybrid_vector_limit_input,
        parsed_hybrid_fts_limit_input,
        None,
    );
    let toggle_semantic_advanced = {
        let semantic_advanced_open = semantic_advanced_open.clone();
        Callback::from(move |_| semantic_advanced_open.set(!*semantic_advanced_open))
    };
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

    let hero_label = if mode == "music" && !keyword.is_empty() {
        keyword.clone()
    } else if mode == "music" {
        "MUSIC SEARCH".to_string()
    } else if mode == "image" && !keyword.is_empty() {
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
        <main onclick={intercept_search_links} class={classes!(
            "search-page",
            "min-h-[60vh]",
            "mt-[var(--header-height-mobile)]",
            "md:mt-[var(--header-height-desktop)]",
            "pb-20"
        )}>
            <div class={classes!("container")}>
                <button onclick={Callback::from(|_: MouseEvent| {
                        if let Some(w) = web_sys::window() { let _ = w.history().map(|h| h.back()); }
                    })} type="button"
                    class="flex items-center gap-1.5 text-sm text-[var(--muted)] hover:text-[var(--text)] transition-colors mt-4 mb-0">
                    <i class="fas fa-arrow-left text-xs" />
                    {"Back"}
                </button>
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
                        if mode == "music" && !keyword.is_empty() {
                            if *music_loading {
                                <span class={classes!("search-status-loading")}>
                                    <i class={classes!("fas", "fa-spinner", "fa-spin", "mr-2")}></i>
                                    { t::MUSIC_SEARCHING }
                                </span>
                            } else if music_results.is_empty() {
                                { fill_one(t::MUSIC_MISS_TEMPLATE, &keyword) }
                            } else {
                                <span class={classes!("search-status-found")}>
                                    { fill_one(t::MUSIC_FOUND_TEMPLATE, music_results.len().to_string()) }
                                </span>
                            }
                        } else if mode == "music" {
                            { "MUSIC SEARCH" }
                        } else if mode == "image" && !keyword.is_empty() {
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
                                if mode == "music" {
                                    if *music_loading {
                                        { t::STATUS_SCANNING }
                                    } else if !keyword.is_empty() {
                                        { format!("{} RESULTS", music_results.len()) }
                                    } else {
                                        { t::STATUS_READY }
                                    }
                                } else if mode == "image" {
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
                        <a
                            href={music_href}
                            class={classes!(
                                mode_button_base.clone(),
                                if mode == "music" { "border-[var(--primary)]" } else { "border-[var(--border)]" },
                                if mode == "music" { "text-[var(--primary)]" } else { "text-[var(--muted)]" },
                                if mode == "music" { "bg-[var(--primary)]/10" } else { "" },
                                if mode != "music" { "hover:text-[var(--primary)]" } else { "" },
                                if mode != "music" { "hover:border-[var(--primary)]/60" } else { "" }
                            )}
                        >
                            { t::MODE_MUSIC }
                        </a>
                    </div>

                    if mode == "music" && !keyword.is_empty() {
                        <div class="mt-6 flex items-center justify-center gap-3 flex-wrap">
                            <a
                                href={music_sub_keyword_href}
                                class={classes!(
                                    "inline-flex", "items-center", "gap-2",
                                    "px-5", "py-2.5", "rounded-xl",
                                    "border", "text-sm", "font-semibold",
                                    "transition-all", "duration-200",
                                    if music_sub_mode == "keyword" { "border-[var(--primary)] text-[var(--primary)] bg-[var(--primary)]/10 shadow-sm" }
                                    else { "border-[var(--border)] text-[var(--muted)] hover:text-[var(--primary)] hover:border-[var(--primary)]/60" }
                                )}
                            >
                                <i class="fas fa-font text-xs" />
                                { "Keyword" }
                            </a>
                            <a
                                href={music_sub_semantic_href.clone()}
                                class={classes!(
                                    "inline-flex", "items-center", "gap-2",
                                    "px-5", "py-2.5", "rounded-xl",
                                    "border", "text-sm", "font-semibold",
                                    "transition-all", "duration-200",
                                    if music_sub_mode == "semantic" { "border-[var(--primary)] text-[var(--primary)] bg-[var(--primary)]/10 shadow-sm" }
                                    else { "border-[var(--border)] text-[var(--muted)] hover:text-[var(--primary)] hover:border-[var(--primary)]/60" }
                                )}
                            >
                                <i class="fas fa-brain text-xs" />
                                { "Semantic" }
                            </a>
                            <a
                                href={music_sub_hybrid_href.clone()}
                                class={classes!(
                                    "inline-flex", "items-center", "gap-2",
                                    "px-5", "py-2.5", "rounded-xl",
                                    "border", "text-sm", "font-semibold",
                                    "transition-all", "duration-200",
                                    if music_sub_mode == "hybrid" { "border-[var(--primary)] text-[var(--primary)] bg-[var(--primary)]/10 shadow-sm" }
                                    else { "border-[var(--border)] text-[var(--muted)] hover:text-[var(--primary)] hover:border-[var(--primary)]/60" }
                                )}
                            >
                                <i class="fas fa-layer-group text-xs" />
                                { "Hybrid" }
                            </a>
                        </div>
                    }

                    if mode != "image" && mode != "music" && !keyword.is_empty() {
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

                    if mode == "semantic" && !keyword.is_empty() {
                        <div class={classes!(
                            "mt-4",
                            "mx-auto",
                            "max-w-4xl",
                            "rounded-xl",
                            "border",
                            "border-[var(--primary)]/30",
                            "bg-[var(--surface)]",
                            "liquid-glass",
                            "px-4",
                            "py-4"
                        )}>
                            <div class={classes!(
                                "flex",
                                "items-center",
                                "justify-between",
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
                                    { t::HYBRID_PANEL_TITLE }
                                </span>
                                <button
                                    type="button"
                                    onclick={toggle_semantic_advanced}
                                    class={classes!(
                                        mode_button_base.clone(),
                                        "text-xs",
                                        "border-[var(--border)]",
                                        "text-[var(--muted)]",
                                        "hover:text-[var(--primary)]",
                                        "hover:border-[var(--primary)]/60"
                                    )}
                                >
                                    {
                                        if *semantic_advanced_open {
                                            t::HYBRID_ADVANCED_HIDE
                                        } else {
                                            t::HYBRID_ADVANCED_SHOW
                                        }
                                    }
                                </button>
                            </div>
                            <p class={classes!("mt-3", "text-sm", "text-[var(--muted)]")}>
                                { t::HYBRID_PANEL_DESC }
                            </p>
                            <p class={classes!("mt-2", "text-xs", "text-[var(--muted)]")}>
                                { hybrid_default_scope_hint.clone() }
                            </p>

                            if *semantic_advanced_open {
                                <div class={classes!(
                                    "mt-4",
                                    "flex",
                                    "items-center",
                                    "justify-center",
                                    "gap-3",
                                    "flex-wrap"
                                )}>
                                    <a
                                        href={semantic_hybrid_off_href}
                                        class={classes!(
                                            mode_button_base.clone(),
                                            "text-xs",
                                            if !hybrid { "border-[var(--primary)]" } else { "border-[var(--border)]" },
                                            if !hybrid { "text-[var(--primary)]" } else { "text-[var(--muted)]" },
                                            if !hybrid { "bg-[var(--primary)]/10" } else { "" },
                                            if hybrid { "hover:text-[var(--primary)]" } else { "" },
                                            if hybrid { "hover:border-[var(--primary)]/60" } else { "" }
                                        )}
                                    >
                                        { t::HYBRID_OFF }
                                    </a>
                                    <a
                                        href={semantic_hybrid_on_href}
                                        class={classes!(
                                            mode_button_base.clone(),
                                            "text-xs",
                                            if hybrid { "border-[var(--primary)]" } else { "border-[var(--border)]" },
                                            if hybrid { "text-[var(--primary)]" } else { "text-[var(--muted)]" },
                                            if hybrid { "bg-[var(--primary)]/10" } else { "" },
                                            if !hybrid { "hover:text-[var(--primary)]" } else { "" },
                                            if !hybrid { "hover:border-[var(--primary)]/60" } else { "" }
                                        )}
                                    >
                                        { t::HYBRID_ON }
                                    </a>
                                </div>
                            }

                            if *semantic_advanced_open && hybrid {
                                <div class={classes!(
                                    "mt-4",
                                    "grid",
                                    "grid-cols-1",
                                    "md:grid-cols-3",
                                    "gap-3"
                                )}>
                                    <input
                                        type="number"
                                        step="1"
                                        min="1"
                                        value={(*hybrid_rrf_k_input).clone()}
                                        placeholder={t::HYBRID_RRF_K}
                                        oninput={on_hybrid_rrf_k_input}
                                        class={classes!(
                                            "h-10",
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
                                    <input
                                        type="number"
                                        step="1"
                                        min="1"
                                        value={(*hybrid_vector_limit_input).clone()}
                                        placeholder={hybrid_vector_limit_placeholder.clone()}
                                        oninput={on_hybrid_vector_limit_input}
                                        class={classes!(
                                            "h-10",
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
                                    <input
                                        type="number"
                                        step="1"
                                        min="1"
                                        value={(*hybrid_fts_limit_input).clone()}
                                        placeholder={hybrid_fts_limit_placeholder.clone()}
                                        oninput={on_hybrid_fts_limit_input}
                                        class={classes!(
                                            "h-10",
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
                                </div>
                            }

                            if *semantic_advanced_open && hybrid {
                                <div class={classes!(
                                    "mt-3",
                                    "flex",
                                    "items-center",
                                    "justify-center",
                                    "gap-3",
                                    "flex-wrap"
                                )}>
                                    <a
                                        href={semantic_hybrid_apply_href}
                                        class={classes!(
                                            mode_button_base.clone(),
                                            "text-xs",
                                            "border-[var(--border)]",
                                            "text-[var(--muted)]",
                                            "hover:text-[var(--primary)]",
                                            "hover:border-[var(--primary)]/60"
                                        )}
                                    >
                                        { t::HYBRID_APPLY }
                                    </a>
                                </div>
                            }
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
                                    if *image_text_has_more {
                                        <div class={classes!(
                                            "flex",
                                            "items-center",
                                            "justify-center",
                                            "py-2"
                                        )}>
                                            <button
                                                type="button"
                                                onclick={load_more_image_text.clone()}
                                                disabled={*image_scroll_loading}
                                                class={classes!(
                                                    mode_button_base.clone(),
                                                    "text-xs",
                                                    "border-[var(--primary)]/40",
                                                    "text-[var(--primary)]",
                                                    "bg-[var(--primary)]/6",
                                                    "hover:bg-[var(--primary)]/12",
                                                    "disabled:opacity-60",
                                                    "disabled:cursor-not-allowed"
                                                )}
                                            >
                                                <i class={classes!(
                                                    "fas",
                                                    if *image_scroll_loading { "fa-spinner fa-spin" } else { "fa-plus" }
                                                )}></i>
                                                <span class="ml-2">
                                                    { if *image_scroll_loading { t::IMAGE_SCROLL_LOADING } else { t::IMAGE_SCROLL_HINT } }
                                                </span>
                                            </button>
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
                                if *image_catalog_has_more {
                                    <div class={classes!(
                                        "flex",
                                        "items-center",
                                        "justify-center",
                                        "py-2"
                                    )}>
                                        <button
                                            type="button"
                                            onclick={load_more_image_catalog.clone()}
                                            disabled={*image_scroll_loading}
                                            class={classes!(
                                                mode_button_base.clone(),
                                                "text-xs",
                                                "border-[var(--primary)]/40",
                                                "text-[var(--primary)]",
                                                "bg-[var(--primary)]/6",
                                                "hover:bg-[var(--primary)]/12",
                                                "disabled:opacity-60",
                                                "disabled:cursor-not-allowed"
                                            )}
                                        >
                                            <i class={classes!(
                                                "fas",
                                                if *image_scroll_loading { "fa-spinner fa-spin" } else { "fa-plus" }
                                            )}></i>
                                            <span class="ml-2">
                                                { if *image_scroll_loading { t::IMAGE_SCROLL_LOADING } else { t::IMAGE_SCROLL_HINT } }
                                            </span>
                                        </button>
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
                                    if *image_similar_has_more {
                                        <div class={classes!(
                                            "flex",
                                            "items-center",
                                            "justify-center",
                                            "py-2"
                                        )}>
                                            <button
                                                type="button"
                                                onclick={load_more_similar_images.clone()}
                                                disabled={*image_scroll_loading}
                                                class={classes!(
                                                    mode_button_base.clone(),
                                                    "text-xs",
                                                    "border-[var(--primary)]/40",
                                                    "text-[var(--primary)]",
                                                    "bg-[var(--primary)]/6",
                                                    "hover:bg-[var(--primary)]/12",
                                                    "disabled:opacity-60",
                                                    "disabled:cursor-not-allowed"
                                                )}
                                            >
                                                <i class={classes!(
                                                    "fas",
                                                    if *image_scroll_loading { "fa-spinner fa-spin" } else { "fa-plus" }
                                                )}></i>
                                                <span class="ml-2">
                                                    { if *image_scroll_loading { t::IMAGE_SCROLL_LOADING } else { t::IMAGE_SCROLL_HINT } }
                                                </span>
                                            </button>
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
                    } else if mode == "music" && *music_loading {
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
                    } else if mode == "music" && !music_results.is_empty() {
                        <div class={classes!(
                            "grid",
                            "grid-cols-2",
                            "sm:grid-cols-3",
                            "lg:grid-cols-4",
                            "xl:grid-cols-5",
                            "gap-5"
                        )}>
                            { for music_results.iter().map(|r| {
                                let cover_url = crate::api::song_cover_url(r.cover_image.as_deref());
                                let id = r.id.clone();
                                html! {
                                    <Link<Route> to={Route::MusicPlayer { id }}>
                                        <div class="group bg-[var(--surface)] liquid-glass border border-[var(--border)] rounded-xl \
                                                    overflow-hidden flex flex-col transition-all duration-300 ease-out \
                                                    hover:shadow-[var(--shadow-8)] hover:border-[var(--primary)] hover:-translate-y-2">
                                            <div class="aspect-square bg-[var(--surface-alt)] relative overflow-hidden">
                                                if cover_url.is_empty() {
                                                    <div class="w-full h-full flex items-center justify-center text-[var(--muted)]">
                                                        <i class="fas fa-music text-5xl opacity-30"></i>
                                                    </div>
                                                } else {
                                                    <ImageWithLoading
                                                        src={cover_url}
                                                        alt={r.title.clone()}
                                                        loading={Some(AttrValue::from("lazy"))}
                                                        referrerpolicy={Some(AttrValue::from("no-referrer"))}
                                                        class="w-full h-full object-cover transition-transform duration-500 ease-out group-hover:scale-105"
                                                        container_class={classes!("w-full", "h-full")}
                                                    />
                                                }
                                                <div class="absolute inset-0 bg-black/0 group-hover:bg-black/30 transition-all duration-300 \
                                                            flex items-center justify-center opacity-0 group-hover:opacity-100">
                                                    <div class="w-12 h-12 rounded-full bg-white/90 flex items-center justify-center shadow-lg">
                                                        <i class="fas fa-play text-black text-lg"></i>
                                                    </div>
                                                </div>
                                            </div>
                                            <div class="p-3">
                                                <h3 class="text-sm font-semibold text-[var(--text)] truncate"
                                                    style="font-family: 'Fraunces', serif;">
                                                    {&r.title}
                                                </h3>
                                                <p class="text-xs text-[var(--muted)] truncate mt-0.5">
                                                    {&r.artist}
                                                </p>
                                            </div>
                                        </div>
                                    </Link<Route>>
                                }
                            })}
                        </div>
                    } else if mode == "music" && !keyword.is_empty() {
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
                                "fa-music",
                                "text-6xl",
                                "text-[var(--primary)]",
                                "mb-6",
                                "opacity-50"
                            )}></i>
                            <p class={classes!("text-xl", "mb-2", "font-bold")} style="font-family: 'Space Mono', monospace;">
                                { t::NO_RESULTS_TITLE }
                            </p>
                            <p class={classes!("text-base", "text-[var(--muted)]", "opacity-70")}>
                                { fill_one(t::MUSIC_MISS_TEMPLATE, &keyword) }
                            </p>
                            if music_sub_mode == "keyword" {
                                <p class="text-sm text-[var(--muted)] mt-3 mb-5">
                                    { t::MUSIC_TRY_HINT }
                                </p>
                                <div class="flex items-center justify-center gap-3 flex-wrap">
                                    <a href={music_sub_semantic_href.clone()}
                                        class="inline-flex items-center gap-2 px-6 py-3 rounded-xl border-2 border-[var(--primary)] \
                                               text-[var(--primary)] bg-[var(--primary)]/10 font-semibold text-sm \
                                               hover:bg-[var(--primary)]/20 transition-all duration-200 shadow-sm">
                                        <i class="fas fa-brain" />
                                        { t::MUSIC_TRY_SEMANTIC }
                                    </a>
                                    <a href={music_sub_hybrid_href.clone()}
                                        class="inline-flex items-center gap-2 px-6 py-3 rounded-xl border-2 border-[var(--primary)]/60 \
                                               text-[var(--primary)] font-semibold text-sm \
                                               hover:bg-[var(--primary)]/10 hover:border-[var(--primary)] transition-all duration-200">
                                        <i class="fas fa-layer-group" />
                                        { t::MUSIC_TRY_HYBRID }
                                    </a>
                                </div>
                            }
                        </div>
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
                                            <>
                                                <button
                                                    type="button"
                                                    class={classes!(
                                                        "rounded-full",
                                                        "bg-black/70",
                                                        "px-3",
                                                        "py-1.5",
                                                        "text-sm",
                                                        "font-semibold",
                                                        "text-white",
                                                        "hover:bg-black"
                                                    )}
                                                    aria-label={t::LIGHTBOX_ZOOM_OUT_ARIA}
                                                    onclick={zoom_out_click.clone()}
                                                >
                                                    { "-" }
                                                </button>
                                                <button
                                                    type="button"
                                                    class={classes!(
                                                        "rounded-full",
                                                        "bg-black/70",
                                                        "px-3",
                                                        "py-1.5",
                                                        "text-sm",
                                                        "font-semibold",
                                                        "text-white",
                                                        "hover:bg-black"
                                                    )}
                                                    aria-label={t::LIGHTBOX_ZOOM_RESET_ARIA}
                                                    onclick={zoom_reset_click.clone()}
                                                >
                                                    { format!("{:.0}%", *preview_zoom * 100.0) }
                                                </button>
                                                <button
                                                    type="button"
                                                    class={classes!(
                                                        "rounded-full",
                                                        "bg-black/70",
                                                        "px-3",
                                                        "py-1.5",
                                                        "text-sm",
                                                        "font-semibold",
                                                        "text-white",
                                                        "hover:bg-black"
                                                    )}
                                                    aria-label={t::LIGHTBOX_ZOOM_IN_ARIA}
                                                    onclick={zoom_in_click.clone()}
                                                >
                                                    { "+" }
                                                </button>
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
                                            </>
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
                                    "overflow-auto",
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
                                                        "transition-transform",
                                                        "duration-150"
                                                    )}
                                                    style={format!("transform: scale({}); transform-origin: center center;", *preview_zoom)}
                                                    loading="eager"
                                                    decoding="async"
                                                    onerror={mark_preview_failed.clone()}
                                                    onload={mark_preview_loaded.clone()}
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


fn render_search_result(result: &SearchResult) -> Html {
    let highlight_html = AttrValue::from(result.highlight.clone());

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
                    <RawHtml html={highlight_html} />
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
    music_sub_mode: Option<String>,
    enhanced_highlight: Option<bool>,
    hybrid: Option<bool>,
    hybrid_rrf_k: Option<f32>,
    hybrid_vector_limit: Option<usize>,
    hybrid_fts_limit: Option<usize>,
    limit: Option<usize>,
    all: Option<bool>,
    max_distance: Option<f32>,
}

#[allow(clippy::too_many_arguments)]
fn build_search_href(
    mode: Option<&str>,
    keyword: &str,
    enhanced_highlight: bool,
    limit: Option<usize>,
    all: bool,
    max_distance: Option<f32>,
    hybrid: bool,
    hybrid_rrf_k: Option<f32>,
    hybrid_vector_limit: Option<usize>,
    hybrid_fts_limit: Option<usize>,
    music_sub_mode: Option<&str>,
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
        if mode == Some("semantic") && hybrid {
            params.push("hybrid=true".to_string());
            if let Some(rrf_k) = hybrid_rrf_k.filter(|value| value.is_finite() && *value > 0.0) {
                params.push(format!("hybrid_rrf_k={rrf_k}"));
            }
            if let Some(vector_limit) = hybrid_vector_limit.filter(|value| *value > 0) {
                params.push(format!("hybrid_vector_limit={vector_limit}"));
            }
            if let Some(fts_limit) = hybrid_fts_limit.filter(|value| *value > 0) {
                params.push(format!("hybrid_fts_limit={fts_limit}"));
            }
        }
        if mode == Some("music") {
            if let Some(sub) = music_sub_mode {
                if sub != "keyword" {
                    params.push(format!("music_sub_mode={sub}"));
                }
            }
        }
    }

    if params.is_empty() {
        crate::config::route_path("/search")
    } else {
        crate::config::route_path(&format!("/search?{}", params.join("&")))
    }
}
