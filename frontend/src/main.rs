mod api;
mod components;
mod config;
pub mod hooks;
mod i18n;
mod models;
mod pages;
mod router;
mod utils;

use yew::prelude::*;

#[function_component(App)]
fn app() -> Html {
    html! {
        <>
            <router::AppRouter />
        </>
    }
}

fn main() {
    yew::Renderer::<App>::new().render();
}
