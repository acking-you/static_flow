//! Public Kiro access page shown to end users.

use gloo_timers::callback::Timeout;
use wasm_bindgen::prelude::*;
use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{
    api::{fetch_kiro_access, KiroAccessResponse},
    pages::llm_access_shared::{format_reset_hint, kiro_credit_ratio},
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
/// Render the public Kiro access page, including connection examples and the
/// cached status cards for all configured Kiro accounts.
pub fn kiro_access_page() -> Html {
    let access = use_state(|| None::<KiroAccessResponse>);
    let loading = use_state(|| true);
    let error = use_state(|| None::<String>);
    let copied = use_state(|| None::<String>);
    let copy_timeout = use_mut_ref(|| None::<Timeout>);
    // 0 = Claude Code env, 1 = curl
    let active_tab = use_state(|| 0u8);

    {
        let access = access.clone();
        let loading = loading.clone();
        let error = error.clone();
        // effect: fetch kiro access on mount
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                loading.set(true);
                error.set(None);
                match fetch_kiro_access().await {
                    Ok(response) => access.set(Some(response)),
                    Err(err) => error.set(Some(err)),
                }
                loading.set(false);
            });
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
         ANTHROPIC_API_KEY=\"{example_secret}\"\nclaude"
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


            // ── Quota Cards ──
            {
                if let Some(ref access_data) = access_value {
                    if !access_data.accounts.is_empty() {
                        html! {
                            <section class={classes!("grid", "gap-4", "lg:grid-cols-2")}>
                                { for access_data.accounts.iter().map(|status| {
                                    let ratio = kiro_credit_ratio(status.current_usage, status.usage_limit);
                                    let pct = (ratio * 100.0).round() as i32;
                                    let remaining_text = status.remaining.map(|v| format!("{v:.0}")).unwrap_or_else(|| "-".to_string());
                                    let limit_text = status.usage_limit.map(|v| format!("{v:.0}")).unwrap_or_else(|| "-".to_string());
                                    html! {
                                        <article class={classes!(
                                            "group", "overflow-hidden", "rounded-lg", "border", "border-[var(--border)]",
                                            "bg-[var(--surface)]", "p-5",
                                            "transition-all", "duration-200",
                                            "hover:-translate-y-0.5", "hover:shadow-[0_8px_24px_rgba(0,0,0,0.08)]",
                                        )}>
                                            <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                                                <span class={classes!("font-mono", "text-sm", "font-bold", "text-[var(--text)]")}>{ status.name.clone() }</span>
                                                if status.is_active {
                                                    <span class={classes!("inline-flex", "rounded-full", "border", "border-emerald-500/20", "bg-emerald-500/10", "px-2", "py-0.5", "text-[10px]", "font-semibold", "uppercase", "tracking-[0.12em]", "text-emerald-700", "dark:text-emerald-200")}>
                                                        { "active" }
                                                    </span>
                                                }
                                                if status.disabled {
                                                    <span class={classes!("inline-flex", "rounded-full", "bg-amber-500/10", "px-2", "py-0.5", "text-[10px]", "font-semibold", "uppercase", "tracking-[0.12em]", "text-amber-700", "dark:text-amber-200")}>
                                                        { "disabled" }
                                                    </span>
                                                }
                                                if let Some(ref plan) = status.subscription_title {
                                                    <span class={classes!("ml-auto", "rounded-full", "bg-[var(--surface-alt)]", "px-2.5", "py-0.5", "font-mono", "text-[10px]", "font-semibold", "text-[var(--muted)]")}>
                                                        { plan.clone() }
                                                    </span>
                                                }
                                            </div>
                                            <div class={classes!("mt-4", "grid", "gap-3", "grid-cols-2")}>
                                                <div>
                                                    <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "剩余" }</div>
                                                    <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black", "text-[var(--text)]")}>
                                                        { remaining_text }
                                                    </div>
                                                </div>
                                                <div>
                                                    <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "总额度" }</div>
                                                    <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black", "text-[var(--text)]")}>
                                                        { limit_text }
                                                    </div>
                                                </div>
                                            </div>
                                            <div class={classes!("mt-4")}>
                                                <div class={classes!("flex", "items-center", "justify-between", "font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>
                                                    <span>{ "用量" }</span>
                                                    <span>{ format!("{pct}%") }</span>
                                                </div>
                                                <div class={classes!("mt-1.5", "h-2", "overflow-hidden", "rounded-full", "bg-[var(--surface-alt)]")}>
                                                    <div
                                                        class={classes!("h-full", "rounded-full", "bg-[linear-gradient(90deg,#0f766e,#2563eb)]", "transition-[width]", "duration-300")}
                                                        style={format!("width: {}%;", pct.clamp(0, 100))}
                                                    />
                                                </div>
                                                <div class={classes!("mt-2", "font-mono", "text-[11px]", "text-[var(--muted)]")}>
                                                    { format_reset_hint(status.next_reset_at) }
                                                </div>
                                            </div>
                                            if let Some(ref cache_error) = status.cache.error_message {
                                                <div class={classes!("mt-3", "rounded-lg", "bg-amber-500/8", "px-3", "py-1.5", "font-mono", "text-[11px]", "text-amber-700", "dark:text-amber-200")}>
                                                    { cache_error.clone() }
                                                </div>
                                            }
                                        </article>
                                    }
                                }) }
                            </section>
                        }
                    } else {
                        html! {
                            <section class={classes!("rounded-lg", "border", "border-dashed", "border-[var(--border)]", "px-5", "py-10", "text-center", "font-mono", "text-sm", "text-[var(--muted)]")}>
                                { "当前还没有导入 Kiro 账号" }
                            </section>
                        }
                    }
                } else {
                    html! {}
                }
            }

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
