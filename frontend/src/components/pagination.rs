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

    let base_btn_classes = classes!(
        "inline-flex",
        "items-center",
        "justify-center",
        "min-w-[2.5rem]",
        "h-10",
        "px-3",
        "rounded-lg",
        "border",
        "border-[var(--border)]",
        "bg-[rgba(var(--surface-rgb),0.95)]",
        "text-[var(--text)]",
        "text-sm",
        "font-semibold",
        "ring-1",
        "ring-[rgba(15,23,42,0.08)]",
        "dark:ring-[rgba(255,255,255,0.08)]",
        "shadow-sm",
        "transition-all",
        "duration-200",
        "ease-[var(--ease-spring)]",
        "hover:-translate-y-[1px]",
        "hover:shadow-[var(--shadow)]",
        "hover:border-[var(--primary)]",
        "hover:text-[var(--primary)]",
        "disabled:opacity-50",
        "disabled:cursor-not-allowed",
        "disabled:hover:translate-y-0",
        "disabled:hover:shadow-none"
    );

    let prev_classes = classes!(base_btn_classes.clone(), "min-w-[2.75rem]");

    let next_classes = classes!(base_btn_classes.clone(), "min-w-[2.75rem]");

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
            <div class={classes!("flex", "flex-wrap", "items-center", "gap-2")}>
                { for slots.into_iter().map(|slot| match slot {
                    PageSlot::Page(page) => {
                        let page_classes = classes!(
                            base_btn_classes.clone(),
                            "min-w-[2.75rem]",
                            if page == current_page {
                                "bg-[var(--primary)] text-white border-transparent ring-[rgba(var(--primary-rgb),0.45)] drop-shadow-[0_10px_25px_rgba(var(--primary-rgb),0.4)] cursor-default pointer-events-none"
                            } else {
                                ""
                            }
                        );
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
                    PageSlot::Ellipsis(id) => {
                        let ellipsis_classes = classes!(
                            base_btn_classes.clone(),
                            "select-none",
                            "cursor-default",
                            "opacity-60",
                            "pointer-events-none"
                        );
                        html! {
                            <span
                                key={format!("ellipsis-{id}-{current_page}")}
                                class={ellipsis_classes}
                                aria-hidden="true"
                            >
                                {"..."}
                            </span>
                        }
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
