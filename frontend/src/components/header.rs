use web_sys::HtmlInputElement;
use yew::{events::InputEvent, prelude::*};
use yew_router::prelude::Link;

use crate::{components::theme_toggle::ThemeToggle, router::Route};

#[function_component(Header)]
pub fn header() -> Html {
    let mobile_menu_open = use_state(|| false);
    let search_open = use_state(|| false);
    let search_query = use_state(String::new);

    let toggle_mobile_menu = {
        let mobile_menu_open = mobile_menu_open.clone();
        Callback::from(move |_| mobile_menu_open.set(!*mobile_menu_open))
    };

    let close_mobile_menu = {
        let mobile_menu_open = mobile_menu_open.clone();
        Callback::from(move |_| mobile_menu_open.set(false))
    };

    let toggle_search = {
        let search_open = search_open.clone();
        Callback::from(move |_| search_open.set(!*search_open))
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

    let mut desktop_search_classes = classes!("header-search");
    if *search_open {
        desktop_search_classes.push("open");
    }

    let mut mobile_menu_classes = classes!("mobile-menu-overlay");
    if *mobile_menu_open {
        mobile_menu_classes.push("open");
    }

    let mut hamburger_classes = classes!("hamburger");
    if *mobile_menu_open {
        hamburger_classes.push("open");
    }

    let mut clear_button_classes = classes!("search-clear");
    if !search_query.is_empty() {
        clear_button_classes.push("visible");
    }

    let nav_links = [("文章", Route::Posts), ("标签", Route::Tags), ("分类", Route::Categories)];

    html! {
        <>
            <header class="header">
                <div class="container header-desktop">
                    <div class="header-brand">
                        <Link<Route> to={Route::Home} classes={classes!("brand-link")}>
                            {"L_B__"}
                        </Link<Route>>
                    </div>
                    <div class="header-actions">
                        <nav class="header-nav" aria-label="主导航">
                            { for nav_links.iter().map(|(label, route)| {
                                html! {
                                    <Link<Route> to={route.clone()} classes={classes!("nav-link")}>
                                        { *label }
                                    </Link<Route>>
                                }
                            }) }
                        </nav>
                        <div class={desktop_search_classes}>
                            <input
                                type="text"
                                placeholder="搜索文章标题或内容..."
                                value={(*search_query).clone()}
                                oninput={on_search_input.clone()}
                            />
                            <button
                                class="search-toggle"
                                type="button"
                                aria-label="展开搜索"
                                aria-pressed={(*search_open).to_string()}
                                aria-expanded={(*search_open).to_string()}
                                onclick={toggle_search.clone()}
                            >
                                {"搜索"}
                            </button>
                            <button
                                type="button"
                                aria-label="清空搜索"
                                class={clear_button_classes.clone()}
                                onclick={clear_search.clone()}
                            >
                                {"清空"}
                            </button>
                        </div>
                        <div class="header-theme-toggle">
                            <ThemeToggle />
                        </div>
                    </div>
                </div>

                <div class="container header-mobile">
                    <div class="header-brand">
                        <Link<Route> to={Route::Home} classes={classes!("brand-link")}>
                            {"L_B__"}
                        </Link<Route>>
                    </div>
                    <button
                        type="button"
                        class={hamburger_classes}
                        aria-label="打开菜单"
                        aria-expanded={(*mobile_menu_open).to_string()}
                        onclick={toggle_mobile_menu.clone()}
                    >
                        <span></span>
                        <span></span>
                        <span></span>
                    </button>
                </div>
            </header>

            <div class={mobile_menu_classes}>
                <div class="mobile-menu-backdrop" onclick={close_mobile_menu.clone()}></div>
                <div class="mobile-menu-panel" role="dialog" aria-modal="true">
                    <div class="mobile-menu-header">
                        <span class="mobile-menu-title">{"导航"}</span>
                        <button
                            type="button"
                            class="mobile-close"
                            onclick={close_mobile_menu.clone()}
                            aria-label="关闭菜单"
                        >
                            {"关闭"}
                        </button>
                    </div>
                    <div class="mobile-search">
                        <input
                            type="text"
                            placeholder="搜索文章标题或内容..."
                            value={(*search_query).clone()}
                            oninput={on_search_input}
                        />
                        <button
                            type="button"
                            class="mobile-search-clear"
                            onclick={clear_search}
                            disabled={search_query.is_empty()}
                        >
                            {"清空"}
                        </button>
                    </div>
                    <nav class="mobile-nav" aria-label="移动端导航">
                        { for nav_links.iter().map(|(label, route)| {
                            let close_cb = close_mobile_menu.clone();
                            html! {
                                <div class="mobile-nav-item" onclick={close_cb}>
                                    <Link<Route> to={route.clone()} classes={classes!("mobile-nav-link")}>
                                        { *label }
                                    </Link<Route>>
                                </div>
                            }
                        }) }
                    </nav>
                    <div class="mobile-theme-toggle">
                        <ThemeToggle />
                    </div>
                </div>
            </div>
        </>
    }
}
