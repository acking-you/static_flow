use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{models::ArticleListItem, router::Route, utils::image_url};

#[derive(Properties, PartialEq, Clone)]
pub struct ArticleCardProps {
    pub article: ArticleListItem,
}

#[function_component(ArticleCard)]
pub fn article_card(props: &ArticleCardProps) -> Html {
    let article = props.article.clone();
    let detail_route = Route::ArticleDetail {
        id: article.id.clone(),
    };

    // 组件类 + 内部工具类混合模式：复杂轮廓仍使用 article-card，内部简单元素改用 Tailwind utilities
    html! {
        <article class="article-card">
            {
                if let Some(image) = article.featured_image.as_ref() {
                    html! {
                        <Link<Route> to={detail_route.clone()} classes={classes!("featured-image")}>
                            <img src={image_url(&image)} alt={article.title.clone()} loading="lazy" />
                        </Link<Route>>
                    }
                } else {
                    html! {}
                }
            }
            <h3 class="text-xl font-semibold leading-snug text-[var(--text)]">
                <Link<Route>
                    to={detail_route.clone()}
                    classes={classes!(
                        "text-[var(--text)]",
                        "transition-colors",
                        "duration-200",
                        "hover:text-primary"
                    )}
                >
                    { &article.title }
                </Link<Route>>
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
                                    classes={classes!(
                                        "inline-flex",
                                        "items-center",
                                        "gap-1.5",
                                        "rounded-full",
                                        "border",
                                        "border-border",
                                        "px-3",
                                        "py-1",
                                        "text-sm",
                                        "text-muted",
                                        "transition-colors",
                                        "duration-200",
                                        "hover:border-primary",
                                        "hover:bg-primary",
                                        "hover:text-white",
                                        "cursor-pointer"
                                    )}
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
