use static_flow_shared::{Article, ArticleListItem};
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{window, HtmlImageElement, KeyboardEvent};
use yew::{prelude::*, virtual_dom::AttrValue};
use yew_router::prelude::{use_navigator, use_route, Link};

use crate::{
    api::fetch_related_articles,
    components::{
        article_card::ArticleCard,
        icons::IconName,
        image_with_loading::ImageWithLoading,
        loading_spinner::{LoadingSpinner, SpinnerSize},
        scroll_to_top_button::ScrollToTopButton,
        toc_button::TocButton,
        tooltip::{TooltipIconButton, TooltipPosition},
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

    // Handle back navigation - use browser history
    let handle_back = {
        let navigator = navigator.clone();
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();

            // Try to go back in browser history
            if let Some(win) = window() {
                if let Ok(history) = win.history() {
                    // Check if there's history to go back to
                    if let Ok(length) = history.length() {
                        if length > 1 {
                            let _ = history.back();
                            return;
                        }
                    }
                }
            }

            // Fallback: navigate to posts page if no history
            if let Some(nav) = navigator.as_ref() {
                nav.push(&Route::Posts);
            }
        })
    };

    {
        let article = article.clone();
        let article_id = article_id.clone();
        let loading = loading.clone();
        use_effect_with(article_id.clone(), move |id| {
            let id = id.clone();
            let article = article.clone();
            let loading = loading.clone();
            loading.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                match crate::api::fetch_article_detail(&id).await {
                    Ok(data) => {
                        article.set(data);
                        loading.set(false);
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

    let article_data = (*article).clone();
    let content_language = use_state(|| ArticleContentLanguage::Zh);
    let is_lightbox_open = use_state(|| false);
    let is_brief_open = use_state(|| false);
    let preview_image_url = use_state_eq(|| None::<String>);
    let preview_image_failed = use_state(|| false);
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
        use_effect_with(article_id.clone(), move |_| {
            is_brief_open.set(false);
            || ()
        });
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

    let open_brief_click = {
        let is_brief_open = is_brief_open.clone();
        Callback::from(move |_| is_brief_open.set(true))
    };

    let close_brief_click = {
        let is_brief_open = is_brief_open.clone();
        Callback::from(move |_| is_brief_open.set(false))
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

    let stop_lightbox_bubble = Callback::from(|event: MouseEvent| event.stop_propagation());
    let stop_brief_bubble = Callback::from(|event: MouseEvent| event.stop_propagation());
    let mark_preview_failed = {
        let preview_image_failed = preview_image_failed.clone();
        Callback::from(move |_: Event| preview_image_failed.set(true))
    };
    let mark_preview_loaded = {
        let preview_image_failed = preview_image_failed.clone();
        Callback::from(move |_: Event| preview_image_failed.set(false))
    };

    let markdown_render_key = if let Some(article) = article_data.as_ref() {
        let lang_key = if *content_language == ArticleContentLanguage::En { "en" } else { "zh" };
        format!("{}:{lang_key}", article.id)
    } else {
        String::new()
    };

    // Initialize markdown rendering after content/language is loaded
    use_effect_with(markdown_render_key.clone(), |render_key| {
        if !render_key.is_empty() {
            // Use setTimeout to ensure DOM is fully updated
            if let Some(win) = window() {
                let callback = wasm_bindgen::closure::Closure::wrap(Box::new(move || {
                    if let Some(win) = window() {
                        if let Ok(init_fn) =
                            js_sys::Reflect::get(&win, &JsValue::from_str("initMarkdownRendering"))
                        {
                            if let Ok(func) = init_fn.dyn_into::<js_sys::Function>() {
                                let _ = func.call0(&win);
                            }
                        }
                    }
                })
                    as Box<dyn FnMut()>);

                let _ = win.set_timeout_with_callback_and_timeout_and_arguments_0(
                    callback.as_ref().unchecked_ref(),
                    100,
                );
                callback.forget();
            }
        }
        move || {
            // Cleanup TOC on unmount or when switching away
            if let Some(win) = window() {
                if let Ok(cleanup_fn) = js_sys::Reflect::get(&win, &JsValue::from_str("cleanupTOC"))
                {
                    if let Ok(func) = cleanup_fn.dyn_into::<js_sys::Function>() {
                        let _ = func.call0(&win);
                    }
                }
            }
        }
    });

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

        html! {
            <article class={classes!(
                "bg-[var(--surface)]",
                "border",
                "border-[var(--border)]",
                "rounded-[var(--radius)]",
                "shadow-[var(--shadow)]",
                "p-10",
                "my-10",
                "mx-auto",
                "max-w-[820px]",
                "sm:p-5",
                "sm:my-6"
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
                                "-mx-10",
                                "-mt-10",
                                "mb-8",
                                "rounded-t-[calc(var(--radius)-2px)]",
                                "overflow-hidden",
                                "max-h-[420px]",
                                "relative",
                                "group",
                                "sm:-mx-5",
                                "sm:-mt-5",
                                "sm:mb-5"
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

                <header class={classes!(
                    "flex",
                    "flex-col",
                    "gap-3",
                    "mb-5",
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
                    {
                        if show_language_toggle || has_detailed_summary {
                            html! {
                                <div class={classes!(
                                    "flex",
                                    "flex-wrap",
                                    "items-center",
                                    "gap-3"
                                )}>
                                    {
                                        if show_language_toggle {
                                            html! {
                                                <div class={classes!(
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
                                                        class={zh_button_class}
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
                                                        class={en_button_class}
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
                                    {
                                        if has_detailed_summary {
                                            html! {
                                                <button
                                                    type="button"
                                                    class={classes!(
                                                        "inline-flex",
                                                        "items-center",
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
                                </div>
                            }
                        } else {
                            html! {}
                        }
                    }
                    <div class={classes!(
                        "flex",
                        "flex-wrap",
                        "gap-3",
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
                    </div>
                </header>

                <section class={classes!("article-content")} aria-label={t::ARTICLE_BODY_ARIA}>
                    { content }
                </section>

                <footer class={classes!(
                    "mt-10",
                    "border-t",
                    "border-[var(--border)]",
                    "pt-6"
                )}>
                    <h2 class={classes!(
                        "m-0",
                        "mb-4",
                        "text-[1rem]",
                        "text-[var(--muted)]",
                        "tracking-[0.15em]",
                        "uppercase"
                    )}>{ t::TAGS_TITLE }</h2>
                    <ul class={classes!(
                        "list-none",
                        "flex",
                        "flex-wrap",
                        "gap-3",
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
                    "mt-12",
                    "pt-8",
                    "border-t",
                    "border-[var(--border)]"
                )}>
                    <h2 class={classes!(
                        "m-0",
                        "mb-6",
                        "text-[1.1rem]",
                        "text-[var(--muted)]",
                        "tracking-[0.15em]",
                        "uppercase"
                    )}>{ t::RELATED_TITLE }</h2>
                    if *related_loading {
                        <div class={classes!(
                            "flex",
                            "items-center",
                            "gap-3",
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
                            "gap-6",
                            "md:grid-cols-2"
                        )}>
                            { for related_articles.iter().cloned().map(|article| {
                                html! { <ArticleCard key={article.id.clone()} article={article.clone()} /> }
                            }) }
                        </div>
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

    let is_overlay_open = *is_lightbox_open || *is_brief_open;

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
                    "max-sm:top-[calc(var(--header-height-mobile)+1.5rem)]"
                )}>
                    <TooltipIconButton
                        icon={IconName::ArrowLeft}
                        tooltip={t::BACK_TOOLTIP}
                        position={TooltipPosition::Right}
                        onclick={handle_back}
                        size={20}
                    />
                </div>
            }

            <div class={classes!("container")}>
                { body }
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
                                                        "object-contain"
                                                    )}
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
            // Hide scroll-to-top button and TOC button when overlay is open
            if !is_overlay_open {
                <ScrollToTopButton />
                <TocButton />
            }
        </main>
    }
}
