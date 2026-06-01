use yew::prelude::*;

use crate::i18n::{current::pagination as t, fill_one};

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
        "w-10",
        "h-10",
        "rounded-full",
        "border-[1.5px]",
        "border-[#a1a1aa]",            // 明亮模式深边框（zinc-400）
        "dark:border-[var(--border)]", // 暗黑模式使用 CSS 变量
        "bg-[var(--surface)]",
        "text-sm",
        "font-semibold",
        "transition-all",
        "duration-150",
        "ease-[var(--ease-snap)]",
        "hover:bg-[var(--primary)]",
        "hover:text-white",
        "hover:border-[var(--primary)]",
        "hover:shadow-[0_2px_8px_rgba(0,120,212,0.3)]",
        "disabled:opacity-50",
        "disabled:cursor-not-allowed",
        "disabled:hover:bg-[var(--surface)]",
        "disabled:hover:text-[#27272a]",
        "disabled:hover:border-[#a1a1aa]",
        "disabled:hover:shadow-none"
    );

    // Prev/Next 按钮需要添加文字颜色
    let prev_classes = classes!(
        base_btn_classes.clone(),
        "text-[var(--text)]" // 使用 CSS 变量，自动适配 data-theme
    );

    let next_classes = classes!(base_btn_classes.clone(), "text-[var(--text)]");

    html! {
        <nav class="flex flex-wrap items-center gap-2" aria-label={t::ARIA_NAV}>
            <button
                type="button"
                class={prev_classes}
                disabled={prev_disabled}
                onclick={prev_onclick}
                aria-label={t::ARIA_PREV}
            >
                {"<"}
            </button>
            <div class={classes!("flex", "flex-wrap", "items-center", "gap-2")}>
                { for slots.into_iter().map(|slot| match slot {
                    PageSlot::Page(page) => {
                        let page_classes = if page == current_page {
                            classes!(
                                base_btn_classes.clone(),
                                "!bg-[var(--primary)]",
                                "!text-white",
                                "!border-[var(--primary)]",
                                "shadow-[var(--shadow-2)]",
                                "cursor-default",
                                "pointer-events-none"
                            )
                        } else {
                            classes!(
                                base_btn_classes.clone(),
                                "text-[var(--text)]"
                            )
                        };
                        let onclick = {
                            let on_page_change = on_page_change.clone();
                            Callback::from(move |_| on_page_change.emit(page))
                        };

                        html! {
                            <button
                                key={format!("page-{page}")}
                                type="button"
                                class={page_classes.clone()}
                                aria-label={fill_one(t::ARIA_GOTO_PAGE_TEMPLATE, page)}
                                aria-current={if page == current_page {
                                    Some(AttrValue::from("page"))
                                } else {
                                    None
                                }}
                                onclick={onclick}
                            >
                                { page }
                            </button>
                        }
                    }
                    PageSlot::Ellipsis(id) => {
                        let ellipsis_classes = classes!(
                            "inline-flex",
                            "items-center",
                            "justify-center",
                            "w-10",
                            "h-10",
                            "rounded-full",
                            "text-sm",
                            "text-[var(--muted)]",
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
                aria-label={t::ARIA_NEXT}
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
