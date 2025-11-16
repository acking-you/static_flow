use yew::prelude::*;
use yew_router::prelude::{use_navigator, Link};

use crate::{models::ArticleListItem, router::Route, utils::image_url};

#[derive(Properties, PartialEq, Clone)]
pub struct ArticleCardProps {
    pub article: ArticleListItem,
    #[prop_or_default]
    pub on_before_navigate: Option<Callback<()>>,
}

#[function_component(ArticleCard)]
pub fn article_card(props: &ArticleCardProps) -> Html {
    let article = props.article.clone();
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

    // 组件类 + 内部工具类混合模式：复杂轮廓仍使用 article-card，内部简单元素改用
    // Tailwind utilities
    html! {
        <article class="article-card">
            {
                if let Some(image) = article.featured_image.as_ref() {
                    let image_url_val = image_url(&image);
                    let title = article.title.clone();
                    html! {
                        <a
                            href={format!("/article/{}", article.id)}
                            class="featured-image"
                            onclick={handle_image_click}
                        >
                            <img
                                src={image_url_val}
                                alt={title}
                                loading="lazy"
                                decoding="async"
                            />
                        </a>
                    }
                } else {
                    html! {}
                }
            }
            <h3 class="text-xl font-semibold leading-snug text-[var(--text)]">
                <a
                    href={format!("/article/{}", article.id)}
                    class="text-[var(--text)] transition-colors duration-200 hover:text-primary"
                    onclick={handle_title_click}
                >
                    { &article.title }
                </a>
            </h3>
            <div class="mb-1 flex flex-wrap items-center gap-3 text-sm text-muted">
                <span class="inline-flex items-center gap-1.5">
                    <i class="fas fa-user-circle" aria-hidden="true"></i>
                    { &article.author }
                </span>
                <span class="inline-flex items-center gap-1.5">
                    <i class="far fa-calendar-alt" aria-hidden="true"></i>
                    { &article.date }
                </span>
                <Link<Route>
                    to={Route::CategoryDetail { category: article.category.clone() }}
                    classes={classes!(
                        "inline-flex",
                        "items-center",
                        "gap-1.5",
                        "text-muted",
                        "transition-colors",
                        "duration-200",
                        "hover:text-primary"
                    )}
                >
                    <i class="far fa-folder" aria-hidden="true"></i>
                    { &article.category }
                </Link<Route>>
            </div>
            <p class={classes!("article-excerpt", "text-base", "leading-relaxed", "text-muted")}>
                { &article.summary }
            </p>
            <div class="mt-auto pt-4">
                <ul class="m-0 flex list-none flex-wrap gap-2 p-0">
                    { for article.tags.iter().map(|tag| {
                        let tag_route = Route::TagDetail { tag: tag.clone() };
                        let tag_label = format!("#{}", tag);
                        html! {
                            <li class="m-0">
                                <Link<Route>
                                    to={tag_route}
                                    classes="tag-pill"
                                >
                                    { tag_label }
                                </Link<Route>>
                            </li>
                        }
                    }) }
                </ul>
            </div>
        </article>
    }
}
