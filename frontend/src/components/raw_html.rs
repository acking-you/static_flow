use web_sys::Element;
use yew::prelude::*;

#[derive(Properties, Clone, PartialEq)]
pub struct RawHtmlProps {
    pub html: AttrValue,
    #[prop_or_default]
    pub class: Classes,
}

/// Render trusted HTML into a host element without letting Yew diff its
/// children. This avoids VDOM/removeChild panics when external JS
/// enhances/mutates the DOM.
#[function_component(RawHtml)]
pub fn raw_html(props: &RawHtmlProps) -> Html {
    let host_ref = use_node_ref();

    {
        let host_ref = host_ref.clone();
        let html = props.html.clone();
        use_effect_with(html.clone(), move |next_html| {
            if let Some(host) = host_ref.cast::<Element>() {
                host.set_inner_html(next_html.as_str());
            }
            || ()
        });
    }

    html! {
        <div ref={host_ref} class={props.class.clone()} />
    }
}
