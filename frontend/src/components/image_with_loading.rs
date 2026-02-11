use yew::prelude::*;

#[derive(Properties, PartialEq, Clone)]
pub struct ImageWithLoadingProps {
    pub src: String,
    pub alt: String,
    #[prop_or_default]
    pub class: Classes,
    #[prop_or_default]
    pub container_class: Classes,
    #[prop_or_default]
    pub loading: Option<AttrValue>,
    #[prop_or_default]
    pub decoding: Option<AttrValue>,
    #[prop_or_default]
    pub onclick: Option<Callback<MouseEvent>>,
}

#[function_component(ImageWithLoading)]
pub fn image_with_loading(props: &ImageWithLoadingProps) -> Html {
    let image_loaded = use_state(|| false);

    let on_image_load = {
        let image_loaded = image_loaded.clone();
        Callback::from(move |_: Event| image_loaded.set(true))
    };
    let on_image_error = {
        let image_loaded = image_loaded.clone();
        Callback::from(move |_: Event| image_loaded.set(true))
    };

    let container_classes = classes!(
        props.container_class.clone(),
        "relative",
        "overflow-hidden",
        if !*image_loaded { "bg-[var(--surface-alt)]" } else { "" }
    );

    let image_classes = classes!(
        props.class.clone(),
        "transition-opacity",
        "duration-500",
        if *image_loaded { "opacity-100" } else { "opacity-0" }
    );

    html! {
        <div class={container_classes} onclick={props.onclick.clone()}>
            {
                if !*image_loaded {
                    html! {
                        <div class={classes!(
                            "absolute",
                            "inset-0",
                            "bg-gradient-to-br",
                            "from-[var(--surface-alt)]",
                            "to-[var(--surface)]",
                            "animate-pulse",
                            "pointer-events-none"
                        )} />
                    }
                } else {
                    html! {}
                }
            }
            <img
                src={props.src.clone()}
                alt={props.alt.clone()}
                class={image_classes}
                loading={props.loading.clone().unwrap_or(AttrValue::from("lazy"))}
                decoding={props.decoding.clone()}
                onload={on_image_load}
                onerror={on_image_error}
            />
        </div>
    }
}
