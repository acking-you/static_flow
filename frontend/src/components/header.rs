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

    let mobile_search_input = on_search_input.clone();
    let mobile_search_keypress = on_search_keypress.clone();
    let mobile_do_search = do_search.clone();
    let mobile_clear_search = clear_search.clone();

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
                        <div class="header-search">
                            <input
                                type="text"
                                placeholder="搜索文章标题或内容..."
                                value={(*search_query).clone()}
                                oninput={on_search_input.clone()}
                                onkeypress={on_search_keypress.clone()}
                                class="header-search-input"
                            />
                            <div class="header-search-actions">
                                <TooltipIconButton
                                    icon={IconName::Search}
                                    tooltip="搜索"
                                    position={TooltipPosition::Bottom}
                                    onclick={do_search.clone()}
                                    size={20}
                                    class="header-search-btn"
                                />
                                <TooltipIconButton
                                    icon={IconName::X}
                                    tooltip="清空"
                                    position={TooltipPosition::Bottom}
                                    onclick={clear_search.clone()}
                                    size={18}
                                    class="header-search-clear"
                                    disabled={search_query.is_empty()}
                                />
                            </div>
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
                    <div class="mobile-close">
                        <TooltipIconButton
                            icon={IconName::ArrowLeft}
                            tooltip="关闭菜单"
                            position={TooltipPosition::Bottom}
                            onclick={close_mobile_menu.clone()}
                            size={20}
                            class="mobile-close-btn"
                        />
                    </div>
                    <div class="mobile-search mobile-menu-search">
                        <input
                            type="text"
                            placeholder="搜索文章标题或内容..."
                            value={(*search_query).clone()}
                            oninput={mobile_search_input.clone()}
                            onkeypress={mobile_search_keypress.clone()}
                            class="mobile-search-input"
                        />
                        <div class="mobile-search-actions">
                            <TooltipIconButton
                                icon={IconName::Search}
                                tooltip="搜索"
                                position={TooltipPosition::Bottom}
                                onclick={mobile_do_search.clone()}
                                size={20}
                                class="mobile-search-btn"
                            />
                            <TooltipIconButton
                                icon={IconName::X}
                                tooltip="清空"
                                position={TooltipPosition::Bottom}
                                onclick={mobile_clear_search.clone()}
                                size={18}
                                class="mobile-search-clear"
                                disabled={search_query.is_empty()}
                            />
                        </div>
                    </div>
                    <div class="mobile-menu-header">
                        <span class="mobile-menu-title">{"导航"}</span>
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
