use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{
    components::{
        loading_spinner::{LoadingSpinner, SpinnerSize},
        scroll_to_top_button::ScrollToTopButton,
    },
    router::Route,
};

#[function_component(TagsPage)]
pub fn tags_page() -> Html {
    let tag_stats = use_state(|| Vec::<crate::api::TagInfo>::new());
    let loading = use_state(|| true);

    {
        let tag_stats = tag_stats.clone();
        let loading = loading.clone();
        use_effect_with((), move |_| {
            loading.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                match crate::api::fetch_tags().await {
                    Ok(data) => {
                        tag_stats.set(data);
                        loading.set(false);
                    },
                    Err(e) => {
                        web_sys::console::error_1(&format!("Failed to fetch tags: {}", e).into());
                        loading.set(false);
                    },
                }
            });
            || ()
        });
    }

    let total_tags = tag_stats.len();
    let total_articles: usize = tag_stats.iter().map(|t| t.count).sum();
    let max_count = tag_stats.iter().map(|t| t.count as f32).fold(1.0, f32::max);

    html! {
        <main class={classes!("main", "mt-[var(--space-lg)]", "py-12", "pb-16") }>
            <div class={classes!("container")}>
                <section class={classes!(
                    "page-section",
                    "flex",
                    "flex-col",
                    "items-center",
                    "text-center",
                    "gap-2"
                )}>
                    <p class={classes!("page-kicker")}>{ "标签" }</p>
                    <h1 class={classes!("page-title")}>{ "标签索引" }</h1>
                    <p class={classes!(
                        "page-description",
                        "max-w-3xl",
                        "mx-auto",
                        "text-center"
                    )}>
                        { format!("汇总 {} 个标签，覆盖 {} 篇文章。点击任意标签将跳转到对应的标签详情页并展示时间线。", total_tags, total_articles) }
                    </p>
                </section>

                {
                    if *loading {
                        html! {
                            <div class={classes!("flex", "min-h-[40vh]", "items-center", "justify-center")}>
                                <LoadingSpinner size={SpinnerSize::Large} />
                            </div>
                        }
                    } else if tag_stats.is_empty() {
                        html! { <p class={classes!("empty-hint")}>{ "暂无标签，敬请期待。" }</p> }
                    } else {
                        html! {
                            <div
                                class={classes!("flex", "flex-wrap", "justify-center", "gap-3", "p-4")}
                                role="list"
                                aria-label="标签云"
                            >
                                { for tag_stats.iter().map(|tag_info| {
                                    let weight = (tag_info.count as f32 / max_count).max(0.35);
                                    let style = format!("--tag-weight: {:.2}", weight);
                                    html! {
                                        <Link<Route>
                                            to={Route::TagDetail { tag: tag_info.name.clone() }}
                                            classes={classes!(
                                                "inline-flex",
                                                "items-center",
                                                "gap-2",
                                                "px-5",
                                                "py-3",
                                                "border",
                                                "border-[var(--border)]",
                                                "rounded-full",
                                                "bg-[var(--surface)]",
                                                "text-[var(--text)]",
                                                "font-medium",
                                                "transition-all",
                                                "duration-[280ms]",
                                                "ease-[cubic-bezier(0.34,1.56,0.64,1)]",
                                                "hover:-translate-y-0.5",
                                                "hover:border-[var(--primary)]",
                                                "hover:shadow-[var(--shadow)]"
                                            )}
                                        >
                                            <span
                                                class={classes!("text-[calc(1rem+var(--tag-weight,0.4)*0.35rem)]")}
                                                style={style}
                                            >
                                                { &tag_info.name }
                                            </span>
                                            <span class={classes!("text-sm", "text-[var(--muted)]")}>
                                                { format!("{} 篇", tag_info.count) }
                                            </span>
                                        </Link<Route>>
                                    }
                                }) }
                            </div>
                        }
                    }
                }
            </div>
            <ScrollToTopButton />
        </main>
    }
}
