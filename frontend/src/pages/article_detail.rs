use static_flow_shared::Article;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{window, HtmlImageElement, KeyboardEvent};
use yew::{prelude::*, virtual_dom::AttrValue};
use yew_router::prelude::{use_navigator, use_route, Link};

use crate::{
    components::{
        icons::IconName,
        loading_spinner::{LoadingSpinner, SpinnerSize},
        scroll_to_top_button::ScrollToTopButton,
        toc_button::TocButton,
        tooltip::{TooltipIconButton, TooltipPosition},
    },
    router::Route,
    utils::{image_url, markdown_to_html},
};

#[derive(Properties, Clone, PartialEq)]
pub struct ArticleDetailProps {
    #[prop_or_default]
    pub id: String,
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

    let article_data = (*article).clone();
    let is_lightbox_open = use_state(|| false);
    let preview_image_url = use_state_eq(|| None::<String>);

    let open_image_preview = {
        let is_lightbox_open = is_lightbox_open.clone();
        let preview_image_url = preview_image_url.clone();
        Callback::from(move |src: String| {
            preview_image_url.set(Some(src));
            is_lightbox_open.set(true);
        })
    };

    let close_lightbox_click = {
        let is_lightbox_open = is_lightbox_open.clone();
        let preview_image_url = preview_image_url.clone();
        Callback::from(move |_| {
            is_lightbox_open.set(false);
            preview_image_url.set(None);
        })
    };

    {
        let is_lightbox_open = is_lightbox_open.clone();
        let preview_image_url = preview_image_url.clone();
        use_effect_with(*is_lightbox_open, move |is_open| {
            let keydown_listener_opt = if *is_open {
                let handle = is_lightbox_open.clone();
                let preview_url = preview_image_url.clone();
                let listener =
                    wasm_bindgen::closure::Closure::wrap(Box::new(move |event: KeyboardEvent| {
                        if event.key() == "Escape" {
                            handle.set(false);
                            preview_url.set(None);
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

    // Initialize markdown rendering after content is loaded
    use_effect_with(article_data.clone(), |article_opt| {
        if article_opt.is_some() {
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
        || ()
    });

    {
        let open_image_preview = open_image_preview.clone();
        use_effect_with(article_data.clone(), move |article_opt| {
            let mut listeners: Vec<(
                web_sys::Element,
                wasm_bindgen::closure::Closure<dyn FnMut(web_sys::Event)>,
            )> = Vec::new();

            if article_opt.is_some() {
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
        <div class="flex min-h-[50vh] items-center justify-center">
            <LoadingSpinner size={SpinnerSize::Large} />
        </div>
    };

    let body = if *loading {
        loading_view
    } else if let Some(article) = article_data.clone() {
        let word_count = article
            .content
            .chars()
            .filter(|c| !c.is_whitespace())
            .count();
        let render_html = markdown_to_html(&article.content);
        let content = Html::from_html_unchecked(AttrValue::from(render_html));

        html! {
            <article class="article-detail">
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
                            <div class="article-featured relative group">
                                <img
                                    class="cursor-zoom-in"
                                    src={image_src.clone()}
                                    alt={article.title.clone()}
                                    loading="lazy"
                                    onclick={open_featured_preview.clone()}
                                />
                                <button
                                    type="button"
                                    class="hidden md:inline-flex absolute bottom-4 right-4 rounded-full bg-black/70 px-4 py-2 text-sm text-white backdrop-blur hover:bg-black/80 dark:bg-white/20 dark:text-white"
                                    onclick={open_featured_preview}
                                >
                                    { "查看原图" }
                                </button>
                            </div>
                        }
                    } else {
                        html! {}
                    }
                }

                <header class="article-header fade-in">
                    <Link<Route>
                        to={Route::CategoryDetail { category: article.category.clone() }}
                        classes={classes!("article-category")}
                    >
                        { article.category.clone() }
                    </Link<Route>>
                    <h1 class="article-title">
                        { article.title.clone() }
                    </h1>
                    <div class="article-meta" aria-label="文章元信息">
                        <span class="article-meta-item">
                            <i class="fas fa-user-circle" aria-hidden="true"></i>
                            { article.author.clone() }
                        </span>
                        <span class="article-meta-item">
                            <i class="far fa-calendar-alt" aria-hidden="true"></i>
                            { article.date.clone() }
                        </span>
                        <Link<Route>
                            to={Route::CategoryDetail { category: article.category.clone() }}
                            classes={classes!("article-meta-item")}
                        >
                            <i class="far fa-folder-open" aria-hidden="true"></i>
                            { article.category.clone() }
                        </Link<Route>>
                        <span class="article-meta-item">
                            <i class="far fa-file-alt" aria-hidden="true"></i>
                            { format!("{} 字", word_count) }
                        </span>
                        <span class="article-meta-item">
                            <i class="far fa-clock" aria-hidden="true"></i>
                            { format!("约 {} 分钟", article.read_time) }
                        </span>
                    </div>
                </header>

                <section class="article-content" aria-label="文章正文">
                    { content }
                </section>

                <footer class="article-footer">
                    <h2 class="article-footer-title">{ "标签" }</h2>
                    <ul class="article-tags">
                        { for article.tags.iter().cloned().map(|tag| {
                            html! {
                                <li>
                                    <Link<Route>
                                        to={Route::TagDetail { tag: tag.clone() }}
                                        classes={classes!("article-tag-pill")}
                                    >
                                        { format!("#{}", tag) }
                                    </Link<Route>>
                                </li>
                            }
                        }) }
                    </ul>
                </footer>
            </article>
        }
    } else {
        html! {
            <section class="article-detail not-found">
                <div class="article-header fade-in">
                    <p class="article-category">{ "404" }</p>
                    <h1 class="article-title">{ "文章未找到" }</h1>
                    <p class="article-empty">{ "抱歉，没有找到对应的文章，请返回列表重试。" }</p>
                </div>
            </section>
        }
    };

    html! {
        <main class="main">
            // Fixed back button - hide when lightbox is open
            if !*is_lightbox_open {
                <div class="article-back-button">
                    <TooltipIconButton
                        icon={IconName::ArrowLeft}
                        tooltip="返回"
                        position={TooltipPosition::Right}
                        onclick={handle_back}
                        size={20}
                    />
                </div>
            }

            <div class="container">
                { body }
            </div>
            {
                if *is_lightbox_open {
                    html! {
                        <div
                            class="fixed inset-0 z-[100] flex items-center justify-center bg-black/80 p-4 text-white backdrop-blur-sm transition dark:bg-black/80"
                            role="dialog"
                            aria-modal="true"
                            onclick={close_lightbox_click.clone()}
                        >
                            <button
                                type="button"
                                class="absolute right-4 top-4 z-[101] rounded-full bg-black/70 px-3 py-1 text-lg leading-none text-white hover:bg-black"
                                aria-label="关闭图片"
                                onclick={close_lightbox_click.clone()}
                            >
                                { "X" }
                            </button>
                            <div class="max-h-full max-w-full cursor-pointer" onclick={close_lightbox_click.clone()}>
                                {
                                    if let Some(src) = (*preview_image_url).clone() {
                                        let alt_text = article_data
                                            .as_ref()
                                            .map(|article| article.title.clone())
                                            .unwrap_or_else(|| "文章图片".to_string());
                                        html! {
                                            <img
                                                src={src}
                                                alt={alt_text}
                                                class="max-h-[90vh] max-w-[90vw] object-contain cursor-pointer"
                                                loading="lazy"
                                            />
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
            // Hide scroll-to-top button and TOC button when lightbox is open
            if !*is_lightbox_open {
                <ScrollToTopButton />
                <TocButton />
            }
        </main>
    }
}
