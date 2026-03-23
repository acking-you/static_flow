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
    body: AttrValue,
    button_label: AttrValue,
    copy_label: AttrValue,
    code: String,
    on_copy: Callback<(String, String)>,
}

#[function_component(GuideCodePanel)]
fn guide_code_panel(props: &GuideCodePanelProps) -> Html {
    html! {
        <section class={classes!(
            "rounded-[1.25rem]",
            "border",
            "border-[var(--border)]",
            "bg-[var(--surface)]/78",
            "p-5"
        )}>
            <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                <div class={classes!("max-w-xl")}>
                    <p class={classes!("m-0", "text-xs", "uppercase", "tracking-[0.18em]", "text-[var(--muted)]")}>{ props.eyebrow.clone() }</p>
                    <h4 class={classes!("mt-2", "text-2xl", "font-bold", "tracking-[-0.03em]", "text-[var(--text)]")}>{ props.title.clone() }</h4>
                    <p class={classes!("mt-3", "m-0", "text-sm", "leading-7", "text-[var(--muted)]")}>{ props.body.clone() }</p>
                </div>
                <button
                    class={classes!("btn-fluent-primary")}
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
            <pre class={classes!("mt-4", "overflow-x-auto", "rounded-[1.1rem]", "bg-slate-950", "p-4", "text-xs", "leading-6", "text-emerald-200")}>
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
            <div class={classes!("mt-10", "rounded-[1.25rem]", "border", "border-dashed", "border-[var(--border)]", "px-5", "py-12", "text-center", "text-[var(--muted)]")}>
                { "正在读取帮助页所需的接入信息" }
            </div>
        }
    } else if let Some(err) = (*error).clone() {
        html! {
            <div class={classes!("mt-10", "rounded-[1.25rem]", "border", "border-red-400/35", "bg-red-500/8", "px-5", "py-5", "text-sm", "text-red-700", "dark:text-red-200")}>
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
                <section class={classes!(
                    "llm-access-hero",
                    "mt-8",
                    "overflow-hidden",
                    "rounded-[1.8rem]",
                    "border",
                    "border-emerald-500/15",
                    "bg-[linear-gradient(140deg,rgba(236,253,245,0.98),rgba(239,246,255,0.96))]",
                    "p-6",
                    "text-slate-900",
                    "shadow-[0_30px_90px_rgba(16,185,129,0.10)]",
                    "lg:p-8"
                )}>
                    <div class={classes!("grid", "gap-8", "xl:grid-cols-[minmax(0,1.15fr)_minmax(0,0.85fr)]", "xl:items-end")}>
                        <div class={classes!("max-w-4xl")}>
                            <p class={classes!("llm-access-hero-eyebrow", "m-0", "text-xs", "font-semibold", "uppercase", "tracking-[0.26em]", "text-emerald-700")}>
                                { "Three-Step Guide" }
                            </p>
                            <h1 class={classes!("llm-access-hero-title", "mt-4", "text-4xl", "font-black", "tracking-[-0.06em]", "text-slate-950", "sm:text-[3.45rem]")}>
                                { "三步就能接进 Codex 或养龙虾🦞，不用自己猜 provider" }
                            </h1>
                            <p class={classes!("llm-access-hero-body", "mt-4", "m-0", "max-w-3xl", "text-base", "leading-8", "text-slate-700")}>
                                { "这一页只做一件事：把能直接复制的接入方法按顺序摆好。先配 provider，再写入公开 Key，最后直接开始聊天" }
                            </p>
                            <div class={classes!("mt-6", "flex", "flex-wrap", "gap-3")}>
                                <Link<Route> to={Route::LlmAccess} classes={classes!("btn-fluent-secondary")}>
                                    { "返回 Key 大厅" }
                                </Link<Route>>
                                <button
                                    class={classes!("btn-fluent-primary")}
                                    onclick={{
                                        let on_copy = on_copy.clone();
                                        let base_url = base_url.clone();
                                        Callback::from(move |_| on_copy.emit((" /v1 Base URL".to_string(), base_url.clone())))
                                    }}
                                >
                                    { "复制 /v1 Base URL" }
                                </button>
                            </div>
                        </div>

                        <div class={classes!("rounded-[1.35rem]", "border", "border-slate-900/10", "bg-white/62", "p-5", "backdrop-blur-xl")}>
                            <p class={classes!("m-0", "text-[11px]", "font-semibold", "uppercase", "tracking-[0.22em]", "text-sky-700")}>{ "Quick Check" }</p>
                            <div class={classes!("mt-3", "space-y-3", "text-sm", "leading-7", "text-slate-700")}>
                                <p class={classes!("m-0")}>{ "1 先复制 provider 配置，不要把站点当普通 OpenAI 壳来猜参数" }</p>
                                <p class={classes!("m-0")}>{ format!("2 再把公开 Key 写进 Codex，当前示例 Key 是 {}", example_key_name) }</p>
                                <p class={classes!("m-0")}>{ "3 只是想给龙虾🦞喂接口的话，直接用下面的 curl 或 Python 即可" }</p>
                            </div>
                        </div>
                    </div>
                </section>

                <section class={classes!(
                    "llm-access-warning",
                    "mt-8",
                    "overflow-hidden",
                    "rounded-[1.75rem]",
                    "border",
                    "border-amber-500/20",
                    "bg-[linear-gradient(135deg,rgba(255,247,237,0.98),rgba(254,249,195,0.92))]",
                    "p-6",
                    "text-slate-900",
                    "shadow-[0_24px_80px_rgba(245,158,11,0.12)]",
                    "lg:p-8"
                )}>
                    <div class={classes!("grid", "gap-6", "xl:grid-cols-[minmax(0,1.12fr)_minmax(0,0.88fr)]", "xl:items-end")}>
                        <div class={classes!("max-w-4xl")}>
                            <p class={classes!("llm-access-warning-eyebrow", "m-0", "text-xs", "font-semibold", "uppercase", "tracking-[0.26em]", "text-amber-700")}>
                                { "Before You Paste" }
                            </p>
                            <h2 class={classes!("llm-access-warning-title", "mt-4", "text-4xl", "font-black", "tracking-[-0.06em]", "text-slate-950", "sm:text-[3.05rem]")}>
                                { "很多中转站接法都在偷掉 remote compact，这不是小问题" }
                            </h2>
                            <p class={classes!("llm-access-warning-body", "mt-4", "m-0", "max-w-3xl", "text-base", "leading-8", "text-slate-700")}>
                                { "所以这里明确把 provider 写法固定下来：保持 OpenAI 语义，直接用 /v1 Base URL，并显式关掉 websocket 重试，这样 Codex 才不会先卡再 fallback" }
                            </p>
                        </div>

                        <div class={classes!("llm-access-warning-card", "rounded-[1.35rem]", "border", "border-slate-900/10", "bg-white/68", "p-5", "backdrop-blur-xl")}>
                            <Link<Route>
                                to={Route::ArticleDetail {
                                    id: REMOTE_COMPACT_ARTICLE_ID.to_string(),
                                }}
                                classes={classes!(
                                    "llm-access-warning-link",
                                    "inline-flex", "w-full", "items-center", "justify-center", "rounded-full",
                                    "border", "border-amber-500/24", "bg-amber-500/10", "px-5", "py-3",
                                    "text-sm", "font-semibold", "text-amber-800", "transition-colors", "hover:bg-amber-500/16"
                                )}
                            >
                                { "看 compact 深潜文章" }
                            </Link<Route>>
                        </div>
                    </div>
                </section>

                <section class={classes!("mt-10", "space-y-6")}>
                    <article class={classes!("rounded-[1.5rem]", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-6", "shadow-[0_28px_72px_rgba(15,23,42,0.10)]", "lg:p-7")}>
                        <div class={classes!("max-w-3xl")}>
                            <p class={classes!("m-0", "text-[11px]", "font-semibold", "uppercase", "tracking-[0.22em]", "text-[var(--primary)]")}>{ "Step 01" }</p>
                            <h2 class={classes!("mt-3", "text-3xl", "font-black", "tracking-[-0.05em]", "text-[var(--text)]")}>
                                { "先把 provider 配对，不要自己猜" }
                            </h2>
                            <p class={classes!("mt-3", "m-0", "text-sm", "leading-7", "text-[var(--muted)]")}>
                                { "最稳的接法是显式声明一个 provider：保持 OpenAI 语义，Base URL 直接复制这里给出的 /v1 地址，同时关掉 websocket，这样 Codex 会直接走 responses 和 remote compact" }
                            </p>
                        </div>

                        <div class={classes!("mt-6")}>
                            <GuideCodePanel
                                eyebrow={"~/.codex/config.toml"}
                                title={"直接可复制的 provider 配置"}
                                body={"这个写法就是为了保住 remote compact，不要再额外自己拼 /v1，也不要把 supports_websockets 留成默认值"}
                                button_label={"复制 provider 配置"}
                                copy_label={"provider 配置"}
                                code={provider_config.clone()}
                                on_copy={on_copy.clone()}
                            />
                        </div>
                    </article>

                    <article class={classes!("rounded-[1.5rem]", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-6", "shadow-[0_28px_72px_rgba(15,23,42,0.10)]", "lg:p-7")}>
                        <div class={classes!("max-w-3xl")}>
                            <p class={classes!("m-0", "text-[11px]", "font-semibold", "uppercase", "tracking-[0.22em]", "text-[var(--primary)]")}>{ "Step 02" }</p>
                            <h2 class={classes!("mt-3", "text-3xl", "font-black", "tracking-[-0.05em]", "text-[var(--text)]")}>
                                { "再把公开 Key 写进 Codex" }
                            </h2>
                            <p class={classes!("mt-3", "m-0", "text-sm", "leading-7", "text-[var(--muted)]")}>
                                { "推荐方式永远是让 Codex 自己写 auth.json。只有在你必须手工维护文件的时候，才退回到备用 JSON 写法" }
                            </p>
                        </div>

                        <div class={classes!("mt-6", "grid", "gap-4", "xl:grid-cols-2")}>
                            <GuideCodePanel
                                eyebrow={"Recommended"}
                                title={"优先用 codex login --with-api-key"}
                                body={"执行命令后把当前公开放出的任意一把 Key 粘进去即可，这是最稳的写法"}
                                button_label={"复制登录命令"}
                                copy_label={"登录命令"}
                                code={login_command.clone()}
                                on_copy={on_copy.clone()}
                            />
                            <GuideCodePanel
                                eyebrow={"Fallback"}
                                title={"手工写 auth.json 的备用写法"}
                                body={"只有在 CLI 登录不可用时再使用；这里默认塞入了第一把公开 Key 作为示例"}
                                button_label={"复制 auth.json"}
                                copy_label={"auth.json"}
                                code={auth_json.clone()}
                                on_copy={on_copy.clone()}
                            />
                        </div>
                    </article>

                    <article class={classes!("rounded-[1.5rem]", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-6", "shadow-[0_28px_72px_rgba(15,23,42,0.10)]", "lg:p-7")}>
                        <div class={classes!("max-w-3xl")}>
                            <p class={classes!("m-0", "text-[11px]", "font-semibold", "uppercase", "tracking-[0.22em]", "text-[var(--primary)]")}>{ "Step 03" }</p>
                            <h2 class={classes!("mt-3", "text-3xl", "font-black", "tracking-[-0.05em]", "text-[var(--text)]")}>
                                { "接完就用，不管是 Codex 还是养龙虾🦞" }
                            </h2>
                            <p class={classes!("mt-3", "m-0", "text-sm", "leading-7", "text-[var(--muted)]")}>
                                { "只想拿去当聊天接口时，不用再翻 provider 文档。下面就是直接可发请求的最小示例" }
                            </p>
                        </div>

                        <div class={classes!("mt-6", "grid", "gap-4", "xl:grid-cols-2")}>
                            <GuideCodePanel
                                eyebrow={"curl"}
                                title={"最小 curl 请求"}
                                body={"适合先快速验证 Base URL 和公开 Key 是否已经通了"}
                                button_label={"复制 curl 示例"}
                                copy_label={"curl 示例"}
                                code={curl_example.clone()}
                                on_copy={on_copy.clone()}
                            />
                            <GuideCodePanel
                                eyebrow={"Python SDK"}
                                title={"养龙虾🦞 / OpenAI SDK 风格"}
                                body={"只要是 OpenAI 风格客户端，Base URL 和 API key 都可以按这个方式喂进去"}
                                button_label={"复制 Python 示例"}
                                copy_label={"Python 示例"}
                                code={python_example.clone()}
                                on_copy={on_copy.clone()}
                            />
                        </div>
                    </article>

                    <section class={classes!("rounded-[1.5rem]", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-6", "shadow-[0_28px_72px_rgba(15,23,42,0.10)]", "lg:p-7")}>
                        <div class={classes!("flex", "items-start", "justify-between", "gap-4", "flex-wrap")}>
                            <div class={classes!("max-w-3xl")}>
                                <p class={classes!("m-0", "text-[11px]", "font-semibold", "uppercase", "tracking-[0.22em]", "text-[var(--primary)]")}>{ "Ready To Copy" }</p>
                                <h2 class={classes!("mt-3", "text-3xl", "font-black", "tracking-[-0.05em]", "text-[var(--text)]")}>
                                    { "已经配好了，回到 Key 大厅直接复制" }
                                </h2>
                                <p class={classes!("mt-3", "m-0", "text-sm", "leading-7", "text-[var(--muted)]")}>
                                    { format!("当前帮助页示例默认使用的是 {}，真正复制时你可以回到 Key 大厅挑任意一把公开 Key", example_key_name) }
                                </p>
                            </div>
                            <Link<Route> to={Route::LlmAccess} classes={classes!("btn-fluent-primary", "!px-6", "!py-3")}>
                                { "回到公开 Key 大厅" }
                            </Link<Route>>
                        </div>
                    </section>
                </section>
            </>
        }
    } else {
        Html::default()
    };

    html! {
        <main class={classes!(
            "relative",
            "min-h-screen",
            "overflow-hidden",
            "bg-[radial-gradient(circle_at_top_left,rgba(15,118,110,0.18),transparent_32%),radial-gradient(circle_at_top_right,rgba(37,99,235,0.18),transparent_28%),linear-gradient(180deg,var(--bg),color-mix(in_srgb,var(--bg)_84%,var(--surface-alt)))]"
        )}>
            <div class={classes!("relative", "mx-auto", "max-w-[96rem]", "px-4", "pb-16", "pt-8", "lg:px-6", "lg:pt-10")}>
                { content }
            </div>

            if let Some((message, is_error)) = (*toast).clone() {
                <div class={classes!(
                    "fixed",
                    "bottom-5",
                    "right-5",
                    "z-[90]",
                    "rounded-full",
                    "border",
                    "px-4",
                    "py-3",
                    "text-sm",
                    "font-semibold",
                    "shadow-[0_18px_42px_rgba(15,23,42,0.24)]",
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
