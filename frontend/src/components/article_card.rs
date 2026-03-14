use yew::prelude::*;
use yew_router::prelude::{use_navigator, Link};

use crate::{
    components::image_with_loading::ImageWithLoading,
    models::{ArticleKind, ArticleListItem},
    router::Route,
    utils::image_url,
};

#[derive(Properties, PartialEq, Clone)]
pub struct ArticleCardProps {
    pub article: ArticleListItem,
    #[prop_or_default]
    pub on_before_navigate: Option<Callback<()>>,
}

#[function_component(ArticleCard)]
pub fn article_card(props: &ArticleCardProps) -> Html {
    let article = props.article.clone();
    let is_interactive = matches!(article.article_kind, ArticleKind::InteractiveRepost);
    let detail_route = Route::ArticleDetail {
        id: article.id.clone(),
    };

    let navigator = use_navigator();
    let on_before_navigate = props.on_before_navigate.clone();

    // Handle title click with before-navigate hook
    let handle_title_click = {
        let navigator = navigator.clone();
        let route = detail_route.clone();
        let on_before_navigate = on_before_navigate.clone();

        Callback::from(move |e: MouseEvent| {
            e.prevent_default();

            // Execute before-navigate callback if provided
            if let Some(callback) = on_before_navigate.as_ref() {
                callback.emit(());
            }

            // Navigate to article detail
            if let Some(nav) = navigator.as_ref() {
                nav.push(&route);
            }
        })
    };

    // Handle image click
    let handle_image_click = handle_title_click.clone();

    // Editorial Magazine Style Card - 编辑性杂志风格卡片
    html! {
        <article
          class={classes!(
            "editorial-card",
            is_interactive.then_some("editorial-card--interactive"),
            "bg-[var(--surface)]",
            "liquid-glass",
            "border",
            "border-[var(--border)]",
            "rounded-xl",
            "overflow-hidden",
            "flex",
            "flex-col",
            "h-full",
            "transition-all",
            "duration-300",
            "ease-out",
            "hover:shadow-[var(--shadow-8)]",
            "hover:border-[var(--primary)]",
            "hover:-translate-y-2",
            "group"
        )}>
            {
                if let Some(image) = article.featured_image.as_ref() {
                    let image_url_val = image_url(image);
                    let title = article.title.clone();
                    html! {
                        <a
                            href={format!("/article/{}", article.id)}
                            class={classes!(
                                "block",
                                "aspect-video",
                                "overflow-hidden",
                                "relative"
                            )}
                            onclick={handle_image_click}
                        >
                            <ImageWithLoading
                                src={image_url_val}
                                alt={title}
                                loading={Some(AttrValue::from("lazy"))}
                                decoding={Some(AttrValue::from("async"))}
                                class={classes!(
                                    "w-full",
                                    "h-full",
                                    "object-cover",
                                    "transition-transform",
                                    "duration-500",
                                    "ease-out",
                                    "group-hover:scale-105"
                                )}
                                container_class={classes!("w-full", "h-full")}
                            />
                            {
                                if is_interactive {
                                    html! {
                                        <span class="interactive-card-seal" aria-hidden="true">
                                            <span class="interactive-card-seal__icon">
                                                <span class="interactive-card-seal__core"></span>
                                            </span>
                                            <span class="interactive-card-seal__label">{ "Interactive" }</span>
                                        </span>
                                    }
                                } else {
                                    html! {}
                                }
                            }
                        </a>
                    }
                } else {
                    html! {}
                }
            }

            <div class={classes!("p-6", "flex", "flex-col", "gap-3", "flex-1")}>
                <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                    // Date badge - 日期徽章
                    <time class={classes!(
                        "text-xs",
                        "tracking-[0.2em]",
                        "uppercase",
                        "text-[var(--muted)]",
                        "font-semibold",
                        "pt-1"
                    )}>
                        { &article.date }
                    </time>

                    {
                        if is_interactive {
                            html! {
                                <div
                                    class="interactive-card-badge"
                                    role="note"
                                    aria-label="交互式文章，包含可运行演示"
                                    title="交互式文章：进入后可打开独立交互页"
                                >
                                    <span class="interactive-card-badge__icon" aria-hidden="true">
                                        <span class="interactive-card-badge__core"></span>
                                    </span>
                                    <span class="interactive-card-badge__copy">
                                        <span class="interactive-card-badge__eyebrow">{ "Interactive" }</span>
                                        <span class="interactive-card-badge__label">{ "含可运行演示" }</span>
                                    </span>
                                </div>
                            }
                        } else {
                            html! {}
                        }
                    }
                </div>

                // Title with Fraunces font - 使用 Fraunces 字体的标题
                <h3 class={classes!("m-0", "leading-tight")}>
                    <a
                        href={format!("/article/{}", article.id)}
                        class={classes!(
                            "text-xl",
                            "md:text-2xl",
                            "font-bold",
                            "text-[var(--text)]",
                            "transition-colors",
                            "duration-200",
                            "hover:text-[var(--primary)]",
                            "line-clamp-2"
                        )}
                        style="font-family: 'Fraunces', serif;"
                        onclick={handle_title_click}
                    >
                        { &article.title }
                    </a>
                </h3>

                // Metadata row - 元数据行
                <div class={classes!(
                    "flex",
                    "flex-wrap",
                    "items-center",
                    "gap-3",
                    "text-sm",
                    "text-[var(--muted)]",
                    "pb-3",
                    is_interactive.then_some("interactive-card-meta"),
                    "border-b",
                    "border-[var(--border)]"
                )}>
                    <span class={classes!("inline-flex", "items-center", "gap-1.5")}>
                        <i class="fas fa-user-circle" aria-hidden="true"></i>
                        { &article.author }
                    </span>
                    <span class={classes!("text-[var(--border)]")}>{ "•" }</span>
                    <Link<Route>
                        to={Route::CategoryDetail { category: article.category.clone() }}
                        classes={classes!(
                            "inline-flex",
                            "items-center",
                            "gap-1.5",
                            "text-[var(--muted)]",
                            "transition-colors",
                            "duration-200",
                            "hover:text-[var(--primary)]"
                        )}
                    >
                        <i class="far fa-folder" aria-hidden="true"></i>
                        { &article.category }
                    </Link<Route>>
                </div>

                {
                    if is_interactive {
                        html! {
                            <div class="interactive-card-note">
                                <span class="interactive-card-note__pulse" aria-hidden="true"></span>
                                <span>{ "文章内可直接打开交互镜像" }</span>
                            </div>
                        }
                    } else {
                        html! {}
                    }
                }

                // Summary - 摘要
                <p class={classes!(
                    "m-0",
                    "text-base",
                    "leading-relaxed",
                    "text-[var(--muted)]",
                    "line-clamp-3",
                    "flex-1"
                )}>
                    { &article.summary }
                </p>

                // Tags - 标签
                <div class={classes!("mt-auto", "pt-2")}>
                    <ul class={classes!("m-0", "flex", "list-none", "flex-wrap", "gap-2", "p-0")}>
                        { for article.tags.iter().take(3).map(|tag| {
                            let tag_route = Route::TagDetail { tag: tag.clone() };
                            let tag_label = format!("#{}", tag);
                            html! {
                                <li class={classes!("m-0")}>
                                    <Link<Route>
                                        to={tag_route}
                                        classes={classes!(
                                            "inline-flex",
                                            "items-center",
                                            "px-3",
                                            "py-1",
                                            "border",
                                            "border-[var(--border)]",
                                            "rounded-full",
                                            "text-xs",
                                            "text-[var(--muted)]",
                                            "bg-[var(--surface-alt)]",
                                            "transition-all",
                                            "duration-200",
                                            "hover:border-[var(--primary)]",
                                            "hover:text-[var(--primary)]"
                                        )}
                                    >
                                        { tag_label }
                                    </Link<Route>>
                                </li>
                            }
                        }) }
                    </ul>
                </div>
            </div>
        </article>
    }
}
