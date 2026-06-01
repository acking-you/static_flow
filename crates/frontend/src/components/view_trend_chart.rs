use yew::prelude::*;

use crate::api::ArticleViewPoint;

#[derive(Properties, Clone, PartialEq)]
pub struct ViewTrendChartProps {
    pub points: Vec<ArticleViewPoint>,

    #[prop_or_default]
    pub empty_text: String,

    #[prop_or_default]
    pub class: Classes,
}

#[function_component(ViewTrendChart)]
pub fn view_trend_chart(props: &ViewTrendChartProps) -> Html {
    let hovered_index = use_state(|| None::<usize>);

    if props.points.is_empty() {
        return html! {
            <div class={classes!(
                "rounded-xl",
                "border",
                "border-[var(--border)]",
                "bg-[var(--surface)]",
                "px-4",
                "py-8",
                "text-center",
                "text-sm",
                "text-[var(--muted)]",
                props.class.clone()
            )}>
                {
                    if props.empty_text.is_empty() {
                        "No trend data"
                    } else {
                        props.empty_text.as_str()
                    }
                }
            </div>
        };
    }

    let width = 760.0_f64;
    let height = 250.0_f64;
    let padding_left = 44.0_f64;
    let padding_right = 16.0_f64;
    let padding_top = 16.0_f64;
    let padding_bottom = 36.0_f64;
    let plot_width = width - padding_left - padding_right;
    let plot_height = height - padding_top - padding_bottom;

    let max_value = props
        .points
        .iter()
        .map(|point| point.views)
        .max()
        .unwrap_or(1)
        .max(1) as f64;

    let points_len = props.points.len();
    let x_step =
        if points_len > 1 { plot_width / (points_len.saturating_sub(1) as f64) } else { 0.0 };

    let point_positions = props
        .points
        .iter()
        .enumerate()
        .map(|(index, point)| {
            let x = if points_len > 1 {
                padding_left + (index as f64) * x_step
            } else {
                padding_left + plot_width / 2.0
            };
            let ratio = (point.views as f64) / max_value;
            let y = padding_top + (1.0 - ratio) * plot_height;
            (x, y, point)
        })
        .collect::<Vec<_>>();

    let polyline_points = point_positions
        .iter()
        .map(|(x, y, _)| format!("{x:.2},{y:.2}"))
        .collect::<Vec<_>>()
        .join(" ");

    let mut x_label_indices = vec![0, points_len.saturating_sub(1)];
    if points_len > 4 {
        x_label_indices.push(points_len / 2);
    }
    x_label_indices.sort_unstable();
    x_label_indices.dedup();

    let hover_point = (*hovered_index).and_then(|index| {
        point_positions
            .get(index)
            .map(|(x, y, point)| (*x, *y, point.key.clone(), point.views))
    });

    html! {
        <div class={classes!(
            "rounded-xl",
            "border",
            "border-[var(--border)]",
            "bg-[var(--surface)]",
            "px-3",
            "py-3",
            "overflow-x-auto",
            props.class.clone()
        )}>
            <svg
                viewBox={format!("0 0 {width} {height}")}
                class={classes!("w-full", "min-w-[520px]")}
                role="img"
                aria-label="view trend chart"
            >
                // horizontal grid lines
                { for (0..=4).map(|idx| {
                    let ratio = idx as f64 / 4.0;
                    let y = padding_top + ratio * plot_height;
                    html! {
                        <line
                            x1={padding_left.to_string()}
                            y1={format!("{y:.2}")}
                            x2={(padding_left + plot_width).to_string()}
                            y2={format!("{y:.2}")}
                            stroke="rgba(128,128,128,0.18)"
                            stroke-width="1"
                        />
                    }
                }) }

                <polyline
                    fill="none"
                    stroke="var(--primary)"
                    stroke-width="2.5"
                    points={polyline_points}
                />

                {
                    if let Some((x, y, key, views)) = hover_point.as_ref() {
                        let tooltip_width = 158.0_f64;
                        let tooltip_height = 56.0_f64;
                        let mut tooltip_x = *x + 12.0;
                        if tooltip_x + tooltip_width > width - 4.0 {
                            tooltip_x = *x - tooltip_width - 12.0;
                        }
                        if tooltip_x < 4.0 {
                            tooltip_x = 4.0;
                        }

                        let mut tooltip_y = *y - tooltip_height - 12.0;
                        if tooltip_y < 4.0 {
                            tooltip_y = *y + 12.0;
                        }
                        if tooltip_y + tooltip_height > height - 4.0 {
                            tooltip_y = height - tooltip_height - 4.0;
                        }

                        html! {
                            <>
                                <line
                                    x1={format!("{x:.2}")}
                                    y1={padding_top.to_string()}
                                    x2={format!("{x:.2}")}
                                    y2={(padding_top + plot_height).to_string()}
                                    stroke="var(--primary)"
                                    stroke-dasharray="4 4"
                                    stroke-width="1.3"
                                    opacity="0.52"
                                />
                                <line
                                    x1={padding_left.to_string()}
                                    y1={format!("{y:.2}")}
                                    x2={(padding_left + plot_width).to_string()}
                                    y2={format!("{y:.2}")}
                                    stroke="var(--primary)"
                                    stroke-dasharray="4 4"
                                    stroke-width="1.2"
                                    opacity="0.42"
                                />
                                <g style="pointer-events:none;">
                                    <rect
                                        x={format!("{tooltip_x:.2}")}
                                        y={format!("{tooltip_y:.2}")}
                                        width={tooltip_width.to_string()}
                                        height={tooltip_height.to_string()}
                                        rx="8"
                                        fill="var(--surface)"
                                        stroke="var(--border)"
                                        stroke-width="1"
                                        style="filter: drop-shadow(0 6px 20px rgba(0,0,0,0.18));"
                                    />
                                    <text
                                        x={format!("{:.2}", tooltip_x + 10.0)}
                                        y={format!("{:.2}", tooltip_y + 22.0)}
                                        fill="var(--text)"
                                        style="font-size: 11.5px; font-weight: 600;"
                                    >
                                        { format!("x: {key}") }
                                    </text>
                                    <text
                                        x={format!("{:.2}", tooltip_x + 10.0)}
                                        y={format!("{:.2}", tooltip_y + 40.0)}
                                        fill="var(--text)"
                                        style="font-size: 11.5px; font-weight: 600;"
                                    >
                                        { format!("y: {views}") }
                                    </text>
                                </g>
                            </>
                        }
                    } else {
                        html! {}
                    }
                }

                { for point_positions.iter().enumerate().map(|(index, (x, y, point))| {
                    let hovered_for_enter = hovered_index.clone();
                    let hovered_for_leave = hovered_index.clone();
                    let on_mouse_enter = Callback::from(move |_| hovered_for_enter.set(Some(index)));
                    let on_mouse_leave = Callback::from(move |_| hovered_for_leave.set(None));
                    let is_active = *hovered_index == Some(index);
                    html! {
                        <g onmouseenter={on_mouse_enter} onmouseleave={on_mouse_leave}>
                            <circle
                                cx={format!("{x:.2}")}
                                cy={format!("{y:.2}")}
                                r={if is_active { "11" } else { "0" }}
                                fill="var(--primary)"
                                opacity={if is_active { "0.22" } else { "0" }}
                                style="transition: r 140ms ease, opacity 140ms ease;"
                            />
                            <circle
                                cx={format!("{x:.2}")}
                                cy={format!("{y:.2}")}
                                r={if is_active { "5.8" } else { "3.5" }}
                                fill="var(--primary)"
                                stroke={if is_active { "white" } else { "transparent" }}
                                stroke-width={if is_active { "1.8" } else { "0" }}
                                style="transition: r 140ms ease, stroke-width 140ms ease, opacity 140ms ease;"
                                cursor="pointer"
                            />
                            <title>{ format!("{}: {}", point.key, point.views) }</title>
                        </g>
                    }
                }) }

                { for x_label_indices.iter().map(|index| {
                    let (x, _, point) = point_positions[*index];
                    html! {
                        <text
                            x={format!("{x:.2}")}
                            y={(height - 10.0).to_string()}
                            text-anchor="middle"
                            fill="var(--muted)"
                            style="font-size: 11px;"
                        >
                            { point.key.clone() }
                        </text>
                    }
                }) }

                <text
                    x={(padding_left - 8.0).to_string()}
                    y={(padding_top + 2.0).to_string()}
                    text-anchor="end"
                    fill="var(--muted)"
                    style="font-size: 11px;"
                >
                    { max_value.round().to_string() }
                </text>
                <text
                    x={(padding_left - 8.0).to_string()}
                    y={(padding_top + plot_height + 4.0).to_string()}
                    text-anchor="end"
                    fill="var(--muted)"
                    style="font-size: 11px;"
                >
                    { "0" }
                </text>
            </svg>
        </div>
    }
}
