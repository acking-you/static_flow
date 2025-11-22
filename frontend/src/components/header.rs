use web_sys::HtmlInputElement;
use yew::{events::InputEvent, prelude::*};
use yew_router::prelude::*;

use crate::{
    components::{
        icons::IconName,
        theme_toggle::ThemeToggle,
        tooltip::{TooltipIconButton, TooltipPosition},
    },
    router::Route,
};

#[function_component(Header)]
pub fn header() -> Html {
    let mobile_menu_open = use_state(|| false);
    let search_query = use_state(String::new);

    let toggle_mobile_menu = {
        let mobile_menu_open = mobile_menu_open.clone();
        Callback::from(move |_| mobile_menu_open.set(!*mobile_menu_open))
    };

    let close_mobile_menu = {
        let mobile_menu_open = mobile_menu_open.clone();
        Callback::from(move |_| mobile_menu_open.set(false))
    };

    let on_search_input = {
        let search_query = search_query.clone();
        Callback::from(move |event: InputEvent| {
            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                search_query.set(target.value());
            }
        })
    };

    let clear_search = {
        let search_query = search_query.clone();
        Callback::from(move |_| search_query.set(String::new()))
    };

    // 执行搜索
    let do_search = {
        let search_query = search_query.clone();
        Callback::from(move |_: MouseEvent| {
            let query = (*search_query).trim();
            if !query.is_empty() {
                let encoded_query = urlencoding::encode(query);
                let search_url = crate::config::route_path(&format!("/search?q={}", encoded_query));
                if let Some(window) = web_sys::window() {
                    if let Ok(history) = window.history() {
                        let _ = history.push_state_with_url(
                            &wasm_bindgen::JsValue::NULL,
                            "",
                            Some(&search_url),
                        );
                        if let Ok(event) = web_sys::Event::new("popstate") {
                            let _ = window.dispatch_event(&event);
                        }
                    }
                }
            }
        })
    };

    // Enter键搜索
    let on_search_keypress = {
        let search_query = search_query.clone();
        Callback::from(move |e: KeyboardEvent| {
            if e.key() == "Enter" {
                let query = (*search_query).trim();
                if !query.is_empty() {
                    let encoded_query = urlencoding::encode(query);
                    let search_url =
                        crate::config::route_path(&format!("/search?q={}", encoded_query));
                    if let Some(window) = web_sys::window() {
                        if let Ok(history) = window.history() {
                            let _ = history.push_state_with_url(
                                &wasm_bindgen::JsValue::NULL,
                                "",
                                Some(&search_url),
                            );
                            if let Ok(event) = web_sys::Event::new("popstate") {
                                let _ = window.dispatch_event(&event);
                            }
                        }
                    }
                }
            }
        })
    };

    let mobile_menu_classes = classes!(
        "fixed",
        "inset-0",
        "z-[120]",
        "transition-opacity",
        "duration-300",
        "ease-[var(--ease-spring)]",
        if *mobile_menu_open {
            "opacity-100 pointer-events-auto"
        } else {
            "opacity-0 pointer-events-none"
        }
    );

    let mobile_panel_classes = classes!(
        "absolute",
        "inset-0",
                                "bg-[var(--acrylic-bg-light)]",
                                "[.dark_&]:bg-[var(--acrylic-bg-dark)]",
        "text-[var(--text)]",
        "p-[4.5rem_1.5rem_2rem]",
        "flex",
        "flex-col",
        "gap-5",
        "overflow-y-auto",
        "[backdrop-filter:blur(50px)_saturate(var(--acrylic-saturate))]",
        "shadow-[var(--shadow-16)]",
        "rounded-tr-lg", "rounded-br-lg",
        "transition-all",
        "duration-[350ms]",
        "ease-[var(--ease-spring)]",
        if *mobile_menu_open { "translate-y-0 opacity-100" } else { "-translate-y-4 opacity-0" }
    );

    let hamburger_classes = classes!(
        "w-12",
        "h-12",
        "min-w-[3rem]",
        "min-h-[3rem]",
        "border",
        "border-[var(--border)]",
        "rounded-lg",
        "bg-[var(--surface)]",
        "text-[var(--text)]",
        "flex",
        "flex-col",
        "justify-center",
        "items-center",
        "gap-[0.35rem]",
        "cursor-pointer",
        "transition-colors",
        "duration-100",
        "ease-[var(--ease-snap)]",
        "hover:bg-[var(--surface-alt)]"
    );

    let hamburger_line = classes!(
        "block",
        "w-[1.4rem]",
        "h-[2px]",
        "rounded-[1px]",
        "bg-[var(--text)]",
        "transition-all",
        "duration-200",
        "ease-in-out"
    );

    // Icon-based navigation
    let nav_items = [
        ("最新", Route::LatestArticles, "fa-clock"),
        ("文章", Route::Posts, "fa-file-lines"),
        ("标签", Route::Tags, "fa-tag"),
        ("分类", Route::Categories, "fa-folder-open"),
    ];

    let mobile_search_input = on_search_input.clone();
    let mobile_search_keypress = on_search_keypress.clone();
    let mobile_do_search = do_search.clone();
    let mobile_clear_search = clear_search.clone();

    html! {
        <>
            // Header container - sticky at top
            <header class={classes!(
                "header-minimal",
                "sticky", "top-0", "left-0", "right-0", "z-[80]", "w-full",
                "shadow-[0_1px_0_rgba(var(--primary-rgb),0.08)]",
                "transition-all", "duration-200", "ease-[var(--ease-snap)]"
            )}>
                // Desktop header
                <div class={classes!(
                    "desktop-header",
                    "items-center", "gap-4",
                    "min-h-[var(--header-height-mobile)]", "md:min-h-[var(--header-height-desktop)]",
                    "max-w-7xl", "mx-auto", "px-4", "sm:px-6", "lg:px-8"
                )}>
                    // Brand
                    <div>
                        <Link<Route> to={Route::Home} classes="brand-logo">
                            <span class="brand-logo-shine"></span>
                            {"L_B__"}
                            <span class="brand-logo-cursor"></span>
                        </Link<Route>>
                    </div>

                    // Actions - right-aligned
                    <div class={classes!("ml-auto", "flex", "items-center", "gap-2")}>
                        // Icon navigation
                        <nav class={classes!("flex", "items-center", "gap-1")} aria-label="主导航">
                            { for nav_items.iter().map(|(label, route, icon)| {
                                html! {
                                    <Link<Route> to={route.clone()} classes={classes!(
                                        "nav-icon-btn",
                                        "w-10", "h-10",
                                        "rounded-lg",
                                        "inline-flex", "items-center", "justify-center",
                                        "text-[var(--muted)]",
                                        "transition-all", "duration-200",
                                        "hover:text-[var(--primary)]",
                                        "hover:bg-[var(--surface-alt)]",
                                        "hover:scale-110"
                                    )}>
                                        <i class={classes!("fas", *icon, "text-[1.1rem]")} title={*label}></i>
                                    </Link<Route>>
                                }
                            }) }
                        </nav>

                        // Search
                        <div class={classes!("flex", "items-center", "gap-1")}>
                            <input
                                type="text"
                                placeholder="搜索..."
                                value={(*search_query).clone()}
                                oninput={on_search_input.clone()}
                                onkeypress={on_search_keypress.clone()}
                                class={classes!(
                                    "search-minimal",
                                    "w-[180px]", "lg:w-[220px]",
                                    "border", "border-[var(--border)]", "rounded-lg",
                                    "px-3", "h-10",
                                    "bg-[var(--surface)]", "text-[var(--text)]",
                                    "text-sm",
                                    "transition-all", "duration-200",
                                    "focus:outline-none",
                                    "focus:border-[var(--primary)]",
                                    "focus:w-[220px]", "lg:focus:w-[280px]"
                                )}
                            />
                            <button
                                type="button"
                                onclick={do_search.clone()}
                                class={classes!(
                                    "icon-btn",
                                    "w-10", "h-10",
                                    "rounded-lg",
                                    "inline-flex", "items-center", "justify-center",
                                    "text-[var(--muted)]",
                                    "transition-all", "duration-200",
                                    "hover:text-[var(--primary)]",
                                    "hover:bg-[var(--surface-alt)]"
                                )}
                                aria-label="搜索"
                            >
                                <i class="fas fa-search"></i>
                            </button>
                            <button
                                type="button"
                                onclick={clear_search.clone()}
                                disabled={search_query.is_empty()}
                                class={classes!(
                                    "icon-btn",
                                    "w-10", "h-10",
                                    "rounded-lg",
                                    "inline-flex", "items-center", "justify-center",
                                    "text-[var(--muted)]",
                                    "transition-all", "duration-200",
                                    "hover:text-[var(--primary)]",
                                    "hover:bg-[var(--surface-alt)]",
                                    "disabled:opacity-30", "disabled:pointer-events-none"
                                )}
                                aria-label="清空"
                            >
                                <i class="fas fa-times text-sm"></i>
                            </button>
                        </div>

                        // Theme toggle
                        <div>
                            <ThemeToggle />
                        </div>
                    </div>
                </div>

                // Mobile header
                <div class={classes!(
                    "mobile-header",
                    "items-center",
                    "justify-between",
                    "gap-3",
                    "min-h-[var(--header-height-mobile)]",
                    "max-w-7xl",
                    "mx-auto",
                    "px-4",
                    "sm:px-6",
                    "lg:px-8"
                )}>
                    // Brand
                    <div>
                        <Link<Route> to={Route::Home} classes="brand-logo">
                            <span class="brand-logo-shine"></span>
                            {"L_B__"}
                            <span class="brand-logo-cursor"></span>
                        </Link<Route>>
                    </div>

                    // Hamburger
                    <button
                        type="button"
                        class={hamburger_classes}
                        aria-label="打开菜单"
                        aria-expanded={(*mobile_menu_open).to_string()}
                        onclick={toggle_mobile_menu.clone()}
                    >
                        <span
                            class={classes!(
                                hamburger_line.clone(),
                                if *mobile_menu_open { "translate-y-[6px] rotate-45" } else { "" }
                            )}
                        />
                        <span
                            class={classes!(
                                hamburger_line.clone(),
                                if *mobile_menu_open { "opacity-0" } else { "opacity-100" }
                            )}
                        />
                        <span
                            class={classes!(
                                hamburger_line,
                                if *mobile_menu_open { "-translate-y-[6px] -rotate-45" } else { "" }
                            )}
                        />
                    </button>
                </div>
            </header>

            // Mobile menu overlay
            <div class={mobile_menu_classes}>
                // Backdrop
                <div
                    class={classes!(
                        "absolute",
                        "inset-0",
                        "bg-[rgba(15,23,42,0.45)]",
                        "backdrop-blur-[12px]",
                        "transition-opacity",
                        "duration-300",
                        if *mobile_menu_open { "opacity-100" } else { "opacity-0" }
                    )}
                    onclick={close_mobile_menu.clone()}
                />

                // Menu panel
                <div
                    class={mobile_panel_classes}
                    role="dialog"
                    aria-modal="true"
                >
                    // Close button
                    <div class={classes!("absolute", "right-5", "top-5", "z-10")}>
                        <TooltipIconButton
                            icon={IconName::ArrowLeft}
                            tooltip="关闭"
                            position={TooltipPosition::Bottom}
                            onclick={close_mobile_menu.clone()}
                            size={20}
                            class={classes!(
                                "!bg-[var(--surface)]",
                                "!border",
                                "!border-[var(--border)]",
                                "!rounded-lg",
                                "!shadow-[var(--shadow-2)]"
                            )}
                        />
                    </div>

                    // Mobile search
                    <div class={classes!("flex", "gap-2", "items-center", "mb-3")}>
                        <input
                            type="text"
                            placeholder="搜索..."
                            value={(*search_query).clone()}
                            oninput={mobile_search_input.clone()}
                            onkeypress={mobile_search_keypress.clone()}
                            class={classes!(
                                "flex-1",
                                "border", "border-[var(--border)]", "rounded-lg",
                                "px-4", "h-12",
                                "bg-[var(--surface)]",
                                "text-[var(--text)]",
                                "focus:outline-none",
                                "focus:border-[var(--primary)]"
                            )}
                        />
                        <button
                            type="button"
                            onclick={mobile_do_search.clone()}
                            class={classes!(
                                "w-12", "h-12",
                                "rounded-lg",
                                "border", "border-[var(--border)]",
                                "bg-[var(--surface)]",
                                "text-[var(--muted)]",
                                "hover:text-[var(--primary)]"
                            )}
                        >
                            <i class="fas fa-search"></i>
                        </button>
                        <button
                            type="button"
                            onclick={mobile_clear_search.clone()}
                            disabled={search_query.is_empty()}
                            class={classes!(
                                "w-12", "h-12",
                                "rounded-lg",
                                "border", "border-[var(--border)]",
                                "bg-[var(--surface)]",
                                "text-[var(--muted)]",
                                "hover:text-[var(--primary)]",
                                "disabled:opacity-30"
                            )}
                        >
                            <i class="fas fa-times"></i>
                        </button>
                    </div>

                    // Navigation
                    <nav class={classes!("flex", "flex-col", "gap-3")} aria-label="移动端导航">
                        { for nav_items.iter().map(|(label, route, icon)| {
                            let close_cb = close_mobile_menu.clone();
                            html! {
                                <div onclick={close_cb}>
                                    <Link<Route> to={route.clone()} classes={classes!(
                                        "mobile-nav-item",
                                        "flex", "items-center", "gap-3",
                                        "py-3", "px-4", "rounded-lg",
                                        "bg-[var(--surface)]",
                                        "border", "border-[var(--border)]",
                                        "text-[var(--text)]",
                                        "hover:border-[var(--primary)]"
                                    )}>
                                        <i class={classes!("fas", *icon, "text-[var(--muted)]", "w-5")}></i>
                                        <span class="font-medium">{ *label }</span>
                                    </Link<Route>>
                                </div>
                            }
                        }) }
                    </nav>

                    // Theme toggle
                    <div class={classes!("text-center", "mt-4")}>
                        <ThemeToggle />
                    </div>
                </div>
            </div>
        </>
    }
}
