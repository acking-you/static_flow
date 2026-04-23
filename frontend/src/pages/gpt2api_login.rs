use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;
use yew_router::prelude::*;

use crate::{
    api::verify_public_gpt2api_key,
    pages::gpt2api_public_shared::{load_auth_key, save_auth_key},
    router::Route,
};

#[function_component(Gpt2ApiLoginPage)]
pub fn gpt2api_login_page() -> Html {
    let navigator = use_navigator();
    let auth_key = use_state(String::new);
    let loading = use_state(|| false);
    let error = use_state(|| None::<String>);

    {
        let auth_key = auth_key.clone();
        use_effect_with((), move |_| {
            spawn_local(async move {
                if let Ok(stored) = load_auth_key().await {
                    auth_key.set(stored);
                }
            });
            || ()
        });
    }

    let on_submit = {
        let navigator = navigator.clone();
        let auth_key = auth_key.clone();
        let loading = loading.clone();
        let error = error.clone();
        Callback::from(move |_| {
            let normalized = (*auth_key).trim().to_string();
            if normalized.is_empty() {
                error.set(Some("请输入可用的 API Key 明文 secret".to_string()));
                return;
            }
            loading.set(true);
            error.set(None);
            let navigator = navigator.clone();
            let loading = loading.clone();
            let error = error.clone();
            spawn_local(async move {
                match verify_public_gpt2api_key(&normalized).await {
                    Ok(_) => {
                        let _ = save_auth_key(&normalized).await;
                        if let Some(navigator) = navigator {
                            navigator.replace(&Route::Gpt2ApiImage);
                        }
                    },
                    Err(err) => error.set(Some(err)),
                }
                loading.set(false);
            });
        })
    };

    html! {
        <div class={classes!("grid", "min-h-[calc(100vh-1rem)]", "w-full", "place-items-center", "px-4", "py-6")}>
            <div class={classes!("w-full", "max-w-[505px]", "rounded-[30px]", "border", "border-white/80", "bg-white/95", "shadow-[0_28px_90px_rgba(28,25,23,0.10)]")}>
                <div class={classes!("space-y-7", "p-6", "sm:p-8")}>
                    <div class={classes!("space-y-4", "text-center")}>
                        <div class={classes!("mx-auto", "inline-flex", "size-14", "items-center", "justify-center", "rounded-[18px]", "bg-stone-950", "text-xl", "font-semibold", "text-white", "shadow-sm")}>
                            { "K" }
                        </div>
                        <div class={classes!("space-y-2")}>
                            <h1 class={classes!("text-3xl", "font-semibold", "tracking-tight", "text-stone-950")}>{ "欢迎回来" }</h1>
                            <p class={classes!("text-sm", "leading-6", "text-stone-500")}>
                                { "输入 Admin 页里保存的明文 secret（sk-...）继续使用图片和聊天工作台，不要填 hash 或其他摘要值。" }
                            </p>
                        </div>
                    </div>

                    <div class={classes!("space-y-3")}>
                        <label for="auth-key" class={classes!("block", "text-sm", "font-medium", "text-stone-700")}>
                            { "API Key Secret" }
                        </label>
                        <input
                            id="auth-key"
                            type="password"
                            value={(*auth_key).clone()}
                            placeholder="sk-..."
                            class={classes!("h-13", "w-full", "rounded-2xl", "border", "border-stone-200", "bg-white", "px-4", "outline-none", "transition", "focus:border-stone-400")}
                            oninput={{
                                let auth_key = auth_key.clone();
                                Callback::from(move |event: InputEvent| {
                                    auth_key.set(event.target_unchecked_into::<HtmlInputElement>().value());
                                })
                            }}
                            onkeydown={{
                                let on_submit = on_submit.clone();
                                Callback::from(move |event: KeyboardEvent| {
                                    if event.key() == "Enter" {
                                        on_submit.emit(());
                                    }
                                })
                            }}
                        />
                        <p class={classes!("m-0", "text-xs", "leading-5", "text-stone-500")}>
                            { "如果你在 /admin/gpt2api-rs 创建过 key，可以直接从 Key Inventory 再次复制明文 secret；如果没有可用明文，就去 Admin 页 Reissue 一条新的。" }
                        </p>
                    </div>

                    if let Some(message) = (*error).clone() {
                        <div class={classes!("rounded-[18px]", "bg-rose-50/80", "px-4", "py-3", "text-sm", "leading-6", "text-rose-600")}>
                            { message }
                        </div>
                    }

                    <button
                        type="button"
                        class={classes!("h-13", "w-full", "rounded-2xl", "bg-stone-950", "text-white", "transition", "hover:bg-stone-800", "disabled:cursor-not-allowed", "disabled:opacity-60")}
                        onclick={{
                            let on_submit = on_submit.clone();
                            Callback::from(move |_| on_submit.emit(()))
                        }}
                        disabled={*loading}
                    >
                        { if *loading { "校验中..." } else { "登录" } }
                    </button>
                </div>
            </div>
        </div>
    }
}
