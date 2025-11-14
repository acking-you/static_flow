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
            class="theme-toggle-btn"
            {onclick}
            aria-label={label}
            title={label}
        >
            <i class="fas fa-adjust fa-fw" aria-hidden="true"></i>
            <span class="visually-hidden">{ label }</span>
        </button>
    }
}
