use yew::prelude::*;

#[function_component(NotFoundPage)]
pub fn not_found_page() -> Html {
    html! {
        <main>
            <h2>{"404 - 页面未找到"}</h2>
            <p>{"抱歉，你访问的页面不存在。"}</p>
        </main>
    }
}
