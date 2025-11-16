use yew::prelude::*;
use yew_router::prelude::*;

use crate::router::Route;

#[derive(Properties, PartialEq, Clone)]
pub struct StatsCardProps {
    pub icon: String,
    pub value: String,
    #[prop_or_default]
    pub route: Option<Route>,
}

#[function_component(StatsCard)]
pub fn stats_card(props: &StatsCardProps) -> Html {
    let content = html! {
        <>
            <span class="stats-card-icon" aria-hidden="true">{ props.icon.clone() }</span>
            <strong class="stats-card-value">{ props.value.clone() }</strong>
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
            <div class="stats-card" role="status">
                { content }
            </div>
        }
    }
}
