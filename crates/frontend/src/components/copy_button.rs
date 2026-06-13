//! Small copy-to-clipboard button shared across admin tables.
//!
//! Replaces the per-page `copy_icon_button` + `copy_text` FFI duplicated in
//! admin.rs / admin_llm_gateway.rs / admin_kiro_gateway.rs, and adds a brief
//! "copied" checkmark so the click registers.

use wasm_bindgen::prelude::*;
use yew::prelude::*;

#[wasm_bindgen(inline_js = r#"
export function sf_copy_text(text) {
    if (navigator.clipboard) {
        navigator.clipboard.writeText(text).catch(function(){});
    }
}
"#)]
extern "C" {
    fn sf_copy_text(text: &str);
}

/// Props for [`CopyButton`].
#[derive(Properties, PartialEq)]
pub struct CopyButtonProps {
    /// Text written to the clipboard on click.
    pub text: AttrValue,
    /// Extra classes merged onto the button.
    #[prop_or_default]
    pub class: Classes,
    /// Tooltip / accessible label.
    #[prop_or(AttrValue::Static("复制"))]
    pub title: AttrValue,
}

/// Inline copy button that flips to a checkmark for ~1.2s after copying.
#[function_component(CopyButton)]
pub fn copy_button(props: &CopyButtonProps) -> Html {
    let copied = use_state(|| false);
    let onclick = {
        let text = props.text.clone();
        let copied = copied.clone();
        Callback::from(move |_: MouseEvent| {
            sf_copy_text(&text);
            copied.set(true);
            let copied = copied.clone();
            gloo_timers::callback::Timeout::new(1200, move || copied.set(false)).forget();
        })
    };
    let icon = if *copied { "fa-check" } else { "fa-copy" };
    html! {
        <button
            type="button"
            class={classes!(
                "btn-copy-inline",
                props.class.clone(),
                copied.then_some("btn-copy-inline--done")
            )}
            onclick={onclick}
            title={props.title.clone()}
            aria-label={props.title.clone()}
        >
            <i class={classes!("fas", icon)} aria-hidden="true"></i>
        </button>
    }
}
