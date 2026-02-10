use web_sys::{ScrollBehavior, ScrollToOptions};
use yew::prelude::*;
use yew_router::prelude::use_location;

/// Paginate arbitrary vectors inside a component.
///
/// # Example
/// ```rust
/// use crate::hooks::use_pagination;
/// use crate::components::pagination::Pagination;
///
/// #[function_component(HomePage)]
/// fn home_page() -> Html {
///     let articles = use_state(|| Vec::<static_flow_shared::ArticleListItem>::new());
///     let (visible, current_page, total_pages, go_to_page) =
///         use_pagination((*articles).clone(), 6);
///
///     html! {
///         <>
///             { for visible.iter().map(|article| html! { <div>{ &article.title }</div> }) }
///             <Pagination
///                 current_page={current_page}
///                 total_pages={total_pages}
///                 on_page_change={go_to_page.clone()}
///             />
///         </>
///     }
/// }
/// ```
#[hook]
pub fn use_pagination<T>(
    items: Vec<T>,
    items_per_page: usize,
) -> (Vec<T>, usize, usize, Callback<usize>)
where
    T: Clone + PartialEq + 'static,
{
    let per_page = items_per_page.max(1);
    let total_pages = calculate_total_pages(items.len(), per_page);
    let current_page = use_state(|| 1usize);

    {
        let current_page = current_page.clone();
        use_effect_with(total_pages, move |total| {
            let safe_page = clamp_page(*current_page, *total);
            if safe_page != *current_page {
                current_page.set(safe_page);
            }
            || ()
        });
    }

    let memoized_slice = {
        let current_snapshot = *current_page;
        use_memo((items, current_snapshot, per_page), move |(items, page, per_page)| {
            if items.is_empty() {
                return Vec::new();
            }

            let total_pages = calculate_total_pages(items.len(), *per_page);
            let safe_page = clamp_page(*page, total_pages);
            let start = (*per_page).saturating_mul(safe_page - 1);
            let end = usize::min(start + *per_page, items.len());
            items[start..end].to_vec()
        })
    };

    let visible_items = (*memoized_slice).clone();
    let visible_page = clamp_page(*current_page, total_pages);
    let go_to_page = {
        let current_page = current_page.clone();
        Callback::from(move |page: usize| {
            let next_page = clamp_page(page, total_pages);
            if next_page != *current_page {
                current_page.set(next_page);
            }
        })
    };

    (visible_items, visible_page, total_pages, go_to_page)
}

/// Automatically scroll the viewport to the top whenever the current route
/// changes.
///
/// Call this hook inside top-level pages (e.g. `HomePage`) to keep navigation
/// consistent: ```rust
/// #[function_component(HomePage)]
/// fn home_page() -> Html {
///     use crate::hooks::{use_pagination, use_scroll_to_top};
///     use_scroll_to_top();
///     let (items, page, total, go_to_page) = use_pagination(vec![1, 2, 3, 4],
/// 2);     html! { <div>{ format!("page {page}/{}", total) }</div> }
/// }
/// ```
#[hook]
pub fn use_scroll_to_top() {
    let location = use_location();

    use_effect_with(location, move |location| {
        if location.is_some() {
            scroll_window_to_top();
        }

        || ()
    });
}

fn scroll_window_to_top() {
    if let Some(window) = web_sys::window() {
        let options = ScrollToOptions::new();
        options.set_left(0.0);
        options.set_top(0.0);
        options.set_behavior(ScrollBehavior::Smooth);
        window.scroll_to_with_scroll_to_options(&options);
    }
}

fn clamp_page(page: usize, total_pages: usize) -> usize {
    page.max(1).min(total_pages)
}

fn calculate_total_pages(len: usize, per_page: usize) -> usize {
    if len == 0 {
        1
    } else {
        let numerator = len.saturating_add(per_page - 1);
        usize::max(numerator / per_page, 1)
    }
}
