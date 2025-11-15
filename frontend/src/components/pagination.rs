use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct PaginationProps {
    pub current_page: usize,
    pub total_pages: usize,
    pub on_page_change: Callback<usize>,
}

enum PageSlot {
    Page(usize),
    Ellipsis(&'static str),
}

#[function_component(Pagination)]
pub fn pagination(props: &PaginationProps) -> Html {
    if props.total_pages <= 1 {
        return Html::default();
    }

    let total_pages = props.total_pages;
    let current_page = props.current_page.clamp(1, total_pages);
    let slots = visible_slots(current_page, total_pages);
    let on_page_change = props.on_page_change.clone();

    let prev_disabled = current_page <= 1;
    let next_disabled = current_page >= total_pages;

    let prev_onclick = {
        let on_page_change = on_page_change.clone();
        Callback::from(move |_| {
            if current_page > 1 {
                on_page_change.emit(current_page - 1);
            }
        })
    };

    let next_onclick = {
        let on_page_change = on_page_change.clone();
        Callback::from(move |_| {
            if current_page < total_pages {
                on_page_change.emit(current_page + 1);
            }
        })
    };

    let mut prev_classes = classes!("pagination-link");
    if prev_disabled {
        prev_classes.push("disabled");
    }

    let mut next_classes = classes!("pagination-link");
    if next_disabled {
        next_classes.push("disabled");
    }

    html! {
        <nav class="flex flex-wrap items-center gap-3" aria-label="分页">
            <button
                type="button"
                class={prev_classes}
                disabled={prev_disabled}
                onclick={prev_onclick}
                aria-label="上一页"
            >
                {"<"}
            </button>
            <div class="pagination-list">
                { for slots.into_iter().map(|slot| match slot {
                    PageSlot::Page(page) => {
                        let mut page_classes = classes!("pagination-link");
                        if page == current_page {
                            page_classes.push("active");
                        }
                        let onclick = {
                            let on_page_change = on_page_change.clone();
                            Callback::from(move |_| on_page_change.emit(page))
                        };

                        html! {
                            <button
                                key={format!("page-{page}")}
                                type="button"
                                class={page_classes.clone()}
                                aria-label={format!("跳转到第 {page} 页")}
                                aria-current={if page == current_page {
                                    Some(AttrValue::from("page"))
                                } else {
                                    None
                                }}
                                disabled={page == current_page}
                                onclick={onclick}
                            >
                                { page }
                            </button>
                        }
                    }
                    PageSlot::Ellipsis(id) => html! {
                        <span
                            key={format!("ellipsis-{id}-{current_page}")}
                            class="pagination-link disabled select-none"
                            aria-hidden="true"
                        >
                            {"..."}
                        </span>
                    }
                }) }
            </div>
            <button
                type="button"
                class={next_classes}
                disabled={next_disabled}
                onclick={next_onclick}
                aria-label="下一页"
            >
                {">"}
            </button>
        </nav>
    }
}

fn visible_slots(current: usize, total: usize) -> Vec<PageSlot> {
    if total <= 7 {
        return (1..=total).map(PageSlot::Page).collect();
    }

    let mut slots = Vec::new();
    slots.push(PageSlot::Page(1));

    let mut start = current.saturating_sub(2).max(2);
    let mut end = (current + 2).min(total - 1);

    if current <= 3 {
        start = 2;
        end = 5;
    } else if current + 2 >= total {
        start = total.saturating_sub(4).max(2);
        end = total - 1;
    }

    if start > 2 {
        slots.push(PageSlot::Ellipsis("left"));
    }

    for page in start..=end {
        slots.push(PageSlot::Page(page));
    }

    if end < total - 1 {
        slots.push(PageSlot::Ellipsis("right"));
    }

    slots.push(PageSlot::Page(total));

    slots
}
