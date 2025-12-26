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
    let related_articles = use_state(|| Vec::<ArticleListItem>::new());
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
        <div class={classes!("flex", "min-h-[50vh]", "items-center", "justify-center")}>
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
                                    { "查看原图" }
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
                    <div class={classes!(
                        "flex",
                        "flex-wrap",
                        "gap-3",
                        "text-[0.9rem]",
                        "text-[var(--muted)]"
                    )} aria-label="文章元信息">
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
                            { format!("{} 字", word_count) }
                        </span>
                        <span class={classes!(
                            "inline-flex",
                            "items-center",
                            "gap-[0.35rem]"
                        )}>
                            <i class={classes!("far", "fa-clock")} aria-hidden="true"></i>
                            { format!("约 {} 分钟", article.read_time) }
                        </span>
                    </div>
                </header>

                <section class={classes!("article-content")} aria-label="文章正文">
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
                    )}>{ "标签" }</h2>
                    <ul class={classes!(
                        "list-none",
                        "flex",
                        "flex-wrap",
                        "gap-3",
                        "m-0",
                        "p-0"
                    )}>
                        { for article.tags.iter().cloned().map(|tag| {
                            html! {
                                <li>
                                    <Link<Route>
                                        to={Route::TagDetail { tag: tag.clone() }}
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
                    )}>{ "相关推荐" }</h2>
                    if *related_loading {
                        <div class={classes!(
                            "flex",
                            "items-center",
                            "gap-3",
                            "text-[var(--muted)]"
                        )}>
                            <LoadingSpinner size={SpinnerSize::Small} />
                            <span>{ "加载相关推荐中..." }</span>
                        </div>
                    } else if related_articles.is_empty() {
                        <p class={classes!("text-[var(--muted)]", "m-0")}>
                            { "暂无相关推荐" }
                        </p>
                    } else {
                        <div class={classes!(
                            "grid",
                            "gap-6",
                            "md:grid-cols-2"
                        )}>
                            { for related_articles.iter().cloned().map(|article| {
                                html! { <ArticleCard article={article} /> }
                            }) }
                        </div>
                    }
                </section>
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
                    )}>{ "文章未找到" }</h1>
                    <p class={classes!(
                        "m-0",
                        "text-[var(--muted)]"
                    )}>{ "抱歉，没有找到对应的文章，请返回列表重试。" }</p>
                </div>
            </section>
        }
    };

    html! {
        <main class={classes!("main", "mt-[var(--space-lg)]")}>
            // Fixed back button - hide when lightbox is open
            if !*is_lightbox_open {
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
                        tooltip="返回"
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
                                aria-label="关闭图片"
                                onclick={close_lightbox_click.clone()}
                            >
                                { "X" }
                            </button>
                            <div class={classes!("max-h-full", "max-w-full", "cursor-pointer")} onclick={close_lightbox_click.clone()}>
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
                                                class={classes!(
                                                    "max-h-[90vh]",
                                                    "max-w-[90vw]",
                                                    "object-contain",
                                                    "cursor-pointer"
                                                )}
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
