use yew::prelude::*;

/// Lucide Icons - 清晰的线性 icon 系统
/// SVG 路径来自 https://lucide.dev
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IconName {
    // Navigation
    ChevronLeft,
    ChevronRight,
    ArrowLeft,
    ArrowUp,
    Home,

    // Content
    FileText,
    BookOpen,
    List,

    // Actions
    Search,
    X,
    Menu,

    // Categories
    Tag,
    Hash,
    Folder,
}

impl IconName {
    /// 获取 Lucide icon 的 SVG path 数据
    pub fn path(&self) -> &'static str {
        match self {
            IconName::ChevronLeft => "m15 18-6-6 6-6",
            IconName::ChevronRight => "m9 18 6-6-6-6",
            IconName::ArrowLeft => "M12 19l-7-7 7-7M5 12h14",
            IconName::ArrowUp => "m18 15-6-6-6 6",
            IconName::Home => "M3 9l9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z",

            IconName::FileText => {
                "M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8zM14 2v6h6M16 13H8M16 \
                 17H8M10 9H8"
            },
            IconName::BookOpen => {
                "M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2zM22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z"
            },
            IconName::List => "M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01",

            IconName::Search => "m21 21-6-6m2-5a7 7 0 1 1-14 0 7 7 0 0 1 14 0z",
            IconName::X => "M18 6 6 18M6 6l12 12",
            IconName::Menu => "M4 12h16M4 6h16M4 18h16",

            IconName::Tag => "M12 2l8 8-10 10L2 12l10-10zM7 7h.01",
            IconName::Hash => "M4 9h16M4 15h16M10 3L8 21M16 3l-2 18",
            IconName::Folder => {
                "M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 \
                 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2z"
            },
        }
    }

    /// 是否需要填充（某些 icon 有多个 path）
    pub fn needs_fill(&self) -> bool {
        matches!(self, IconName::Home | IconName::Folder)
    }
}

#[derive(Properties, PartialEq)]
pub struct IconProps {
    pub name: IconName,

    #[prop_or(24)]
    pub size: u32,

    #[prop_or_else(|| "currentColor".to_string())]
    pub color: String,

    #[prop_or_default]
    pub class: Classes,
}

#[function_component(Icon)]
pub fn icon(props: &IconProps) -> Html {
    let IconProps {
        name,
        size,
        color,
        class,
    } = props;

    let stroke_width = if *size <= 16 { 2.5 } else { 2.0 };
    let fill = if name.needs_fill() { "none" } else { "none" };

    html! {
        <svg
            class={classes!(
                "inline-flex",
                "items-center",
                "justify-center",
                "shrink-0",
                "transition-all",
                "duration-200",
                "ease-[var(--ease-spring)]",
                class.clone()
            )}
            width={size.to_string()}
            height={size.to_string()}
            viewBox="0 0 24 24"
            fill={fill}
            stroke={color.clone()}
            stroke-width={stroke_width.to_string()}
            stroke-linecap="round"
            stroke-linejoin="round"
            xmlns="http://www.w3.org/2000/svg"
        >
            <path d={name.path()} />
        </svg>
    }
}

/// Icon 按钮组件 - 结合 Icon + 圆形背景
#[derive(Properties, PartialEq)]
pub struct IconButtonProps {
    pub icon: IconName,

    #[prop_or(24)]
    pub size: u32,

    #[prop_or_default]
    pub onclick: Callback<MouseEvent>,

    #[prop_or_default]
    pub class: Classes,

    #[prop_or_default]
    pub disabled: bool,
}

#[function_component(IconButton)]
pub fn icon_button(props: &IconButtonProps) -> Html {
    let IconButtonProps {
        icon,
        size,
        onclick,
        class,
        disabled,
    } = props;

    let button_class = classes!(
        "relative",
        "inline-flex",
        "items-center",
        "justify-center",
        "w-[var(--hit-size)]",
        "h-[var(--hit-size)]",
        "min-w-[44px]",
        "min-h-[44px]",
        "rounded-lg",
        "border",
        "border-[var(--border)]",
        "bg-[var(--surface)]",
        "text-[var(--text)]",
        "shadow-[var(--shadow-sm)]",
        "transition-all",
        "duration-100",
        "ease-[var(--ease-snap)]",
        "hover:bg-[var(--surface-alt)]",
        "hover:text-[var(--primary)]",
        "hover:shadow-[var(--shadow-2)]",
        "active:bg-[var(--surface-alt)]",
        "active:shadow-[var(--shadow-sm)]",
        "disabled:opacity-40",
        "disabled:cursor-not-allowed",
        "disabled:hover:text-[var(--text)]",
        "disabled:hover:bg-[var(--surface)]",
        class.clone()
    );

    html! {
        <button
            class={button_class}
            onclick={onclick}
            disabled={*disabled}
            type="button"
        >
            <Icon name={*icon} size={*size} />
        </button>
    }
}
