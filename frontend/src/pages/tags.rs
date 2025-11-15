use yew::prelude::*;
use yew_router::prelude::Link;

use crate::router::Route;

#[function_component(TagsPage)]
pub fn tags_page() -> Html {
    let tag_stats = use_state(|| Vec::<crate::api::TagInfo>::new());

    {
        let tag_stats = tag_stats.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match crate::api::fetch_tags().await {
                    Ok(data) => tag_stats.set(data),
                    Err(e) => {
                        web_sys::console::error_1(&format!("Failed to fetch tags: {}", e).into());
                    }
                }
            });
            || ()
        });
    }

    let total_tags = tag_stats.len();
    let total_articles: usize = tag_stats.iter().map(|t| t.count).sum();
    let max_count = tag_stats
        .iter()
        .map(|t| t.count as f32)
        .fold(1.0, f32::max);

    html! {
        <main class="main tags-page">
            <div class="container">
                <section class="page-section">
                    <p class="page-kicker">{ "标签" }</p>
                    <h1 class="page-title">{ "标签索引" }</h1>
                    <p class="page-description">
                        { format!("汇总 {} 个标签，覆盖 {} 篇文章。点击任意标签将跳转到对应的标签详情页并展示时间线。", total_tags, total_articles) }
                    </p>
                </section>

                {
                    if tag_stats.is_empty() {
                        html! {
                            <p class="empty-hint">{ "暂无标签，敬请期待。" }</p>
                        }
                    } else {
                        html! {
                            <div class="tag-cloud" role="list" aria-label="标签云">
                                { for tag_stats.iter().map(|tag_info| {
                                    let weight = (tag_info.count as f32 / max_count).max(0.35);
                                    let style = format!("--tag-weight: {:.2}", weight);
                                    html! {
                                        <Link<Route>
                                            to={Route::TagDetail { tag: tag_info.name.clone() }}
                                            classes={classes!("tag-chip")}
                                        >
                                            <span class="tag-label" style={style}>{ &tag_info.name }</span>
                                            <span class="tag-count">{ format!("{} 篇", tag_info.count) }</span>
                                        </Link<Route>>
                                    }
                                }) }
                            </div>
                        }
                    }
                }
            </div>
        </main>
    }
}
