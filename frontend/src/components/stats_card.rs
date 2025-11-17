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
    let content = html! {
        <>
            <span class="stats-card-icon" aria-hidden="true">
                <Icon name={props.icon} size={32} />
            </span>
            <strong class="stats-card-value">{ props.value.clone() }</strong>
            <span class="stats-card-label">{ props.label.clone() }</span>
        </>
    };

    if let Some(route) = &props.route {
        html! {
            <Link<Route> to={route.clone()} classes={classes!("stats-card")}>
                { content }
            </Link<Route>>
        }
    } else {
        html! {
            <div class="stats-card" role="status" title={props.label.clone()}>
                { content }
            </div>
        }
    }
}
