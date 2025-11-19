use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{
    components::{
        loading_spinner::{LoadingSpinner, SpinnerSize},
        scroll_to_top_button::ScrollToTopButton,
    },
    router::Route,
};

#[function_component(CategoriesPage)]
pub fn categories_page() -> Html {
    let categories = use_state(|| Vec::<crate::api::CategoryInfo>::new());
    let loading = use_state(|| true);

    {
        let categories = categories.clone();
        let loading = loading.clone();
        use_effect_with((), move |_| {
            loading.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                match crate::api::fetch_categories().await {
                    Ok(data) => {
                        categories.set(data);
                        loading.set(false);
                    },
                    Err(e) => {
                        web_sys::console::error_1(
                            &format!("Failed to fetch categories: {}", e).into(),
                        );
                        loading.set(false);
                    },
                }
            });
            || ()
        });
    }

    html! {
        <main class={classes!("main", "mt-[var(--space-lg)]", "py-12", "pb-16")}>
            <div class={classes!("container")}>
                <section class={classes!(
                    "page-section",
                    "flex",
                    "flex-col",
                    "items-center",
                    "text-center",
                    "gap-2"
                )}>
                    <p class={classes!("page-kicker")}>{ "分类" }</p>
                    <h1 class={classes!("page-title")}>{ "知识图谱" }</h1>
                    <p class={classes!(
                        "page-description",
                        "max-w-3xl",
                        "mx-auto",
                        "text-center"
                    )}>
                        { format!("当前整理 {} 个分类，持续更新中。点击卡片跳转到分类详情页并查看文章时间线。", categories.len()) }
                    </p>
                </section>

                {
                    if *loading {
                        html! {
                            <div class={classes!("flex", "min-h-[40vh]", "items-center", "justify-center")}>
                                <LoadingSpinner size={SpinnerSize::Large} />
                            </div>
                        }
                    } else if categories.is_empty() {
                        html! {
                            <p class={classes!("empty-hint")}>{ "暂无分类" }</p>
                        }
                    } else {
                        html! {
                            <section
                                class={classes!(
                                    "grid",
                                    "grid-cols-[repeat(auto-fit,minmax(220px,1fr))]",
                                    "gap-5",
                                    "mt-6"
                                )}
                                aria-label="分类列表"
                            >
                                { for categories.iter().map(|category| {
                                    html! {
                                        <Link<Route>
                                            to={Route::CategoryDetail { category: category.name.clone() }}
                                            classes={classes!(
                                                "flex",
                                                "flex-col",
                                                "justify-between",
                                                "border",
                                                "border-[var(--border)]",
                                                "rounded-[var(--radius)]",
                                                "p-5",
                                                "bg-[var(--surface)]",
                                                "text-[var(--text)]",
                                                "transition-all",
                                                "duration-[280ms]",
                                                "ease-[cubic-bezier(0.34,1.56,0.64,1)]",
                                                "hover:border-[var(--primary)]",
                                                "hover:shadow-[var(--shadow)]",
                                                "hover:-translate-y-0.5"
                                            )}
                                        >
                                            <div class={classes!("flex-1")}>
                                                <p class={classes!("m-0", "mb-2", "text-xl", "font-semibold", "text-[var(--text)]")}>
                                                    { &category.name }
                                                </p>
                                                <p class={classes!("m-0", "text-[0.95rem]", "leading-relaxed", "text-[var(--muted)]")}>
                                                    { &category.description }
                                                </p>
                                            </div>
                                            <span class={classes!("mt-4", "font-semibold", "text-[var(--muted)]")}>
                                                { format!("{} 篇", category.count) }
                                            </span>
                                        </Link<Route>>
                                    }
                                }) }
                            </section>
                        }
                    }
                }
            </div>
            <ScrollToTopButton />
        </main>
    }
}
