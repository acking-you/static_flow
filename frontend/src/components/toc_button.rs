use yew::prelude::*;

use crate::components::{
    icons::IconName,
    tooltip::{TooltipIconButton, TooltipPosition},
};

/// 目录移动端切换按钮组件
/// 修复原 bug：将 JS 创建的按钮改为 Yew 组件，路由切换时自动清理
#[function_component(TocButton)]
pub fn toc_button() -> Html {
    let on_click = Callback::from(|_: MouseEvent| {
        // 触发目录显示（与 JS 生成的目录交互）
        if let Some(toc) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.query_selector(".article-toc").ok())
            .flatten()
        {
            let _ = toc.class_list().add_1("mobile-open");
        }
    });

    html! {
        <TooltipIconButton
            icon={IconName::List}
            tooltip="目录"
            position={TooltipPosition::Top}
            onclick={on_click}
            class="toc-mobile-toggle"
            size={20}
        />
    }
}
