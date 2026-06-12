//! Form building blocks: labeled fields with inline validation wiring.
//!
//! [`FormField`] renders the label / hint / error chrome and links the error
//! text to the control via `aria-describedby`; [`TextInput`], [`SelectInput`]
//! and [`TextArea`] are the styled controls (one shared `.form-input` class
//! instead of the utility string previously copy-pasted across admin pages).

use web_sys::{HtmlInputElement, HtmlSelectElement, HtmlTextAreaElement};
use yew::prelude::*;

/// Props for [`FormField`].
#[derive(Properties, PartialEq)]
pub struct FormFieldProps {
    /// Field label text.
    pub label: AttrValue,
    /// Stable id used to link label, control, and error message. The child
    /// control should use the same id (and `{id}-error` is reserved for the
    /// error paragraph referenced by `aria-describedby`).
    pub id: AttrValue,
    /// Optional helper text shown under the control when there is no error.
    #[prop_or_default]
    pub hint: Option<AttrValue>,
    /// Validation error; when set it replaces the hint and is announced.
    #[prop_or_default]
    pub error: Option<String>,
    /// Marks the label with a required indicator.
    #[prop_or(false)]
    pub required: bool,
    /// The form control (typically one of the inputs from this module).
    pub children: Html,
}

/// Label + control + hint/error wrapper.
#[function_component(FormField)]
pub fn form_field(props: &FormFieldProps) -> Html {
    let error_id = format!("{}-error", props.id);
    html! {
        <div class={classes!("form-field")}>
            <label class={classes!("form-label")} for={props.id.clone()}>
                { &props.label }
                if props.required {
                    <span class={classes!("form-required")} aria-hidden="true">{ " *" }</span>
                }
            </label>
            { props.children.clone() }
            if let Some(error) = props.error.as_ref() {
                <p id={error_id} class={classes!("form-error")} role="alert">{ error }</p>
            } else if let Some(hint) = props.hint.as_ref() {
                <p class={classes!("form-hint")}>{ hint }</p>
            }
        </div>
    }
}

/// Props for [`TextInput`].
#[derive(Properties, PartialEq)]
pub struct TextInputProps {
    /// Id matching the surrounding [`FormField`].
    pub id: AttrValue,
    /// Current value.
    pub value: AttrValue,
    /// Emits the full new value on every input event.
    pub on_change: Callback<String>,
    /// HTML input type (text, email, number, password, ...).
    #[prop_or(AttrValue::Static("text"))]
    pub kind: AttrValue,
    /// Placeholder text.
    #[prop_or_default]
    pub placeholder: AttrValue,
    /// Use the monospace variant (admin/terminal contexts).
    #[prop_or(false)]
    pub mono: bool,
    /// Marks the control invalid and wires `aria-describedby` to the
    /// surrounding field's error paragraph.
    #[prop_or(false)]
    pub invalid: bool,
    /// Disables the control.
    #[prop_or(false)]
    pub disabled: bool,
}

/// Styled single-line text input.
#[function_component(TextInput)]
pub fn text_input(props: &TextInputProps) -> Html {
    let oninput = {
        let on_change = props.on_change.clone();
        Callback::from(move |event: InputEvent| {
            let input: HtmlInputElement = event.target_unchecked_into();
            on_change.emit(input.value());
        })
    };
    let class = classes!("form-input", props.mono.then_some("form-input--mono"));
    html! {
        <input
            id={props.id.clone()}
            class={class}
            type={props.kind.clone()}
            value={props.value.clone()}
            placeholder={props.placeholder.clone()}
            disabled={props.disabled}
            aria-invalid={if props.invalid { "true" } else { "false" }}
            aria-describedby={props.invalid.then(|| format!("{}-error", props.id))}
            {oninput}
        />
    }
}

/// One option for [`SelectInput`].
#[derive(Debug, Clone, PartialEq)]
pub struct SelectOption {
    /// Submitted value.
    pub value: String,
    /// Visible label.
    pub label: String,
}

/// Props for [`SelectInput`].
#[derive(Properties, PartialEq)]
pub struct SelectInputProps {
    /// Id matching the surrounding [`FormField`].
    pub id: AttrValue,
    /// Currently selected value.
    pub value: AttrValue,
    /// Emits the newly selected value.
    pub on_change: Callback<String>,
    /// Options in render order.
    pub options: Vec<SelectOption>,
    /// Disables the control.
    #[prop_or(false)]
    pub disabled: bool,
}

/// Styled native select.
#[function_component(SelectInput)]
pub fn select_input(props: &SelectInputProps) -> Html {
    let onchange = {
        let on_change = props.on_change.clone();
        Callback::from(move |event: Event| {
            if let Some(select) = event.target_dyn_into::<HtmlSelectElement>() {
                on_change.emit(select.value());
            }
        })
    };
    html! {
        <select
            id={props.id.clone()}
            class={classes!("form-input")}
            value={props.value.clone()}
            disabled={props.disabled}
            {onchange}
        >
            { for props.options.iter().map(|opt| html! {
                <option value={opt.value.clone()} selected={*props.value == opt.value}>
                    { &opt.label }
                </option>
            }) }
        </select>
    }
}

/// Props for [`TextArea`].
#[derive(Properties, PartialEq)]
pub struct TextAreaProps {
    /// Id matching the surrounding [`FormField`].
    pub id: AttrValue,
    /// Current value.
    pub value: AttrValue,
    /// Emits the full new value on every input event.
    pub on_change: Callback<String>,
    /// Placeholder text.
    #[prop_or_default]
    pub placeholder: AttrValue,
    /// Visible rows.
    #[prop_or(4)]
    pub rows: u32,
    /// Marks the control invalid (see [`TextInputProps::invalid`]).
    #[prop_or(false)]
    pub invalid: bool,
    /// Disables the control.
    #[prop_or(false)]
    pub disabled: bool,
}

/// Styled multi-line text area.
#[function_component(TextArea)]
pub fn text_area(props: &TextAreaProps) -> Html {
    let oninput = {
        let on_change = props.on_change.clone();
        Callback::from(move |event: InputEvent| {
            let area: HtmlTextAreaElement = event.target_unchecked_into();
            on_change.emit(area.value());
        })
    };
    html! {
        <textarea
            id={props.id.clone()}
            class={classes!("form-input")}
            value={props.value.clone()}
            placeholder={props.placeholder.clone()}
            rows={props.rows.to_string()}
            disabled={props.disabled}
            aria-invalid={if props.invalid { "true" } else { "false" }}
            aria-describedby={props.invalid.then(|| format!("{}-error", props.id))}
            {oninput}
        />
    }
}
