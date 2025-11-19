use yew::prelude::*;
use yew_router::prelude::*;

use crate::{
    components::icons::{Icon, IconName},
    router::Route,
};

#[derive(Properties, PartialEq, Clone)]
pub struct StatsCardProps {
    pub icon: IconName,
    pub value: String,
    pub label: String, // 添加文字标签用于tooltip
    #[prop_or_default]
    pub route: Option<Route>,
}

#[function_component(StatsCard)]
pub fn stats_card(props: &StatsCardProps) -> Html {
    let card_classes = classes!(
        "group",
        "relative",
        "bg-[var(--surface)]",
        "border",
        "border-[var(--border)]",
        "rounded-[12px]",
        "p-5",
        "text-center",
        "flex",
        "flex-col",
        "items-center",
        "gap-[0.35rem]",
        "shadow-[var(--shadow-sm)]",
        "transition-all",
        "duration-[250ms]",
        "ease-[var(--ease-spring)]",
        "hover:-translate-y-1",
        "hover:shadow-[var(--shadow-lg)]",
        "text-[var(--text)]",
        "no-underline"
    );

    let icon_classes = classes!(
        "flex",
        "items-center",
        "justify-center",
        "w-16",
        "h-16",
        "mb-3",
        "rounded-full",
        "bg-gradient-to-br",
        "from-[var(--primary)]",
        "to-[var(--link)]",
        "text-white",
        "shadow-[0_8px_20px_rgba(29,158,216,0.25)]",
        "transition-transform",
        "duration-300",
        "ease-[var(--ease-spring)]",
        "group-hover:scale-110",
        "group-hover:rotate-3"
    );

    let value_classes =
        classes!("block", "text-[1.75rem]", "text-[var(--primary)]", "font-bold", "leading-none");

    let label_classes = classes!(
        "absolute",
        "left-1/2",
        "-bottom-8",
        "-translate-x-1/2",
        "px-3",
        "py-1.5",
        "text-xs",
        "font-medium",
        "whitespace-nowrap",
        "bg-[var(--text)]",
        "text-[var(--bg)]",
        "rounded-md",
        "opacity-0",
        "pointer-events-none",
        "transition-all",
        "duration-200",
        "ease-in-out",
        "shadow-[0_4px_12px_rgba(0,0,0,0.15)]",
        "group-hover:opacity-100",
        "group-hover:translate-y-1"
    );

    let content = html! {
        <>
            <span class={icon_classes} aria-hidden="true">
                <Icon name={props.icon} size={32} />
            </span>
            <strong class={value_classes}>{ props.value.clone() }</strong>
            <span class={label_classes}>{ props.label.clone() }</span>
        </>
    };

    if let Some(route) = &props.route {
        html! {
            <Link<Route> to={route.clone()} classes={card_classes.clone()}>
                { content }
            </Link<Route>>
        }
    } else {
        html! {
            <div class={card_classes} role="status" title={props.label.clone()}>
                { content }
            </div>
        }
    }
}
