use yew::prelude::*;
use yew_router::prelude::Link;

use crate::router::Route;

#[function_component(CategoriesPage)]
pub fn categories_page() -> Html {
    let categories = use_state(|| Vec::<crate::api::CategoryInfo>::new());

    {
        let categories = categories.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match crate::api::fetch_categories().await {
                    Ok(data) => categories.set(data),
                    Err(e) => {
                        web_sys::console::error_1(&format!("Failed to fetch categories: {}", e).into());
                    }
                }
            });
            || ()
        });
    }

    html! {
        <main class="main categories-page">
            <div class="container">
                <section class="page-section">
                    <p class="page-kicker">{ "分类" }</p>
                    <h1 class="page-title">{ "知识图谱" }</h1>
                    <p class="page-description">
                        { format!("当前整理 {} 个分类，持续更新中。点击卡片跳转到分类详情页并查看文章时间线。", categories.len()) }
                    </p>
                </section>

                {
                    if categories.is_empty() {
                        html! {
                            <p class="empty-hint">{ "暂无分类" }</p>
                        }
                    } else {
                        html! {
                            <section class="category-grid" aria-label="分类列表">
                                { for categories.iter().map(|category| {
                                    html! {
                                        <Link<Route>
                                            to={Route::CategoryDetail { category: category.name.clone() }}
                                            classes={classes!("category-card")}
                                        >
                                            <div class="category-card-body">
                                                <p class="category-name">{ &category.name }</p>
                                                <p class="category-description">{ &category.description }</p>
                                            </div>
                                            <span class="category-count">
                                                { format!("{} 篇", category.count) }
                                            </span>
                                        </Link<Route>>
                                    }
                                }) }
                            </section>
                        }
                    }
                }
            </div>
        </main>
    }
}
