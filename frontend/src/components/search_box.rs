//! Reusable inline search box for admin list filtering.
//!
//! Behavior:
//! * Controlled by parent: `value` / `on_change` + debounced `on_debounced_change`.
//! * Optional `on_submit` fired on Enter (for server-driven searches that
//!   trigger a refetch only on explicit confirmation).
//! * `x` button clears the value (fires `on_change("")`).
//!
//! Client-side filter callers wire `on_debounced_change` to their local
//! filter state; server-side callers wire `on_submit` to kick a refetch.

use gloo_timers::callback::Timeout;
use web_sys::HtmlInputElement;
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct SearchBoxProps {
    pub value: String,
    pub on_change: Callback<String>,

    #[prop_or_default]
    pub on_debounced_change: Option<Callback<String>>,
    #[prop_or_default]
    pub on_submit: Option<Callback<String>>,
    #[prop_or(AttrValue::Static("搜索..."))]
    pub placeholder: AttrValue,
    #[prop_or(300)]
    pub debounce_ms: u32,
    #[prop_or_default]
    pub input_class: Classes,
    /// When `true`, the clear (×) button is hidden even if `value` is non-empty.
    #[prop_or(false)]
    pub hide_clear: bool,
}

#[function_component(SearchBox)]
pub fn search_box(props: &SearchBoxProps) -> Html {
    // Keep the debounce timer alive between renders.
    let timer = use_mut_ref(|| None::<Timeout>);

    let oninput = {
        let on_change = props.on_change.clone();
        let on_debounced = props.on_debounced_change.clone();
        let debounce_ms = props.debounce_ms;
        let timer = timer.clone();
        Callback::from(move |event: InputEvent| {
            let input: HtmlInputElement = event.target_unchecked_into();
            let value = input.value();
            on_change.emit(value.clone());
            if let Some(cb) = on_debounced.clone() {
                // Cancel previous pending fire.
                *timer.borrow_mut() = None;
                let cb_inner = cb.clone();
                let next = value.clone();
                let t = Timeout::new(debounce_ms, move || {
                    cb_inner.emit(next);
                });
                *timer.borrow_mut() = Some(t);
            }
        })
    };

    let onkeydown = {
        let on_submit = props.on_submit.clone();
        let value = props.value.clone();
        Callback::from(move |event: KeyboardEvent| {
            if event.key() == "Enter" {
                if let Some(cb) = on_submit.clone() {
                    event.prevent_default();
                    cb.emit(value.clone());
                }
            }
        })
    };

    let on_clear = {
        let on_change = props.on_change.clone();
        let on_debounced = props.on_debounced_change.clone();
        let timer = timer.clone();
        Callback::from(move |_| {
            *timer.borrow_mut() = None;
            on_change.emit(String::new());
            if let Some(cb) = on_debounced.clone() {
                cb.emit(String::new());
            }
        })
    };

    let has_value = !props.value.is_empty() && !props.hide_clear;
    let base_input_class = classes!(
        "w-full",
        "rounded-lg",
        "border",
        "border-[var(--border)]",
        "bg-[var(--surface-alt)]",
        "px-3",
        "py-2",
        "pr-8",
        "text-sm",
        "font-mono",
        props.input_class.clone(),
    );

    html! {
        <div class={classes!("relative", "w-full")}>
            <input
                type="search"
                class={base_input_class}
                placeholder={props.placeholder.clone()}
                value={props.value.clone()}
                oninput={oninput}
                onkeydown={onkeydown}
                aria-label={props.placeholder.clone()}
            />
            if has_value {
                <button
                    type="button"
                    class={classes!(
                        "absolute", "right-2", "top-1/2", "-translate-y-1/2",
                        "text-[var(--muted)]", "hover:text-[var(--text)]",
                        "text-xs", "px-1"
                    )}
                    title="清空"
                    aria-label="清空搜索"
                    onclick={on_clear}
                >
                    { "✕" }
                </button>
            }
        </div>
    }
}
