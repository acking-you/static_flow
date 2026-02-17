use gloo_timers::{callback::Timeout, future::TimeoutFuture};
use static_flow_shared::{Article, ArticleListItem};
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{
    window, Element, HtmlImageElement, HtmlSelectElement, HtmlTextAreaElement, KeyboardEvent, Node,
};
use yew::{prelude::*, virtual_dom::AttrValue};
use yew_router::prelude::{use_navigator, use_route, Link};

use crate::{
    api::{
        fetch_article_comment_stats, fetch_article_comments, fetch_article_view_trend,
        fetch_related_articles, submit_article_comment, track_article_view, ArticleComment,
        ArticleViewPoint, SubmitCommentRequest,
    },
    components::{
        article_card::ArticleCard,
        icons::IconName,
        image_with_loading::ImageWithLoading,
        loading_spinner::{LoadingSpinner, SpinnerSize},
        scroll_to_top_button::ScrollToTopButton,
        toc_button::TocButton,
        tooltip::{TooltipIconButton, TooltipPosition},
        view_trend_chart::ViewTrendChart,
    },
    i18n::{current::article_detail_page as t, fill_one},
    router::Route,
    utils::{image_url, markdown_to_html},
};

#[derive(Properties, Clone, PartialEq)]
pub struct ArticleDetailProps {
    #[prop_or_default]
    pub id: String,
}

type ImageClickListener =
    (web_sys::Element, wasm_bindgen::closure::Closure<dyn FnMut(web_sys::Event)>);

#[derive(Clone, Copy, PartialEq, Eq)]
enum ArticleContentLanguage {
    Zh,
    En,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TrendGranularity {
    Day,
    Hour,
}

const LIGHTBOX_MIN_ZOOM: f64 = 0.5;
const LIGHTBOX_MAX_ZOOM: f64 = 3.0;
const LIGHTBOX_ZOOM_STEP: f64 = 0.25;
const COMMENT_SECTION_ID: &str = "article-comments-section";

#[derive(Clone, PartialEq)]
struct SelectionCommentDraft {
    selected_text: String,
    anchor_block_id: Option<String>,
    anchor_context_before: Option<String>,
    anchor_context_after: Option<String>,
}

#[derive(Clone, PartialEq)]
struct CommentReplyDraft {
    comment_id: String,
    author_name: String,
    comment_text: String,
    ai_reply_markdown: Option<String>,
}

fn normalize_excerpt(value: &str, max_chars: usize) -> Option<String> {
    let compact = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();
    if compact.is_empty() {
        return None;
    }
    Some(compact.chars().take(max_chars).collect::<String>())
}

fn extract_anchor_context(
    block_text: Option<String>,
    selected_text: &str,
) -> (Option<String>, Option<String>) {
    let Some(block_text) = block_text else {
        return (None, None);
    };
    let selected = selected_text.trim();
    if selected.is_empty() {
        return (None, None);
    }

    if let Some(found_at) = block_text.find(selected) {
        let before = block_text[..found_at]
            .chars()
            .rev()
            .take(120)
            .collect::<String>();
        let before = before.chars().rev().collect::<String>();
        let after_start = found_at.saturating_add(selected.len());
        let after = block_text[after_start..]
            .chars()
            .take(120)
            .collect::<String>();
        return (normalize_excerpt(&before, 120), normalize_excerpt(&after, 120));
    }

    (None, None)
}

fn node_in_article(node: &Node, article_root: &Element) -> bool {
    let mut cursor = if node.node_type() == Node::ELEMENT_NODE {
        node.clone().dyn_into::<Element>().ok()
    } else {
        node.parent_element()
    };

    while let Some(el) = cursor {
        if el.is_same_node(Some(article_root)) {
            return true;
        }
        cursor = el.parent_element();
    }
    false
}

fn find_anchor_block(
    common_node: &Node,
    article_root: &Element,
) -> (Option<String>, Option<String>) {
    let mut cursor = if common_node.node_type() == Node::ELEMENT_NODE {
        common_node.clone().dyn_into::<Element>().ok()
    } else {
        common_node.parent_element()
    };

    while let Some(el) = cursor {
        if let Some(block_id) = el
            .get_attribute("data-sf-block-id")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            return (Some(block_id), el.text_content());
        }
        if el.is_same_node(Some(article_root)) {
            break;
        }
        cursor = el.parent_element();
    }

    (None, None)
}

fn capture_selection_draft() -> Option<(SelectionCommentDraft, (f64, f64))> {
    let win = window()?;
    let selection = win.get_selection().ok().flatten()?;
    let selected_text: String = selection.to_string().into();
    let selected_text = selected_text.trim().to_string();
    if selected_text.chars().count() < 2 {
        return None;
    }

    let range = selection.get_range_at(0).ok()?;
    if range.collapsed() {
        return None;
    }

    let document = win.document()?;
    let article_root = document.query_selector(".article-content").ok().flatten()?;
    let common_node = range.common_ancestor_container().ok()?;
    if !node_in_article(&common_node, &article_root) {
        return None;
    }

    let (anchor_block_id, block_text) = find_anchor_block(&common_node, &article_root);
    let (before, after) = extract_anchor_context(block_text, &selected_text);

    let rect = range.get_bounding_client_rect();
    let viewport_w = win
        .inner_width()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(1280.0);
    let viewport_h = win
        .inner_height()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(720.0);

    let mut left = rect.x() + (rect.width() / 2.0) - 68.0;
    let mut top = rect.y() - 48.0;
    if !left.is_finite() || !top.is_finite() {
        left = 24.0;
        top = 24.0;
    }

    let max_left = (viewport_w - 136.0).max(12.0);
    let max_top = (viewport_h - 54.0).max(12.0);
    left = left.clamp(12.0, max_left);
    top = top.clamp(12.0, max_top);

    Some((
        SelectionCommentDraft {
            selected_text,
            anchor_block_id,
            anchor_context_before: before,
            anchor_context_after: after,
        },
        (left, top),
    ))
}

fn scroll_to_element_id(id: &str) {
    if let Some(target) = window()
        .and_then(|win| win.document())
        .and_then(|doc| doc.get_element_by_id(id))
    {
        target.scroll_into_view();
    }
}

fn scroll_to_anchor_block(block_id: &str) {
    let target = window().and_then(|win| win.document()).and_then(|doc| {
        let selector =
            format!(".article-content [data-sf-block-id=\"{}\"]", block_id.replace('"', "\\\""));
        doc.query_selector(&selector).ok().flatten()
    });

    let Some(target) = target else {
        return;
    };

    target.scroll_into_view();
    let _ = target.class_list().add_1("sf-comment-anchor-flash");
    let target_clone = target.clone();
    Timeout::new(1600, move || {
        let _ = target_clone
            .class_list()
            .remove_1("sf-comment-anchor-flash");
    })
    .forget();
}

fn scroll_to_comment_card(comment_id: &str) {
    let target = window()
        .and_then(|win| win.document())
        .and_then(|doc| doc.get_element_by_id(&format!("comment-item-{}", comment_id)));

    let Some(target) = target else {
        return;
    };
    target.scroll_into_view();
    let _ = target.class_list().add_1("sf-comment-anchor-flash");
    let target_clone = target.clone();
    Timeout::new(1600, move || {
        let _ = target_clone
            .class_list()
            .remove_1("sf-comment-anchor-flash");
    })
    .forget();
}

fn avatar_hue(seed: &str) -> u32 {
    let mut hash: u32 = 0;
    for byte in seed.bytes() {
        hash = hash.wrapping_mul(16777619) ^ (byte as u32);
    }
    hash % 360
}

fn comment_avatar_initials(author_name: &str, avatar_seed: &str) -> String {
    let trimmed = author_name.trim();
    if let Some(suffix) = trimmed
        .strip_prefix("Reader-")
        .or_else(|| trimmed.strip_prefix("reader-"))
    {
        let value = suffix
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .take(2)
            .collect::<String>();
        if !value.is_empty() {
            return value.to_ascii_uppercase();
        }
    }

    let parts = trimmed
        .split_whitespace()
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();

    if let (Some(first), Some(last)) = (parts.first(), parts.last()) {
        if parts.len() >= 2 {
            let mut value = String::new();
            if let Some(ch) = first.chars().find(|ch| ch.is_alphanumeric()) {
                value.push(ch);
            }
            if let Some(ch) = last.chars().find(|ch| ch.is_alphanumeric()) {
                value.push(ch);
            }
            let value = value.trim().to_string();
            if !value.is_empty() {
                return value.to_uppercase();
            }
        }
    }

    let single = trimmed
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .take(2)
        .collect::<String>();
    if !single.is_empty() {
        return single.to_uppercase();
    }

    let fallback = avatar_seed
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(2)
        .collect::<String>();
    if !fallback.is_empty() {
        return fallback.to_ascii_uppercase();
    }

    "RD".to_string()
}

fn comment_avatar_style(seed: &str) -> String {
    let base = avatar_hue(seed);
    let accent = (base + 38) % 360;
    let ring = (base + 184) % 360;
    format!(
        "background: linear-gradient(136deg, hsl({base} 72% 38%), hsl({accent} 72% 52%)); \
         box-shadow: inset 0 0 0 1px rgba(255,255,255,0.24), 0 0 0 1px hsl({ring} 46% 56% / \
         0.28), 0 8px 16px rgba(10, 16, 30, 0.16);"
    )
}

fn display_comment_region(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("unknown")
        || trimmed.eq_ignore_ascii_case("lan")
    {
        return "-".to_string();
    }

    let segments = trimmed
        .split('/')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments.len() < 2 {
        return "-".to_string();
    }

    segments.join("/")
}

fn format_published_time(ts_ms: i64) -> String {
    let value = js_sys::Date::new(&JsValue::from_f64(ts_ms as f64))
        .to_iso_string()
        .as_string()
        .unwrap_or_else(|| ts_ms.to_string());
    value.replace('T', " ").trim_end_matches('Z').to_string()
}

#[function_component(ArticleDetailPage)]
pub fn article_detail_page(props: &ArticleDetailProps) -> Html {
    let route = use_route::<Route>();
    let navigator = use_navigator();

    let article_id = route
        .as_ref()
        .and_then(|r| match r {
            Route::ArticleDetail {
                id,
            } => Some(id.clone()),
            _ => None,
        })
        .unwrap_or_else(|| props.id.clone());

    let article = use_state(|| None::<Article>);
    let loading = use_state(|| true);
    let related_articles = use_state(Vec::<ArticleListItem>::new);
    let related_loading = use_state(|| false);
    let view_total = use_state(|| None::<usize>);
    let view_today = use_state(|| None::<u32>);
    let trend_points = use_state(Vec::<ArticleViewPoint>::new);
    let trend_day_options = use_state(Vec::<String>::new);
    let trend_selected_day = use_state(|| None::<String>);
    let trend_loading = use_state(|| false);
    let trend_error = use_state(|| None::<String>);
    let trend_granularity = use_state(|| TrendGranularity::Day);
    let comments = use_state(Vec::<ArticleComment>::new);
    let comments_total = use_state(|| 0usize);
    let comments_loading = use_state(|| false);
    let comments_error = use_state(|| None::<String>);
    let comments_refresh_key = use_state(|| 0u64);
    let footer_comment_input = use_state(String::new);
    let footer_reply_target = use_state(|| None::<CommentReplyDraft>);
    let footer_submit_loading = use_state(|| false);
    let footer_submit_feedback = use_state(|| None::<(bool, String)>);
    let selection_draft = use_state(|| None::<SelectionCommentDraft>);
    let selection_button_pos = use_state(|| None::<(f64, f64)>);
    let selection_modal_open = use_state(|| false);
    let selection_comment_input = use_state(String::new);
    let selection_submit_loading = use_state(|| false);
    let selection_submit_feedback = use_state(|| None::<(bool, String)>);

    // Back to where user came from (non-article route) with robust fallback.
    let handle_back = {
        let navigator = navigator.clone();
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();

            if let Some(context) = crate::navigation_context::peek_context() {
                crate::navigation_context::arm_context_for_return();
                if crate::navigation_context::navigate_spa_to(&context.source_url) {
                    return;
                }
                if let Some(win) = window() {
                    if win.location().set_href(&context.source_url).is_ok() {
                        return;
                    }
                }
            }
            crate::navigation_context::clear_context();

            if let Some(nav) = navigator.as_ref() {
                nav.push(&Route::Posts);
            } else if let Some(win) = window() {
                let _ = win
                    .location()
                    .set_href(&crate::config::route_path("/posts"));
            }
        })
    };

    {
        let article = article.clone();
        let article_id = article_id.clone();
        let loading = loading.clone();
        let view_total = view_total.clone();
        let view_today = view_today.clone();
        let trend_points = trend_points.clone();
        let trend_day_options = trend_day_options.clone();
        let trend_selected_day = trend_selected_day.clone();
        let trend_error = trend_error.clone();
        use_effect_with(article_id.clone(), move |id| {
            let id = id.clone();
            let article = article.clone();
            let loading = loading.clone();
            let view_total = view_total.clone();
            let view_today = view_today.clone();
            let trend_points = trend_points.clone();
            let trend_day_options = trend_day_options.clone();
            let trend_selected_day = trend_selected_day.clone();
            let trend_error = trend_error.clone();
            loading.set(true);
            view_total.set(None);
            view_today.set(None);
            trend_points.set(vec![]);
            trend_day_options.set(vec![]);
            trend_selected_day.set(None);
            trend_error.set(None);
            wasm_bindgen_futures::spawn_local(async move {
                match crate::api::fetch_article_detail(&id).await {
                    Ok(data) => {
                        let has_article = data.is_some();
                        article.set(data);
                        loading.set(false);
                        if has_article {
                            match track_article_view(&id).await {
                                Ok(metrics) => {
                                    let daily_points = metrics.daily_points.clone();
                                    let mut days = daily_points
                                        .iter()
                                        .map(|item| item.key.clone())
                                        .collect::<Vec<_>>();
                                    days.sort();
                                    days.dedup();
                                    let selected_day = days.last().cloned();

                                    view_total.set(Some(metrics.total_views));
                                    view_today.set(Some(metrics.today_views));
                                    trend_points.set(daily_points);
                                    trend_day_options.set(days);
                                    trend_selected_day.set(selected_day);
                                },
                                Err(e) => {
                                    web_sys::console::warn_1(
                                        &format!("Failed to track article view: {}", e).into(),
                                    );
                                    trend_error.set(Some(e));
                                },
                            }
                        }
                    },
                    Err(e) => {
                        web_sys::console::error_1(
                            &format!("Failed to fetch article: {}", e).into(),
                        );
                        article.set(None);
                        loading.set(false);
                    },
                }
            });
            || ()
        });
    }

    {
        let related_articles = related_articles.clone();
        let related_loading = related_loading.clone();
        let article_id = article_id.clone();
        use_effect_with(article_id.clone(), move |id| {
            let id = id.clone();
            related_loading.set(true);
            related_articles.set(vec![]);
            let related_articles = related_articles.clone();
            let related_loading = related_loading.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_related_articles(&id).await {
                    Ok(data) => {
                        related_articles.set(data);
                        related_loading.set(false);
                    },
                    Err(e) => {
                        web_sys::console::error_1(
                            &format!("Failed to fetch related articles: {}", e).into(),
                        );
                        related_loading.set(false);
                    },
                }
            });
            || ()
        });
    }

    {
        let article_id = article_id.clone();
        let comments = comments.clone();
        let comments_total = comments_total.clone();
        let comments_loading = comments_loading.clone();
        let comments_error = comments_error.clone();
        let comments_refresh_key = comments_refresh_key.clone();
        let footer_reply_target = footer_reply_target.clone();
        use_effect_with((article_id.clone(), *comments_refresh_key), move |(id, _refresh)| {
            let id = id.clone();
            let comments = comments.clone();
            let comments_total = comments_total.clone();
            let comments_loading = comments_loading.clone();
            let comments_error = comments_error.clone();
            let footer_reply_target = footer_reply_target.clone();
            comments_loading.set(true);
            comments_error.set(None);
            if *_refresh == 0 {
                footer_reply_target.set(None);
            }

            wasm_bindgen_futures::spawn_local(async move {
                let list_result = fetch_article_comments(&id, Some(80)).await;
                let stats_result = fetch_article_comment_stats(&id).await;

                match (list_result, stats_result) {
                    (Ok(list), Ok(stats)) => {
                        comments.set(list.comments);
                        comments_total.set(stats.total);
                        comments_error.set(None);
                    },
                    (Ok(list), Err(err)) => {
                        let list_len = list.comments.len();
                        comments.set(list.comments);
                        comments_total.set(list_len);
                        comments_error.set(Some(format!("Failed to load comment stats: {}", err)));
                    },
                    (Err(err), _) => {
                        comments.set(vec![]);
                        comments_total.set(0);
                        comments_error.set(Some(format!("Failed to load comments: {}", err)));
                    },
                }
                comments_loading.set(false);
            });
            || ()
        });
    }

    let article_data = (*article).clone();
    let content_language = use_state(|| ArticleContentLanguage::Zh);
    let is_lightbox_open = use_state(|| false);
    let is_brief_open = use_state(|| false);
    let is_trend_open = use_state(|| false);
    let markdown_copied = use_state(|| false);
    let preview_image_url = use_state_eq(|| None::<String>);
    let preview_image_failed = use_state(|| false);
    let preview_zoom = use_state(|| 1.0_f64);
    let switch_to_zh = {
        let content_language = content_language.clone();
        Callback::from(move |_| content_language.set(ArticleContentLanguage::Zh))
    };
    let switch_to_en = {
        let content_language = content_language.clone();
        Callback::from(move |_| content_language.set(ArticleContentLanguage::En))
    };

    {
        let content_language = content_language.clone();
        use_effect_with(article_data.clone(), move |article_opt| {
            if let Some(article) = article_opt {
                let has_zh = !article.content.trim().is_empty();
                let has_en = article
                    .content_en
                    .as_deref()
                    .map(|value| !value.trim().is_empty())
                    .unwrap_or(false);
                let next_language = if has_zh {
                    ArticleContentLanguage::Zh
                } else if has_en {
                    ArticleContentLanguage::En
                } else {
                    ArticleContentLanguage::Zh
                };
                if *content_language != next_language {
                    content_language.set(next_language);
                }
            }
            || ()
        });
    }

    {
        let is_brief_open = is_brief_open.clone();
        let is_trend_open = is_trend_open.clone();
        let trend_granularity = trend_granularity.clone();
        let footer_comment_input = footer_comment_input.clone();
        let footer_submit_feedback = footer_submit_feedback.clone();
        let selection_draft = selection_draft.clone();
        let selection_button_pos = selection_button_pos.clone();
        let selection_modal_open = selection_modal_open.clone();
        let selection_comment_input = selection_comment_input.clone();
        let selection_submit_feedback = selection_submit_feedback.clone();
        use_effect_with(article_id.clone(), move |_| {
            is_brief_open.set(false);
            is_trend_open.set(false);
            trend_granularity.set(TrendGranularity::Day);
            footer_comment_input.set(String::new());
            footer_submit_feedback.set(None);
            selection_draft.set(None);
            selection_button_pos.set(None);
            selection_modal_open.set(false);
            selection_comment_input.set(String::new());
            selection_submit_feedback.set(None);
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

    let open_brief_click = {
        let is_brief_open = is_brief_open.clone();
        Callback::from(move |_| is_brief_open.set(true))
    };

    let close_brief_click = {
        let is_brief_open = is_brief_open.clone();
        Callback::from(move |_| is_brief_open.set(false))
    };

    let open_trend_click = {
        let is_trend_open = is_trend_open.clone();
        Callback::from(move |_| is_trend_open.set(true))
    };

    let close_trend_click = {
        let is_trend_open = is_trend_open.clone();
        Callback::from(move |_| is_trend_open.set(false))
    };

    let switch_trend_to_day = {
        let trend_granularity = trend_granularity.clone();
        Callback::from(move |_| trend_granularity.set(TrendGranularity::Day))
    };

    let switch_trend_to_hour = {
        let trend_granularity = trend_granularity.clone();
        Callback::from(move |_| trend_granularity.set(TrendGranularity::Hour))
    };

    let on_trend_day_change = {
        let trend_selected_day = trend_selected_day.clone();
        Callback::from(move |event: Event| {
            if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                trend_selected_day.set(Some(target.value()));
            }
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

    let on_article_mouseup = {
        let selection_draft = selection_draft.clone();
        let selection_button_pos = selection_button_pos.clone();
        let selection_submit_feedback = selection_submit_feedback.clone();
        Callback::from(move |_: MouseEvent| {
            if let Some((draft, pos)) = capture_selection_draft() {
                selection_draft.set(Some(draft));
                selection_button_pos.set(Some(pos));
                selection_submit_feedback.set(None);
            } else {
                selection_draft.set(None);
                selection_button_pos.set(None);
            }
        })
    };

    let open_selection_modal_click = {
        let selection_modal_open = selection_modal_open.clone();
        let selection_submit_feedback = selection_submit_feedback.clone();
        Callback::from(move |event: MouseEvent| {
            event.stop_propagation();
            selection_submit_feedback.set(None);
            selection_modal_open.set(true);
        })
    };

    let close_selection_modal_click = {
        let selection_modal_open = selection_modal_open.clone();
        Callback::from(move |_| {
            selection_modal_open.set(false);
        })
    };

    let on_selection_comment_input = {
        let selection_comment_input = selection_comment_input.clone();
        Callback::from(move |event: InputEvent| {
            if let Some(target) = event.target_dyn_into::<HtmlTextAreaElement>() {
                selection_comment_input.set(target.value());
            }
        })
    };

    let on_footer_comment_input = {
        let footer_comment_input = footer_comment_input.clone();
        Callback::from(move |event: InputEvent| {
            if let Some(target) = event.target_dyn_into::<HtmlTextAreaElement>() {
                footer_comment_input.set(target.value());
            }
        })
    };

    let clear_footer_reply_target = {
        let footer_reply_target = footer_reply_target.clone();
        Callback::from(move |_| {
            footer_reply_target.set(None);
        })
    };

    let submit_selection_comment_click = {
        let article_id = article_id.clone();
        let selection_draft = selection_draft.clone();
        let selection_comment_input = selection_comment_input.clone();
        let selection_submit_loading = selection_submit_loading.clone();
        let selection_submit_feedback = selection_submit_feedback.clone();
        let selection_modal_open = selection_modal_open.clone();
        let selection_button_pos = selection_button_pos.clone();
        let comments_refresh_key = comments_refresh_key.clone();
        Callback::from(move |_| {
            let Some(draft) = (*selection_draft).clone() else {
                selection_submit_feedback
                    .set(Some((false, "请先选中文章中的一段内容。".to_string())));
                return;
            };

            let comment_text = selection_comment_input.trim().to_string();
            if comment_text.is_empty() {
                selection_submit_feedback
                    .set(Some((false, "请输入你对选中内容的疑问或评价。".to_string())));
                return;
            }

            let request = SubmitCommentRequest {
                article_id: article_id.clone(),
                entry_type: "selection".to_string(),
                comment_text,
                selected_text: Some(draft.selected_text.clone()),
                anchor_block_id: draft.anchor_block_id.clone(),
                anchor_context_before: draft.anchor_context_before.clone(),
                anchor_context_after: draft.anchor_context_after.clone(),
                reply_to_comment_id: None,
                client_meta: None,
            };

            let selection_comment_input = selection_comment_input.clone();
            let selection_submit_loading = selection_submit_loading.clone();
            let selection_submit_feedback = selection_submit_feedback.clone();
            let selection_modal_open = selection_modal_open.clone();
            let selection_draft = selection_draft.clone();
            let selection_button_pos = selection_button_pos.clone();
            let comments_refresh_key = comments_refresh_key.clone();

            selection_submit_loading.set(true);
            selection_submit_feedback.set(None);
            wasm_bindgen_futures::spawn_local(async move {
                match submit_article_comment(request).await {
                    Ok(resp) => {
                        selection_submit_feedback.set(Some((
                            true,
                            format!("评论已提交，等待审核（任务 {}）。", resp.task_id),
                        )));
                        selection_comment_input.set(String::new());
                        selection_modal_open.set(false);
                        selection_draft.set(None);
                        selection_button_pos.set(None);
                        comments_refresh_key.set((*comments_refresh_key).saturating_add(1));
                    },
                    Err(err) => {
                        selection_submit_feedback.set(Some((false, format!("提交失败：{}", err))));
                    },
                }
                selection_submit_loading.set(false);
            });
        })
    };

    let submit_footer_comment_click = {
        let article_id = article_id.clone();
        let footer_comment_input = footer_comment_input.clone();
        let footer_reply_target = footer_reply_target.clone();
        let footer_submit_loading = footer_submit_loading.clone();
        let footer_submit_feedback = footer_submit_feedback.clone();
        let comments_refresh_key = comments_refresh_key.clone();
        Callback::from(move |_| {
            let comment_text = footer_comment_input.trim().to_string();
            if comment_text.is_empty() {
                footer_submit_feedback.set(Some((false, "请输入评论内容。".to_string())));
                return;
            }

            let request = SubmitCommentRequest {
                article_id: article_id.clone(),
                entry_type: "footer".to_string(),
                comment_text,
                selected_text: None,
                anchor_block_id: None,
                anchor_context_before: None,
                anchor_context_after: None,
                reply_to_comment_id: footer_reply_target
                    .as_ref()
                    .as_ref()
                    .map(|target| target.comment_id.clone()),
                client_meta: None,
            };

            let footer_comment_input = footer_comment_input.clone();
            let footer_reply_target = footer_reply_target.clone();
            let footer_submit_loading = footer_submit_loading.clone();
            let footer_submit_feedback = footer_submit_feedback.clone();
            let comments_refresh_key = comments_refresh_key.clone();
            footer_submit_loading.set(true);
            footer_submit_feedback.set(None);
            wasm_bindgen_futures::spawn_local(async move {
                match submit_article_comment(request).await {
                    Ok(resp) => {
                        footer_submit_feedback.set(Some((
                            true,
                            format!("评论已提交，等待审核（任务 {}）。", resp.task_id),
                        )));
                        footer_comment_input.set(String::new());
                        footer_reply_target.set(None);
                        comments_refresh_key.set((*comments_refresh_key).saturating_add(1));
                    },
                    Err(err) => {
                        footer_submit_feedback.set(Some((false, format!("提交失败：{}", err))));
                    },
                }
                footer_submit_loading.set(false);
            });
        })
    };

    let jump_to_comments_click = Callback::from(move |_| {
        scroll_to_element_id(COMMENT_SECTION_ID);
    });

    let refresh_comments_click = {
        let comments_refresh_key = comments_refresh_key.clone();
        Callback::from(move |_| {
            comments_refresh_key.set((*comments_refresh_key).saturating_add(1));
        })
    };

    {
        let article_id = article_id.clone();
        let is_trend_open = is_trend_open.clone();
        let trend_granularity = trend_granularity.clone();
        let trend_selected_day = trend_selected_day.clone();
        let trend_points = trend_points.clone();
        let trend_loading = trend_loading.clone();
        let trend_error = trend_error.clone();
        let view_total = view_total.clone();
        let trend_day_options = trend_day_options.clone();
        use_effect_with(
            (article_id.clone(), *is_trend_open, *trend_granularity, (*trend_selected_day).clone()),
            move |(id, is_open, granularity, selected_day)| {
                if *is_open {
                    let article_id = id.clone();
                    let trend_points = trend_points.clone();
                    let trend_loading = trend_loading.clone();
                    let trend_error = trend_error.clone();
                    let view_total = view_total.clone();
                    let trend_selected_day = trend_selected_day.clone();
                    let trend_day_options = trend_day_options.clone();
                    let selected_day = selected_day.clone();
                    let granularity = *granularity;

                    trend_loading.set(true);
                    trend_error.set(None);

                    wasm_bindgen_futures::spawn_local(async move {
                        let response = match granularity {
                            TrendGranularity::Day => {
                                fetch_article_view_trend(&article_id, "day", None, None).await
                            },
                            TrendGranularity::Hour => {
                                let day = selected_day.unwrap_or_default();
                                if day.trim().is_empty() {
                                    trend_loading.set(false);
                                    trend_error.set(Some("missing trend day".to_string()));
                                    return;
                                }
                                fetch_article_view_trend(
                                    &article_id,
                                    "hour",
                                    None,
                                    Some(day.as_str()),
                                )
                                .await
                            },
                        };

                        match response {
                            Ok(data) => {
                                trend_points.set(data.points.clone());
                                view_total.set(Some(data.total_views));
                                trend_loading.set(false);

                                if data.granularity == "day" {
                                    let mut days = data
                                        .points
                                        .iter()
                                        .map(|item| item.key.clone())
                                        .collect::<Vec<_>>();
                                    days.sort();
                                    days.dedup();
                                    let selected = days.last().cloned();
                                    trend_day_options.set(days);
                                    if selected.is_some() {
                                        trend_selected_day.set(selected);
                                    }
                                }
                            },
                            Err(error) => {
                                trend_loading.set(false);
                                trend_error.set(Some(error));
                            },
                        }
                    });
                }

                || ()
            },
        );
    }

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

    {
        let is_brief_open = is_brief_open.clone();
        use_effect_with(*is_brief_open, move |is_open| {
            let keydown_listener_opt = if *is_open {
                let handle = is_brief_open.clone();
                let listener =
                    wasm_bindgen::closure::Closure::wrap(Box::new(move |event: KeyboardEvent| {
                        if event.key() == "Escape" {
                            handle.set(false);
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

    {
        let is_trend_open = is_trend_open.clone();
        use_effect_with(*is_trend_open, move |is_open| {
            let keydown_listener_opt = if *is_open {
                let handle = is_trend_open.clone();
                let listener =
                    wasm_bindgen::closure::Closure::wrap(Box::new(move |event: KeyboardEvent| {
                        if event.key() == "Escape" {
                            handle.set(false);
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

    {
        let selection_modal_open = selection_modal_open.clone();
        use_effect_with(*selection_modal_open, move |is_open| {
            let keydown_listener_opt = if *is_open {
                let handle = selection_modal_open.clone();
                let listener =
                    wasm_bindgen::closure::Closure::wrap(Box::new(move |event: KeyboardEvent| {
                        if event.key() == "Escape" {
                            handle.set(false);
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
    let stop_brief_bubble = Callback::from(|event: MouseEvent| event.stop_propagation());
    let stop_trend_bubble = Callback::from(|event: MouseEvent| event.stop_propagation());
    let stop_selection_modal_bubble = Callback::from(|event: MouseEvent| event.stop_propagation());
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

    let markdown_render_key = if let Some(article) = article_data.as_ref() {
        let lang_key = if *content_language == ArticleContentLanguage::En { "en" } else { "zh" };
        format!("{}:{lang_key}", article.id)
    } else {
        String::new()
    };
    let comments_render_key = (*comments)
        .iter()
        .map(|comment| {
            format!(
                "{}:{}:{}",
                comment.comment_id,
                comment.published_at,
                comment
                    .ai_reply_markdown
                    .as_ref()
                    .map(|value| value.len())
                    .unwrap_or(0)
            )
        })
        .collect::<Vec<_>>()
        .join("|");

    {
        let markdown_copied = markdown_copied.clone();
        use_effect_with(markdown_render_key.clone(), move |_| {
            markdown_copied.set(false);
            || ()
        });
    }

    // Initialize markdown rendering after content/language is loaded
    use_effect_with(markdown_render_key.clone(), |render_key| {
        let timeout = if !render_key.is_empty() {
            Some(Timeout::new(100, move || {
                if let Some(win) = window() {
                    let article_root = win
                        .document()
                        .and_then(|doc| doc.query_selector(".article-content").ok())
                        .flatten();

                    if let Ok(init_fn) =
                        js_sys::Reflect::get(&win, &JsValue::from_str("initMarkdownRendering"))
                    {
                        if let Ok(func) = init_fn.dyn_into::<js_sys::Function>() {
                            if let Some(root) = article_root {
                                let _ = func.call1(&win, root.as_ref());
                            } else {
                                let _ = func.call0(&win);
                            }
                        }
                    }
                }
            }))
        } else {
            None
        };

        move || {
            drop(timeout);

            // Cleanup markdown/TOC/fullscreen state on unmount or content switch
            if let Some(win) = window() {
                if let Ok(cleanup_fn) =
                    js_sys::Reflect::get(&win, &JsValue::from_str("cleanupMarkdownRendering"))
                {
                    if let Ok(func) = cleanup_fn.dyn_into::<js_sys::Function>() {
                        let _ = func.call0(&win);
                    }
                }
            }
        }
    });

    // Re-run markdown enhancements for dynamic comment markdown fragments.
    // We intentionally avoid TOC regeneration here.
    {
        let article_id = article_id.clone();
        let comments_render_key = comments_render_key.clone();
        use_effect_with(
            (article_id.clone(), comments_render_key.clone()),
            move |(id, comments_key)| {
                let timeout = if !id.trim().is_empty() && !comments_key.is_empty() {
                    Some(Timeout::new(120, move || {
                        if let Some(win) = window() {
                            let Some(document) = win.document() else {
                                return;
                            };
                            let init_fn = js_sys::Reflect::get(
                                &win,
                                &JsValue::from_str("initMarkdownFragmentRendering"),
                            )
                            .ok()
                            .and_then(|value| value.dyn_into::<js_sys::Function>().ok())
                            .or_else(|| {
                                js_sys::Reflect::get(
                                    &win,
                                    &JsValue::from_str("initMarkdownRendering"),
                                )
                                .ok()
                                .and_then(|value| value.dyn_into::<js_sys::Function>().ok())
                            });
                            let Some(func) = init_fn else {
                                return;
                            };

                            if let Ok(node_list) =
                                document.query_selector_all(".comment-ai-markdown")
                            {
                                for idx in 0..node_list.length() {
                                    if let Some(node) = node_list.item(idx) {
                                        if let Ok(element) = node.dyn_into::<Element>() {
                                            let _ = func.call1(&win, element.as_ref());
                                        }
                                    }
                                }
                            }
                        }
                    }))
                } else {
                    None
                };

                move || {
                    drop(timeout);
                }
            },
        );
    }

    {
        let open_image_preview = open_image_preview.clone();
        use_effect_with(markdown_render_key.clone(), move |render_key| {
            let mut listeners: Vec<ImageClickListener> = Vec::new();

            if !render_key.is_empty() {
                if let Some(document) = window().and_then(|win| win.document()) {
                    if let Ok(node_list) = document.query_selector_all(".article-content img") {
                        for idx in 0..node_list.length() {
                            if let Some(node) = node_list.item(idx) {
                                if let Ok(element) = node.dyn_into::<web_sys::Element>() {
                                    let callback = open_image_preview.clone();
                                    let listener = wasm_bindgen::closure::Closure::wrap(Box::new(
                                        move |event: web_sys::Event| {
                                            if let Some(target) = event.current_target() {
                                                if let Ok(img) =
                                                    target.dyn_into::<HtmlImageElement>()
                                                {
                                                    if let Some(src) = img.get_attribute("src") {
                                                        callback.emit(src);
                                                    }
                                                }
                                            }
                                        },
                                    )
                                        as Box<dyn FnMut(_)>);

                                    if let Err(err) = element.add_event_listener_with_callback(
                                        "click",
                                        listener.as_ref().unchecked_ref(),
                                    ) {
                                        web_sys::console::error_1(&err);
                                    }

                                    listeners.push((element, listener));
                                }
                            }
                        }
                    }
                }
            }

            move || {
                for (element, listener) in listeners {
                    let _ = element.remove_event_listener_with_callback(
                        "click",
                        listener.as_ref().unchecked_ref(),
                    );
                }
            }
        });
    }

    let loading_view = html! {
        <div class={classes!("flex", "min-h-[50vh]", "items-center", "justify-center")}>
            <LoadingSpinner size={SpinnerSize::Large} />
        </div>
    };

    let is_overlay_open =
        *is_lightbox_open || *is_brief_open || *is_trend_open || *selection_modal_open;

    let body = if *loading {
        loading_view
    } else if let Some(article) = article_data.clone() {
        let has_zh_content = !article.content.trim().is_empty();
        let has_en_content = article
            .content_en
            .as_deref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false);
        let show_language_toggle = has_zh_content && has_en_content;
        let active_content = if *content_language == ArticleContentLanguage::En && has_en_content {
            article
                .content_en
                .as_deref()
                .unwrap_or(article.content.as_str())
        } else {
            article.content.as_str()
        };
        let active_detailed_summary = article.detailed_summary.as_ref().and_then(|summary| {
            let preferred = if *content_language == ArticleContentLanguage::En {
                summary.en.as_ref().or(summary.zh.as_ref())
            } else {
                summary.zh.as_ref().or(summary.en.as_ref())
            };
            preferred
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        });
        let has_detailed_summary = active_detailed_summary.is_some();
        let word_count = active_content
            .chars()
            .filter(|c| !c.is_whitespace())
            .count();
        let render_html = markdown_to_html(active_content);
        let content = Html::from_html_unchecked(AttrValue::from(render_html));
        let detailed_summary_html = active_detailed_summary
            .as_ref()
            .map(|summary| Html::from_html_unchecked(AttrValue::from(markdown_to_html(summary))));
        let zh_button_class = if *content_language == ArticleContentLanguage::Zh {
            classes!(
                "rounded-full",
                "border",
                "border-[var(--primary)]",
                "bg-[var(--primary)]",
                "px-3",
                "py-1",
                "text-xs",
                "font-semibold",
                "uppercase",
                "tracking-[0.08em]",
                "text-white"
            )
        } else {
            classes!(
                "rounded-full",
                "border",
                "border-[var(--border)]",
                "bg-[var(--surface)]",
                "px-3",
                "py-1",
                "text-xs",
                "font-semibold",
                "uppercase",
                "tracking-[0.08em]",
                "text-[var(--muted)]",
                "hover:border-[var(--primary)]",
                "hover:text-[var(--primary)]"
            )
        };
        let en_button_class = if *content_language == ArticleContentLanguage::En {
            classes!(
                "rounded-full",
                "border",
                "border-[var(--primary)]",
                "bg-[var(--primary)]",
                "px-3",
                "py-1",
                "text-xs",
                "font-semibold",
                "uppercase",
                "tracking-[0.08em]",
                "text-white"
            )
        } else {
            classes!(
                "rounded-full",
                "border",
                "border-[var(--border)]",
                "bg-[var(--surface)]",
                "px-3",
                "py-1",
                "text-xs",
                "font-semibold",
                "uppercase",
                "tracking-[0.08em]",
                "text-[var(--muted)]",
                "hover:border-[var(--primary)]",
                "hover:text-[var(--primary)]"
            )
        };
        let summary_title = if *content_language == ArticleContentLanguage::En {
            t::DETAILED_SUMMARY_TITLE_EN
        } else {
            t::DETAILED_SUMMARY_TITLE_ZH
        };
        let brief_button_text = if *content_language == ArticleContentLanguage::En {
            t::OPEN_BRIEF_BUTTON_EN
        } else {
            t::OPEN_BRIEF_BUTTON_ZH
        };
        let raw_markdown_button_text = if *content_language == ArticleContentLanguage::En {
            t::OPEN_RAW_MARKDOWN_BUTTON_EN
        } else {
            t::OPEN_RAW_MARKDOWN_BUTTON_ZH
        };
        let can_export_markdown = !active_content.trim().is_empty();
        let show_article_actions =
            show_language_toggle || has_detailed_summary || can_export_markdown;
        let show_side_actions_rail = show_article_actions && !is_overlay_open;
        let export_button_label = if *markdown_copied {
            if *content_language == ArticleContentLanguage::En {
                "Copied"
            } else {
                "已复制"
            }
        } else if *content_language == ArticleContentLanguage::En {
            "Copy Markdown"
        } else {
            "导出 Markdown"
        };
        let export_button_icon =
            if *markdown_copied { classes!("fas", "fa-check") } else { classes!("far", "fa-copy") };
        let export_button_class = if *markdown_copied {
            classes!(
                "article-action-btn",
                "inline-flex",
                "items-center",
                "justify-center",
                "gap-2",
                "rounded-full",
                "border",
                "border-emerald-500/55",
                "bg-emerald-500/14",
                "px-3",
                "py-2",
                "text-xs",
                "font-semibold",
                "uppercase",
                "tracking-[0.08em]",
                "text-emerald-700",
                "transition-[var(--transition-base)]",
                "dark:text-emerald-200"
            )
        } else {
            classes!(
                "article-action-btn",
                "inline-flex",
                "items-center",
                "justify-center",
                "gap-2",
                "rounded-full",
                "border",
                "border-[var(--border)]",
                "bg-[var(--surface)]",
                "px-3",
                "py-2",
                "text-xs",
                "font-semibold",
                "uppercase",
                "tracking-[0.08em]",
                "text-[var(--muted)]",
                "transition-[var(--transition-base)]",
                "hover:border-[var(--primary)]",
                "hover:text-[var(--primary)]"
            )
        };
        let copy_markdown_click = {
            let markdown_copied = markdown_copied.clone();
            let markdown_source = active_content.to_string();
            Callback::from(move |_| {
                let markdown_copied = markdown_copied.clone();
                let markdown_source = markdown_source.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    let mut copied = false;

                    if let Some(win) = window() {
                        let navigator = win.navigator();
                        if let Ok(clipboard) =
                            js_sys::Reflect::get(&navigator, &JsValue::from_str("clipboard"))
                        {
                            if !clipboard.is_undefined() && !clipboard.is_null() {
                                if let Ok(write_text) = js_sys::Reflect::get(
                                    &clipboard,
                                    &JsValue::from_str("writeText"),
                                ) {
                                    if let Some(write_fn) = write_text.dyn_ref::<js_sys::Function>()
                                    {
                                        if let Ok(promise_value) = write_fn
                                            .call1(&clipboard, &JsValue::from_str(&markdown_source))
                                        {
                                            if let Ok(promise) =
                                                promise_value.dyn_into::<js_sys::Promise>()
                                            {
                                                copied =
                                                    wasm_bindgen_futures::JsFuture::from(promise)
                                                        .await
                                                        .is_ok();
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if copied {
                        markdown_copied.set(true);
                        TimeoutFuture::new(1800).await;
                        markdown_copied.set(false);
                    } else {
                        web_sys::console::warn_1(&JsValue::from_str(
                            "Failed to copy markdown to clipboard.",
                        ));
                    }
                });
            })
        };
        let open_raw_markdown_click = {
            let navigator = navigator.clone();
            let article_id = article.id.clone();
            let lang = if *content_language == ArticleContentLanguage::En {
                "en".to_string()
            } else {
                "zh".to_string()
            };
            Callback::from(move |_| {
                if let Some(nav) = navigator.as_ref() {
                    nav.push(&Route::ArticleRaw {
                        id: article_id.clone(),
                        lang: lang.clone(),
                    });
                }
            })
        };
        let render_article_actions = |side_rail: bool| -> Html {
            let stack_class = if side_rail {
                classes!(
                    "article-actions-stack",
                    "article-actions-stack-rail",
                    "flex",
                    "flex-col",
                    "gap-2"
                )
            } else {
                classes!(
                    "article-actions-stack",
                    "article-actions-stack-inline",
                    "flex",
                    "flex-wrap",
                    "items-center",
                    "gap-2"
                )
            };
            let cta_row_class = if side_rail {
                classes!("article-actions-cta-row", "flex", "flex-col", "gap-2")
            } else {
                classes!("article-actions-cta-row", "flex", "flex-wrap", "items-center", "gap-2")
            };

            html! {
                <div class={stack_class}>
                    {
                        if show_language_toggle {
                            html! {
                                <div class={classes!(
                                    "article-language-toggle",
                                    "inline-flex",
                                    "items-center",
                                    "gap-2",
                                    "self-start",
                                    "rounded-full",
                                    "border",
                                    "border-[var(--border)]",
                                    "bg-[var(--surface)]",
                                    "px-2",
                                    "py-2"
                                )}>
                                    <span class={classes!(
                                        "px-2",
                                        "text-[0.72rem]",
                                        "font-semibold",
                                        "uppercase",
                                        "tracking-[0.12em]",
                                        "text-[var(--muted)]"
                                    )}>{ t::LANG_SWITCH_LABEL }</span>
                                    <button
                                        type="button"
                                        class={zh_button_class.clone()}
                                        aria-pressed={if *content_language == ArticleContentLanguage::Zh {
                                            "true"
                                        } else {
                                            "false"
                                        }}
                                        onclick={switch_to_zh.clone()}
                                    >
                                        { t::LANG_SWITCH_ZH }
                                    </button>
                                    <button
                                        type="button"
                                        class={en_button_class.clone()}
                                        aria-pressed={if *content_language == ArticleContentLanguage::En {
                                            "true"
                                        } else {
                                            "false"
                                        }}
                                        onclick={switch_to_en.clone()}
                                    >
                                        { t::LANG_SWITCH_EN }
                                    </button>
                                </div>
                            }
                        } else {
                            html! {}
                        }
                    }
                    <div class={cta_row_class}>
                        {
                            if has_detailed_summary {
                                html! {
                                    <button
                                        type="button"
                                        class={classes!(
                                            "article-action-btn",
                                            "inline-flex",
                                            "items-center",
                                            "justify-center",
                                            "gap-2",
                                            "rounded-full",
                                            "border",
                                            "border-[var(--primary)]/45",
                                            "bg-[var(--surface)]",
                                            "px-3",
                                            "py-2",
                                            "text-xs",
                                            "font-semibold",
                                            "uppercase",
                                            "tracking-[0.1em]",
                                            "text-[var(--primary)]",
                                            "transition-[var(--transition-base)]",
                                            "hover:bg-[var(--primary)]",
                                            "hover:text-white"
                                        )}
                                        onclick={open_brief_click.clone()}
                                    >
                                        <i class={classes!("fas", "fa-list-check")} aria-hidden="true"></i>
                                        { brief_button_text }
                                    </button>
                                }
                            } else {
                                html! {}
                            }
                        }
                        {
                            if can_export_markdown {
                                html! {
                                    <button
                                        type="button"
                                        class={classes!(
                                            "article-action-btn",
                                            "inline-flex",
                                            "items-center",
                                            "justify-center",
                                            "gap-2",
                                            "rounded-full",
                                            "border",
                                            "border-[var(--border)]",
                                            "bg-[var(--surface)]",
                                            "px-3",
                                            "py-2",
                                            "text-xs",
                                            "font-semibold",
                                            "uppercase",
                                            "tracking-[0.08em]",
                                            "text-[var(--muted)]",
                                            "transition-[var(--transition-base)]",
                                            "hover:border-[var(--primary)]",
                                            "hover:text-[var(--primary)]"
                                        )}
                                        onclick={open_raw_markdown_click.clone()}
                                    >
                                        <i class={classes!("far", "fa-file-lines")} aria-hidden="true"></i>
                                        { raw_markdown_button_text }
                                    </button>
                                }
                            } else {
                                html! {}
                            }
                        }
                        {
                            if can_export_markdown {
                                html! {
                                    <button
                                        type="button"
                                        class={export_button_class.clone()}
                                        onclick={copy_markdown_click.clone()}
                                    >
                                        <i class={export_button_icon.clone()} aria-hidden="true"></i>
                                        { export_button_label }
                                    </button>
                                }
                            } else {
                                html! {}
                            }
                        }
                    </div>
                </div>
            }
        };

        html! {
            <section class={classes!(
                "article-layout-shell",
                show_side_actions_rail.then_some("article-layout-shell--with-side-actions")
            )}>
                {
                    if show_side_actions_rail {
                        html! {
                            <aside class={classes!("article-side-actions-rail")} aria-label="Article Actions">
                                { render_article_actions(true) }
                            </aside>
                        }
                    } else {
                        html! {}
                    }
                }
                <div class={classes!("article-main-column")}>
                    <article class={classes!(
                        "article-detail",
                        "bg-[var(--surface)]",
                        "border",
                        "border-[var(--border)]",
                        "rounded-[var(--radius)]",
                        "shadow-[var(--shadow)]",
                        "p-8",
                        "my-8",
                        "mx-auto",
                        "sm:p-4",
                        "sm:my-5"
                    )}>
                        {
                            if let Some(image) = article.featured_image.clone() {
                                let image_src = image_url(&image);
                                let open_featured_preview = {
                                    let open_image_preview = open_image_preview.clone();
                                    let image_src = image_src.clone();
                                    Callback::from(move |_| {
                                        open_image_preview.emit(image_src.clone());
                                    })
                                };
                                html! {
                                    <div class={classes!(
                                        "-mx-8",
                                        "-mt-8",
                                        "mb-6",
                                        "rounded-t-[calc(var(--radius)-2px)]",
                                        "overflow-hidden",
                                        "max-h-[420px]",
                                        "relative",
                                        "group",
                                        "sm:-mx-4",
                                        "sm:-mt-4",
                                        "sm:mb-4"
                                    )}>
                                        <ImageWithLoading
                                            src={image_src.clone()}
                                            alt={article.title.clone()}
                                            loading={Some(AttrValue::from("lazy"))}
                                            onclick={Some(open_featured_preview.clone())}
                                            class={classes!(
                                                "w-full",
                                                "h-full",
                                                "object-cover",
                                                "block",
                                                "cursor-zoom-in"
                                            )}
                                            container_class={classes!("w-full", "h-full")}
                                        />
                                        <button
                                            type="button"
                                            class={classes!(
                                                "hidden",
                                                "md:inline-flex",
                                                "absolute",
                                                "bottom-4",
                                                "right-4",
                                                "rounded-full",
                                                "bg-black/70",
                                                "px-4",
                                                "py-2",
                                                "text-sm",
                                                "text-white",
                                                "backdrop-blur",
                                                "hover:bg-black/80",
                                                "dark:bg-white/20",
                                                "dark:text-white"
                                            )}
                                            onclick={open_featured_preview}
                                        >
                                            { t::VIEW_ORIGINAL_IMAGE }
                                        </button>
                                    </div>
                                }
                            } else {
                                html! {}
                            }
                        }

                        {
                            if show_article_actions {
                                html! {
                                    <div class={classes!("article-inline-actions")}>
                                        { render_article_actions(false) }
                                    </div>
                                }
                            } else {
                                html! {}
                            }
                        }

                        <header class={classes!(
                            "flex",
                            "flex-col",
                            "gap-2",
                            "mb-4",
                            "fade-in"
                        )}>
                            <Link<Route>
                                to={Route::CategoryDetail { category: article.category.clone() }}
                                classes={classes!(
                                    "m-0",
                                    "inline-flex",
                                    "items-center",
                                    "gap-[0.35rem]",
                                    "uppercase",
                                    "text-[0.85rem]",
                                    "tracking-[0.2em]",
                                    "text-[var(--primary)]",
                                    "no-underline",
                                    "cursor-pointer",
                                    "transition-[var(--transition-base)]",
                                    "hover:text-[var(--link)]"
                                )}
                            >
                                { article.category.clone() }
                            </Link<Route>>
                            <h1 class={classes!(
                                "m-0",
                                "text-[2.25rem]",
                                "leading-[1.25]",
                                "sm:text-[1.65rem]"
                            )}>
                                { article.title.clone() }
                            </h1>
                            <div class={classes!(
                                "flex",
                                "flex-wrap",
                                "gap-2",
                                "text-[0.9rem]",
                                "text-[var(--muted)]"
                            )} aria-label={t::ARTICLE_META_ARIA}>
                                <span class={classes!(
                                    "inline-flex",
                                    "items-center",
                                    "gap-[0.35rem]"
                                )}>
                                    <i class={classes!("fas", "fa-user-circle")} aria-hidden="true"></i>
                                    { article.author.clone() }
                                </span>
                                <span class={classes!(
                                    "inline-flex",
                                    "items-center",
                                    "gap-[0.35rem]"
                                )}>
                                    <i class={classes!("far", "fa-calendar-alt")} aria-hidden="true"></i>
                                    { article.date.clone() }
                                </span>
                                <Link<Route>
                                    to={Route::CategoryDetail { category: article.category.clone() }}
                                    classes={classes!(
                                        "inline-flex",
                                        "items-center",
                                        "gap-[0.35rem]"
                                    )}
                                >
                                    <i class={classes!("far", "fa-folder-open")} aria-hidden="true"></i>
                                    { article.category.clone() }
                                </Link<Route>>
                                <span class={classes!(
                                    "inline-flex",
                                    "items-center",
                                    "gap-[0.35rem]"
                                )}>
                                    <i class={classes!("far", "fa-file-alt")} aria-hidden="true"></i>
                                    { fill_one(t::WORD_COUNT_TEMPLATE, word_count) }
                                </span>
                                <span class={classes!(
                                    "inline-flex",
                                    "items-center",
                                    "gap-[0.35rem]"
                                )}>
                                    <i class={classes!("far", "fa-clock")} aria-hidden="true"></i>
                                    { fill_one(t::READ_TIME_TEMPLATE, article.read_time) }
                                </span>
                                <span class={classes!(
                                    "inline-flex",
                                    "items-center",
                                    "gap-[0.35rem]"
                                )}>
                                    <i class={classes!("far", "fa-eye")} aria-hidden="true"></i>
                                    {
                                        if let Some(total) = *view_total {
                                            fill_one(t::VIEW_COUNT_TEMPLATE, total)
                                        } else {
                                            t::VIEW_COUNT_LOADING.to_string()
                                        }
                                    }
                                </span>
                            </div>
                        </header>

                        <section
                            class={classes!("article-content")}
                            aria-label={t::ARTICLE_BODY_ARIA}
                            onmouseup={on_article_mouseup.clone()}
                        >
                            { content }
                        </section>

                        <footer class={classes!(
                            "mt-8",
                            "border-t",
                            "border-[var(--border)]",
                            "pt-5"
                        )}>
                            <h2 class={classes!(
                                "m-0",
                                "mb-3",
                                "text-[1rem]",
                                "text-[var(--muted)]",
                                "tracking-[0.15em]",
                                "uppercase"
                            )}>{ t::TAGS_TITLE }</h2>
                            <ul class={classes!(
                                "list-none",
                                "flex",
                                "flex-wrap",
                                "gap-2",
                                "m-0",
                                "p-0"
                            )}>
                                { for article.tags.iter().map(|tag| {
                                    html! {
                                        <li>
                                            <Link<Route>
                                                to={Route::TagDetail { tag: tag.to_string() }}
                                                classes={classes!(
                                                    "py-[0.4rem]",
                                                    "px-[1.1rem]",
                                                    "border",
                                                    "border-[var(--border)]",
                                                    "rounded-[6px]",
                                                    "text-[0.9rem]",
                                                    "text-[var(--muted)]",
                                                    "bg-[var(--surface)]",
                                                    "transition-[background-color_0.2s_var(--ease-spring),color_0.2s_var(--ease-spring),border-color_0.2s_var(--ease-spring)]",
                                                    "hover:bg-[var(--primary)]",
                                                    "hover:border-[var(--primary)]",
                                                    "hover:text-white"
                                                )}
                                            >
                                                { format!("#{}", tag) }
                                            </Link<Route>>
                                        </li>
                                    }
                                }) }
                            </ul>
                        </footer>

                        <section class={classes!(
                            "mt-10",
                            "pt-6",
                            "border-t",
                            "border-[var(--border)]"
                        )}>
                            <h2 class={classes!(
                                "m-0",
                                "mb-4",
                                "text-[1.1rem]",
                                "text-[var(--muted)]",
                                "tracking-[0.15em]",
                                "uppercase"
                            )}>{ t::RELATED_TITLE }</h2>
                            if *related_loading {
                                <div class={classes!(
                                    "flex",
                                    "items-center",
                                    "gap-2",
                                    "text-[var(--muted)]"
                                )}>
                                    <LoadingSpinner size={SpinnerSize::Small} />
                                    <span>{ t::RELATED_LOADING }</span>
                                </div>
                            } else if related_articles.is_empty() {
                                <p class={classes!("text-[var(--muted)]", "m-0")}>
                                    { t::NO_RELATED }
                                </p>
                            } else {
                                <div class={classes!(
                                    "grid",
                                    "gap-5",
                                    "md:grid-cols-2"
                                )}>
                                    { for related_articles.iter().cloned().map(|article| {
                                        html! { <ArticleCard key={article.id.clone()} article={article.clone()} /> }
                                    }) }
                                </div>
                            }
                        </section>

                        <section
                            id={COMMENT_SECTION_ID}
                            class={classes!(
                                "mt-10",
                                "pt-6",
                                "border-t",
                                "border-[var(--border)]",
                                "flex",
                                "flex-col",
                                "gap-4"
                            )}
                        >
                            <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                                <div>
                                    <h2 class={classes!(
                                        "m-0",
                                        "text-[1.1rem]",
                                        "text-[var(--muted)]",
                                        "tracking-[0.15em]",
                                        "uppercase"
                                    )}>{ "评论区" }</h2>
                                    <p class={classes!("m-0", "mt-1", "text-sm", "text-[var(--muted)]")}>
                                        { format!("当前评论 {} 条", *comments_total) }
                                    </p>
                                </div>
                                <button
                                    type="button"
                                    class={classes!(
                                        "article-action-btn",
                                        "inline-flex",
                                        "items-center",
                                        "justify-center",
                                        "gap-2",
                                        "rounded-full",
                                        "border",
                                        "border-[var(--border)]",
                                        "bg-[var(--surface)]",
                                        "px-3",
                                        "py-2",
                                        "text-xs",
                                        "font-semibold",
                                        "uppercase",
                                        "tracking-[0.08em]",
                                        "text-[var(--muted)]",
                                        "transition-[var(--transition-base)]",
                                        "hover:border-[var(--primary)]",
                                        "hover:text-[var(--primary)]"
                                    )}
                                    onclick={refresh_comments_click}
                                >
                                    <i class={classes!("fas", "fa-rotate-right")} aria-hidden="true"></i>
                                    { "刷新评论" }
                                </button>
                            </div>

                            {
                                if let Some((is_success, message)) = (*selection_submit_feedback).clone() {
                                    let status_class = if is_success {
                                        classes!(
                                            "rounded-[var(--radius)]",
                                            "border",
                                            "border-emerald-500/35",
                                            "bg-emerald-500/10",
                                            "px-3",
                                            "py-2",
                                            "text-sm",
                                            "text-emerald-700",
                                            "dark:text-emerald-200"
                                        )
                                    } else {
                                        classes!(
                                            "rounded-[var(--radius)]",
                                            "border",
                                            "border-red-400/45",
                                            "bg-red-500/10",
                                            "px-3",
                                            "py-2",
                                            "text-sm",
                                            "text-red-700",
                                            "dark:text-red-200"
                                        )
                                    };
                                    html! {
                                        <p class={status_class}>{ message }</p>
                                    }
                                } else {
                                    html! {}
                                }
                            }

                            <div class={classes!(
                                "rounded-[var(--radius)]",
                                "border",
                                "border-[var(--border)]",
                                "bg-[var(--surface-alt)]",
                                "p-4",
                                "flex",
                                "flex-col",
                                "gap-3"
                            )}>
                                <label class={classes!(
                                    "text-sm",
                                    "font-semibold",
                                    "text-[var(--text)]"
                                )}>
                                    { "发表评论（文末入口）" }
                                </label>
                                if let Some(reply_target) = (*footer_reply_target).clone() {
                                    <div class={classes!(
                                        "rounded-[var(--radius)]",
                                        "border",
                                        "border-[var(--border)]",
                                        "bg-[var(--surface)]",
                                        "px-3",
                                        "py-2",
                                        "text-sm",
                                        "flex",
                                        "flex-col",
                                        "gap-2"
                                    )}>
                                        <div class={classes!("flex", "items-center", "justify-between", "gap-2", "flex-wrap")}>
                                            <p class={classes!("m-0", "text-xs", "uppercase", "tracking-[0.08em]", "text-[var(--muted)]")}>
                                                { format!("正在引用：{}", reply_target.author_name) }
                                            </p>
                                            <button
                                                type="button"
                                                class={classes!("btn-fluent-secondary", "!px-2", "!py-1", "!text-xs")}
                                                onclick={clear_footer_reply_target.clone()}
                                            >
                                                { "取消引用" }
                                            </button>
                                        </div>
                                        <p class={classes!("m-0", "text-sm", "text-[var(--text)]")}>{ reply_target.comment_text.clone() }</p>
                                        if let Some(ai_reply) = reply_target.ai_reply_markdown {
                                            <p class={classes!("m-0", "text-xs", "text-[var(--muted)]")}>
                                                { format!("引用 AI 回复：{}", ai_reply.chars().take(140).collect::<String>()) }
                                            </p>
                                        }
                                    </div>
                                }
                                <textarea
                                    class={classes!(
                                        "comment-compose-textarea"
                                    )}
                                    placeholder={"写下你对本文的疑问、勘误或补充建议..."}
                                    value={(*footer_comment_input).clone()}
                                    oninput={on_footer_comment_input}
                                />
                                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                    {
                                        if let Some((is_success, message)) = (*footer_submit_feedback).clone() {
                                            let status_class = if is_success {
                                                classes!("text-sm", "text-emerald-700", "dark:text-emerald-200")
                                            } else {
                                                classes!("text-sm", "text-red-700", "dark:text-red-200")
                                            };
                                            html! { <span class={status_class}>{ message }</span> }
                                        } else {
                                            html! { <span class={classes!("text-sm", "text-[var(--muted)]")}>{ "每个用户每分钟最多提交 1 条评论。" }</span> }
                                        }
                                    }
                                    <button
                                        type="button"
                                        class={classes!(
                                            "btn-fluent-primary",
                                            "px-4",
                                            "py-2"
                                        )}
                                        onclick={submit_footer_comment_click}
                                        disabled={*footer_submit_loading}
                                    >
                                        {
                                            if *footer_submit_loading {
                                                "提交中..."
                                            } else {
                                                "提交评论"
                                            }
                                        }
                                    </button>
                                </div>
                            </div>

                            {
                                if *comments_loading {
                                    html! {
                                        <div class={classes!(
                                            "rounded-[var(--radius)]",
                                            "border",
                                            "border-[var(--border)]",
                                            "bg-[var(--surface)]",
                                            "px-4",
                                            "py-6",
                                            "text-sm",
                                            "text-[var(--muted)]",
                                            "inline-flex",
                                            "items-center",
                                            "gap-2"
                                        )}>
                                            <LoadingSpinner size={SpinnerSize::Small} />
                                            <span>{ "评论加载中..." }</span>
                                        </div>
                                    }
                                } else if let Some(error) = (*comments_error).clone() {
                                    html! {
                                        <p class={classes!(
                                            "m-0",
                                            "rounded-[var(--radius)]",
                                            "border",
                                            "border-red-400/45",
                                            "bg-red-500/10",
                                            "px-3",
                                            "py-2",
                                            "text-sm",
                                            "text-red-700",
                                            "dark:text-red-200"
                                        )}>
                                            { error }
                                        </p>
                                    }
                                } else if comments.is_empty() {
                                    html! {
                                        <p class={classes!("m-0", "text-sm", "text-[var(--muted)]")}>
                                            { "暂无评论，欢迎成为第一个留言的人。" }
                                        </p>
                                    }
                                } else {
                                    html! {
                                        <div class={classes!("comment-thread-list")}>
                                            { for comments.iter().map(|comment| {
                                                let anchor_block_id = comment.anchor_block_id.clone();
                                                let selected_text = comment.selected_text.clone();
                                                let reply_to_comment_id = comment.reply_to_comment_id.clone();
                                                let reply_to_comment_text = comment.reply_to_comment_text.clone();
                                                let reply_to_ai_reply_markdown = comment.reply_to_ai_reply_markdown.clone();
                                                let jump_quote_click = {
                                                    let anchor_block_id = anchor_block_id.clone();
                                                    Callback::from(move |_| {
                                                        if let Some(block_id) = anchor_block_id.clone() {
                                                            scroll_to_anchor_block(&block_id);
                                                        }
                                                    })
                                                };
                                                let jump_reply_comment_click = {
                                                    let reply_to_comment_id = reply_to_comment_id.clone();
                                                    Callback::from(move |_| {
                                                        if let Some(comment_id) = reply_to_comment_id.clone() {
                                                            scroll_to_comment_card(&comment_id);
                                                        }
                                                    })
                                                };
                                                let quote_reply_click = {
                                                    let footer_reply_target = footer_reply_target.clone();
                                                    let comment_id = comment.comment_id.clone();
                                                    let author_name = comment.author_name.clone();
                                                    let comment_text = comment.comment_text.clone();
                                                    let ai_reply_markdown = comment.ai_reply_markdown.clone();
                                                    Callback::from(move |_| {
                                                        footer_reply_target.set(Some(CommentReplyDraft {
                                                            comment_id: comment_id.clone(),
                                                            author_name: author_name.clone(),
                                                            comment_text: comment_text.clone(),
                                                            ai_reply_markdown: ai_reply_markdown.clone(),
                                                        }));
                                                        scroll_to_element_id(COMMENT_SECTION_ID);
                                                    })
                                                };
                                                let avatar_initial =
                                                    comment_avatar_initials(
                                                        &comment.author_name,
                                                        &comment.author_avatar_seed,
                                                    );
                                                let avatar_style =
                                                    comment_avatar_style(&comment.author_avatar_seed);
                                                let region_label = display_comment_region(&comment.ip_region);
                                                let ai_reply_html = comment
                                                    .ai_reply_markdown
                                                    .clone()
                                                    .filter(|value| !value.trim().is_empty())
                                                    .map(|value| {
                                                        Html::from_html_unchecked(AttrValue::from(
                                                            markdown_to_html(&value)
                                                        ))
                                                    });

                                                html! {
                                                    <article id={format!("comment-item-{}", comment.comment_id)} class={classes!("comment-thread-item")}>
                                                        <header class={classes!("comment-thread-head")}>
                                                            <div class={classes!("comment-avatar")} style={avatar_style}>
                                                                { avatar_initial }
                                                            </div>
                                                            <div class={classes!("comment-head-meta")}>
                                                                <p class={classes!("comment-author-name")}>{ comment.author_name.clone() }</p>
                                                                <p class={classes!("comment-meta-line")}>
                                                                    { format!("{} · {}", region_label, format_published_time(comment.published_at)) }
                                                                </p>
                                                            </div>
                                                            <button
                                                                type="button"
                                                                class={classes!("comment-jump-btn")}
                                                                onclick={quote_reply_click}
                                                            >
                                                                { "引用并回复" }
                                                            </button>
                                                        </header>

                                                        if let Some(quote_text) = selected_text {
                                                            <div class={classes!("comment-quote-card")}>
                                                                <p class={classes!("comment-quote-label")}>{ "选中段落" }</p>
                                                                <p class={classes!("comment-quote-text")}>{ quote_text }</p>
                                                                {
                                                                    if anchor_block_id.is_some() {
                                                                        html! {
                                                                            <button
                                                                                type="button"
                                                                                class={classes!("comment-jump-btn")}
                                                                                onclick={jump_quote_click}
                                                                            >
                                                                                { "定位到正文" }
                                                                            </button>
                                                                        }
                                                                    } else {
                                                                        html! {}
                                                                    }
                                                                }
                                                            </div>
                                                        }

                                                        if let Some(reply_text) = reply_to_comment_text {
                                                            <div class={classes!("comment-quote-card")}>
                                                                <p class={classes!("comment-quote-label")}>{ "引用评论" }</p>
                                                                <p class={classes!("comment-quote-text")}>{ reply_text }</p>
                                                                if let Some(ai_reply) = reply_to_ai_reply_markdown {
                                                                    <p class={classes!("comment-meta-line")}>
                                                                        { format!("被引用 AI 回复：{}", ai_reply.chars().take(140).collect::<String>()) }
                                                                    </p>
                                                                }
                                                                if reply_to_comment_id.is_some() {
                                                                    <button
                                                                        type="button"
                                                                        class={classes!("comment-jump-btn")}
                                                                        onclick={jump_reply_comment_click}
                                                                    >
                                                                        { "定位到被引用评论" }
                                                                    </button>
                                                                }
                                                            </div>
                                                        }

                                                        <section class={classes!("comment-user-card")}>
                                                            <p class={classes!("comment-section-label")}>{ "用户评论" }</p>
                                                            <p class={classes!("comment-user-text")}>{ comment.comment_text.clone() }</p>
                                                        </section>

                                                        if let Some(ai_reply_html) = ai_reply_html {
                                                            <section class={classes!("comment-ai-card")}>
                                                                <p class={classes!("comment-section-label")}>{ "AI 回复" }</p>
                                                                <div class={classes!("article-content", "comment-ai-markdown")}>
                                                                    { ai_reply_html }
                                                                </div>
                                                            </section>
                                                        }
                                                    </article>
                                                }
                                            }) }
                                        </div>
                                    }
                                }
                            }
                        </section>
                        {
                            if *is_brief_open {
                                html! {
                                    <div
                                        class={classes!(
                                            "fixed",
                                            "inset-0",
                                            "z-[95]",
                                            "flex",
                                            "items-center",
                                            "justify-center",
                                            "bg-black/55",
                                            "p-4",
                                            "backdrop-blur-sm"
                                        )}
                                        role="dialog"
                                        aria-modal="true"
                                        aria-label={t::DETAILED_SUMMARY_ARIA}
                                        onclick={close_brief_click.clone()}
                                    >
                                        <section
                                            class={classes!(
                                                "w-full",
                                                "max-w-[760px]",
                                                "max-h-[85vh]",
                                                "overflow-auto",
                                                "rounded-[var(--radius)]",
                                                "border",
                                                "border-[var(--border)]",
                                                "bg-[var(--surface)]",
                                                "px-6",
                                                "py-5",
                                                "shadow-[var(--shadow-lg)]",
                                                "sm:px-4",
                                                "sm:py-4"
                                            )}
                                            onclick={stop_brief_bubble.clone()}
                                        >
                                            <div class={classes!(
                                                "mb-4",
                                                "flex",
                                                "items-center",
                                                "justify-between",
                                                "gap-3"
                                            )}>
                                                <p class={classes!(
                                                    "m-0",
                                                    "inline-flex",
                                                    "items-center",
                                                    "gap-2",
                                                    "text-sm",
                                                    "font-semibold",
                                                    "uppercase",
                                                    "tracking-[0.12em]",
                                                    "text-[var(--primary)]"
                                                )}>
                                                    <i class={classes!("fas", "fa-list-check")} aria-hidden="true"></i>
                                                    { summary_title }
                                                </p>
                                                <button
                                                    type="button"
                                                    class={classes!(
                                                        "rounded-full",
                                                        "border",
                                                        "border-[var(--border)]",
                                                        "bg-[var(--surface)]",
                                                        "px-3",
                                                        "py-1",
                                                        "text-xs",
                                                        "font-semibold",
                                                        "tracking-[0.08em]",
                                                        "text-[var(--muted)]",
                                                        "hover:border-[var(--primary)]",
                                                        "hover:text-[var(--primary)]"
                                                    )}
                                                    aria-label={t::CLOSE_BRIEF_ARIA}
                                                    onclick={close_brief_click.clone()}
                                                >
                                                    { t::CLOSE_BRIEF_BUTTON }
                                                </button>
                                            </div>
                                            {
                                                if let Some(summary_html) = detailed_summary_html.clone() {
                                                    html! {
                                                        <div class={classes!(
                                                            "article-content",
                                                            "text-[0.97rem]",
                                                            "leading-[1.8]"
                                                        )}>
                                                            { summary_html }
                                                        </div>
                                                    }
                                                } else {
                                                    html! {
                                                        <p class={classes!("m-0", "text-[var(--muted)]")}>
                                                            { "No brief available." }
                                                        </p>
                                                    }
                                                }
                                            }
                                        </section>
                                    </div>
                                }
                            } else {
                                html! {}
                            }
                        }
                    </article>
                </div>
            </section>
        }
    } else {
        html! {
            <section class={classes!(
                "bg-[var(--surface)]",
                "border",
                "border-[var(--border)]",
                "rounded-[var(--radius)]",
                "shadow-[var(--shadow)]",
                "p-10",
                "my-10",
                "mx-auto",
                "max-w-[820px]",
                "flex",
                "flex-col",
                "gap-[0.9rem]",
                "sm:p-5",
                "sm:my-6"
            )}>
                <div class={classes!(
                    "flex",
                    "flex-col",
                    "gap-3",
                    "fade-in"
                )}>
                    <p class={classes!(
                        "m-0",
                        "inline-flex",
                        "items-center",
                        "gap-[0.35rem]",
                        "uppercase",
                        "text-[0.85rem]",
                        "tracking-[0.2em]",
                        "text-[var(--primary)]"
                    )}>{ "404" }</p>
                    <h1 class={classes!(
                        "m-0",
                        "text-[2.25rem]",
                        "leading-[1.25]",
                        "sm:text-[1.65rem]"
                    )}>{ t::NOT_FOUND_TITLE }</h1>
                    <p class={classes!(
                        "m-0",
                        "text-[var(--muted)]"
                    )}>{ t::NOT_FOUND_DESC }</p>
                </div>
            </section>
        }
    };

    html! {
        <main class={classes!("main", "mt-[var(--space-lg)]")}>
            // Fixed back button - hide when any overlay is open
            if !is_overlay_open {
                <div class={classes!(
                    "fixed",
                    "left-8",
                    "top-[calc(var(--header-height-desktop)+2rem)]",
                    "z-50",
                    "max-sm:left-6",
                    "max-sm:top-[calc(var(--header-height-mobile)+1.5rem)]",
                    "flex",
                    "flex-col",
                    "gap-3"
                )}>
                    <div>
                        <TooltipIconButton
                            icon={IconName::ArrowLeft}
                            tooltip={t::BACK_TOOLTIP}
                            position={TooltipPosition::Right}
                            onclick={handle_back}
                            size={20}
                        />
                    </div>
                    {
                        if article_data.is_some() {
                            html! {
                                <>
                                    <div>
                                        <TooltipIconButton
                                            icon={IconName::TrendingUp}
                                            tooltip={t::TREND_TOOLTIP}
                                            position={TooltipPosition::Right}
                                            onclick={open_trend_click.clone()}
                                            size={20}
                                        />
                                    </div>
                                    <div>
                                        <TooltipIconButton
                                            icon={IconName::MessageSquare}
                                            tooltip={"定位到评论区".to_string()}
                                            position={TooltipPosition::Right}
                                            onclick={jump_to_comments_click.clone()}
                                            size={20}
                                        />
                                    </div>
                                </>
                            }
                        } else {
                            html! {}
                        }
                    }
                </div>
            }

            <div class={classes!("container", "article-page-container")}>
                { body }
            </div>
            {
                if !is_overlay_open {
                    if let (Some((left, top)), Some(_)) = ((*selection_button_pos).clone(), (*selection_draft).clone()) {
                        html! {
                            <button
                                type="button"
                                class={classes!("selection-comment-fab")}
                                style={format!("left: {left:.1}px; top: {top:.1}px;")}
                                onclick={open_selection_modal_click.clone()}
                            >
                                <i class={classes!("fas", "fa-comment-dots")} aria-hidden="true"></i>
                                { "评论所选" }
                            </button>
                        }
                    } else {
                        html! {}
                    }
                } else {
                    html! {}
                }
            }
            {
                if *selection_modal_open {
                    let draft_snapshot = (*selection_draft).clone();
                    let selected_excerpt = draft_snapshot
                        .as_ref()
                        .map(|draft| draft.selected_text.chars().take(240).collect::<String>())
                        .unwrap_or_default();
                    let has_ellipsis = draft_snapshot
                        .as_ref()
                        .map(|draft| draft.selected_text.chars().count() > 240)
                        .unwrap_or(false);

                    html! {
                        <div
                            class={classes!(
                                "fixed",
                                "inset-0",
                                "z-[97]",
                                "flex",
                                "items-center",
                                "justify-center",
                                "bg-black/55",
                                "p-4",
                                "backdrop-blur-sm"
                            )}
                            role="dialog"
                            aria-modal="true"
                            aria-label={"评论所选内容"}
                            onclick={close_selection_modal_click.clone()}
                        >
                            <section
                                class={classes!("selection-comment-modal")}
                                onclick={stop_selection_modal_bubble.clone()}
                            >
                                <header class={classes!("selection-comment-modal-header")}>
                                    <div>
                                        <h3 class={classes!("selection-comment-modal-title")}>{ "评论所选内容" }</h3>
                                        <p class={classes!("selection-comment-modal-subtitle")}>
                                            { "你可以基于选中的段落提交疑问、勘误或补充观点。" }
                                        </p>
                                    </div>
                                    <button
                                        type="button"
                                        class={classes!("selection-comment-close")}
                                        aria-label={"关闭评论弹窗"}
                                        onclick={close_selection_modal_click.clone()}
                                    >
                                        { "关闭" }
                                    </button>
                                </header>

                                <div class={classes!("selection-comment-quote")}>
                                    <p class={classes!("selection-comment-quote-label")}>{ "选中内容" }</p>
                                    <p class={classes!("selection-comment-quote-text")}>
                                        {
                                            if has_ellipsis {
                                                format!("{selected_excerpt}...")
                                            } else {
                                                selected_excerpt
                                            }
                                        }
                                    </p>
                                </div>

                                <textarea
                                    class={classes!("comment-compose-textarea")}
                                    placeholder={"请输入你的疑问或评论..."}
                                    value={(*selection_comment_input).clone()}
                                    oninput={on_selection_comment_input}
                                />

                                {
                                    if let Some((is_success, message)) = (*selection_submit_feedback).clone() {
                                        let feedback_class = if is_success {
                                            classes!("text-sm", "text-emerald-700", "dark:text-emerald-200")
                                        } else {
                                            classes!("text-sm", "text-red-700", "dark:text-red-200")
                                        };
                                        html! {
                                            <p class={feedback_class}>{ message }</p>
                                        }
                                    } else {
                                        html! {}
                                    }
                                }

                                <div class={classes!("selection-comment-modal-actions")}>
                                    <button
                                        type="button"
                                        class={classes!("btn-fluent-secondary", "px-4", "py-2")}
                                        onclick={close_selection_modal_click}
                                    >
                                        { "取消" }
                                    </button>
                                    <button
                                        type="button"
                                        class={classes!("btn-fluent-primary", "px-4", "py-2")}
                                        onclick={submit_selection_comment_click}
                                        disabled={*selection_submit_loading}
                                    >
                                        {
                                            if *selection_submit_loading {
                                                "提交中..."
                                            } else {
                                                "提交评论"
                                            }
                                        }
                                    </button>
                                </div>
                            </section>
                        </div>
                    }
                } else {
                    html! {}
                }
            }
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
                                <button
                                    type="button"
                                    class={classes!(
                                        "rounded-full",
                                        "bg-black/70",
                                        "px-3",
                                        "py-1",
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
                                        "py-1",
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
                                        "py-1",
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
                            </div>
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
                                aria-label={t::CLOSE_IMAGE_ARIA}
                                onclick={close_lightbox_click.clone()}
                            >
                                { "X" }
                            </button>
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
                                        let alt_text = article_data
                                            .as_ref()
                                            .map(|article| article.title.clone())
                                            .unwrap_or_else(|| t::DEFAULT_IMAGE_ALT.to_string());
                                        html! {
                                            <>
                                                <img
                                                    src={src.clone()}
                                                    alt={alt_text}
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
                                                                { fill_one(t::IMAGE_PREVIEW_FAILED, &src) }
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
            {
                if *is_trend_open {
                    html! {
                        <div
                            class={classes!(
                                "fixed",
                                "inset-0",
                                "z-[96]",
                                "flex",
                                "items-center",
                                "justify-center",
                                "bg-black/55",
                                "p-4",
                                "backdrop-blur-sm"
                            )}
                            role="dialog"
                            aria-modal="true"
                            aria-label={t::TREND_TITLE}
                            onclick={close_trend_click.clone()}
                        >
                            <section
                                class={classes!(
                                    "w-full",
                                    "max-w-[920px]",
                                    "max-h-[88vh]",
                                    "overflow-auto",
                                    "rounded-[var(--radius)]",
                                    "border",
                                    "border-[var(--border)]",
                                    "bg-[var(--surface)]",
                                    "px-5",
                                    "py-5",
                                    "shadow-[var(--shadow-lg)]",
                                    "sm:px-4",
                                    "sm:py-4"
                                )}
                                onclick={stop_trend_bubble.clone()}
                            >
                                <div class={classes!(
                                    "mb-4",
                                    "flex",
                                    "items-start",
                                    "justify-between",
                                    "gap-3"
                                )}>
                                    <div class={classes!("flex", "flex-col", "gap-1")}>
                                        <h2 class={classes!("m-0", "text-[1.1rem]", "font-semibold")}>{ t::TREND_TITLE }</h2>
                                        <p class={classes!("m-0", "text-sm", "text-[var(--muted)]")}>{ t::TREND_SUBTITLE }</p>
                                        <p class={classes!("m-0", "text-xs", "text-[var(--muted)]")}>
                                            {
                                                if let Some(total) = *view_total {
                                                    fill_one(t::TREND_TOTAL_TEMPLATE, total)
                                                } else {
                                                    t::VIEW_COUNT_LOADING.to_string()
                                                }
                                            }
                                            {
                                                if let Some(today) = *view_today {
                                                    format!(" · 今日 {}", today)
                                                } else {
                                                    String::new()
                                                }
                                            }
                                        </p>
                                    </div>
                                    <button
                                        type="button"
                                        class={classes!(
                                            "rounded-full",
                                            "border",
                                            "border-[var(--border)]",
                                            "bg-[var(--surface)]",
                                            "px-3",
                                            "py-1",
                                            "text-xs",
                                            "font-semibold",
                                            "tracking-[0.08em]",
                                            "text-[var(--muted)]",
                                            "hover:border-[var(--primary)]",
                                            "hover:text-[var(--primary)]"
                                        )}
                                        aria-label={t::TREND_CLOSE_ARIA}
                                        onclick={close_trend_click.clone()}
                                    >
                                        { t::CLOSE_BRIEF_BUTTON }
                                    </button>
                                </div>

                                <div class={classes!("mb-4", "flex", "items-center", "gap-2", "flex-wrap")}>
                                    <button
                                        type="button"
                                        class={classes!(
                                            "rounded-full",
                                            "border",
                                            "px-3",
                                            "py-1.5",
                                            "text-xs",
                                            "font-semibold",
                                            "tracking-[0.08em]",
                                            if *trend_granularity == TrendGranularity::Day {
                                                classes!("border-[var(--primary)]", "bg-[var(--primary)]", "text-white")
                                            } else {
                                                classes!("border-[var(--border)]", "text-[var(--muted)]", "hover:border-[var(--primary)]", "hover:text-[var(--primary)]")
                                            }
                                        )}
                                        onclick={switch_trend_to_day}
                                    >
                                        { t::TREND_TAB_DAY }
                                    </button>
                                    <button
                                        type="button"
                                        class={classes!(
                                            "rounded-full",
                                            "border",
                                            "px-3",
                                            "py-1.5",
                                            "text-xs",
                                            "font-semibold",
                                            "tracking-[0.08em]",
                                            if *trend_granularity == TrendGranularity::Hour {
                                                classes!("border-[var(--primary)]", "bg-[var(--primary)]", "text-white")
                                            } else {
                                                classes!("border-[var(--border)]", "text-[var(--muted)]", "hover:border-[var(--primary)]", "hover:text-[var(--primary)]")
                                            }
                                        )}
                                        onclick={switch_trend_to_hour}
                                    >
                                        { t::TREND_TAB_HOUR }
                                    </button>
                                </div>

                                if *trend_granularity == TrendGranularity::Hour {
                                    <div class={classes!("mb-4", "flex", "items-center", "gap-2")}>
                                        <label class={classes!("text-sm", "text-[var(--muted)]")}>{ t::TREND_SELECT_DAY }</label>
                                        <select
                                            class={classes!(
                                                "rounded-lg",
                                                "border",
                                                "border-[var(--border)]",
                                                "bg-[var(--surface)]",
                                                "px-3",
                                                "py-1.5",
                                                "text-sm",
                                                "text-[var(--text)]",
                                                "outline-none",
                                                "focus:border-[var(--primary)]"
                                            )}
                                            onchange={on_trend_day_change}
                                            value={(*trend_selected_day).clone().unwrap_or_default()}
                                        >
                                            { for (*trend_day_options).iter().map(|day| {
                                                html! { <option value={day.clone()}>{ day.clone() }</option> }
                                            }) }
                                        </select>
                                    </div>
                                }

                                {
                                    if *trend_loading {
                                        html! {
                                            <div class={classes!(
                                                "rounded-xl",
                                                "border",
                                                "border-[var(--border)]",
                                                "bg-[var(--surface)]",
                                                "px-4",
                                                "py-8",
                                                "text-center",
                                                "text-sm",
                                                "text-[var(--muted)]"
                                            )}>
                                                <i class={classes!("fas", "fa-spinner", "fa-spin", "mr-2")}></i>
                                                { t::TREND_LOADING }
                                            </div>
                                        }
                                    } else if let Some(error) = (*trend_error).clone() {
                                        html! {
                                            <div class={classes!(
                                                "rounded-xl",
                                                "border",
                                                "border-red-400/50",
                                                "bg-red-500/10",
                                                "px-4",
                                                "py-3",
                                                "text-sm",
                                                "text-red-700",
                                                "dark:text-red-200"
                                            )}>
                                                { error }
                                            </div>
                                        }
                                    } else {
                                        html! {
                                            <ViewTrendChart
                                                points={(*trend_points).clone()}
                                                empty_text={t::TREND_EMPTY.to_string()}
                                            />
                                        }
                                    }
                                }
                            </section>
                        </div>
                    }
                } else {
                    html! {}
                }
            }
            // Hide scroll-to-top button and TOC button when overlay is open
            if !is_overlay_open {
                <ScrollToTopButton />
                <TocButton />
            }
        </main>
    }
}
