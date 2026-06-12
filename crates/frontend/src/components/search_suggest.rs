//! Header search input with a debounced article-suggestion dropdown.
//!
//! Controlled by the parent (`value` / `on_change`), so the header's search
//! and clear buttons keep working off the same state. The component owns the
//! suggestion lifecycle: 300ms debounce, top-5 keyword results, full
//! keyboard navigation (arrows / Enter / Escape), outside-click dismissal,
//! and `listbox` / `aria-activedescendant` semantics.

use gloo_events::EventListener;
use gloo_timers::callback::Timeout;
use gloo_utils::document;
use wasm_bindgen::JsCast;
use web_sys::{HtmlInputElement, KeyboardEvent, Node};
use yew::prelude::*;
use yew_router::prelude::*;

use crate::{api, api::SearchResult, i18n::current::common as common_text, router::Route};

/// Debounce window before firing the suggestion query.
const SUGGEST_DEBOUNCE_MS: u32 = 300;
/// Minimum characters before suggesting.
const SUGGEST_MIN_CHARS: usize = 2;
/// Maximum suggestions shown.
const SUGGEST_LIMIT: usize = 5;

/// Props for [`SearchSuggest`].
#[derive(Properties, PartialEq)]
pub struct SearchSuggestProps {
    /// Current query text (owned by the parent).
    pub value: AttrValue,
    /// Emits every input change so the parent stays the source of truth.
    pub on_change: Callback<String>,
    /// Invoked with the trimmed query when the user submits free text.
    pub on_submit: Callback<String>,
    /// Extra classes merged onto the input element.
    #[prop_or_default]
    pub input_class: Classes,
}

/// Search input + suggestion dropdown (desktop header).
#[function_component(SearchSuggest)]
pub fn search_suggest(props: &SearchSuggestProps) -> Html {
    let container_ref = use_node_ref();
    let suggestions = use_state(Vec::<SearchResult>::new);
    let open = use_state(|| false);
    let highlighted = use_state(|| None::<usize>);
    let debounce = use_mut_ref(|| None::<Timeout>);
    let request_seq = use_mut_ref(|| 0_u64);
    let navigator = use_navigator();

    // Debounced suggestion fetch when the (parent-owned) value changes.
    {
        let suggestions = suggestions.clone();
        let open = open.clone();
        let highlighted = highlighted.clone();
        let debounce = debounce.clone();
        let request_seq = request_seq.clone();
        use_effect_with(props.value.clone(), move |value| {
            let query = value.trim().to_string();
            debounce.borrow_mut().take();
            if query.len() < SUGGEST_MIN_CHARS {
                suggestions.set(Vec::new());
                open.set(false);
                highlighted.set(None);
            } else {
                let request_id = {
                    let mut seq = request_seq.borrow_mut();
                    *seq += 1;
                    *seq
                };
                let suggestions = suggestions.clone();
                let open = open.clone();
                let highlighted = highlighted.clone();
                let request_seq = request_seq.clone();
                let timeout = Timeout::new(SUGGEST_DEBOUNCE_MS, move || {
                    wasm_bindgen_futures::spawn_local(async move {
                        if let Ok(results) = api::search_articles(&query, Some(SUGGEST_LIMIT)).await
                        {
                            if *request_seq.borrow() != request_id {
                                return;
                            }
                            open.set(!results.is_empty());
                            highlighted.set(None);
                            suggestions.set(results);
                        }
                    });
                });
                *debounce.borrow_mut() = Some(timeout);
            }
            || ()
        });
    }

    // Outside click closes the dropdown.
    {
        let container_ref = container_ref.clone();
        let open = open.clone();
        use_effect_with((), move |_| {
            let listener = EventListener::new(&document(), "mousedown", move |event| {
                let Some(target) = event.target().and_then(|t| t.dyn_into::<Node>().ok()) else {
                    return;
                };
                let inside = container_ref
                    .get()
                    .is_some_and(|container| container.contains(Some(&target)));
                if !inside {
                    open.set(false);
                }
            });
            move || drop(listener)
        });
    }

    let select_suggestion = {
        let navigator = navigator.clone();
        let open = open.clone();
        let suggestions = suggestions.clone();
        Callback::from(move |index: usize| {
            if let (Some(navigator), Some(result)) = (navigator.clone(), suggestions.get(index)) {
                open.set(false);
                navigator.push(&Route::ArticleDetail {
                    id: result.id.clone(),
                });
            }
        })
    };

    let oninput = {
        let on_change = props.on_change.clone();
        Callback::from(move |event: InputEvent| {
            let input: HtmlInputElement = event.target_unchecked_into();
            on_change.emit(input.value());
        })
    };

    let onkeydown = {
        let open = open.clone();
        let highlighted = highlighted.clone();
        let suggestions = suggestions.clone();
        let select_suggestion = select_suggestion.clone();
        let on_submit = props.on_submit.clone();
        let value = props.value.clone();
        Callback::from(move |event: KeyboardEvent| {
            let count = suggestions.len();
            match event.key().as_str() {
                "ArrowDown" if *open && count > 0 => {
                    event.prevent_default();
                    highlighted.set(Some(highlighted.map_or(0, |i| (i + 1) % count)));
                },
                "ArrowUp" if *open && count > 0 => {
                    event.prevent_default();
                    highlighted
                        .set(Some(highlighted.map_or(count - 1, |i| (i + count - 1) % count)));
                },
                "Enter" => {
                    if *open {
                        if let Some(index) = *highlighted {
                            event.prevent_default();
                            select_suggestion.emit(index);
                            return;
                        }
                    }
                    let query = value.trim().to_string();
                    if !query.is_empty() {
                        open.set(false);
                        on_submit.emit(query);
                    }
                },
                "Escape" => {
                    open.set(false);
                    highlighted.set(None);
                },
                _ => {},
            }
        })
    };

    let active_descendant = highlighted.map(|index| format!("search-suggest-{index}"));

    html! {
        <div ref={container_ref} class={classes!("search-suggest")}>
            <input
                type="text"
                role="combobox"
                aria-expanded={if *open { "true" } else { "false" }}
                aria-controls="search-suggest-list"
                aria-activedescendant={active_descendant}
                aria-autocomplete="list"
                placeholder={common_text::SEARCH_PLACEHOLDER}
                value={props.value.clone()}
                {oninput}
                {onkeydown}
                class={props.input_class.clone()}
            />
            if *open && !suggestions.is_empty() {
                <ul
                    id="search-suggest-list"
                    class={classes!("search-suggest-list", "acrylic-surface")}
                    role="listbox"
                >
                    { for suggestions.iter().enumerate().map(|(index, result)| {
                        let is_active = *highlighted == Some(index);
                        let onclick = {
                            let select_suggestion = select_suggestion.clone();
                            Callback::from(move |_: MouseEvent| select_suggestion.emit(index))
                        };
                        html! {
                            <li
                                id={format!("search-suggest-{index}")}
                                class={classes!(
                                    "search-suggest-item",
                                    is_active.then_some("search-suggest-item--active")
                                )}
                                role="option"
                                aria-selected={if is_active { "true" } else { "false" }}
                                onmousedown={Callback::from(|e: MouseEvent| e.prevent_default())}
                                {onclick}
                            >
                                <span class={classes!("search-suggest-title")}>
                                    { &result.title }
                                </span>
                                <span class={classes!("search-suggest-meta")}>
                                    { &result.category }
                                </span>
                            </li>
                        }
                    }) }
                </ul>
            }
        </div>
    }
}
