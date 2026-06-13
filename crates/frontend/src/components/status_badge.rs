//! Reusable status pill for admin tables.
//!
//! Centralizes what used to be per-page `status_badge_class` helpers: a single
//! status string maps to one consistent tone + icon, so every admin surface
//! (tasks, wishes, gateway accounts/keys, …) reads the same way and adapts to
//! dark mode in one place. Styling lives in `input.css` under `.status-badge`.

use yew::prelude::*;

/// Props for [`StatusBadge`].
#[derive(Properties, PartialEq)]
pub struct StatusBadgeProps {
    /// Raw status string (matched case-insensitively against known values).
    pub status: AttrValue,
    /// Optional display label; defaults to the status text itself.
    #[prop_or_default]
    pub label: Option<AttrValue>,
}

/// Maps a status to its `(tone-class, fontawesome-icon)`.
fn tone_and_icon(status: &str) -> (&'static str, &'static str) {
    match status {
        "pending" | "queued" | "waiting" => ("status-badge--pending", "fa-clock"),
        "approved" | "approve" => ("status-badge--approved", "fa-circle-check"),
        "running" | "processing" | "in_progress" | "in-progress" | "streaming" => {
            ("status-badge--running", "fa-spinner fa-spin")
        },
        "done" | "success" | "succeeded" | "completed" | "ingested" | "issued" | "active"
        | "enabled" | "ok" | "healthy" | "valid" => ("status-badge--done", "fa-circle-check"),
        "failed" | "error" | "errored" | "invalid" | "delete" | "deleted" | "remove" => {
            ("status-badge--failed", "fa-circle-exclamation")
        },
        "rejected" | "reject" | "disabled" | "expired" | "cancelled" | "canceled" | "revoked" => {
            ("status-badge--rejected", "fa-ban")
        },
        _ => ("status-badge--neutral", "fa-circle-info"),
    }
}

/// A consistent status pill: tinted background, matching icon, uppercase label.
#[function_component(StatusBadge)]
pub fn status_badge(props: &StatusBadgeProps) -> Html {
    let lower = props.status.to_lowercase();
    let (tone, icon) = tone_and_icon(lower.as_str());
    let label = props.label.clone().unwrap_or_else(|| props.status.clone());
    html! {
        <span class={classes!("status-badge", tone)}>
            <i class={classes!("fas", icon, "status-badge__icon")} aria-hidden="true"></i>
            <span>{ label }</span>
        </span>
    }
}
