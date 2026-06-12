//! KPI stat card: icon, value, label, optional route link.
//!
//! Restrained by design: hover feedback is shadow elevation plus a border
//! accent (see `.stats-card` in input.css) — no tilt, ripple, or magnetic
//! tracking.

use yew::prelude::*;
use yew_router::prelude::*;

use crate::{
    components::icons::{Icon, IconName},
    router::Route,
};

/// Props for [`StatsCard`].
#[allow(
    dead_code,
    reason = "Some pages instantiate the card without navigation, but the reusable props keep \
              route support available."
)]
#[derive(Properties, PartialEq, Clone)]
pub struct StatsCardProps {
    /// Icon shown in the accent block.
    pub icon: IconName,
    /// Headline value (already formatted).
    pub value: String,
    /// Short label; doubles as the hover tooltip.
    pub label: String,
    /// Optional navigation target for the whole card.
    #[prop_or_default]
    pub route: Option<Route>,
}

/// One statistic card with optional link behavior.
#[function_component(StatsCard)]
pub fn stats_card(props: &StatsCardProps) -> Html {
    let card_classes = classes!(
        "group",
        "relative",
        "bg-[var(--surface)]",
        "border-t",
        "border-r",
        "border-b",
        "border-[var(--border)]",
        "border-l-[4px]",
        "border-l-[var(--primary)]",
        "rounded-lg",
        "p-6",
        "flex",
        "items-center",
        "gap-4",
        "shadow-[var(--shadow-2)]",
        "overflow-hidden",
        "stats-card",
        "text-[var(--text)]",
        "no-underline"
    );

    let icon_classes = classes!(
        "flex",
        "items-center",
        "justify-center",
        "w-12",
        "h-12",
        "shrink-0",
        "rounded-lg",
        "bg-[var(--surface-alt)]",
        "text-[var(--primary)]"
    );

    let value_classes =
        classes!("block", "text-3xl", "text-[var(--text)]", "font-bold", "leading-none");

    let label_classes = classes!("text-sm", "text-[var(--muted)]");

    let content = html! {
        <>
            <span class={icon_classes} aria-hidden="true">
                <Icon name={props.icon} size={28} />
            </span>
            <div class={classes!("flex", "flex-col", "gap-1", "items-start", "min-w-0")}>
                <strong class={value_classes}>{ props.value.clone() }</strong>
                <span class={label_classes}>{ props.label.clone() }</span>
            </div>
        </>
    };

    if let Some(route) = &props.route {
        html! {
            <div class={card_classes}>
                <Link<Route> to={route.clone()} classes="contents">
                    { content }
                </Link<Route>>
            </div>
        }
    } else {
        html! {
            <div class={card_classes} role="status" title={props.label.clone()}>
                { content }
            </div>
        }
    }
}
