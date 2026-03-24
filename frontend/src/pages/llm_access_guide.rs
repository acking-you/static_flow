use gloo_timers::callback::Timeout;
use wasm_bindgen::prelude::*;
use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{
    api::{fetch_llm_gateway_access, LlmGatewayAccessResponse},
    pages::llm_access_shared::{
        chat_curl_example, chat_python_example, codex_auth_json, codex_login_command,
        codex_provider_config, example_key_name, example_key_secret, resolved_base_url,
        REMOTE_COMPACT_ARTICLE_ID,
    },
    router::Route,
};

#[wasm_bindgen(inline_js = r#"
export function copy_text(text) {
    if (navigator.clipboard) {
        navigator.clipboard.writeText(text).catch(function(){});
    }
}
"#)]
extern "C" {
    fn copy_text(text: &str);
}

#[derive(Properties, PartialEq)]
struct GuideCodePanelProps {
    eyebrow: AttrValue,
    title: AttrValue,
    button_label: AttrValue,
    copy_label: AttrValue,
    code: String,
    on_copy: Callback<(String, String)>,
}

#[function_component(GuideCodePanel)]
fn guide_code_panel(props: &GuideCodePanelProps) -> Html {
    html! {
        <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-4")}>
            <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                <div>
                    <span class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ props.eyebrow.clone() }</span>
                    <h4 class={classes!("m-0", "mt-1", "text-sm", "font-bold", "text-[var(--text)]")}>{ props.title.clone() }</h4>
                </div>
                <button
                    class={classes!("btn-terminal", "btn-terminal-primary", "!text-xs")}
                    onclick={{
                        let label = props.copy_label.to_string();
                        let code = props.code.clone();
                        let on_copy = props.on_copy.clone();
                        Callback::from(move |_| on_copy.emit((label.clone(), code.clone())))
                    }}
                >
                    { props.button_label.clone() }
                </button>
            </div>
            <pre class={classes!("mt-3", "overflow-x-auto", "rounded-lg", "bg-slate-950", "p-3", "text-xs", "leading-6", "text-emerald-200")}>
                { props.code.clone() }
            </pre>
        </section>
    }
}

#[function_component(LlmAccessGuidePage)]
pub fn llm_access_guide_page() -> Html {
    let access = use_state(|| None::<LlmGatewayAccessResponse>);
    let loading = use_state(|| true);
    let error = use_state(|| None::<String>);
    let toast = use_state(|| None::<(String, bool)>);
    let toast_timeout = use_mut_ref(|| None::<Timeout>);

    {
        let access = access.clone();
        let loading = loading.clone();
        let error = error.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_llm_gateway_access().await {
                    Ok(data) => {
                        access.set(Some(data));
                        error.set(None);
                    },
                    Err(err) => {
                        access.set(None);
                        error.set(Some(err));
                    },
                }
                loading.set(false);
            });
            || ()
        });
    }

    let on_copy = {
        let toast = toast.clone();
        let toast_timeout = toast_timeout.clone();
        Callback::from(move |(label, value): (String, String)| {
            copy_text(&value);
            toast.set(Some((format!("已复制{}", label), false)));
            toast_timeout.borrow_mut().take();
            let toast = toast.clone();
            let clear_handle = toast_timeout.clone();
            let timeout = Timeout::new(1800, move || {
                toast.set(None);
                clear_handle.borrow_mut().take();
            });
            *toast_timeout.borrow_mut() = Some(timeout);
        })
    };

    let content = if *loading {
        html! {
            <div class={classes!("mt-10", "rounded-xl", "border", "border-dashed", "border-[var(--border)]", "px-5", "py-12", "text-center", "text-[var(--muted)]")}>
                { "正在读取接入信息" }
            </div>
        }
    } else if let Some(err) = (*error).clone() {
        html! {
            <div class={classes!("mt-10", "rounded-xl", "border", "border-red-400/35", "bg-red-500/8", "px-5", "py-5", "text-sm", "text-red-700", "dark:text-red-200")}>
                { err }
            </div>
        }
    } else if let Some(access) = (*access).clone() {
        let base_url = resolved_base_url(&access);
        let example_key = example_key_secret(&access);
        let example_key_name = example_key_name(&access);
        let provider_config = codex_provider_config(&base_url);
        let login_command = codex_login_command();
        let auth_json = codex_auth_json(&example_key);
        let curl_example = chat_curl_example(&base_url, &example_key);
        let python_example = chat_python_example(&base_url, &example_key);

        html! {
            <>
                // Page header
                <section class={classes!("mt-8", "rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-start", "justify-between", "gap-4", "flex-wrap")}>
                        <div>
                            <h1 class={classes!("m-0", "text-2xl", "font-bold", "text-[var(--text)]")}>
                                { "三步接入 Codex / 养龙虾🦞" }
                            </h1>
                            <p class={classes!("mt-2", "m-0", "text-sm", "text-[var(--muted)]")}>
                                { format!("示例 Key: {}", example_key_name) }
                            </p>
                        </div>
                        <div class={classes!("flex", "items-center", "gap-2")}>
                            <Link<Route> to={Route::LlmAccess} classes={classes!("btn-terminal")}>
                                <i class="fas fa-arrow-left"></i>
                                { "Key 大厅" }
                            </Link<Route>>
                            <button
                                class={classes!("btn-terminal", "btn-terminal-primary")}
                                onclick={{
                                    let on_copy = on_copy.clone();
                                    let base_url = base_url.clone();
                                    Callback::from(move |_| on_copy.emit(("Base URL".to_string(), base_url.clone())))
                                }}
                            >
                                <i class="fas fa-copy"></i>
                                { "复制 URL" }
                            </button>
                        </div>
                    </div>
                </section>

                // Notice bar
                <div class={classes!("mt-4", "llm-access-notice")}>
                    { "保住 remote compact 是接 Codex 的前提 — " }
                    <Link<Route>
                        to={Route::ArticleDetail { id: REMOTE_COMPACT_ARTICLE_ID.to_string() }}
                        classes={classes!("underline", "text-[var(--primary)]")}
                    >
                        { "深潜文章" }
                    </Link<Route>>
                </div>

                // PLACEHOLDER_STEPS

                // Step 01: Provider config
                <section class={classes!("mt-6", "rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-center", "gap-2")}>
                        <span class={classes!("text-xs", "font-semibold", "uppercase", "tracking-widest", "text-[var(--primary)]")}>{ "Step 01" }</span>
                        <h2 class={classes!("m-0", "text-lg", "font-bold", "text-[var(--text)]")}>{ "配置 Provider" }</h2>
                    </div>
                    <div class={classes!("mt-4")}>
                        <GuideCodePanel
                            eyebrow={"~/.codex/config.toml"}
                            title={"Provider 配置"}
                            button_label={"复制"}
                            copy_label={"provider 配置"}
                            code={provider_config.clone()}
                            on_copy={on_copy.clone()}
                        />
                    </div>
                </section>

                // Step 02: Auth
                <section class={classes!("mt-4", "rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-center", "gap-2")}>
                        <span class={classes!("text-xs", "font-semibold", "uppercase", "tracking-widest", "text-[var(--primary)]")}>{ "Step 02" }</span>
                        <h2 class={classes!("m-0", "text-lg", "font-bold", "text-[var(--text)]")}>{ "写入 Key" }</h2>
                    </div>
                    <div class={classes!("mt-4", "grid", "gap-3", "xl:grid-cols-2")}>
                        <GuideCodePanel
                            eyebrow={"推荐"}
                            title={"codex login --with-api-key"}
                            button_label={"复制"}
                            copy_label={"登录命令"}
                            code={login_command.clone()}
                            on_copy={on_copy.clone()}
                        />
                        <GuideCodePanel
                            eyebrow={"备用"}
                            title={"手写 auth.json"}
                            button_label={"复制"}
                            copy_label={"auth.json"}
                            code={auth_json.clone()}
                            on_copy={on_copy.clone()}
                        />
                    </div>
                </section>

                // Step 03: Usage
                <section class={classes!("mt-4", "rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-center", "gap-2")}>
                        <span class={classes!("text-xs", "font-semibold", "uppercase", "tracking-widest", "text-[var(--primary)]")}>{ "Step 03" }</span>
                        <h2 class={classes!("m-0", "text-lg", "font-bold", "text-[var(--text)]")}>{ "开始使用" }</h2>
                    </div>
                    <div class={classes!("mt-4", "grid", "gap-3", "xl:grid-cols-2")}>
                        <GuideCodePanel
                            eyebrow={"curl"}
                            title={"最小请求示例"}
                            button_label={"复制"}
                            copy_label={"curl 示例"}
                            code={curl_example.clone()}
                            on_copy={on_copy.clone()}
                        />
                        <GuideCodePanel
                            eyebrow={"Python"}
                            title={"OpenAI SDK 风格"}
                            button_label={"复制"}
                            copy_label={"Python 示例"}
                            code={python_example.clone()}
                            on_copy={on_copy.clone()}
                        />
                    </div>
                </section>

                // Back to keys
                <section class={classes!("mt-4", "flex", "items-center", "justify-between", "gap-4", "rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5", "flex-wrap")}>
                    <h2 class={classes!("m-0", "text-lg", "font-bold", "text-[var(--text)]")}>
                        { "配好了，回去复制 Key" }
                    </h2>
                    <Link<Route> to={Route::LlmAccess} classes={classes!("btn-terminal", "btn-terminal-primary")}>
                        <i class="fas fa-key"></i>
                        { "Key 大厅" }
                    </Link<Route>>
                </section>
            </>
        }
    } else {
        Html::default()
    };

    html! {
        <main class={classes!("relative", "min-h-screen", "bg-[var(--bg)]")}>
            <div class={classes!("relative", "mx-auto", "max-w-5xl", "px-4", "pb-16", "pt-8", "lg:px-6")}>
                { content }
            </div>

            if let Some((message, is_error)) = (*toast).clone() {
                <div class={classes!(
                    "fixed", "bottom-5", "right-5", "z-[90]",
                    "rounded-full", "border", "px-4", "py-3",
                    "text-sm", "font-semibold",
                    "shadow-[0_8px_24px_rgba(0,0,0,0.15)]",
                    if is_error {
                        classes!("border-red-400/35", "bg-red-500/92", "text-white")
                    } else {
                        classes!("border-emerald-400/35", "bg-emerald-500/92", "text-white")
                    }
                )}>
                    { message }
                </div>
            }
        </main>
    }
}
