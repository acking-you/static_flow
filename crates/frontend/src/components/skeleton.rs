//! Loading skeletons that reserve final layout space (zero CLS).
//!
//! Use these instead of full-page spinners wherever the loaded layout is
//! predictable: card grids, list rows, and the article reading column.
//! Purely presentational — the shimmer lives in CSS and pauses under
//! `prefers-reduced-motion`.

use yew::prelude::*;

/// Props for [`SkeletonBlock`].
#[allow(
    dead_code,
    reason = "The standalone block is part of the skeleton kit's public surface; page-specific \
              skeletons compose it during the feedback-adoption phase."
)]
#[derive(Properties, PartialEq)]
pub struct SkeletonBlockProps {
    /// Extra classes controlling the block's size/shape (width, height,
    /// rounding); the base `.skeleton` class supplies color and shimmer.
    #[prop_or_default]
    pub class: Classes,
}

/// One shimmering placeholder rectangle.
#[function_component(SkeletonBlock)]
pub fn skeleton_block(props: &SkeletonBlockProps) -> Html {
    html! { <div class={classes!("skeleton", props.class.clone())} aria-hidden="true"></div> }
}

/// Props for [`SkeletonText`].
#[derive(Properties, PartialEq)]
pub struct SkeletonTextProps {
    /// Number of text lines to render; the last line is shortened.
    #[prop_or(3)]
    pub lines: u8,
}

/// A paragraph-shaped stack of shimmer lines.
#[function_component(SkeletonText)]
pub fn skeleton_text(props: &SkeletonTextProps) -> Html {
    let lines = props.lines.max(1);
    html! {
        <div class={classes!("flex", "flex-col", "gap-2")} aria-hidden="true">
            { for (0..lines).map(|i| {
                let width = if i + 1 == lines { "w-3/5" } else { "w-full" };
                html! { <div class={classes!("skeleton", "h-4", "rounded", width)}></div> }
            }) }
        </div>
    }
}

/// Card-shaped skeleton matching the article/list card footprint:
/// cover area, meta line, title, and two body lines.
#[function_component(SkeletonCard)]
pub fn skeleton_card() -> Html {
    html! {
        <div
            class={classes!(
                "rounded-xl", "border", "border-[var(--border)]",
                "bg-[var(--surface)]", "overflow-hidden"
            )}
            aria-hidden="true"
        >
            <div class={classes!("skeleton", "h-40", "w-full", "rounded-none")}></div>
            <div class={classes!("p-4", "flex", "flex-col", "gap-3")}>
                <div class={classes!("skeleton", "h-3", "w-24", "rounded")}></div>
                <div class={classes!("skeleton", "h-5", "w-4/5", "rounded")}></div>
                <SkeletonText lines={2} />
            </div>
        </div>
    }
}

/// Reading-column skeleton for the article detail page: hero, title,
/// meta row, and paragraph blocks sized to the article measure.
#[function_component(SkeletonArticle)]
pub fn skeleton_article() -> Html {
    html! {
        <div class={classes!("flex", "flex-col", "gap-6", "w-full")} aria-hidden="true">
            <div class={classes!("skeleton", "h-56", "w-full", "rounded-xl")}></div>
            <div class={classes!("skeleton", "h-8", "w-3/4", "rounded")}></div>
            <div class={classes!("skeleton", "h-4", "w-40", "rounded")}></div>
            <SkeletonText lines={4} />
            <SkeletonText lines={5} />
            <SkeletonText lines={3} />
        </div>
    }
}
