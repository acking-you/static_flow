use gloo_timers::callback::Timeout;
use web_sys::TouchEvent;
use yew::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TooltipPosition {
    Top,
    Bottom,
    Left,
    Right,
}

#[derive(Properties, PartialEq)]
pub struct TooltipProps {
    pub text: String,

    #[prop_or(TooltipPosition::Top)]
    pub position: TooltipPosition,

    #[prop_or_default]
    pub children: Children,

    #[prop_or_default]
    pub class: Classes,
}

#[function_component(Tooltip)]
pub fn tooltip(props: &TooltipProps) -> Html {
    let TooltipProps {
        text,
        position,
        children,
        class,
    } = props;

    let visible = use_state(|| false);
    let touch_timeout = use_mut_ref(|| None::<Timeout>);

    // 桌面端：hover 显示（300ms 延迟）
    let on_mouse_enter = {
        let visible = visible.clone();
        let touch_timeout = touch_timeout.clone();
        Callback::from(move |_: MouseEvent| {
            // 清除可能存在的触摸延迟
            touch_timeout.borrow_mut().take();

            visible.set(true);
        })
    };

    let on_mouse_leave = {
        let visible = visible.clone();
        Callback::from(move |_: MouseEvent| {
            visible.set(false);
        })
    };

    // 移动端：长按 300ms 显示
    let on_touch_start = {
        let visible = visible.clone();
        let touch_timeout = touch_timeout.clone();
        Callback::from(move |_: TouchEvent| {
            let visible = visible.clone();
            let timeout = Timeout::new(300, move || {
                visible.set(true);
            });
            *touch_timeout.borrow_mut() = Some(timeout);
        })
    };

    let on_touch_end = {
        let visible = visible.clone();
        let touch_timeout = touch_timeout.clone();
        Callback::from(move |_: TouchEvent| {
            // 清除长按计时器
            touch_timeout.borrow_mut().take();

            // 隐藏 tooltip
            visible.set(false);
        })
    };

    let on_touch_cancel = {
        let visible = visible.clone();
        let touch_timeout = touch_timeout.clone();
        Callback::from(move |_: TouchEvent| {
            touch_timeout.borrow_mut().take();
            visible.set(false);
        })
    };

    let (position_classes, visible_transforms) = match position {
        TooltipPosition::Top => (
            classes!("bottom-[calc(100%+8px)]", "left-1/2", "-translate-x-1/2", "translate-y-1"),
            classes!("translate-y-0"),
        ),
        TooltipPosition::Bottom => (
            classes!("top-[calc(100%+8px)]", "left-1/2", "-translate-x-1/2", "-translate-y-1"),
            classes!("translate-y-0"),
        ),
        TooltipPosition::Left => (
            classes!("right-[calc(100%+8px)]", "top-1/2", "-translate-y-1/2", "translate-x-1"),
            classes!("translate-x-0"),
        ),
        TooltipPosition::Right => (
            classes!("left-[calc(100%+8px)]", "top-1/2", "-translate-y-1/2", "-translate-x-1"),
            classes!("translate-x-0"),
        ),
    };

    let tooltip_class = classes!(
        "absolute",
        "z-[999]",
        "px-3",
        "py-2",
        "text-[0.8125rem]",
        "font-medium",
        "leading-snug",
        "whitespace-nowrap",
        "bg-[var(--text)]",
        "text-[var(--bg)]",
        "rounded-md",
        "pointer-events-none",
        "opacity-0",
        "shadow-[0_4px_12px_rgba(0,0,0,0.15)]",
        "transition-all",
        "duration-200",
        "ease-in-out",
        position_classes,
        class.clone(),
        if *visible { classes!("opacity-100", visible_transforms) } else { Classes::new() }
    );

    html! {
        <div
            class={classes!("relative", "inline-flex")}
            onmouseenter={on_mouse_enter}
            onmouseleave={on_mouse_leave}
            ontouchstart={on_touch_start}
            ontouchend={on_touch_end}
            ontouchcancel={on_touch_cancel}
        >
            { for children.iter() }
            <div class={tooltip_class} role="tooltip">
                { text }
            </div>
        </div>
    }
}

/// 带 Tooltip 的 Icon 按钮组件 - 最常用的组合
#[derive(Properties, PartialEq)]
pub struct TooltipIconButtonProps {
    pub icon: crate::components::icons::IconName,
    pub tooltip: String,

    #[prop_or(24)]
    pub size: u32,

    #[prop_or(TooltipPosition::Top)]
    pub position: TooltipPosition,

    #[prop_or_default]
    pub onclick: Callback<MouseEvent>,

    #[prop_or_default]
    pub class: Classes,

    #[prop_or_default]
    pub disabled: bool,
}

#[function_component(TooltipIconButton)]
pub fn tooltip_icon_button(props: &TooltipIconButtonProps) -> Html {
    use crate::components::icons::IconButton;

    let TooltipIconButtonProps {
        icon,
        tooltip,
        size,
        position,
        onclick,
        class,
        disabled,
    } = props;

    html! {
        <Tooltip text={tooltip.clone()} position={*position}>
            <IconButton
                icon={*icon}
                size={*size}
                onclick={onclick}
                class={class.clone()}
                disabled={*disabled}
            />
        </Tooltip>
    }
}
