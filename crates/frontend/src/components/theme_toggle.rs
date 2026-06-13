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

    // Dark active -> reveal the sun (clicking goes to light); light active ->
    // reveal the moon. The hidden glyph rotates + scales out while the visible
    // one rotates + scales in, so the theme swap cross-fades on-token instead of
    // hard-cutting one FontAwesome character to another.
    let (sun_anim, moon_anim) = if *theme_state {
        ("opacity-100 rotate-0 scale-100", "opacity-0 -rotate-90 scale-50")
    } else {
        ("opacity-0 rotate-90 scale-50", "opacity-100 rotate-0 scale-100")
    };

    let button_class = classes!(
        "group",
        "btn-fluent-icon",
        "border",
        "border-[var(--border)]",
        "bg-transparent",
        "hover:bg-[var(--surface-alt)]",
        "transition-all",
        "duration-[var(--motion-fast)]",
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
            <span class="relative inline-flex h-[1.25em] w-[1.25em] items-center justify-center">
                <i
                    class={classes!(
                        "fas", "fa-sun", "fa-lg", "absolute",
                        "transition-all", "duration-[var(--motion-base)]", "ease-[var(--ease-spring)]",
                        "text-[var(--text)]", "group-hover:text-[var(--primary)]",
                        sun_anim
                    )}
                    aria-hidden="true"
                ></i>
                <i
                    class={classes!(
                        "fas", "fa-moon", "fa-lg", "absolute",
                        "transition-all", "duration-[var(--motion-base)]", "ease-[var(--ease-spring)]",
                        "text-[var(--text)]", "group-hover:text-[var(--primary)]",
                        moon_anim
                    )}
                    aria-hidden="true"
                ></i>
            </span>
            <span class="sr-only">{ label }</span>
        </button>
    }
}
