use js_sys::Reflect;
use wasm_bindgen::{JsCast, JsValue};
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
        if let Some(window) = web_sys::window() {
            // 如果已存在目录，则直接切换展开/收起
            if let Some(existing) = window
                .document()
                .and_then(|d| d.query_selector(".article-toc").ok())
                .flatten()
            {
                let class_list = existing.class_list();
                if class_list.contains("mobile-open") {
                    let _ = class_list.remove_1("mobile-open");
                } else {
                    let _ = class_list.add_1("mobile-open");
                }
                return;
            }

            // 若目录未生成，尝试调用全局 generateTOC 后再展开
            if let Ok(gen) = Reflect::get(&window, &JsValue::from_str("generateTOC")) {
                if let Ok(func) = gen.dyn_into::<js_sys::Function>() {
                    let _ = func.call0(&window);
                }
            }

            if let Some(toc) = window
                .document()
                .and_then(|d| d.query_selector(".article-toc").ok())
                .flatten()
            {
                let class_list = toc.class_list();
                if class_list.contains("mobile-open") {
                    let _ = class_list.remove_1("mobile-open");
                } else {
                    let _ = class_list.add_1("mobile-open");
                }
            }
        }
    });

    html! {
        <div class={classes!(
            "mobile-toc-fab",
            "fixed", "left-8", "bottom-8", "z-[99]",
            "w-12", "h-12",
            "bg-[var(--primary)]", "text-white",
            "rounded-full",
            "items-center", "justify-center",
            "shadow-[0_4px_12px_rgba(29,158,216,0.4)]",
            "transition-all", "duration-300",
            "active:scale-95",
            "max-md:left-6", "max-md:bottom-6", "max-md:w-11", "max-md:h-11", "max-md:text-base"
        )}>
            <TooltipIconButton
                icon={IconName::List}
                tooltip="目录"
                position={TooltipPosition::Top}
                onclick={on_click}
                size={20}
            />
        </div>
    }
}
