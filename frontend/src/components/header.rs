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

    // 执行搜索 - 使用 history.pushState 避免页面重新加载
    let do_search = {
        let search_query = search_query.clone();
        Callback::from(move |_: MouseEvent| {
            let query = (*search_query).trim();
            if !query.is_empty() {
                let encoded_query = urlencoding::encode(query);
                let search_url = crate::config::route_path(&format!("/search?q={}", encoded_query));
                if let Some(window) = web_sys::window() {
                    if let Ok(history) = window.history() {
                        // 使用 pushState 改变 URL 但不触发页面加载
                        let _ = history.push_state_with_url(
                            &wasm_bindgen::JsValue::NULL,
                            "",
                            Some(&search_url),
                        );
                        // 触发 popstate 事件让 yew-router 响应
                        if let Ok(event) = web_sys::Event::new("popstate") {
                            let _ = window.dispatch_event(&event);
                        }
                    }
                }
            }
        })
    };

    // Enter键搜索 - 使用 history.pushState 避免页面重新加载
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
                            // 使用 pushState 改变 URL 但不触发页面加载
                            let _ = history.push_state_with_url(
                                &wasm_bindgen::JsValue::NULL,
                                "",
                                Some(&search_url),
                            );
                            // 触发 popstate 事件让 yew-router 响应
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
        "bg-[var(--surface)]",
        "dark:bg-[#0b1220]",
        "text-[var(--text)]",
        "dark:text-[#eef2ff]",
        "p-[4.5rem_1.5rem_2rem]",
        "flex",
        "flex-col",
        "gap-5",
        "overflow-y-auto",
        "backdrop-blur-[20px]",
        "shadow-[0_20px_50px_rgba(15,23,42,0.2)]",
        "[.dark_&]:shadow-[0_25px_70px_rgba(0,0,0,0.7)]",
        "transition-all",
        "duration-[350ms]",
        "ease-[var(--ease-spring)]",
        if *mobile_menu_open { "translate-y-0 opacity-100" } else { "-translate-y-4 opacity-0" }
    );

    // Hamburger button + animated lines
    let hamburger_classes = classes!(
        "w-12",
        "h-12",
        "min-w-[3rem]",
        "min-h-[3rem]",
        "border",
        "border-[var(--border)]",
        "rounded-full",
        "bg-[var(--surface)]",
        "flex",
        "flex-col",
        "justify-center",
        "items-center",
        "gap-[0.35rem]",
        "cursor-pointer",
        "transition-all",
        "duration-[280ms]",
        "ease-in-out"
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

    let nav_links = [
        ("最新", Route::LatestArticles),
        ("文章", Route::Posts),
        ("标签", Route::Tags),
        ("分类", Route::Categories),
    ];

    let mobile_search_input = on_search_input.clone();
    let mobile_search_keypress = on_search_keypress.clone();
    let mobile_do_search = do_search.clone();
    let mobile_clear_search = clear_search.clone();

    html! {
        <>
            // Header container - sticky at top
            <header class={classes!(
                "sticky", "top-0", "left-0", "right-0", "z-[80]", "w-full",
                "backdrop-blur-md", "bg-[var(--header-bg)]",
                "border-b", "border-[var(--border)]",
                "shadow-[0_12px_35px_rgba(0,0,0,0.08)]",
                "transition-all", "duration-[280ms]", "ease-in-out"
            )}>
                // Desktop header - hidden on mobile
                <div class={classes!(
                    "desktop-header",
                    "items-center", "gap-3",
                    "min-h-[var(--header-height-mobile)]", "md:min-h-[var(--header-height-desktop)]",
                    "max-w-7xl", "mx-auto", "px-4", "sm:px-6", "lg:px-8"
                )}>
                    // Brand section
                    <div class={classes!("font-bold", "tracking-[0.2em]", "uppercase")}>
                        <Link<Route> to={Route::Home} classes={classes!(
                            "inline-flex", "items-center", "min-h-[var(--hit-size)]",
                            "text-[1.75rem]", "font-extrabold", "tracking-[0.18em]",
                            "bg-gradient-to-br", "from-[var(--primary)]", "to-[var(--link)]",
                            "bg-clip-text", "text-transparent",
                            "transition-all", "duration-300", "ease-[cubic-bezier(0.34,1.56,0.64,1)]",
                            "drop-shadow-[0_2px_4px_rgba(29,158,216,0.15)]",
                            "whitespace-nowrap",
                            "hover:scale-110", "hover:drop-shadow-[0_4px_8px_rgba(29,158,216,0.3)]"
                        )}>
                            {"L_B__"}
                        </Link<Route>>
                    </div>

                    // Actions section - right-aligned
                    <div class={classes!("ml-auto", "flex", "items-center", "gap-3")}>
                        // Navigation links
                        <nav class={classes!("flex", "items-center", "gap-2")} aria-label="主导航">
                            { for nav_links.iter().map(|(label, route)| {
                                html! {
                                    <Link<Route> to={route.clone()} classes={classes!(
                                        "px-[0.9rem]", "rounded-full", "font-medium",
                                        "text-[var(--text)]", "bg-transparent",
                                        "inline-flex", "items-center", "justify-center",
                                        "min-h-[var(--hit-size)]",
                                        "transition-all", "duration-[280ms]", "ease-in-out",
                                        "hover:bg-[rgba(0,0,0,0.05)]",
                                        "[.dark_&]:hover:bg-[rgba(255,255,255,0.08)]"
                                    )}>
                                        { *label }
                                    </Link<Route>>
                                }
                            }) }
                        </nav>

                        // Search section
                        <div class={classes!("flex", "items-center", "gap-[0.35rem]", "relative")}>
                            <input
                                type="text"
                                placeholder="搜索文章标题或内容..."
                                value={(*search_query).clone()}
                                oninput={on_search_input.clone()}
                                onkeypress={on_search_keypress.clone()}
                                class={classes!(
                                    "w-[220px]", "lg:w-[260px]",
                                    "opacity-100", "pointer-events-auto",
                                    "border", "border-[var(--border)]", "rounded-full",
                                    "px-[0.95rem]", "h-[var(--hit-size)]",
                                    "bg-[var(--surface)]", "text-[var(--text)]",
                                    "transition-all", "duration-[250ms]", "ease-[cubic-bezier(0.34,1.56,0.64,1)]",
                                    "focus:outline-[2px]", "focus:outline-[var(--primary)]", "focus:outline-offset-[3px]",
                                    "focus:border-transparent", "focus:shadow-[0_0_0_4px_rgba(29,158,216,0.15)]"
                                )}
                            />
                            <div class={classes!("flex", "items-center", "gap-[0.25rem]")}>
                                <TooltipIconButton
                                    icon={IconName::Search}
                                    tooltip="搜索"
                                    position={TooltipPosition::Bottom}
                                    onclick={do_search.clone()}
                                    size={20}
                                    class={classes!("w-[2.25rem]", "h-[2.25rem]")}
                                />
                                <TooltipIconButton
                                    icon={IconName::X}
                                    tooltip="清空"
                                    position={TooltipPosition::Bottom}
                                    onclick={clear_search.clone()}
                                    size={18}
                                    class={classes!("w-[2.25rem]", "h-[2.25rem]", "disabled:opacity-30")}
                                    disabled={search_query.is_empty()}
                                />
                            </div>
                        </div>

                        // Theme toggle
                        <div>
                            <ThemeToggle />
                        </div>
                    </div>
                </div>

                // Mobile header - visible on mobile only
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
                    // Brand section
                    <div class={classes!("font-bold", "tracking-[0.2em]", "uppercase")}>
                        <Link<Route> to={Route::Home} classes={classes!(
                            "inline-flex", "items-center", "min-h-[var(--hit-size)]",
                            "text-[1.75rem]", "font-extrabold", "tracking-[0.18em]",
                            "bg-gradient-to-br", "from-[var(--primary)]", "to-[var(--link)]",
                            "bg-clip-text", "text-transparent",
                            "transition-all", "duration-300", "ease-[cubic-bezier(0.34,1.56,0.64,1)]",
                            "drop-shadow-[0_2px_4px_rgba(29,158,216,0.15)]",
                            "whitespace-nowrap",
                            "hover:scale-110", "hover:drop-shadow-[0_4px_8px_rgba(29,158,216,0.3)]"
                        )}>
                            {"L_B__"}
                        </Link<Route>>
                    </div>

                    // Hamburger button with animated lines
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
                        "ease-[cubic-bezier(0.34,1.56,0.64,1)]",
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
                            tooltip="关闭菜单"
                            position={TooltipPosition::Bottom}
                            onclick={close_mobile_menu.clone()}
                            size={20}
                            class={classes!(
                                "!bg-[var(--surface)]",
                                "!border-2",
                                "!border-[var(--border)]",
                                "!shadow-[var(--shadow-sm)]"
                            )}
                        />
                    </div>

                    // Mobile search
                    <div class={classes!("flex", "gap-2", "items-center", "order-0", "mb-3")}>
                        <input
                            type="text"
                            placeholder="搜索文章标题或内容..."
                            value={(*search_query).clone()}
                            oninput={mobile_search_input.clone()}
                            onkeypress={mobile_search_keypress.clone()}
                            class={classes!(
                                "flex-1",
                                "border", "border-[var(--border)]", "rounded-[0.85rem]",
                                "px-4",
                                "bg-[var(--surface-alt)]",
                                "text-[var(--text)]",
                                "text-[0.95rem]", "min-h-[3rem]", "h-12", "leading-[3rem]",
                                "transition-all", "duration-[250ms]", "ease-[cubic-bezier(0.34,1.56,0.64,1)]",
                                "focus:outline-[2px]", "focus:outline-[var(--primary)]", "focus:outline-offset-2",
                                "focus:border-transparent", "focus:shadow-[0_0_0_4px_rgba(29,158,216,0.15)]",
                                "placeholder:text-[var(--muted)]"
                            )}
                        />
                        <div class={classes!("flex", "gap-[0.375rem]")}>
                            <TooltipIconButton
                                icon={IconName::Search}
                                tooltip="搜索"
                                position={TooltipPosition::Bottom}
                                onclick={mobile_do_search.clone()}
                                size={20}
                                class={classes!("w-[2.5rem]", "h-[2.5rem]")}
                            />
                            <TooltipIconButton
                                icon={IconName::X}
                                tooltip="清空"
                                position={TooltipPosition::Bottom}
                                onclick={mobile_clear_search.clone()}
                                size={18}
                                class={classes!(
                                    "w-[2.5rem]",
                                    "h-[2.5rem]",
                                    "disabled:opacity-50",
                                    "disabled:cursor-not-allowed"
                                )}
                                disabled={search_query.is_empty()}
                            />
                        </div>
                    </div>

                    // Menu header
                    <div class={classes!(
                        "flex", "items-center", "justify-between", "font-semibold"
                    )}>
                        <span class={classes!(
                            "tracking-[0.15em]",
                            "uppercase",
                            "text-[var(--text)]",
                            "dark:text-white"
                        )}>{"导航"}</span>
                    </div>

                    // Navigation links
                    <nav class={classes!("flex", "flex-col", "gap-3")} aria-label="移动端导航">
                        { for nav_links.iter().map(|(label, route)| {
                            let close_cb = close_mobile_menu.clone();
                            html! {
                                <div
                                    class={classes!("py-[0.2rem]", "active:opacity-85")}
                                    onclick={close_cb}
                                >
                                    <Link<Route> to={route.clone()} classes={classes!(
                                        "block", "py-[0.85rem]", "px-4", "rounded-[0.85rem]",
                                        "bg-[var(--surface-alt)]",
                                        "border", "border-[var(--border)]",
                                        "text-[var(--text)]",
                                        "hover:bg-[rgba(var(--primary-rgb),0.12)]",
                                        "hover:border-[var(--primary)]",
                                        "text-base", "font-medium"
                                    )}>
                                        { *label }
                                    </Link<Route>>
                                </div>
                            }
                        }) }
                    </nav>

                    // Theme toggle
                    <div class={classes!("text-center")}>
                        <ThemeToggle />
                    </div>
                </div>
            </div>
        </>
    }
}
