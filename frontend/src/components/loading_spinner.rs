use yew::prelude::*;

#[derive(Clone, PartialEq)]
pub enum SpinnerSize {
    Small,
    Medium,
    Large,
}

impl SpinnerSize {
    fn dimension(&self) -> u32 {
        match self {
            SpinnerSize::Small => 24,
            SpinnerSize::Medium => 40,
            SpinnerSize::Large => 56,
        }
    }
}

#[derive(Properties, PartialEq)]
pub struct LoadingSpinnerProps {
    #[prop_or(SpinnerSize::Medium)]
    pub size: SpinnerSize,
    #[prop_or(false)]
    pub fullscreen: bool,
}

#[function_component(LoadingSpinner)]
pub fn loading_spinner(props: &LoadingSpinnerProps) -> Html {
    let spinner_style = format!("--spinner-size:{}px;", props.size.dimension());

    let spinner = html! {
        <div
            class={classes!("flex", "items-center", "justify-center", "p-6")}
            role="status"
            aria-live="polite"
            aria-busy="true"
        >
            <div class="loading-spinner-ring" style={spinner_style}></div>
            <span class="sr-only">{ "加载中..." }</span>
        </div>
    };

    if props.fullscreen {
        html! {
            <div
                class={classes!(
                    "loading-spinner-overlay",
                    "fixed",
                    "inset-0",
                    "z-40",
                    "flex",
                    "items-center",
                    "justify-center",
                    "bg-black/30",
                    "dark:bg-black/60"
                )}
            >
                { spinner }
            </div>
        }
    } else {
        html! { spinner }
    }
}
