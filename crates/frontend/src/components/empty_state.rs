//! Centered placeholder for empty / error admin lists and tables.
//!
//! Admin tables used to render an empty `<tbody>` (often with pagination still
//! showing), so "no data" was indistinguishable from "still loading" or "the
//! request failed". This gives a single, legible empty/error state with an
//! optional action slot (e.g. a Retry button).

use yew::prelude::*;

/// Props for [`EmptyState`].
#[allow(
    dead_code,
    reason = "Consumed by the in-progress admin empty/error-state rework; the kit ships complete."
)]
#[derive(Properties, PartialEq)]
pub struct EmptyStateProps {
    /// FontAwesome icon class (e.g. `fa-inbox`, `fa-triangle-exclamation`).
    #[prop_or(AttrValue::Static("fa-inbox"))]
    pub icon: AttrValue,
    /// Primary message.
    pub title: AttrValue,
    /// Optional secondary hint line.
    #[prop_or_default]
    pub hint: Option<AttrValue>,
    /// `"neutral"` (default) or `"error"` for failure states.
    #[prop_or(AttrValue::Static("neutral"))]
    pub tone: AttrValue,
    /// Optional action (e.g. a Retry button) rendered under the hint.
    #[prop_or_default]
    pub children: Html,
}

/// A centered icon + title + hint + optional action block.
#[allow(
    dead_code,
    reason = "Consumed by the in-progress admin empty/error-state rework; the kit ships complete."
)]
#[function_component(EmptyState)]
pub fn empty_state(props: &EmptyStateProps) -> Html {
    let tone_class = (props.tone == "error").then_some("admin-empty--error");
    html! {
        <div class={classes!("admin-empty", tone_class)}>
            <i class={classes!("fas", props.icon.to_string(), "admin-empty__icon")} aria-hidden="true"></i>
            <p class={classes!("admin-empty__title")}>{ props.title.clone() }</p>
            if let Some(hint) = props.hint.clone() {
                <p class={classes!("admin-empty__hint")}>{ hint }</p>
            }
            { props.children.clone() }
        </div>
    }
}
