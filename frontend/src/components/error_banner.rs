use yew::prelude::*;
use yew::use_effect_with;
use yew_hooks::prelude::use_timeout;

#[derive(Properties, PartialEq)]
pub struct ErrorBannerProps {
    pub message: String,
    #[prop_or_default]
    pub on_close: Option<Callback<()>>,
    #[prop_or(true)]
    pub auto_dismiss: bool,
}

#[function_component(ErrorBanner)]
pub fn error_banner(props: &ErrorBannerProps) -> Html {
    let is_open = use_state(|| true);

    let dismiss = {
        let is_open = is_open.clone();
        let on_close = props.on_close.clone();
        Callback::from(move |_| {
            if !*is_open {
                return;
            }
            is_open.set(false);
            if let Some(cb) = on_close.as_ref() {
                cb.emit(());
            }
        })
    };

    let auto_timeout = {
        let dismiss = dismiss.clone();
        use_timeout(move || dismiss.emit(()), if props.auto_dismiss { 3000 } else { 0 })
    };

    {
        let is_open = is_open.clone();
        use_effect_with(props.message.clone(), move |_| {
            is_open.set(true);
        });
    }

    {
        let auto_timeout = auto_timeout.clone();
        use_effect_with(
            (*is_open, props.auto_dismiss, props.message.clone()),
            move |(visible, auto_dismiss, _message)| {
                if *auto_dismiss && *visible {
                    auto_timeout.reset();
                } else {
                    auto_timeout.cancel();
                }
            },
        );
    }

    if props.message.trim().is_empty() {
        return Html::default();
    }

    let mut wrapper_classes = classes!(
        "error-banner",
        "flex",
        "items-start",
        "gap-3",
        "rounded-2xl",
        "px-5",
        "py-4",
        "text-sm",
        "shadow-xl",
        "transition-all",
        "duration-300",
        "ease-out",
        "overflow-hidden",
        "w-full",
        "max-w-2xl"
    );

    if *is_open {
        wrapper_classes.push("opacity-100");
        wrapper_classes.push("translate-y-0");
        wrapper_classes.push("scale-100");
        wrapper_classes.push("max-h-48");
    } else {
        wrapper_classes.push("opacity-0");
        wrapper_classes.push("-translate-y-2");
        wrapper_classes.push("scale-95");
        wrapper_classes.push("pointer-events-none");
        wrapper_classes.push("max-h-0");
    }

    let close_button = {
        let dismiss = dismiss.clone();
        Callback::from(move |_| dismiss.emit(()))
    };

    html! {
        <div class={wrapper_classes} role="alert" aria-live="assertive">
            <span class="text-2xl" aria-hidden="true">{"⚠️"}</span>
            <div class="flex-1 space-y-1">
                <p class="font-semibold text-base">{"发生错误"}</p>
                <p>{ props.message.clone() }</p>
            </div>
            <button
                type="button"
                class={classes!(
                    "ml-4",
                    "inline-flex",
                    "h-8",
                    "w-8",
                    "items-center",
                    "justify-center",
                    "rounded-full",
                    "bg-transparent",
                    "text-lg",
                    "transition",
                    "duration-200",
                    "hover:bg-black/10",
                    "dark:hover:bg-white/15"
                )}
                aria-label="关闭错误提示"
                onclick={close_button}
            >
                {"×"}
            </button>
        </div>
    }
}
