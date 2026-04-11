//! Public Kiro access page shown to end users.

use gloo_timers::callback::Timeout;
use wasm_bindgen::prelude::*;
use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{
    api::{fetch_kiro_access, KiroAccessResponse},
    router::Route,
};

const CLAUDE_CODE_ENV_HINTS: [(&str, &str); 4] = [
    ("DISABLE_TELEMETRY=1", "禁用 Datadog + 1P 事件 + 反馈调查"),
    (
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1",
        "禁用所有非必要网络（遥测 + 更新 + GrowthBook）",
    ),
    ("CLAUDE_CODE_USE_BEDROCK=1", "使用 AWS Bedrock（自动禁用所有分析）"),
    ("CLAUDE_CODE_USE_VERTEX=1", "使用 GCP Vertex（自动禁用所有分析）"),
];

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

fn resolve_base_url(access: &KiroAccessResponse) -> String {
    if access.base_url.starts_with("http://") || access.base_url.starts_with("https://") {
        return access.base_url.clone();
    }
    let origin = web_sys::window()
        .and_then(|window| window.location().origin().ok())
        .unwrap_or_default();
    if origin.is_empty() {
        access.base_url.clone()
    } else {
        format!("{origin}{}", access.gateway_path)
    }
}

#[function_component(KiroAccessPage)]
/// Render the public Kiro access page, including connection examples and base
/// URL info.
pub fn kiro_access_page() -> Html {
    let access = use_state(|| None::<KiroAccessResponse>);
    let loading = use_state(|| true);
    let error = use_state(|| None::<String>);
    let copied = use_state(|| None::<String>);
    let copy_timeout = use_mut_ref(|| None::<Timeout>);
    // 0 = Claude Code env, 1 = curl
    let active_tab = use_state(|| 0u8);
    // Reusable callback to (re-)fetch Kiro access data from the backend.
    // Shared between the initial mount effect and manual refresh buttons.
    let reload_access = {
        let access = access.clone();
        let loading = loading.clone();
        let error = error.clone();
        Callback::from(move |_| {
            let access = access.clone();
            let loading = loading.clone();
            let error = error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                loading.set(true);
                error.set(None);
                match fetch_kiro_access().await {
                    Ok(response) => access.set(Some(response)),
                    Err(err) => error.set(Some(err)),
                }
                loading.set(false);
            });
        })
    };

    {
        let reload_access = reload_access.clone();
        // effect: fetch kiro access on mount
        use_effect_with((), move |_| {
            reload_access.emit(());
            || ()
        });
    }

    let on_copy = {
        let copied = copied.clone();
        let copy_timeout = copy_timeout.clone();
        Callback::from(move |value: String| {
            copy_text(&value);
            copied.set(Some("Copied!".to_string()));
            let copied = copied.clone();
            *copy_timeout.borrow_mut() = Some(Timeout::new(2_000, move || {
                copied.set(None);
            }));
        })
    };

    let access_value = (*access).clone();
    let resolved_base = access_value
        .as_ref()
        .map(resolve_base_url)
        .unwrap_or_else(|| "<loading>".to_string());
    let example_secret = "<your-kiro-key>".to_string();
    let claude_env_example = format!(
        "export ANTHROPIC_BASE_URL=\"{resolved_base}\"\nexport \
         ANTHROPIC_AUTH_TOKEN=\"{example_secret}\"\nclaude"
    );
    let curl_example = format!(
        "curl {resolved_base}/v1/messages \\\n  -H 'x-api-key: {example_secret}' \\\n  -H \
         'anthropic-version: 2023-06-01' \\\n  -H 'content-type: application/json' \\\n  -d \
         '{{\n    \"model\": \"claude-sonnet-4-6\",\n    \"max_tokens\": 128,\n    \"messages\": \
         [\n      {{\"role\": \"user\", \"content\": \"Reply exactly OK.\"}}\n    ]\n  }}'"
    );

    html! {
        <main class={classes!("container", "py-8", "space-y-5")}>
            // ── Header ──
            <section class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                <div class={classes!("flex", "items-center", "justify-between", "gap-4", "flex-wrap")}>
                    <div class={classes!("flex", "items-center", "gap-3")}>
                        <span class={classes!("inline-flex", "items-center", "rounded-full", "bg-slate-900", "px-2.5", "py-1", "font-mono", "text-[11px]", "font-semibold", "uppercase", "tracking-[0.16em]", "text-emerald-300")}>
                            { "Kiro" }
                        </span>
                        <h1 class={classes!("m-0", "font-mono", "text-xl", "font-bold", "text-[var(--text)]")}>
                            { "Kiro Access" }
                        </h1>
                    </div>
                    <div class={classes!("flex", "items-center", "gap-2")}>
                        <button
                            class={classes!("btn-terminal")}
                            onclick={{
                                let reload_access = reload_access.clone();
                                Callback::from(move |_| reload_access.emit(()))
                            }}
                            disabled={*loading}
                            title="刷新接入信息"
                            aria-label="刷新接入信息"
                        >
                            <i class={classes!("fas", if *loading { "fa-spinner animate-spin" } else { "fa-rotate-right" })}></i>
                        </button>
                        <button
                            class={classes!("btn-terminal")}
                            onclick={{
                                let on_copy = on_copy.clone();
                                let resolved_base = resolved_base.clone();
                                Callback::from(move |_| on_copy.emit(resolved_base.clone()))
                            }}
                        >
                            <i class="fas fa-copy"></i>
                        </button>
                        <Link<Route> to={Route::LlmAccess} classes={classes!("btn-terminal")}>
                            { "Codex" }
                        </Link<Route>>
                        <Link<Route> to={Route::AdminKiroGateway} classes={classes!("btn-terminal")}>
                            <i class="fas fa-sliders"></i>
                        </Link<Route>>
                    </div>
                </div>
                <div class={classes!("mt-2", "flex", "items-center", "gap-2")}>
                    <code class={classes!("break-all", "font-mono", "text-sm", "text-[var(--muted)]")}>{ resolved_base.clone() }</code>
                </div>
                if *loading {
                    <div class={classes!("mt-4", "font-mono", "text-sm", "text-[var(--muted)]")}>{ "> loading..." }</div>
                } else if let Some(err) = (*error).clone() {
                    <div class={classes!("mt-4", "rounded-lg", "bg-red-500/10", "px-3", "py-2", "font-mono", "text-xs", "text-red-700", "dark:text-red-200")}>
                        { err }
                    </div>
                }
            </section>

            // ── Code Examples (tabbed) ──
            <section class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                <div class={classes!("flex", "items-center", "gap-1")}>
                    <button
                        type="button"
                        class={classes!(
                            "rounded-t-lg", "px-3", "py-1.5", "font-mono", "text-xs", "font-semibold",
                            "transition-colors", "duration-150",
                            if *active_tab == 0 { "bg-[var(--surface-alt)] text-[var(--text)]" } else { "text-[var(--muted)] hover:text-[var(--text)]" },
                        )}
                        onclick={{ let active_tab = active_tab.clone(); Callback::from(move |_| active_tab.set(0)) }}
                    >
                        { "Claude Code" }
                    </button>
                    <button
                        type="button"
                        class={classes!(
                            "rounded-t-lg", "px-3", "py-1.5", "font-mono", "text-xs", "font-semibold",
                            "transition-colors", "duration-150",
                            if *active_tab == 1 { "bg-[var(--surface-alt)] text-[var(--text)]" } else { "text-[var(--muted)] hover:text-[var(--text)]" },
                        )}
                        onclick={{ let active_tab = active_tab.clone(); Callback::from(move |_| active_tab.set(1)) }}
                    >
                        { "curl" }
                    </button>
                    <button
                        type="button"
                        class={classes!("ml-auto", "btn-terminal")}
                        onclick={{
                            let on_copy = on_copy.clone();
                            let claude_env_example = claude_env_example.clone();
                            let curl_example = curl_example.clone();
                            let active_tab = active_tab.clone();
                            Callback::from(move |_| {
                                let text = if *active_tab == 0 { claude_env_example.clone() } else { curl_example.clone() };
                                on_copy.emit(text);
                            })
                        }}
                    >
                        <i class="fas fa-copy"></i>
                        { " 复制" }
                    </button>
                </div>
                <pre class={classes!("mt-0", "overflow-x-auto", "rounded-b-xl", "rounded-tr-xl", "bg-[var(--surface-alt)]", "p-4", "font-mono", "text-xs")}>
                    <code>{ if *active_tab == 0 { claude_env_example } else { curl_example } }</code>
                </pre>
                <div class={classes!("mt-3", "flex", "items-center", "gap-3", "font-mono", "text-[10px]", "text-[var(--muted)]", "flex-wrap")}>
                    <span>{ "/v1/models" }</span>
                    <span>{ "/v1/messages" }</span>
                    <span>{ "/v1/messages/count_tokens" }</span>
                    <span>{ "/cc/v1/messages" }</span>
                </div>
            </section>

            // ── Recommended Settings ──
            <section class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                <h2 class={classes!("m-0", "font-mono", "text-sm", "font-bold", "uppercase", "tracking-[0.18em]", "text-[var(--muted)]")}>
                    { "Recommended ~/.claude/settings.json" }
                </h2>
                <ul class={classes!("mt-3", "space-y-3", "list-none", "p-0", "m-0")}>
                    <li class={classes!("rounded-lg", "bg-[var(--surface-alt)]", "p-3")}>
                        <code class={classes!("font-mono", "text-xs", "text-[var(--text)]")}>
                            { r#""skipWebFetchPreflight": true"# }
                        </code>
                        <p class={classes!("mt-1", "mb-0", "font-mono", "text-[11px]", "text-[var(--muted)]")}>
                            { "允许 Claude Code 正常使用 WebFetch 工具，跳过预检限制" }
                        </p>
                    </li>
                    <li class={classes!("rounded-lg", "bg-[var(--surface-alt)]", "p-3")}>
                        <code class={classes!("font-mono", "text-xs", "text-[var(--text)]")}>
                            { r#""showClearContextOnPlanAccept": true"# }
                        </code>
                        <p class={classes!("mt-1", "mb-0", "font-mono", "text-[11px]", "text-[var(--muted)]")}>
                            { "Plan Mode 批准时保留旧版 context 感知能力，显示清除上下文选项" }
                        </p>
                    </li>
                </ul>
            </section>

            // ── Optional Env Flags ──
            <section class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                <h2 class={classes!("m-0", "font-mono", "text-sm", "font-bold", "uppercase", "tracking-[0.18em]", "text-[var(--muted)]")}>
                    { "Optional Claude Code Env Flags" }
                </h2>
                <div class={classes!("mt-3", "space-y-3")}>
                    { for CLAUDE_CODE_ENV_HINTS.iter().map(|(key, description)| html! {
                        <article class={classes!("rounded-lg", "bg-[var(--surface-alt)]", "p-3")}>
                            <code class={classes!("block", "break-all", "font-mono", "text-xs", "text-[var(--text)]")}>
                                { *key }
                            </code>
                            <p class={classes!("mt-1", "mb-0", "font-mono", "text-[11px]", "text-[var(--muted)]")}>
                                { *description }
                            </p>
                        </article>
                    }) }
                </div>
            </section>

            // Fixed bottom-right toast
            if let Some(message) = (*copied).clone() {
                <div class={classes!(
                    "fixed", "bottom-6", "right-6", "z-[80]",
                    "rounded-lg", "bg-slate-900", "px-4", "py-2.5",
                    "font-mono", "text-xs", "font-semibold", "text-emerald-300",
                    "shadow-[0_8px_24px_rgba(0,0,0,0.25)]",
                    "animate-[fade-in_0.2s_ease-out]",
                )}>
                    <i class={classes!("fas", "fa-check", "mr-2")}></i>
                    { message }
                </div>
            }
        </main>
    }
}
