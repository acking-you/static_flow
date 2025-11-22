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
            SpinnerSize::Small => 16,
            SpinnerSize::Medium => 32,
            SpinnerSize::Large => 48,
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
            aria-label="Loading"
        >
            <div
                style={spinner_style}
                class={classes!(
                    "w-[var(--spinner-size)]",
                    "h-[var(--spinner-size)]",
                    "rounded-full",
                    "border-[3px]",
                    "border-[var(--surface-alt)]",
                    "border-t-[var(--primary)]",
                    "animate-spin"
                )}
            />
            <span class={classes!("sr-only")}>{ "加载中..." }</span>
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
                    "bg-[var(--acrylic-bg-light)]",
                    "backdrop-blur",
                    "[backdrop-filter:saturate(var(--acrylic-saturate))]",
                    "dark:bg-black/40"
                )}
            >
                { spinner }
            </div>
        }
    } else {
        html! { spinner }
    }
}
