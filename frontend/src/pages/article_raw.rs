use gloo_timers::future::TimeoutFuture;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::window;
use yew::prelude::*;
use yew_router::prelude::{use_navigator, use_route};

use crate::{i18n::current::article_raw_page as t, router::Route};

#[derive(Properties, Clone, PartialEq)]
pub struct ArticleRawProps {
    #[prop_or_default]
    pub id: String,
    #[prop_or_default]
    pub lang: String,
}

fn normalize_lang(raw: &str) -> Option<&'static str> {
    let normalized = raw.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "zh" => Some("zh"),
        "en" => Some("en"),
        _ => None,
    }
}

#[function_component(ArticleRawPage)]
pub fn article_raw_page(props: &ArticleRawProps) -> Html {
    let route = use_route::<Route>();
    let navigator = use_navigator();

    let (article_id, raw_lang) = route
        .as_ref()
        .and_then(|value| match value {
            Route::ArticleRaw {
                id,
                lang,
            } => Some((id.clone(), lang.clone())),
            _ => None,
        })
        .unwrap_or_else(|| (props.id.clone(), props.lang.clone()));

    let lang = normalize_lang(&raw_lang).unwrap_or("zh").to_string();
    let markdown = use_state(|| None::<String>);
    let loading = use_state(|| true);
    let error = use_state(|| None::<String>);
    let copied = use_state(|| false);

    {
        let article_id = article_id.clone();
        let lang = lang.clone();
        let markdown = markdown.clone();
        let loading = loading.clone();
        let error = error.clone();
        use_effect_with((article_id.clone(), lang.clone()), move |_| {
            loading.set(true);
            markdown.set(None);
            error.set(None);

            let markdown = markdown.clone();
            let loading = loading.clone();
            let error = error.clone();
            let article_id = article_id.clone();
            let lang = lang.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match crate::api::fetch_article_raw_markdown(&article_id, &lang).await {
                    Ok(content) => {
                        markdown.set(Some(content));
                        loading.set(false);
                    },
                    Err(err_text) => {
                        error.set(Some(err_text));
                        loading.set(false);
                    },
                }
            });

            || ()
        });
    }

    let back_click = {
        let navigator = navigator.clone();
        let article_id = article_id.clone();
        Callback::from(move |_| {
            if let Some(nav) = navigator.as_ref() {
                nav.push(&Route::ArticleDetail {
                    id: article_id.clone(),
                });
            }
        })
    };

    let copy_click = {
        let copied = copied.clone();
        let markdown = markdown.clone();
        Callback::from(move |_| {
            let copied = copied.clone();
            let text = (*markdown).clone().unwrap_or_default();
            wasm_bindgen_futures::spawn_local(async move {
                let mut ok = false;
                if let Some(win) = window() {
                    let navigator = win.navigator();
                    if let Ok(clipboard) =
                        js_sys::Reflect::get(&navigator, &JsValue::from_str("clipboard"))
                    {
                        if !clipboard.is_undefined() && !clipboard.is_null() {
                            if let Ok(write_text) =
                                js_sys::Reflect::get(&clipboard, &JsValue::from_str("writeText"))
                            {
                                if let Some(write_fn) = write_text.dyn_ref::<js_sys::Function>() {
                                    if let Ok(promise_value) =
                                        write_fn.call1(&clipboard, &JsValue::from_str(&text))
                                    {
                                        if let Ok(promise) =
                                            promise_value.dyn_into::<js_sys::Promise>()
                                        {
                                            ok = wasm_bindgen_futures::JsFuture::from(promise)
                                                .await
                                                .is_ok();
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if ok {
                    copied.set(true);
                    TimeoutFuture::new(1800).await;
                    copied.set(false);
                }
            });
        })
    };

    let lang_label = if lang == "en" { "EN" } else { "ZH" };
    let page_title = t::TITLE_TEMPLATE
        .replacen("{}", &article_id, 1)
        .replacen("{}", lang_label, 1);

    html! {
        <main class={classes!("container", "mx-auto", "px-4", "py-8", "min-h-[70vh]")}>
            <section class={classes!(
                "article-detail",
                "bg-[var(--surface)]",
                "border",
                "border-[var(--border)]",
                "rounded-[var(--radius)]",
                "shadow-[var(--shadow)]",
                "p-6",
                "sm:p-4"
            )}>
                <header class={classes!("mb-4", "flex", "flex-wrap", "items-center", "justify-between", "gap-3")}>
                    <div class={classes!("min-w-0")}>
                        <p class={classes!("m-0", "text-xs", "uppercase", "tracking-[0.1em]", "text-[var(--muted)]")}>
                            { t::RAW_BADGE }
                        </p>
                        <h1 class={classes!("m-0", "text-xl", "font-semibold", "text-[var(--text)]", "break-all")}>
                            { page_title }
                        </h1>
                    </div>
                    <div class={classes!("flex", "items-center", "gap-2")}>
                        <button
                            type="button"
                            class={classes!("btn-fluent-secondary", "!px-3", "!py-2", "!text-xs")}
                            onclick={back_click}
                        >
                            <i class={classes!("fas", "fa-arrow-left")} aria-hidden="true"></i>
                            { t::BACK_BUTTON }
                        </button>
                        <button
                            type="button"
                            class={classes!("btn-fluent-secondary", "!px-3", "!py-2", "!text-xs")}
                            onclick={copy_click}
                            disabled={(*markdown).is_none()}
                        >
                            <i class={if *copied { classes!("fas", "fa-check") } else { classes!("far", "fa-copy") }} aria-hidden="true"></i>
                            {
                                if *copied {
                                    t::COPIED_BUTTON
                                } else {
                                    t::COPY_BUTTON
                                }
                            }
                        </button>
                    </div>
                </header>

                if *loading {
                    <p class={classes!("m-0", "text-sm", "text-[var(--muted)]")}>{ t::LOADING }</p>
                } else if let Some(err_text) = (*error).clone() {
                    <p class={classes!("m-0", "text-sm", "text-red-600", "dark:text-red-300")}>
                        { format!("{}: {}", t::ERROR_PREFIX, err_text) }
                    </p>
                } else if let Some(content) = (*markdown).clone() {
                    <pre class={classes!(
                        "m-0",
                        "rounded-[var(--radius)]",
                        "border",
                        "border-[var(--border)]",
                        "bg-[var(--surface-alt)]",
                        "p-4",
                        "text-sm",
                        "leading-7",
                        "overflow-auto",
                        "whitespace-pre"
                    )}>
                        { content }
                    </pre>
                } else {
                    <p class={classes!("m-0", "text-sm", "text-[var(--muted)]")}>{ t::EMPTY }</p>
                }
            </section>
        </main>
    }
}
