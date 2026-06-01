use static_flow_shared::Article;
use web_sys::window;
use yew::prelude::*;
use yew_router::prelude::{use_route, Link};

use crate::{
    components::loading_spinner::{LoadingSpinner, SpinnerSize},
    config::route_path,
    i18n::current::interactive_article_page as t,
    router::Route,
};

#[derive(Properties, Clone, PartialEq)]
pub struct InteractiveArticlePageProps {
    #[prop_or_default]
    pub id: String,
}

fn interactive_page_url(page_id: &str) -> String {
    route_path(&format!("/interactive-pages/{page_id}?lang=zh"))
}

#[function_component(InteractiveArticlePage)]
pub fn interactive_article_page(props: &InteractiveArticlePageProps) -> Html {
    let route = use_route::<Route>();
    let article_id = route
        .as_ref()
        .and_then(|route| match route {
            Route::ArticleInteractive {
                id,
            } => Some(id.clone()),
            _ => None,
        })
        .unwrap_or_else(|| props.id.clone());

    let article = use_state(|| None::<Article>);
    let loading = use_state(|| true);
    let error = use_state(|| None::<String>);

    {
        let article = article.clone();
        let article_id = article_id.clone();
        let loading = loading.clone();
        let error = error.clone();
        use_effect_with(article_id.clone(), move |id| {
            let id = id.clone();
            article.set(None);
            loading.set(true);
            error.set(None);
            wasm_bindgen_futures::spawn_local(async move {
                match crate::api::fetch_article_detail(&id).await {
                    Ok(data) => {
                        article.set(data);
                        loading.set(false);
                    },
                    Err(err) => {
                        error.set(Some(err));
                        article.set(None);
                        loading.set(false);
                    },
                }
            });
            || ()
        });
    }

    let redirect_url = (*article).as_ref().and_then(|record| {
        record
            .interactive_page_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(interactive_page_url)
    });

    {
        let redirect_url = redirect_url.clone();
        use_effect_with(redirect_url.clone(), move |url| {
            if let Some(url) = url.as_ref() {
                if let Some(win) = window() {
                    let _ = win.location().replace(url);
                }
            }
            || ()
        });
    }

    if *loading {
        return html! {
            <section class={classes!("mx-auto", "my-10", "max-w-5xl", "px-4", "text-center")}>
                <div class={classes!("inline-flex", "items-center", "gap-3", "text-[var(--muted)]")}>
                    <LoadingSpinner size={SpinnerSize::Large} />
                    <span>{ t::LOADING }</span>
                </div>
            </section>
        };
    }

    let Some(article) = (*article).clone() else {
        let detail_route = Route::ArticleDetail {
            id: article_id.clone(),
        };
        let error_text = (*error)
            .clone()
            .unwrap_or_else(|| t::NOT_AVAILABLE_DESC.to_string());
        return html! {
            <section class={classes!("mx-auto", "my-10", "max-w-3xl", "px-4")}>
                <div class={classes!(
                    "rounded-[var(--radius)]",
                    "border",
                    "border-[var(--border)]",
                    "bg-[var(--surface)]",
                    "p-6",
                    "shadow-[var(--shadow)]"
                )}>
                    <p class={classes!("m-0", "text-xs", "font-semibold", "uppercase", "tracking-[0.18em]", "text-[var(--primary)]")}>
                        { t::BADGE }
                    </p>
                    <h1 class={classes!("mb-2", "mt-3", "text-[1.6rem]")}>{ t::NOT_AVAILABLE_TITLE }</h1>
                    <p class={classes!("m-0", "text-[var(--muted)]")}>
                        { format!("{}：{}", t::LOAD_ERROR_PREFIX, error_text) }
                    </p>
                    <div class={classes!("mt-5")}>
                        <Link<Route>
                            to={detail_route}
                            classes={classes!(
                                "inline-flex",
                                "items-center",
                                "gap-2",
                                "rounded-full",
                                "border",
                                "border-[var(--border)]",
                                "bg-[var(--surface)]",
                                "px-4",
                                "py-2",
                                "text-sm",
                                "font-medium",
                                "text-[var(--muted)]",
                                "hover:border-[var(--primary)]",
                                "hover:text-[var(--primary)]"
                            )}
                        >
                            { t::BACK_TO_ARTICLE }
                        </Link<Route>>
                    </div>
                </div>
            </section>
        };
    };

    let Some(page_id) = article
        .interactive_page_id
        .clone()
        .filter(|value| !value.trim().is_empty())
    else {
        return html! {
            <section class={classes!("mx-auto", "my-10", "max-w-3xl", "px-4")}>
                <div class={classes!(
                    "rounded-[var(--radius)]",
                    "border",
                    "border-[var(--border)]",
                    "bg-[var(--surface)]",
                    "p-6",
                    "shadow-[var(--shadow)]"
                )}>
                    <p class={classes!("m-0", "text-xs", "font-semibold", "uppercase", "tracking-[0.18em]", "text-[var(--primary)]")}>
                        { t::BADGE }
                    </p>
                    <h1 class={classes!("mb-2", "mt-3", "text-[1.6rem]")}>{ t::NOT_AVAILABLE_TITLE }</h1>
                    <p class={classes!("m-0", "text-[var(--muted)]")}>{ t::NOT_AVAILABLE_DESC }</p>
                    <div class={classes!("mt-5")}>
                        <Link<Route>
                            to={Route::ArticleDetail { id: article.id.clone() }}
                            classes={classes!(
                                "inline-flex",
                                "items-center",
                                "gap-2",
                                "rounded-full",
                                "border",
                                "border-[var(--border)]",
                                "bg-[var(--surface)]",
                                "px-4",
                                "py-2",
                                "text-sm",
                                "font-medium",
                                "text-[var(--muted)]",
                                "hover:border-[var(--primary)]",
                                "hover:text-[var(--primary)]"
                            )}
                        >
                            { t::BACK_TO_ARTICLE }
                        </Link<Route>>
                    </div>
                </div>
            </section>
        };
    };

    let redirect_url = interactive_page_url(&page_id);

    html! {
        <section class={classes!("mx-auto", "my-10", "max-w-3xl", "px-4")}>
            <div class={classes!(
                "rounded-[var(--radius)]",
                "border",
                "border-[var(--border)]",
                "bg-[var(--surface)]",
                "p-6",
                "shadow-[var(--shadow)]",
                "sm:p-5"
            )}>
                <p class={classes!("m-0", "text-xs", "font-semibold", "uppercase", "tracking-[0.18em]", "text-[var(--primary)]")}>
                    { t::BADGE }
                </p>
                <h1 class={classes!("mb-2", "mt-3", "text-[1.7rem]", "leading-tight", "sm:text-[1.35rem]")}>
                    { article.title.clone() }
                </h1>
                <p class={classes!("m-0", "text-[var(--muted)]")}>{ t::REDIRECT_NOTE }</p>
                <div class={classes!("mt-5", "flex", "flex-wrap", "items-center", "gap-2")}>
                    <a
                        href={redirect_url}
                        class={classes!(
                            "inline-flex",
                            "items-center",
                            "gap-2",
                            "rounded-full",
                            "border",
                            "border-[var(--primary)]/35",
                            "bg-[var(--primary)]/10",
                            "px-4",
                            "py-2",
                            "text-sm",
                            "font-medium",
                            "text-[var(--primary)]",
                            "hover:bg-[var(--primary)]",
                            "hover:text-white"
                        )}
                    >
                        { t::OPEN_INTERACTIVE }
                    </a>
                    <Link<Route>
                        to={Route::ArticleDetail { id: article.id.clone() }}
                        classes={classes!(
                            "inline-flex",
                            "items-center",
                            "gap-2",
                            "rounded-full",
                            "border",
                            "border-[var(--border)]",
                            "bg-[var(--surface)]",
                            "px-4",
                            "py-2",
                            "text-sm",
                            "font-medium",
                            "text-[var(--muted)]",
                            "hover:border-[var(--primary)]",
                            "hover:text-[var(--primary)]"
                        )}
                    >
                        { t::BACK_TO_ARTICLE }
                    </Link<Route>>
                </div>
            </div>
        </section>
    }
}
