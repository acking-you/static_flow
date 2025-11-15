use yew::prelude::*;

#[derive(Properties, PartialEq, Clone)]
pub struct StatsCardProps {
    pub icon: String,
    pub value: String,
    #[prop_or_default]
    pub href: Option<String>,
}

#[function_component(StatsCard)]
pub fn stats_card(props: &StatsCardProps) -> Html {
    let content = html! {
        <>
            <span class="stats-card-icon" aria-hidden="true">{ props.icon.clone() }</span>
            <strong class="stats-card-value">{ props.value.clone() }</strong>
        </>
    };

    if let Some(href) = &props.href {
        html! {
            <a class="stats-card" href={href.clone()}>
                { content }
            </a>
        }
    } else {
        html! {
            <div class="stats-card" role="status">
                { content }
            </div>
        }
    }
}
