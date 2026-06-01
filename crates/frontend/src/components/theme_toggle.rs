use wasm_bindgen::JsCast;
use yew::prelude::*;

use crate::i18n::current::theme_toggle as t;

fn is_dark_theme() -> bool {
    web_sys::window()
        .and_then(|win| win.document())
        .and_then(|doc| doc.document_element())
        .and_then(|el| el.get_attribute("data-theme"))
        .map(|theme| theme.eq_ignore_ascii_case("dark"))
        .unwrap_or(false)
}

#[derive(Properties, PartialEq)]
pub struct ThemeToggleProps {
    #[prop_or_default]
    pub class: Classes,
}

#[function_component(ThemeToggle)]
pub fn theme_toggle(props: &ThemeToggleProps) -> Html {
    let ThemeToggleProps {
        class,
    } = props;
    let theme_state = use_state(is_dark_theme);

    let onclick = {
        let theme_state = theme_state.clone();
        Callback::from(move |_| {
            if let Some(win) = web_sys::window() {
                let _ =
                    js_sys::Reflect::get(&win, &wasm_bindgen::JsValue::from_str("__toggleTheme"))
                        .ok()
                        .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
                        .and_then(|func| func.call0(&wasm_bindgen::JsValue::NULL).ok());
            }
            theme_state.set(is_dark_theme());
        })
    };

    let label = if *theme_state { t::SWITCH_TO_LIGHT } else { t::SWITCH_TO_DARK };

    let icon_class = if *theme_state { "fa-sun" } else { "fa-moon" };

    let button_class = classes!(
        "group",
        "btn-fluent-icon",
        "border",
        "border-[var(--border)]",
        "bg-transparent",
        "hover:bg-[var(--surface-alt)]",
        "transition-all",
        "duration-100",
        "ease-[var(--ease-snap)]",
        class.clone()
    );

    html! {
        <button
            type="button"
            class={button_class}
            {onclick}
            aria-label={label}
            title={label}
            aria-pressed={(*theme_state).to_string()}
        >
            <i
                class={classes!(
                    "fas",
                    icon_class,
                    "fa-lg",
                    "transition-all",
                    "duration-100",
                    "ease-[var(--ease-snap)]",
                    "text-[var(--text)]",
                    "group-hover:text-[var(--primary)]"
                )}
                aria-hidden="true"
            ></i>
            <span class="sr-only">{ label }</span>
        </button>
    }
}
