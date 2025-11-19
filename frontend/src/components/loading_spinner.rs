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
            <div
                style={spinner_style}
                class={classes!(
                    "w-[var(--spinner-size)]",
                    "h-[var(--spinner-size)]",
                    "rounded-full",
                    "border-[3px]",
                    "border-transparent",
                    "bg-[conic-gradient(var(--primary),transparent)]",
                    "[mask:radial-gradient(farthest-side,transparent_calc(100%-4px),#000_calc(100%-3px))]",
                    "animate-[spin_0.9s_linear_infinite]"
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
