use js_sys::Date;
use wasm_bindgen::{prelude::*, JsCast};
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;

#[derive(Clone, PartialEq)]
pub struct Preset {
    pub label: String,
    pub hours: f64,
}

#[derive(Properties, PartialEq)]
pub struct DateRangePickerProps {
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub on_change: Callback<(Option<i64>, Option<i64>)>,
    #[prop_or_default]
    pub presets: Vec<Preset>,
}

fn default_presets() -> Vec<Preset> {
    vec![
        Preset {
            label: "1h".to_string(),
            hours: 1.0,
        },
        Preset {
            label: "24h".to_string(),
            hours: 24.0,
        },
        Preset {
            label: "7d".to_string(),
            hours: 24.0 * 7.0,
        },
        Preset {
            label: "30d".to_string(),
            hours: 24.0 * 30.0,
        },
    ]
}

fn ms_to_local_parts(ms: i64) -> (u32, u32, u32, u32, u32) {
    let date = Date::new(&JsValue::from_f64(ms as f64));
    (date.get_full_year(), date.get_month(), date.get_date(), date.get_hours(), date.get_minutes())
}

fn parts_to_ms(year: u32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
    let date = Date::new_0();
    date.set_full_year(year);
    date.set_month(month);
    date.set_date(day);
    date.set_hours(hour);
    date.set_minutes(minute);
    date.set_seconds(0);
    date.set_milliseconds(0);
    date.get_time() as i64
}

fn days_in_month(year: u32, month: u32) -> u32 {
    let date = Date::new_0();
    date.set_full_year(year);
    date.set_month(month + 1);
    date.set_date(0);
    date.get_date()
}

fn first_weekday_of_month(year: u32, month: u32) -> u32 {
    let date = Date::new_0();
    date.set_full_year(year);
    date.set_month(month);
    date.set_date(1);
    date.get_day()
}

fn format_short_date(year: u32, month: u32, day: u32, hour: u32, minute: u32) -> String {
    format!("{:04}-{:02}-{:02} {:02}:{:02}", year, month + 1, day, hour, minute)
}

fn format_range_display(start_ms: Option<i64>, end_ms: Option<i64>) -> String {
    match (start_ms, end_ms) {
        (None, None) => "选择时间范围".to_string(),
        (Some(s), None) => {
            let (y, m, d, h, min) = ms_to_local_parts(s);
            format!("{} — 现在", format_short_date(y, m, d, h, min))
        },
        (None, Some(e)) => {
            let (y, m, d, h, min) = ms_to_local_parts(e);
            format!("起始 — {}", format_short_date(y, m, d, h, min))
        },
        (Some(s), Some(e)) => {
            let (sy, sm, sd, sh, smin) = ms_to_local_parts(s);
            let (ey, em, ed, eh, emin) = ms_to_local_parts(e);
            if sy == ey && sm == em && sd == ed {
                format!(
                    "{:04}-{:02}-{:02} {:02}:{:02}—{:02}:{:02}",
                    sy,
                    sm + 1,
                    sd,
                    sh,
                    smin,
                    eh,
                    emin
                )
            } else {
                format!(
                    "{} → {}",
                    format_short_date(sy, sm, sd, sh, smin),
                    format_short_date(ey, em, ed, eh, emin)
                )
            }
        },
    }
}

#[function_component(DateRangePicker)]
pub fn date_range_picker(props: &DateRangePickerProps) -> Html {
    let open = use_state(|| false);
    let now = Date::new_0();
    let now_year = now.get_full_year();
    let now_month = now.get_month();

    let initial_view_year = props
        .start_ms
        .or(props.end_ms)
        .map(|ms| ms_to_local_parts(ms).0)
        .unwrap_or(now_year);
    let initial_view_month = props
        .start_ms
        .or(props.end_ms)
        .map(|ms| ms_to_local_parts(ms).1)
        .unwrap_or(now_month);

    let view_year = use_state(|| initial_view_year);
    let view_month = use_state(|| initial_view_month);

    let start_hour = use_state(|| {
        props
            .start_ms
            .map(|ms| ms_to_local_parts(ms).3)
            .unwrap_or(0)
    });
    let start_minute = use_state(|| {
        props
            .start_ms
            .map(|ms| ms_to_local_parts(ms).4)
            .unwrap_or(0)
    });
    let end_hour = use_state(|| props.end_ms.map(|ms| ms_to_local_parts(ms).3).unwrap_or(23));
    let end_minute = use_state(|| props.end_ms.map(|ms| ms_to_local_parts(ms).4).unwrap_or(59));

    let draft_start_day = use_state(|| -> Option<(u32, u32, u32)> {
        props.start_ms.map(|ms| {
            let (y, m, d, _, _) = ms_to_local_parts(ms);
            (y, m, d)
        })
    });
    let draft_end_day = use_state(|| -> Option<(u32, u32, u32)> {
        props.end_ms.map(|ms| {
            let (y, m, d, _, _) = ms_to_local_parts(ms);
            (y, m, d)
        })
    });

    let presets = if props.presets.is_empty() { default_presets() } else { props.presets.clone() };

    // Sync draft state from props when popup opens or props change.
    {
        let open_clone = open.clone();
        let view_year = view_year.clone();
        let view_month = view_month.clone();
        let draft_start_day = draft_start_day.clone();
        let draft_end_day = draft_end_day.clone();
        let start_hour = start_hour.clone();
        let start_minute = start_minute.clone();
        let end_hour = end_hour.clone();
        let end_minute = end_minute.clone();
        let start_ms = props.start_ms;
        let end_ms = props.end_ms;
        use_effect_with(*open_clone, move |is_open| {
            if *is_open {
                match start_ms {
                    Some(ms) => {
                        let (y, m, d, h, min) = ms_to_local_parts(ms);
                        draft_start_day.set(Some((y, m, d)));
                        start_hour.set(h);
                        start_minute.set(min);
                        view_year.set(y);
                        view_month.set(m);
                    },
                    None => {
                        draft_start_day.set(None);
                        start_hour.set(0);
                        start_minute.set(0);
                    },
                }
                match end_ms {
                    Some(ms) => {
                        let (y, m, d, h, min) = ms_to_local_parts(ms);
                        draft_end_day.set(Some((y, m, d)));
                        end_hour.set(h);
                        end_minute.set(min);
                    },
                    None => {
                        draft_end_day.set(None);
                        end_hour.set(23);
                        end_minute.set(59);
                    },
                }
            }
            || ()
        });
    }

    // Click-outside detection: close popup when clicking outside.
    let panel_ref = use_node_ref();
    {
        let open = open.clone();
        let panel_ref = panel_ref.clone();
        use_effect_with(*open, move |is_open| {
            let cleanup: Box<dyn FnOnce()> = if *is_open {
                let document = web_sys::window().and_then(|w| w.document());
                if let Some(doc) = document {
                    let panel_ref = panel_ref.clone();
                    let open = open.clone();
                    let listener =
                        wasm_bindgen::closure::Closure::wrap(Box::new(move |evt: web_sys::Event| {
                            if let Some(panel) = panel_ref.get() {
                                if let Some(target) = evt
                                    .target()
                                    .and_then(|t| t.dyn_into::<web_sys::Node>().ok())
                                {
                                    if !panel.contains(Some(&target)) {
                                        open.set(false);
                                    }
                                }
                            }
                        })
                            as Box<dyn FnMut(web_sys::Event)>);
                    let _ = doc.add_event_listener_with_callback(
                        "mousedown",
                        listener.as_ref().unchecked_ref(),
                    );
                    let doc_clone = doc;
                    Box::new(move || {
                        let _ = doc_clone.remove_event_listener_with_callback(
                            "mousedown",
                            listener.as_ref().unchecked_ref(),
                        );
                        drop(listener);
                    })
                } else {
                    Box::new(|| {})
                }
            } else {
                Box::new(|| {})
            };
            cleanup
        });
    }

    let toggle_open = {
        let open = open.clone();
        Callback::from(move |_: MouseEvent| open.set(!*open))
    };

    let on_prev_month = {
        let view_year = view_year.clone();
        let view_month = view_month.clone();
        Callback::from(move |_: MouseEvent| {
            if *view_month == 0 {
                view_month.set(11);
                view_year.set(*view_year - 1);
            } else {
                view_month.set(*view_month - 1);
            }
        })
    };

    let on_next_month = {
        let view_year = view_year.clone();
        let view_month = view_month.clone();
        Callback::from(move |_: MouseEvent| {
            if *view_month == 11 {
                view_month.set(0);
                view_year.set(*view_year + 1);
            } else {
                view_month.set(*view_month + 1);
            }
        })
    };

    let on_year_change = {
        let view_year = view_year.clone();
        Callback::from(move |e: Event| {
            if let Some(select) = e.target_dyn_into::<HtmlSelectElement>() {
                if let Ok(year) = select.value().parse::<u32>() {
                    view_year.set(year);
                }
            }
        })
    };

    let on_month_change = {
        let view_month = view_month.clone();
        Callback::from(move |e: Event| {
            if let Some(select) = e.target_dyn_into::<HtmlSelectElement>() {
                if let Ok(month) = select.value().parse::<u32>() {
                    view_month.set(month.min(11));
                }
            }
        })
    };

    let on_today = {
        let view_year = view_year.clone();
        let view_month = view_month.clone();
        Callback::from(move |_: MouseEvent| {
            view_year.set(now_year);
            view_month.set(now_month);
        })
    };

    let on_day_click = {
        let draft_start_day = draft_start_day.clone();
        let draft_end_day = draft_end_day.clone();
        Callback::from(move |(year, month, day): (u32, u32, u32)| {
            if draft_start_day.is_none() || draft_end_day.is_some() {
                draft_start_day.set(Some((year, month, day)));
                draft_end_day.set(None);
            } else if let Some(start) = *draft_start_day {
                let start_ms_cmp = parts_to_ms(start.0, start.1, start.2, 0, 0);
                let click_ms_cmp = parts_to_ms(year, month, day, 0, 0);
                if click_ms_cmp >= start_ms_cmp {
                    draft_end_day.set(Some((year, month, day)));
                } else {
                    draft_start_day.set(Some((year, month, day)));
                    draft_end_day.set(None);
                }
            }
        })
    };

    let on_apply = {
        let on_change = props.on_change.clone();
        let draft_start_day = draft_start_day.clone();
        let draft_end_day = draft_end_day.clone();
        let start_hour = start_hour.clone();
        let start_minute = start_minute.clone();
        let end_hour = end_hour.clone();
        let end_minute = end_minute.clone();
        let open = open.clone();
        Callback::from(move |_: MouseEvent| {
            let s = (*draft_start_day)
                .map(|(y, m, d)| parts_to_ms(y, m, d, *start_hour, *start_minute));
            let e = (*draft_end_day).map(|(y, m, d)| parts_to_ms(y, m, d, *end_hour, *end_minute));
            on_change.emit((s, e));
            open.set(false);
        })
    };

    let on_clear = {
        let on_change = props.on_change.clone();
        let draft_start_day = draft_start_day.clone();
        let draft_end_day = draft_end_day.clone();
        let open = open.clone();
        Callback::from(move |_: MouseEvent| {
            draft_start_day.set(None);
            draft_end_day.set(None);
            on_change.emit((None, None));
            open.set(false);
        })
    };

    let on_preset_click = {
        let on_change = props.on_change.clone();
        let open = open.clone();
        move |hours: f64| {
            let on_change = on_change.clone();
            let open = open.clone();
            Callback::from(move |_: MouseEvent| {
                let now_ms = Date::new_0().get_time() as i64;
                let start = now_ms - (hours * 3_600_000.0) as i64;
                on_change.emit((Some(start), Some(now_ms)));
                open.set(false);
            })
        }
    };

    let year = *view_year;
    let month = *view_month;
    let total_days = days_in_month(year, month);
    let first_weekday = first_weekday_of_month(year, month);
    let month_names =
        ["1月", "2月", "3月", "4月", "5月", "6月", "7月", "8月", "9月", "10月", "11月", "12月"];

    let calendar_cells: Vec<Html> = {
        let mut cells: Vec<Html> = Vec::new();
        for _ in 0..first_weekday {
            cells.push(html! { <div class={classes!("h-8")}></div> });
        }
        for day in 1..=total_days {
            let is_start = (*draft_start_day) == Some((year, month, day));
            let is_end = (*draft_end_day) == Some((year, month, day));
            let in_range = match ((*draft_start_day), (*draft_end_day)) {
                (Some(s), Some(e)) => {
                    let d_ms = parts_to_ms(year, month, day, 0, 0);
                    let s_ms = parts_to_ms(s.0, s.1, s.2, 0, 0);
                    let e_ms = parts_to_ms(e.0, e.1, e.2, 0, 0);
                    d_ms > s_ms && d_ms < e_ms
                },
                _ => false,
            };
            let is_today = year == now_year && month == now_month && day == now.get_date();
            let on_day_click = on_day_click.clone();

            let mut style = String::new();
            let mut classes_str = "h-8 rounded-md text-xs font-medium flex items-center \
                                   justify-center cursor-pointer transition-all select-none"
                .to_string();

            if is_start || is_end {
                style.push_str(
                    "background:var(--primary);color:#fff;box-shadow:0 2px 6px \
                     rgba(var(--primary-rgb),0.35);",
                );
            } else if in_range {
                style.push_str("background:rgba(var(--primary-rgb),0.12);color:var(--primary);");
            } else if is_today {
                classes_str.push_str(" hover:bg-[var(--surface-alt)]");
                style.push_str(
                    "border:1px solid rgba(var(--primary-rgb),0.4);color:var(--primary);",
                );
            } else {
                classes_str.push_str(" hover:bg-[var(--surface-alt)] text-[var(--text)]");
            }

            cells.push(html! {
                <button
                    type="button"
                    class={classes_str}
                    style={style}
                    onclick={Callback::from(move |_: MouseEvent| on_day_click.emit((year, month, day)))}
                >
                    { day }
                </button>
            });
        }
        cells
    };

    // Year options: from 5 years ago to next year, plus include current view year
    // if outside.
    let year_options: Vec<u32> = {
        let mut years: Vec<u32> = (now_year.saturating_sub(5)..=now_year + 1).collect();
        if !years.contains(&year) {
            years.push(year);
            years.sort_unstable();
            years.dedup();
        }
        years
    };

    let draft_start_text = (*draft_start_day)
        .map(|(y, m, d)| {
            format!("{:04}-{:02}-{:02} {:02}:{:02}", y, m + 1, d, *start_hour, *start_minute)
        })
        .unwrap_or_else(|| "未选择".to_string());
    let draft_end_text = (*draft_end_day)
        .map(|(y, m, d)| {
            format!("{:04}-{:02}-{:02} {:02}:{:02}", y, m + 1, d, *end_hour, *end_minute)
        })
        .unwrap_or_else(|| "未选择".to_string());

    let has_value = props.start_ms.is_some() || props.end_ms.is_some();
    let trigger_classes = classes!(
        "rounded-lg",
        "border",
        "bg-[var(--surface)]",
        "px-3",
        "py-1.5",
        "text-xs",
        "font-mono",
        "cursor-pointer",
        "transition-colors",
        "whitespace-nowrap",
        "inline-flex",
        "items-center",
        "gap-2",
        if has_value {
            "border-[var(--primary)] text-[var(--primary)]"
        } else {
            "border-[var(--border)] text-[var(--text)] hover:border-[var(--primary)]/50"
        },
    );

    html! {
        <div class={classes!("relative", "inline-block")}>
            <button
                type="button"
                class={trigger_classes}
                onclick={toggle_open}
            >
                <i class={classes!("fas", "fa-calendar-days", "text-[var(--muted)]")}></i>
                <span>{ format_range_display(props.start_ms, props.end_ms) }</span>
                <i class={classes!(
                    "fas", "fa-caret-down", "text-[var(--muted)]", "text-[10px]",
                    "transition-transform", "duration-150",
                    if *open { "rotate-180" } else { "rotate-0" },
                )}></i>
            </button>
            <div
                ref={panel_ref}
                class={classes!(
                    "absolute", "top-full", "left-0", "z-[80]", "mt-2",
                    "rounded-xl", "border", "border-[var(--border)]",
                    "bg-[var(--surface)]",
                    "shadow-[0_12px_40px_rgba(0,0,0,0.18)]",
                    "p-4", "w-[320px]",
                    "transition-all", "duration-150", "ease-out", "origin-top",
                    if *open {
                        "opacity-100 scale-100 translate-y-0 pointer-events-auto"
                    } else {
                        "opacity-0 scale-95 -translate-y-1 pointer-events-none"
                    },
                )}
            >
                    // Selected range summary
                    <div class={classes!("mb-3", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-2.5", "py-2", "text-[11px]", "leading-tight")}>
                        <div class={classes!("flex", "items-center", "justify-between", "gap-2")}>
                            <span class={classes!("text-[var(--muted)]", "font-medium")}>{ "起" }</span>
                            <span class={classes!("font-mono", "text-[var(--text)]")}>{ &draft_start_text }</span>
                        </div>
                        <div class={classes!("mt-1", "flex", "items-center", "justify-between", "gap-2")}>
                            <span class={classes!("text-[var(--muted)]", "font-medium")}>{ "止" }</span>
                            <span class={classes!("font-mono", "text-[var(--text)]")}>{ &draft_end_text }</span>
                        </div>
                    </div>

                    // Year + month + nav row
                    <div class={classes!("mb-2", "flex", "items-center", "gap-1.5")}>
                        <button
                            type="button"
                            class={classes!("rounded-md", "border", "border-[var(--border)]", "bg-[var(--surface)]", "hover:bg-[var(--surface-alt)]", "w-7", "h-7", "flex", "items-center", "justify-center", "text-xs", "cursor-pointer", "transition-colors")}
                            onclick={on_prev_month}
                            title="上一月"
                        >
                            <i class={classes!("fas", "fa-chevron-left", "text-[10px]")}></i>
                        </button>
                        <select
                            class={classes!("flex-1", "min-w-0", "rounded-md", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-1.5", "py-1", "text-xs", "cursor-pointer")}
                            onchange={on_year_change.clone()}
                        >
                            { for year_options.iter().map(|y| html! {
                                <option value={y.to_string()} selected={*y == year}>{ format!("{}年", y) }</option>
                            }) }
                        </select>
                        <select
                            class={classes!("flex-1", "min-w-0", "rounded-md", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-1.5", "py-1", "text-xs", "cursor-pointer")}
                            onchange={on_month_change.clone()}
                        >
                            { for (0u32..12).map(|m| html! {
                                <option value={m.to_string()} selected={m == month}>{ month_names[m as usize] }</option>
                            }) }
                        </select>
                        <button
                            type="button"
                            class={classes!("rounded-md", "border", "border-[var(--border)]", "bg-[var(--surface)]", "hover:bg-[var(--surface-alt)]", "w-7", "h-7", "flex", "items-center", "justify-center", "text-xs", "cursor-pointer", "transition-colors")}
                            onclick={on_next_month}
                            title="下一月"
                        >
                            <i class={classes!("fas", "fa-chevron-right", "text-[10px]")}></i>
                        </button>
                        <button
                            type="button"
                            class={classes!("rounded-md", "border", "border-[var(--border)]", "bg-[var(--surface)]", "hover:bg-[var(--surface-alt)]", "px-2", "h-7", "flex", "items-center", "justify-center", "text-[10px]", "cursor-pointer", "transition-colors", "text-[var(--muted)]")}
                            onclick={on_today}
                            title="跳到本月"
                        >
                            { "今" }
                        </button>
                    </div>

                    // Weekday headers
                    <div class={classes!("grid", "grid-cols-7", "gap-1", "mb-1")}>
                        { for ["日","一","二","三","四","五","六"].iter().map(|d| html! {
                            <div class={classes!("h-6", "flex", "items-center", "justify-center", "text-[10px]", "text-[var(--muted)]", "font-semibold")}>{ *d }</div>
                        }) }
                    </div>

                    // Calendar grid
                    <div class={classes!("grid", "grid-cols-7", "gap-1")}>
                        { for calendar_cells.into_iter() }
                    </div>

                    // Time inputs
                    <div class={classes!("mt-3", "grid", "grid-cols-2", "gap-2")}>
                        <div class={classes!("rounded-md", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-2", "py-1.5")}>
                            <div class={classes!("text-[10px]", "text-[var(--muted)]", "font-semibold", "mb-1")}>{ "开始时间" }</div>
                            <div class={classes!("flex", "items-center", "gap-1")}>
                                <input
                                    type="number"
                                    min="0" max="23"
                                    class={classes!("w-12", "rounded", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-1", "py-0.5", "text-center", "text-xs", "font-mono")}
                                    value={format!("{:02}", *start_hour)}
                                    onchange={{
                                        let start_hour = start_hour.clone();
                                        Callback::from(move |e: Event| {
                                            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                                                start_hour.set(input.value().parse::<u32>().unwrap_or(0).min(23));
                                            }
                                        })
                                    }}
                                />
                                <span class={classes!("text-xs", "font-bold", "text-[var(--muted)]")}>{ ":" }</span>
                                <input
                                    type="number"
                                    min="0" max="59"
                                    class={classes!("w-12", "rounded", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-1", "py-0.5", "text-center", "text-xs", "font-mono")}
                                    value={format!("{:02}", *start_minute)}
                                    onchange={{
                                        let start_minute = start_minute.clone();
                                        Callback::from(move |e: Event| {
                                            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                                                start_minute.set(input.value().parse::<u32>().unwrap_or(0).min(59));
                                            }
                                        })
                                    }}
                                />
                            </div>
                        </div>
                        <div class={classes!("rounded-md", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-2", "py-1.5")}>
                            <div class={classes!("text-[10px]", "text-[var(--muted)]", "font-semibold", "mb-1")}>{ "结束时间" }</div>
                            <div class={classes!("flex", "items-center", "gap-1")}>
                                <input
                                    type="number"
                                    min="0" max="23"
                                    class={classes!("w-12", "rounded", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-1", "py-0.5", "text-center", "text-xs", "font-mono")}
                                    value={format!("{:02}", *end_hour)}
                                    onchange={{
                                        let end_hour = end_hour.clone();
                                        Callback::from(move |e: Event| {
                                            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                                                end_hour.set(input.value().parse::<u32>().unwrap_or(23).min(23));
                                            }
                                        })
                                    }}
                                />
                                <span class={classes!("text-xs", "font-bold", "text-[var(--muted)]")}>{ ":" }</span>
                                <input
                                    type="number"
                                    min="0" max="59"
                                    class={classes!("w-12", "rounded", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-1", "py-0.5", "text-center", "text-xs", "font-mono")}
                                    value={format!("{:02}", *end_minute)}
                                    onchange={{
                                        let end_minute = end_minute.clone();
                                        Callback::from(move |e: Event| {
                                            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                                                end_minute.set(input.value().parse::<u32>().unwrap_or(59).min(59));
                                            }
                                        })
                                    }}
                                />
                            </div>
                        </div>
                    </div>

                    // Presets
                    <div class={classes!("mt-3", "flex", "items-center", "gap-1.5", "flex-wrap")}>
                        <span class={classes!("text-[10px]", "text-[var(--muted)]", "font-semibold", "mr-0.5")}>{ "快捷:" }</span>
                        { for presets.iter().map(|preset| {
                            let cb = on_preset_click(preset.hours);
                            html! {
                                <button type="button" class={classes!("rounded-md", "border", "border-[var(--border)]", "bg-[var(--surface)]", "hover:bg-[var(--primary)]/10", "hover:border-[var(--primary)]/50", "hover:text-[var(--primary)]", "px-2", "py-0.5", "text-[11px]", "cursor-pointer", "transition-colors")} onclick={cb}>
                                    { &preset.label }
                                </button>
                            }
                        }) }
                    </div>

                    // Apply / Clear
                    <div class={classes!("mt-3", "pt-3", "border-t", "border-[var(--border)]", "flex", "items-center", "justify-end", "gap-2")}>
                        <button
                            type="button"
                            class={classes!("rounded-md", "border", "border-[var(--border)]", "bg-[var(--surface)]", "hover:bg-[var(--surface-alt)]", "px-3", "py-1", "text-xs", "cursor-pointer", "transition-colors", "text-[var(--muted)]")}
                            onclick={on_clear}
                        >
                            { "清空" }
                        </button>
                        <button
                            type="button"
                            class={classes!("rounded-md", "px-4", "py-1", "text-xs", "font-semibold", "cursor-pointer", "transition-colors", "text-white", "border", "border-[var(--primary)]")}
                            style="background:var(--primary);"
                            onclick={on_apply}
                        >
                            { "确定" }
                        </button>
                    </div>
                </div>
        </div>
    }
}
