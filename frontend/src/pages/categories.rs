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

    let total_categories = categories.len();
    let total_articles: usize = categories.iter().map(|c| c.count).sum();

    html! {
        <main class={classes!(
            "mt-[var(--header-height-mobile)]",
            "md:mt-[var(--header-height-desktop)]",
            "pb-20"
        )}>
            <div class={classes!("container")}>
                // Hero Section with Geometric Art Deco Style
                <div class={classes!(
                    "text-center",
                    "py-16",
                    "md:py-24",
                    "px-4",
                    "relative",
                    "overflow-hidden"
                )}>
                    <p class={classes!(
                        "text-sm",
                        "tracking-[0.4em]",
                        "uppercase",
                        "text-[var(--muted)]",
                        "mb-6",
                        "font-semibold"
                    )}>{ "Category Index" }</p>

                    <h1 class={classes!(
                        "category-title",
                        "text-5xl",
                        "md:text-7xl",
                        "font-bold",
                        "mb-6",
                        "leading-tight"
                    )}
                    style="font-family: 'Fraunces', serif;">
                        { "知识图谱" }
                    </h1>

                    <p class={classes!(
                        "text-lg",
                        "md:text-xl",
                        "text-[var(--muted)]",
                        "max-w-2xl",
                        "mx-auto",
                        "leading-relaxed",
                        "mb-8"
                    )}>
                        { format!("探索 {} 个领域，汇聚 {} 篇文章", total_categories, total_articles) }
                    </p>

                    // Geometric decorative brackets - 几何装饰括号
                    <div class={classes!(
                        "category-brackets",
                        "flex",
                        "items-center",
                        "justify-center",
                        "gap-6",
                        "mt-8"
                    )}>
                        <div class={classes!(
                            "w-12",
                            "h-1",
                            "bg-gradient-to-r",
                            "from-[var(--primary)]",
                            "via-sky-500/30",
                            "to-transparent"
                        )}></div>
                        <div class={classes!(
                            "category-badge",
                            "inline-flex",
                            "items-center",
                            "gap-2",
                            "px-6",
                            "py-3",
                            "bg-gradient-to-r",
                            "from-[var(--primary)]/10",
                            "to-sky-500/10",
                            "border-2",
                            "border-[var(--primary)]/30",
                            "rounded-lg",
                            "text-sm",
                            "font-bold",
                            "text-[var(--primary)]",
                            "dark:text-sky-400"
                        )}>
                            <i class={classes!("fas", "fa-th-large")}></i>
                            <span>{ format!("{} CATEGORIES", total_categories) }</span>
                        </div>
                        <div class={classes!(
                            "w-12",
                            "h-1",
                            "bg-gradient-to-l",
                            "from-[var(--primary)]",
                            "via-sky-500/30",
                            "to-transparent"
                        )}></div>
                    </div>
                </div>

                // Category Grid with Geometric Style
                {
                    if *loading {
                        html! {
                            <div class={classes!(
                                "flex",
                                "items-center",
                                "justify-center",
                                "min-h-[400px]"
                            )}>
                                <LoadingSpinner size={SpinnerSize::Large} />
                            </div>
                        }
                    } else if categories.is_empty() {
                        html! {
                            <div class={classes!(
                                "empty-state",
                                "text-center",
                                "py-20",
                                "px-4",
                                "bg-[var(--surface)]",
                                "liquid-glass",
                                "rounded-2xl",
                                "border",
                                "border-[var(--border)]"
                            )}>
                                <i class={classes!(
                                    "fas",
                                    "fa-th-large",
                                    "text-6xl",
                                    "text-[var(--muted)]",
                                    "mb-6"
                                )}></i>
                                <p class={classes!("text-xl", "text-[var(--muted)]")}>
                                    { "暂无分类" }
                                </p>
                            </div>
                        }
                    } else {
                        html! {
                            <div class={classes!(
                                "category-grid",
                                "grid",
                                "grid-cols-1",
                                "md:grid-cols-2",
                                "lg:grid-cols-3",
                                "gap-6",
                                "mt-12"
                            )}>
                                { for categories.iter().map(|category| {
                                    html! {
                                        <Link<Route>
                                            to={Route::CategoryDetail { category: category.name.clone() }}
                                            classes={classes!(
                                                "category-card",
                                                "block",
                                                "group",
                                                "relative",
                                                "bg-[var(--surface)]",
                                                "border",
                                                "border-[var(--border)]",
                                                "border-l-[4px]",
                                                "border-l-[var(--primary)]",
                                                "rounded-lg",
                                                "p-6",
                                                "transition-all",
                                                "duration-300",
                                                "hover:border-[var(--primary)]",
                                                "hover:shadow-[0_4px_12px_rgba(var(--primary-rgb),0.4)]",
                                                "hover:-translate-y-1",
                                                "liquid-glass"
                                            )}
                                        >
                                            <div class={classes!("flex", "flex-col", "gap-3", "h-full")}>
                                                // Category name with geometric styling
                                                <h3 class={classes!(
                                                    "m-0",
                                                    "text-2xl",
                                                    "font-bold",
                                                    "text-[var(--text)]",
                                                    "transition-colors",
                                                    "duration-200",
                                                    "group-hover:text-[var(--primary)]",
                                                    "dark:group-hover:text-sky-400"
                                                )}
                                                style="font-family: 'Fraunces', serif;">
                                                    { &category.name }
                                                </h3>

                                                // Description
                                                <p class={classes!(
                                                    "m-0",
                                                    "text-base",
                                                    "leading-relaxed",
                                                    "text-[var(--muted)]",
                                                    "flex-1"
                                                )}>
                                                    { &category.description }
                                                </p>

                                                // Article count badge with geometric design
                                                <div class={classes!(
                                                    "flex",
                                                    "items-center",
                                                    "justify-between",
                                                    "mt-auto",
                                                    "pt-4",
                                                    "border-t",
                                                    "border-[var(--border)]"
                                                )}>
                                                    <span class={classes!(
                                                        "inline-flex",
                                                        "items-center",
                                                        "gap-2",
                                                        "px-3",
                                                        "py-1.5",
                                                        "bg-gradient-to-r",
                                                        "from-[var(--primary)]/10",
                                                        "to-sky-500/10",
                                                        "border",
                                                        "border-[var(--primary)]/30",
                                                        "rounded-[6px]",
                                                        "text-sm",
                                                        "font-bold",
                                                        "text-[var(--primary)]",
                                                        "dark:text-sky-400",
                                                        "transition-all",
                                                        "duration-200",
                                                        "group-hover:border-[var(--primary)]/50",
                                                        "group-hover:bg-gradient-to-r",
                                                        "group-hover:from-[var(--primary)]/20",
                                                        "group-hover:to-sky-500/20"
                                                    )}>
                                                        <i class={classes!("far", "fa-file-alt")}></i>
                                                        <span>{ format!("{} 篇", category.count) }</span>
                                                    </span>

                                                    // Arrow icon
                                                    <i class={classes!(
                                                        "fas",
                                                        "fa-arrow-right",
                                                        "text-[var(--muted)]",
                                                        "transition-all",
                                                        "duration-200",
                                                        "group-hover:text-[var(--primary)]",
                                                        "dark:group-hover:text-sky-400",
                                                        "group-hover:translate-x-2"
                                                    )}></i>
                                                </div>
                                            </div>
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
