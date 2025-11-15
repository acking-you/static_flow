use wasm_bindgen::JsCast;
use yew::prelude::*;

fn is_dark_theme() -> bool {
    web_sys::window()
        .and_then(|win| win.document())
        .and_then(|doc| doc.document_element())
        .and_then(|el| el.get_attribute("data-theme"))
        .map(|theme| theme.eq_ignore_ascii_case("dark"))
        .unwrap_or(false)
}

// Tailwind migration demo component that illustrates utility-first styling.
#[function_component(ThemeToggle)]
pub fn theme_toggle() -> Html {
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

    let label = if *theme_state { "切换到亮色模式" } else { "切换到暗色模式" };

    html! {
        <button
            type="button"
            class={classes!(
                "group",
                "fixed",
                "bottom-4",
                "right-4",
                "z-50",
                "inline-flex",
                "h-12",
                "w-12",
                "items-center",
                "justify-center",
                "rounded-full",
                "border",
                "border-border",
                "bg-surface",
                "text-muted",
                "shadow-lg",
                "transition-all",
                "duration-200",
                "ease-out",
                "hover:scale-110",
                "hover:bg-surface-alt",
                "hover:text-primary",
                "focus-visible:outline-none",
                "focus-visible:ring-2",
                "focus-visible:ring-primary",
                "focus-visible:ring-offset-2",
                "focus-visible:ring-offset-surface",
                "active:scale-95",
                "sm:bottom-6",
                "sm:right-6",
                "dark:bg-surface-alt",
                "dark:text-primary"
            )}
            {onclick}
            aria-label={label}
            title={label}
        >
            <i
                class={classes!(
                    "fas",
                    "fa-adjust",
                    "fa-fw",
                    "text-xl",
                    "text-primary",
                    "transition-transform",
                    "duration-200",
                    "group-hover:rotate-12",
                    "group-active:-rotate-6"
                )}
                aria-hidden="true"
            ></i>
            <span class="sr-only">{ label }</span>
        </button>
    }
}
