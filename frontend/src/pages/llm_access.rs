use gloo_timers::callback::{Interval, Timeout};
use serde::Deserialize;
use wasm_bindgen::{prelude::*, JsCast};
use web_sys::{HtmlInputElement, HtmlTextAreaElement, KeyboardEvent};
use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{
    api::{
        fetch_kiro_access, fetch_llm_gateway_access, fetch_llm_gateway_account_contributions,
        fetch_llm_gateway_sponsors, fetch_llm_gateway_status, fetch_llm_gateway_support_config,
        submit_llm_gateway_account_contribution_request, submit_llm_gateway_sponsor_request,
        submit_llm_gateway_token_request, KiroAccessResponse, LlmGatewayAccessResponse,
        LlmGatewayPublicKeyView, LlmGatewayRateLimitStatusResponse, LlmGatewayRateLimitWindowView,
        LlmGatewaySupportConfigView, PublicLlmGatewayAccountContributionView,
        PublicLlmGatewaySponsorView, SubmitLlmGatewayAccountContributionInput,
        SubmitLlmGatewaySponsorInput, API_BASE,
    },
    pages::llm_access_shared::{
        format_ms, format_number_i64, format_number_u64, format_percent, format_reset_hint,
        format_window_label, kiro_credit_ratio, pretty_limit_name, resolved_base_url, usage_ratio,
        MaskedSecretCode, REMOTE_COMPACT_ARTICLE_ID,
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

fn github_avatar_url(github_id: &str) -> String {
    format!("https://github.com/{}.png?size=96", github_id.trim())
}

fn github_profile_url(github_id: &str) -> String {
    format!("https://github.com/{}", github_id.trim())
}
// PLACEHOLDER_RESOLVE_SUPPORT_ASSET_URL

fn resolve_support_asset_url(path_or_url: &str) -> String {
    let normalized = path_or_url.trim();
    if normalized.starts_with("http://")
        || normalized.starts_with("https://")
        || normalized.starts_with("data:")
    {
        normalized.to_string()
    } else if normalized.starts_with("/api/") {
        format!("{}{}", API_BASE.trim_end_matches("/api"), normalized)
    } else {
        normalized.to_string()
    }
}

fn account_accent_class(index: usize) -> &'static str {
    const ACCENTS: &[&str] = &[
        "border-l-4 border-l-teal-500/70",
        "border-l-4 border-l-violet-500/70",
        "border-l-4 border-l-amber-500/70",
        "border-l-4 border-l-sky-500/70",
        "border-l-4 border-l-rose-500/70",
    ];
    ACCENTS[index % ACCENTS.len()]
}


#[derive(Clone, PartialEq)]
enum ActiveModal {
    None,
    TokenWish,
    AccountContribution,
    Sponsor,
}

#[derive(Debug, Deserialize, Default)]
struct ImportedCodexAuthTokens {
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    account_id: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct ImportedCodexAuthFile {
    #[serde(default)]
    tokens: Option<ImportedCodexAuthTokens>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    account_id: Option<String>,
}

struct ParsedImportedAuthJson {
    id_token: String,
    access_token: String,
    refresh_token: String,
    account_id: Option<String>,
}

fn parse_imported_auth_json(raw: &str) -> Result<ParsedImportedAuthJson, String> {
    let parsed: ImportedCodexAuthFile =
        serde_json::from_str(raw).map_err(|_| "auth.json 不是合法 JSON".to_string())?;
    let tokens = parsed.tokens.unwrap_or_default();
    let id_token = tokens
        .id_token
        .or(parsed.id_token)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "auth.json 缺少 id_token".to_string())?;
    let access_token = tokens
        .access_token
        .or(parsed.access_token)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "auth.json 缺少 access_token".to_string())?;
    let refresh_token = tokens
        .refresh_token
        .or(parsed.refresh_token)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "auth.json 缺少 refresh_token".to_string())?;
    let account_id = tokens
        .account_id
        .or(parsed.account_id)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    Ok(ParsedImportedAuthJson {
        id_token,
        access_token,
        refresh_token,
        account_id,
    })
}

// --- Sub-components ---

#[derive(Properties, PartialEq)]
struct PublicKeyCardProps {
    key_item: LlmGatewayPublicKeyView,
    on_copy: Callback<(String, String)>,
    on_refresh: Callback<(String, String)>,
    refreshing: bool,
}

#[derive(Properties, PartialEq)]
struct RateLimitWindowPanelProps {
    label: AttrValue,
    accent_class: Classes,
    window: LlmGatewayRateLimitWindowView,
}

#[function_component(RateLimitWindowPanel)]
fn rate_limit_window_panel(props: &RateLimitWindowPanelProps) -> Html {
    let width = props.window.remaining_percent.clamp(0.0, 100.0);
    html! {
        <article class={classes!(
            "overflow-hidden", "rounded-lg", "border", "border-[var(--border)]",
            "bg-[var(--surface)]", "p-4",
            "transition-shadow", "duration-200", "hover:shadow-[var(--shadow-sm)]",
        )}>
            <div class={classes!("flex", "items-center", "justify-between", "gap-3")}>
                <h3 class={classes!("m-0", "text-sm", "font-bold", "text-[var(--text)]")}>
                    { props.label.clone() }
                </h3>
                <span class={classes!("font-mono", "text-2xl", "font-black", "tracking-tight", "text-[var(--text)]")}>
                    { format_percent(props.window.remaining_percent) }
                </span>
            </div>
            <div class={classes!("mt-3", "h-2", "overflow-hidden", "rounded-full", "bg-[var(--surface-alt)]")}>
                <div
                    class={classes!("h-full", "rounded-full", "transition-[width]", "duration-500", props.accent_class.clone())}
                    style={format!("width: {width:.2}%;")}
                />
            </div>
            <div class={classes!("mt-2", "flex", "items-center", "gap-4", "font-mono", "text-[11px]", "text-[var(--muted)]")}>
                <span>{ format!("已用 {}", format_percent(props.window.used_percent)) }</span>
                <span>{ format_window_label(props.window.window_duration_mins, "unknown") }</span>
                <span>{ format_reset_hint(props.window.resets_at) }</span>
            </div>
        </article>
    }
}
// PLACEHOLDER_PUBLIC_KEY_CARD

#[function_component(PublicKeyCard)]
fn public_key_card(props: &PublicKeyCardProps) -> Html {
    let key_item = props.key_item.clone();
    let usage_percent = (usage_ratio(&key_item) * 100.0).round() as i32;
    html! {
        <article class={classes!(
            "group", "overflow-hidden", "rounded-lg", "border", "border-[var(--border)]",
            "bg-[var(--surface)]", "p-5",
            "transition-all", "duration-200",
            "hover:-translate-y-0.5", "hover:shadow-[0_8px_24px_rgba(0,0,0,0.08)]",
        )}>
            <div class={classes!("flex", "items-center", "justify-between", "gap-3")}>
                <h3 class={classes!("m-0", "text-base", "font-bold", "text-[var(--text)]")}>
                    { key_item.name.clone() }
                </h3>
                <button
                    type="button"
                    class={classes!("btn-terminal")}
                    title="刷新额度"
                    aria-label="刷新额度"
                    onclick={{
                        let on_refresh = props.on_refresh.clone();
                        let key_id = key_item.id.clone();
                        let key_name = key_item.name.clone();
                        Callback::from(move |_| on_refresh.emit((key_id.clone(), key_name.clone())))
                    }}
                    disabled={props.refreshing}
                >
                    <i class={classes!("fas", if props.refreshing { "fa-spinner animate-spin" } else { "fa-rotate-right" })}></i>
                </button>
            </div>
            <div class={classes!("mt-3", "rounded-lg", "bg-slate-950", "px-3", "py-2.5", "text-emerald-300")}>
                <MaskedSecretCode
                    value={key_item.secret.clone()}
                    copy_label={"Key"}
                    on_copy={props.on_copy.clone()}
                    code_class={classes!("text-emerald-300")}
                />
            </div>
            <div class={classes!("mt-4", "grid", "gap-3", "grid-cols-2")}>
                <div>
                    <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "剩余" }</div>
                    <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black", "text-[var(--text)]")}>
                        { format_number_i64(key_item.remaining_billable) }
                    </div>
                </div>
                <div>
                    <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "总额度" }</div>
                    <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black", "text-[var(--text)]")}>
                        { format_number_u64(key_item.quota_billable_limit) }
                    </div>
                </div>
            </div>
            <div class={classes!("mt-4")}>
                <div class={classes!("flex", "items-center", "justify-between", "font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>
                    <span>{ "用量" }</span>
                    <span>{ format!("{usage_percent}%") }</span>
                </div>
                <div class={classes!("mt-1.5", "h-2", "overflow-hidden", "rounded-full", "bg-[var(--surface-alt)]")}>
                    <div
                        class={classes!("h-full", "rounded-full", "bg-[linear-gradient(90deg,#0f766e,#2563eb)]", "transition-[width]", "duration-300")}
                        style={format!("width: {}%;", usage_percent.clamp(0, 100))}
                    />
                </div>
                <div class={classes!("mt-2", "flex", "items-center", "gap-4", "font-mono", "text-[11px]", "text-[var(--muted)]")}>
                    <span>{ format!("in {}", format_number_u64(key_item.usage_input_uncached_tokens)) }</span>
                    <span>{ format!("cache {}", format_number_u64(key_item.usage_input_cached_tokens)) }</span>
                    <span>{ format!("out {}", format_number_u64(key_item.usage_output_tokens)) }</span>
                    if let Some(ts) = key_item.last_used_at {
                        <span class={classes!("ml-auto")}>{ format_ms(ts) }</span>
                    }
                </div>
            </div>
        </article>
    }
}

// PLACEHOLDER_MAIN_COMPONENT

#[function_component(LlmAccessPage)]
pub fn llm_access_page() -> Html {
    let access = use_state(|| None::<LlmGatewayAccessResponse>);
    let kiro_access = use_state(|| None::<KiroAccessResponse>);
    let rate_limit_status = use_state(|| None::<LlmGatewayRateLimitStatusResponse>);
    let loading = use_state(|| true);
    let kiro_loading = use_state(|| true);
    let status_loading = use_state(|| true);
    let error = use_state(|| None::<String>);
    let kiro_error = use_state(|| None::<String>);
    let status_error = use_state(|| None::<String>);
    let support_config = use_state(|| None::<LlmGatewaySupportConfigView>);
    let support_error = use_state(|| None::<String>);
    let toast = use_state(|| None::<(String, bool)>);
    let toast_timeout = use_mut_ref(|| None::<Timeout>);
    let refreshing_key = use_state(|| None::<String>);
    let refreshing_status = use_state(|| false);
    let active_modal = use_state(|| ActiveModal::None);
    let qr_zoomed = use_state(|| false);
    let status_expanded = use_state(|| false);
    let status_section_ref = use_node_ref();
    // Token wish form
    let wish_quota = use_state(String::new);
    let wish_reason = use_state(String::new);
    let wish_email = use_state(String::new);
    let wish_submitting = use_state(|| false);
    let wish_feedback = use_state(|| None::<(String, bool)>);
    // Contributions
    let contributions = use_state(Vec::<PublicLlmGatewayAccountContributionView>::new);
    let contribution_error = use_state(|| None::<String>);
    let contribution_account_name = use_state(String::new);
    let contribution_raw_auth_json = use_state(String::new);
    let contribution_raw_auth_feedback = use_state(|| None::<(String, bool)>);
    let contribution_account_id = use_state(String::new);
    let contribution_id_token = use_state(String::new);
    let contribution_access_token = use_state(String::new);
    let contribution_refresh_token = use_state(String::new);
    let contribution_email = use_state(String::new);
    let contribution_message = use_state(String::new);
    let contribution_github_id = use_state(String::new);
    let contribution_submitting = use_state(|| false);
    let contribution_feedback = use_state(|| None::<(String, bool)>);
    // Sponsors
    let sponsors = use_state(Vec::<PublicLlmGatewaySponsorView>::new);
    let sponsor_error = use_state(|| None::<String>);
    let sponsor_email = use_state(String::new);
    let sponsor_display_name = use_state(String::new);
    let sponsor_github_id = use_state(String::new);
    let sponsor_message = use_state(String::new);
    let sponsor_submitting = use_state(|| false);
    let sponsor_feedback = use_state(|| None::<(String, bool)>);

    // --- Data fetching effects ---
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
    {
        let kiro_access = kiro_access.clone();
        let kiro_loading = kiro_loading.clone();
        let kiro_error = kiro_error.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_kiro_access().await {
                    Ok(data) => {
                        kiro_access.set(Some(data));
                        kiro_error.set(None);
                    },
                    Err(err) => {
                        kiro_access.set(None);
                        kiro_error.set(Some(err));
                    },
                }
                kiro_loading.set(false);
            });
            || ()
        });
    }
    {
        let rate_limit_status = rate_limit_status.clone();
        let status_loading = status_loading.clone();
        let status_error = status_error.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_llm_gateway_status().await {
                    Ok(data) => {
                        rate_limit_status.set(Some(data));
                        status_error.set(None);
                    },
                    Err(err) => {
                        rate_limit_status.set(None);
                        status_error.set(Some(err));
                    },
                }
                status_loading.set(false);
            });
            || ()
        });
    }
    {
        let contributions = contributions.clone();
        let contribution_error = contribution_error.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_llm_gateway_account_contributions().await {
                    Ok(data) => {
                        contributions.set(data.contributions);
                        contribution_error.set(None);
                    },
                    Err(err) => {
                        contributions.set(vec![]);
                        contribution_error.set(Some(err));
                    },
                }
            });
            || ()
        });
    }
    {
        let support_config = support_config.clone();
        let support_error = support_error.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_llm_gateway_support_config().await {
                    Ok(data) => {
                        support_config.set(Some(data));
                        support_error.set(None);
                    },
                    Err(err) => {
                        support_config.set(None);
                        support_error.set(Some(err));
                    },
                }
            });
            || ()
        });
    }
    {
        let sponsors = sponsors.clone();
        let sponsor_error = sponsor_error.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_llm_gateway_sponsors().await {
                    Ok(data) => {
                        sponsors.set(data.sponsors);
                        sponsor_error.set(None);
                    },
                    Err(err) => {
                        sponsors.set(vec![]);
                        sponsor_error.set(Some(err));
                    },
                }
            });
            || ()
        });
    }
    // Auto-refresh rate limit status every 30s
    {
        let rate_limit_status = rate_limit_status.clone();
        let status_error = status_error.clone();
        use_effect_with((), move |_| {
            let interval = Interval::new(30_000, move || {
                let rate_limit_status = rate_limit_status.clone();
                let status_error = status_error.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    match fetch_llm_gateway_status().await {
                        Ok(data) => {
                            rate_limit_status.set(Some(data));
                            status_error.set(None);
                        },
                        Err(err) => {
                            status_error.set(Some(err));
                        },
                    }
                });
            });
            move || drop(interval)
        });
    }
    // Unified Escape key handler
    {
        let qr_zoomed = qr_zoomed.clone();
        let active_modal = active_modal.clone();
        let has_overlay = *qr_zoomed || *active_modal != ActiveModal::None;
        use_effect_with(has_overlay, move |is_open| {
            let listener_opt = if *is_open {
                let qr_h = qr_zoomed.clone();
                let modal_h = active_modal.clone();
                let listener =
                    wasm_bindgen::closure::Closure::wrap(Box::new(move |event: KeyboardEvent| {
                        if event.key() == "Escape" {
                            if *qr_h {
                                qr_h.set(false);
                            } else if *modal_h != ActiveModal::None {
                                modal_h.set(ActiveModal::None);
                            }
                        }
                    })
                        as Box<dyn FnMut(_)>);
                if let Some(win) = web_sys::window() {
                    let _ = win.add_event_listener_with_callback(
                        "keydown",
                        listener.as_ref().unchecked_ref(),
                    );
                }
                Some(listener)
            } else {
                None
            };
            move || {
                if let Some(listener) = listener_opt {
                    if let Some(win) = web_sys::window() {
                        let _ = win.remove_event_listener_with_callback(
                            "keydown",
                            listener.as_ref().unchecked_ref(),
                        );
                    }
                }
            }
        });
    }
    // PLACEHOLDER_CALLBACKS

    let show_toast = {
        let toast = toast.clone();
        let toast_timeout = toast_timeout.clone();
        move |msg: String, is_error: bool, duration_ms: u32| {
            toast.set(Some((msg, is_error)));
            toast_timeout.borrow_mut().take();
            let toast = toast.clone();
            let clear_handle = toast_timeout.clone();
            let timeout = Timeout::new(duration_ms, move || {
                toast.set(None);
                clear_handle.borrow_mut().take();
            });
            *toast_timeout.borrow_mut() = Some(timeout);
        }
    };

    let on_copy = {
        let show_toast = show_toast.clone();
        Callback::from(move |(label, value): (String, String)| {
            copy_text(&value);
            show_toast(format!("已复制{}", label), false, 1800);
        })
    };

    let on_refresh_key = {
        let access = access.clone();
        let refreshing_key = refreshing_key.clone();
        let show_toast = show_toast.clone();
        Callback::from(move |(key_id, key_name): (String, String)| {
            refreshing_key.set(Some(key_id));
            let access = access.clone();
            let refreshing_key = refreshing_key.clone();
            let show_toast = show_toast.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_llm_gateway_access().await {
                    Ok(data) => {
                        access.set(Some(data));
                        show_toast(format!("已刷新 {}", key_name), false, 2200);
                    },
                    Err(err) => {
                        show_toast(format!("刷新失败：{}", err), true, 2200);
                    },
                }
                refreshing_key.set(None);
            });
        })
    };

    let on_refresh_status = {
        let rate_limit_status = rate_limit_status.clone();
        let status_error = status_error.clone();
        let refreshing_status = refreshing_status.clone();
        let show_toast = show_toast.clone();
        let kiro_access = kiro_access.clone();
        Callback::from(move |_| {
            refreshing_status.set(true);
            let rate_limit_status = rate_limit_status.clone();
            let status_error = status_error.clone();
            let refreshing_status = refreshing_status.clone();
            let show_toast = show_toast.clone();
            let kiro_access = kiro_access.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let (codex_result, kiro_result) =
                    futures::join!(fetch_llm_gateway_status(), fetch_kiro_access(),);
                match codex_result {
                    Ok(data) => {
                        rate_limit_status.set(Some(data));
                        status_error.set(None);
                    },
                    Err(err) => {
                        status_error.set(Some(err.clone()));
                        show_toast(format!("刷新失败：{}", err), true, 2200);
                    },
                }
                if let Ok(data) = kiro_result {
                    kiro_access.set(Some(data));
                }
                show_toast("已刷新限额快照".to_string(), false, 2200);
                refreshing_status.set(false);
            });
        })
    };
    // PLACEHOLDER_FORM_CALLBACKS

    let on_submit_token_wish = {
        let wish_quota = wish_quota.clone();
        let wish_reason = wish_reason.clone();
        let wish_email = wish_email.clone();
        let wish_submitting = wish_submitting.clone();
        let wish_feedback = wish_feedback.clone();
        let active_modal = active_modal.clone();
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            let quota_raw = (*wish_quota).trim().to_string();
            let reason = (*wish_reason).trim().to_string();
            let email = (*wish_email).trim().to_string();
            let Ok(quota) = quota_raw.parse::<u64>() else {
                wish_feedback.set(Some(("token 量必须是正整数".to_string(), true)));
                return;
            };
            if quota == 0 || reason.is_empty() || email.is_empty() {
                wish_feedback.set(Some(("token 量、缘由和邮箱都必须填写".to_string(), true)));
                return;
            }
            let frontend_page_url = web_sys::window().and_then(|w| w.location().href().ok());
            let wish_quota = wish_quota.clone();
            let wish_reason = wish_reason.clone();
            let wish_email = wish_email.clone();
            let wish_submitting = wish_submitting.clone();
            let wish_feedback = wish_feedback.clone();
            let active_modal = active_modal.clone();
            wish_submitting.set(true);
            wish_feedback.set(None);
            wasm_bindgen_futures::spawn_local(async move {
                match submit_llm_gateway_token_request(
                    quota,
                    &reason,
                    &email,
                    frontend_page_url.as_deref(),
                )
                .await
                {
                    Ok(_) => {
                        wish_quota.set(String::new());
                        wish_reason.set(String::new());
                        wish_email.set(String::new());
                        wish_feedback.set(Some((
                            "许愿已提交，审核通过后会创建 token 并发送到你的邮箱。".to_string(),
                            false,
                        )));
                        active_modal.set(ActiveModal::None);
                    },
                    Err(err) => {
                        wish_feedback.set(Some((err, true)));
                    },
                }
                wish_submitting.set(false);
            });
        })
    };

    let on_submit_account_contribution = {
        let contribution_account_name = contribution_account_name.clone();
        let contribution_raw_auth_json = contribution_raw_auth_json.clone();
        let contribution_raw_auth_feedback = contribution_raw_auth_feedback.clone();
        let contribution_account_id = contribution_account_id.clone();
        let contribution_id_token = contribution_id_token.clone();
        let contribution_access_token = contribution_access_token.clone();
        let contribution_refresh_token = contribution_refresh_token.clone();
        let contribution_email = contribution_email.clone();
        let contribution_message = contribution_message.clone();
        let contribution_github_id = contribution_github_id.clone();
        let contribution_submitting = contribution_submitting.clone();
        let contribution_feedback = contribution_feedback.clone();
        let active_modal = active_modal.clone();
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            let account_name = (*contribution_account_name).trim().to_string();
            let account_id = (*contribution_account_id).trim().to_string();
            let id_token = (*contribution_id_token).trim().to_string();
            let access_token_val = (*contribution_access_token).trim().to_string();
            let refresh_token_val = (*contribution_refresh_token).trim().to_string();
            let email = (*contribution_email).trim().to_string();
            let message = (*contribution_message).trim().to_string();
            let github_id = (*contribution_github_id).trim().to_string();
            if account_name.is_empty()
                || id_token.is_empty()
                || access_token_val.is_empty()
                || refresh_token_val.is_empty()
                || email.is_empty()
                || message.is_empty()
            {
                contribution_feedback
                    .set(Some(("账号名、三段 token、邮箱和留言都必须填写".to_string(), true)));
                return;
            }
            let frontend_page_url = web_sys::window().and_then(|w| w.location().href().ok());
            let contribution_account_name = contribution_account_name.clone();
            let contribution_raw_auth_json = contribution_raw_auth_json.clone();
            let contribution_raw_auth_feedback = contribution_raw_auth_feedback.clone();
            let contribution_account_id = contribution_account_id.clone();
            let contribution_id_token = contribution_id_token.clone();
            let contribution_access_token = contribution_access_token.clone();
            let contribution_refresh_token = contribution_refresh_token.clone();
            let contribution_email = contribution_email.clone();
            let contribution_message = contribution_message.clone();
            let contribution_github_id = contribution_github_id.clone();
            let contribution_submitting = contribution_submitting.clone();
            let contribution_feedback = contribution_feedback.clone();
            let active_modal = active_modal.clone();
            contribution_submitting.set(true);
            contribution_feedback.set(None);
            wasm_bindgen_futures::spawn_local(async move {
                let input = SubmitLlmGatewayAccountContributionInput {
                    account_name,
                    account_id: (!account_id.is_empty()).then_some(account_id),
                    id_token,
                    access_token: access_token_val,
                    refresh_token: refresh_token_val,
                    requester_email: email,
                    contributor_message: message,
                    github_id: (!github_id.is_empty()).then_some(github_id),
                    frontend_page_url,
                };
                match submit_llm_gateway_account_contribution_request(&input).await {
                    Ok(_) => {
                        contribution_account_name.set(String::new());
                        contribution_raw_auth_json.set(String::new());
                        contribution_raw_auth_feedback.set(None);
                        contribution_account_id.set(String::new());
                        contribution_id_token.set(String::new());
                        contribution_access_token.set(String::new());
                        contribution_refresh_token.set(String::new());
                        contribution_email.set(String::new());
                        contribution_message.set(String::new());
                        contribution_github_id.set(String::new());
                        contribution_feedback.set(Some((
                            "账号贡献已提交，审核通过后系统会导入账号并把 token 发到你的邮箱。"
                                .to_string(),
                            false,
                        )));
                        active_modal.set(ActiveModal::None);
                    },
                    Err(err) => contribution_feedback.set(Some((err, true))),
                }
                contribution_submitting.set(false);
            });
        })
    };

    let on_submit_sponsor = {
        let sponsor_email = sponsor_email.clone();
        let sponsor_display_name = sponsor_display_name.clone();
        let sponsor_github_id = sponsor_github_id.clone();
        let sponsor_message = sponsor_message.clone();
        let sponsor_submitting = sponsor_submitting.clone();
        let sponsor_feedback = sponsor_feedback.clone();
        let active_modal = active_modal.clone();
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            let email = (*sponsor_email).trim().to_string();
            let display_name = (*sponsor_display_name).trim().to_string();
            let github_id = (*sponsor_github_id).trim().to_string();
            let message = (*sponsor_message).trim().to_string();
            if email.is_empty() || message.is_empty() {
                sponsor_feedback.set(Some(("邮箱和留言都必须填写".to_string(), true)));
                return;
            }
            let frontend_page_url = web_sys::window().and_then(|w| w.location().href().ok());
            let sponsor_email = sponsor_email.clone();
            let sponsor_display_name = sponsor_display_name.clone();
            let sponsor_github_id = sponsor_github_id.clone();
            let sponsor_message = sponsor_message.clone();
            let sponsor_submitting = sponsor_submitting.clone();
            let sponsor_feedback = sponsor_feedback.clone();
            let active_modal = active_modal.clone();
            sponsor_submitting.set(true);
            sponsor_feedback.set(None);
            wasm_bindgen_futures::spawn_local(async move {
                let input = SubmitLlmGatewaySponsorInput {
                    requester_email: email,
                    sponsor_message: message,
                    display_name: (!display_name.is_empty()).then_some(display_name),
                    github_id: (!github_id.is_empty()).then_some(github_id),
                    frontend_page_url,
                };
                match submit_llm_gateway_sponsor_request(&input).await {
                    Ok(response) => {
                        sponsor_email.set(String::new());
                        sponsor_display_name.set(String::new());
                        sponsor_github_id.set(String::new());
                        sponsor_message.set(String::new());
                        let feedback = if response.payment_email_sent {
                            "收款码和付款说明已发到你的邮箱，付款后请直接回复那封邮件。".to_string()
                        } else {
                            format!(
                                "赞助请求已记录（状态 \
                                 {}），付款说明邮件暂未发出，可通过群或邮箱联系。",
                                response.status
                            )
                        };
                        sponsor_feedback.set(Some((feedback, false)));
                        active_modal.set(ActiveModal::None);
                    },
                    Err(err) => sponsor_feedback.set(Some((err, true))),
                }
                sponsor_submitting.set(false);
            });
        })
    };
    // PLACEHOLDER_CONTENT_RENDER

    // Pre-compute QR URL for lightbox (accessible outside content closure)
    let group_qr_url_for_lightbox = (*support_config)
        .as_ref()
        .and_then(|c| c.qq_group_qr_url.as_deref())
        .map(resolve_support_asset_url);

    let content = if *loading {
        html! {
            <div class={classes!("mt-10", "rounded-lg", "border", "border-dashed", "border-[var(--border)]", "px-5", "py-12", "text-center", "font-mono", "text-sm", "text-[var(--muted)]")}>
                { "> loading keys..." }
            </div>
        }
    } else if let Some(err) = (*error).clone() {
        html! {
            <div class={classes!("mt-10", "rounded-lg", "border", "border-red-400/35", "bg-red-500/8", "px-5", "py-5", "font-mono", "text-sm", "text-red-700", "dark:text-red-200")}>
                { err }
            </div>
        }
    } else if let Some(access) = (*access).clone() {
        let base_url = resolved_base_url(&access);
        let kiro_access_value = (*kiro_access).clone();
        let support_config_value = (*support_config).clone();
        let support_error_value = (*support_error).clone();
        let group_name = support_config_value
            .as_ref()
            .map(|c| c.group_name.clone())
            .unwrap_or_else(|| "美区词元魔盗团".to_string());
        let group_number = support_config_value
            .as_ref()
            .map(|c| c.qq_group_number.clone())
            .unwrap_or_default();
        let group_qr_url = support_config_value
            .as_ref()
            .and_then(|c| c.qq_group_qr_url.as_deref())
            .map(resolve_support_asset_url);

        // --- Status view ---
        let status_view = if *status_loading {
            html! {
                <div class={classes!("rounded-lg", "border", "border-dashed", "border-[var(--border)]", "px-5", "py-12", "text-center", "font-mono", "text-sm", "text-[var(--muted)]")}>
                    { "> loading rate limits..." }
                </div>
            }
        } else if let Some(status) = (*rate_limit_status).clone() {
            let effective_status_error = (*status_error)
                .clone()
                .or_else(|| status.error_message.clone());
            let mut account_groups: Vec<(
                Option<String>,
                Vec<crate::api::LlmGatewayRateLimitBucketView>,
            )> = Vec::new();
            {
                let mut seen_order: Vec<Option<String>> = Vec::new();
                let mut map: std::collections::HashMap<
                    Option<String>,
                    Vec<crate::api::LlmGatewayRateLimitBucketView>,
                > = std::collections::HashMap::new();
                for bucket in status.buckets.iter() {
                    let key = bucket.account_name.clone();
                    if !map.contains_key(&key) {
                        seen_order.push(key.clone());
                    }
                    map.entry(key).or_default().push(bucket.clone());
                }
                for key in seen_order {
                    if let Some(buckets) = map.remove(&key) {
                        account_groups.push((key, buckets));
                    }
                }
            }
            html! {
                <section
                    ref={status_section_ref.clone()}
                    class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}
                >
                    <div
                        class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap", "cursor-pointer")}
                        onclick={{
                            let status_expanded = status_expanded.clone();
                            Callback::from(move |_: MouseEvent| status_expanded.set(!*status_expanded))
                        }}
                    >
                        <div class={classes!("flex", "items-center", "gap-3")}>
                            <i class={classes!("fas", if *status_expanded { "fa-chevron-down" } else { "fa-chevron-right" }, "text-xs", "text-[var(--muted)]")}></i>
                            <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>
                                { "限额状态" }
                            </h2>
                            <span class={classes!(
                                "inline-flex", "items-center", "gap-1.5",
                                "rounded-full", "px-2.5", "py-0.5",
                                "font-mono", "text-[11px]", "font-semibold", "uppercase", "tracking-wider",
                                "bg-[var(--surface-alt)]",
                                match status.status.as_str() {
                                    "ready" => "text-emerald-600",
                                    "degraded" => "text-amber-600",
                                    "error" => "text-red-600",
                                    _ => "text-[var(--muted)]",
                                }
                            )}>
                                <span class={classes!("inline-block", "h-1.5", "w-1.5", "rounded-full", match status.status.as_str() {
                                    "ready" => "bg-emerald-500",
                                    "degraded" => "bg-amber-500",
                                    "error" => "bg-red-500",
                                    _ => "bg-slate-400",
                                })} />
                                { status.status.clone() }
                            </span>
                            if kiro_access_value.as_ref().is_some_and(|k| !k.accounts.is_empty()) {
                                <span class={classes!("inline-flex", "items-center", "rounded-full", "bg-slate-900", "px-2", "py-0.5", "font-mono", "text-[10px]", "font-semibold", "uppercase", "tracking-[0.16em]", "text-emerald-300")}>
                                    { "Kiro" }
                                </span>
                            }
                        </div>
                        <button
                            type="button"
                            class={classes!("btn-terminal")}
                            onclick={{
                                let on_refresh_status = on_refresh_status.clone();
                                Callback::from(move |e: MouseEvent| { e.stop_propagation(); on_refresh_status.emit(()); })
                            }}
                            disabled={*refreshing_status}
                        >
                            <i class={classes!("fas", if *refreshing_status { "fa-spinner animate-spin" } else { "fa-rotate-right" })}></i>
                        </button>
                    </div>

                    if *status_expanded {
                    // Account groups
                    { for account_groups.iter().enumerate().map(|(group_idx, (account_name, group_buckets))| {
                        let primary_bucket = group_buckets.iter().find(|b| b.is_primary).cloned()
                            .or_else(|| group_buckets.first().cloned());
                        let additional_buckets: Vec<_> = group_buckets.iter().filter(|b| !b.is_primary).cloned().collect();
                        let group_label = account_name.as_deref().unwrap_or("default");
                        let show_account_header = account_groups.len() > 1 || account_name.is_some();
                        html! {
                            <div class={classes!(
                                "mt-4", "rounded-lg", "border", "border-[var(--border)]",
                                "bg-[var(--surface-alt)]", "p-4",
                                account_accent_class(group_idx),
                            )}>
                                if show_account_header {
                                    <div class={classes!("mb-3")}>
                                        <span class={classes!(
                                            "font-mono", "text-[11px]", "font-bold", "uppercase", "tracking-wider",
                                            "text-[var(--primary)]",
                                        )}>
                                            { group_label }
                                        </span>
                                    </div>
                                }
                                if let Some(primary_bucket) = primary_bucket.clone() {
                                    <div>
                                        <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                            <span class={classes!("font-mono", "text-sm", "font-bold", "text-[var(--text)]")}>
                                                { pretty_limit_name(&primary_bucket.display_name) }
                                            </span>
                                            <div class={classes!("flex", "items-center", "gap-3")}>
                                                if let Some(credits) = primary_bucket.credits.clone() {
                                                    <span class={classes!("font-mono", "text-[11px]", "text-[var(--muted)]")}>
                                                        { if !credits.has_credits { "credits:n/a" } else if credits.unlimited { "credits:∞" } else { "credits:✓" } }
                                                    </span>
                                                }
                                                if let Some(ref plan) = primary_bucket.plan_type {
                                                    <span class={classes!("font-mono", "text-[11px]", "text-[var(--muted)]")}>
                                                        { plan.clone() }
                                                    </span>
                                                }
                                            </div>
                                        </div>
                                        <div class={classes!("mt-3", "grid", "gap-3", "sm:grid-cols-2")}>
                                            if let Some(w) = primary_bucket.primary.clone() {
                                                <RateLimitWindowPanel label={"5h"} accent_class={classes!("bg-[linear-gradient(90deg,#0f766e,#14b8a6)]")} window={w} />
                                            }
                                            if let Some(w) = primary_bucket.secondary.clone() {
                                                <RateLimitWindowPanel label={"weekly"} accent_class={classes!("bg-[linear-gradient(90deg,#2563eb,#7c3aed)]")} window={w} />
                                            }
                                        </div>
                                    </div>
                                }
                                if !additional_buckets.is_empty() {
                                    <div class={classes!("mt-3", "space-y-2")}>
                                        { for additional_buckets.iter().map(|bucket| {
                                            let bp = bucket.primary.clone();
                                            let bs = bucket.secondary.clone();
                                            html! {
                                                <div class={classes!("flex", "items-center", "justify-between", "gap-4", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "flex-wrap")}>
                                                    <span class={classes!("font-mono", "text-xs", "font-semibold", "text-[var(--text)]")}>
                                                        { pretty_limit_name(&bucket.display_name) }
                                                    </span>
                                                    <div class={classes!("flex", "items-center", "gap-3", "font-mono", "text-xs")}>
                                                        if let Some(p) = bp {
                                                            <span class={classes!("text-[var(--text)]")}>
                                                                { format!("5h {}", format_percent(p.remaining_percent)) }
                                                            </span>
                                                        }
                                                        if let Some(s) = bs {
                                                            <span class={classes!("text-[var(--text)]")}>
                                                                { format!("wk {}", format_percent(s.remaining_percent)) }
                                                            </span>
                                                        }
                                                    </div>
                                                </div>
                                            }
                                        }) }
                                    </div>
                                }
                            </div>
                        }
                    }) }

                    // Kiro accounts (inside the collapsible status section)
                    if let Some(kiro_data) = kiro_access_value.clone() {
                        if !kiro_data.accounts.is_empty() {
                            { for kiro_data.accounts.into_iter().map(|acct| {
                                let ratio = kiro_credit_ratio(acct.current_usage, acct.usage_limit);
                                let pct = (ratio * 100.0).round() as i32;
                                let remaining_text = acct.remaining.map(|v| format!("{v:.0}")).unwrap_or_else(|| "-".to_string());
                                let limit_text = acct.usage_limit.map(|v| format!("{v:.0}")).unwrap_or_else(|| "-".to_string());
                                html! {
                                    <div class={classes!(
                                        "mt-4", "rounded-lg", "border", "border-[var(--border)]",
                                        "bg-[var(--surface-alt)]", "p-4",
                                        "border-l-[3px]", "border-l-emerald-500/50",
                                    )}>
                                        <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                                            <span class={classes!("inline-flex", "items-center", "rounded-full", "bg-slate-900", "px-2", "py-0.5", "font-mono", "text-[10px]", "font-semibold", "uppercase", "tracking-[0.16em]", "text-emerald-300")}>
                                                { "Kiro" }
                                            </span>
                                            <span class={classes!("font-mono", "text-[11px]", "font-bold", "uppercase", "tracking-wider", "text-[var(--text)]")}>
                                                { acct.name }
                                            </span>
                                            if acct.is_active {
                                                <span class={classes!("inline-flex", "rounded-full", "border", "border-emerald-500/20", "bg-emerald-500/10", "px-2", "py-0.5", "text-[10px]", "font-semibold", "uppercase", "tracking-[0.12em]", "text-emerald-700", "dark:text-emerald-200")}>
                                                    { "active" }
                                                </span>
                                            }
                                            if acct.disabled {
                                                <span class={classes!("text-[10px]", "font-semibold", "uppercase", "tracking-[0.12em]", "text-amber-700", "dark:text-amber-200")}>
                                                    { "disabled" }
                                                </span>
                                            }
                                            <span class={classes!("ml-auto", "font-mono", "text-xs", "text-[var(--muted)]")}>
                                                { format!("{remaining_text} / {limit_text}") }
                                            </span>
                                        </div>
                                        <div class={classes!("mt-2.5", "h-1.5", "overflow-hidden", "rounded-full", "bg-[var(--surface)]")}>
                                            <div
                                                class={classes!("h-full", "rounded-full", "bg-[linear-gradient(90deg,#0f766e,#2563eb)]", "transition-[width]", "duration-300")}
                                                style={format!("width: {}%;", pct.clamp(0, 100))}
                                            />
                                        </div>
                                        <div class={classes!("mt-1.5", "flex", "items-center", "gap-3", "font-mono", "text-[10px]", "text-[var(--muted)]")}>
                                            <span>{ acct.subscription_title.unwrap_or_else(|| "-".to_string()) }</span>
                                            <span class={classes!("ml-auto")}>{ format_reset_hint(acct.next_reset_at) }</span>
                                        </div>
                                    </div>
                                }
                            }) }
                        }
                    } else if *kiro_loading {
                        <div class={classes!("mt-4", "font-mono", "text-[11px]", "text-[var(--muted)]")}>
                            { "Loading Kiro quota..." }
                        </div>
                    }

                    <div class={classes!("mt-4", "flex", "items-center", "gap-4", "font-mono", "text-[11px]", "text-[var(--muted)]", "flex-wrap")}>
                        <span>{ format!("refresh {}s", status.refresh_interval_seconds) }</span>
                        if let Some(ts) = status.last_success_at {
                            <span>{ format!("last_ok {}", format_ms(ts)) }</span>
                        }
                    </div>
                    if let Some(error_message) = effective_status_error {
                        <div class={classes!("mt-3", "llm-access-notice")}>{ error_message }</div>
                    }
                    } // end status_expanded
                </section>
            }
        } else if let Some(err) = (*status_error).clone() {
            html! {
                <div class={classes!("rounded-lg", "border", "border-red-400/35", "bg-red-500/8", "px-5", "py-5", "font-mono", "text-sm", "text-red-700", "dark:text-red-200")}>
                    { err }
                </div>
            }
        } else {
            Html::default()
        };
        // PLACEHOLDER_FINAL_HTML

        html! {
                    <>
                        // Page header
                        <section class={classes!("mt-8", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                            <div class={classes!("flex", "items-start", "justify-between", "gap-4", "flex-wrap")}>
                                <div>
                                    <div class={classes!("flex", "items-center", "gap-3", "flex-wrap")}>
                                        <h1 class={classes!("m-0", "font-mono", "text-xl", "font-bold", "text-[var(--text)]")}>
                                            { "LLM Gateway" }
                                        </h1>
                                        <span class={classes!("rounded-full", "bg-[var(--surface-alt)]", "px-2.5", "py-0.5", "font-mono", "text-[11px]", "font-semibold", "text-[var(--muted)]")}>
                                            { format!("{} keys", access.keys.len()) }
                                        </span>
                                    </div>
                                    <div class={classes!("mt-2", "flex", "items-center", "gap-2")}>
                                        <code class={classes!("break-all", "font-mono", "text-sm", "text-[var(--muted)]")}>{ base_url.clone() }</code>
                                    </div>
                                </div>
                                <div class={classes!("flex", "items-center", "gap-2")}>
                                    <button
                                        class={classes!("btn-terminal")}
                                        onclick={{
                                            let on_copy = on_copy.clone();
                                            let base_url = base_url.clone();
                                            Callback::from(move |_| on_copy.emit(("Base URL".to_string(), base_url.clone())))
                                        }}
                                    >
                                        <i class="fas fa-copy"></i>
                                    </button>
                                    <Link<Route> to={Route::LlmAccessGuide} classes={classes!("btn-terminal")}>
                                        <i class="fas fa-book"></i>
                                        { "接入帮助" }
                                    </Link<Route>>
                                    <Link<Route> to={Route::KiroAccess} classes={classes!("btn-terminal")}>
                                        <i class="fas fa-bolt"></i>
                                        { "Kiro" }
                                    </Link<Route>>
                                    <Link<Route> to={Route::AdminLlmGateway} classes={classes!("btn-terminal")}>
                                        <i class="fas fa-sliders"></i>
                                    </Link<Route>>
                                </div>
                            </div>
                        </section>

                        // Notice bar
                        <div class={classes!("mt-4", "llm-access-notice", "font-mono", "text-[11px]")}>
                            { "remote compact 必须保留 — " }
                            <Link<Route> to={Route::LlmAccessGuide} classes={classes!("underline", "text-[var(--primary)]")}>
                                { "接入帮助" }
                            </Link<Route>>
                            { " · " }
                            <Link<Route> to={Route::ArticleDetail { id: REMOTE_COMPACT_ARTICLE_ID.to_string() }} classes={classes!("underline", "text-[var(--primary)]")}>
                                { "深潜文章" }
                            </Link<Route>>
                        </div>

                        // Keys section
                        <section class={classes!("mt-6")}>
                            <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>
                                { "公开 Key" }
                            </h2>
                            if access.keys.is_empty() {
                                <div class={classes!("mt-3", "rounded-lg", "border", "border-dashed", "border-[var(--border)]", "px-5", "py-10", "text-center", "font-mono", "text-sm", "text-[var(--muted)]")}>
                                    { "当前没有公开放出的 Key" }
                                </div>
                            } else {
                                <div class={classes!("mt-3", "grid", "gap-4", "lg:grid-cols-2")}>
                                    { for access.keys.iter().map(|key_item| html! {
                                        <PublicKeyCard
                                            key={key_item.id.clone()}
                                            key_item={key_item.clone()}
                                            on_copy={on_copy.clone()}
                                            on_refresh={on_refresh_key.clone()}
                                            refreshing={(*refreshing_key).as_deref() == Some(key_item.id.as_str())}
                                        />
                                    }) }
                                </div>
                            }
                        </section>

                        // Action buttons
                        <section class={classes!("mt-6", "flex", "items-center", "gap-2", "flex-wrap")}>
                            <button
                                type="button"
                                class={classes!("btn-terminal")}
                                onclick={{
                                    let active_modal = active_modal.clone();
                                    Callback::from(move |_| active_modal.set(ActiveModal::TokenWish))
                                }}
                            >
                                <i class="fas fa-wand-magic-sparkles"></i>
                                { "许愿 Token" }
                            </button>
                            <button
                                type="button"
                                class={classes!("btn-terminal")}
                                onclick={{
                                    let active_modal = active_modal.clone();
                                    Callback::from(move |_| active_modal.set(ActiveModal::AccountContribution))
                                }}
                            >
                                <i class="fas fa-user-plus"></i>
                                { "贡献账号" }
                            </button>
                            <button
                                type="button"
                                class={classes!("btn-terminal")}
                                onclick={{
                                    let active_modal = active_modal.clone();
                                    Callback::from(move |_| active_modal.set(ActiveModal::Sponsor))
                                }}
                            >
                                <i class="fas fa-mug-hot"></i>
                                { "赞助站点" }
                            </button>
                        </section>

                        // Status section
                        <section class={classes!("mt-6")}>
                            { status_view }
                        </section>

                        // --- QQ group card ---
                        if !group_number.is_empty() {
                            <section class={classes!(
                                "mt-6", "rounded-lg", "border", "border-[var(--border)]",
                                "bg-[var(--surface)]", "p-5",
                            )}>
                                <div class={classes!("flex", "items-start", "gap-4", "flex-wrap")}>
                                    if let Some(group_qr_url) = group_qr_url.clone() {
                                        <div
                                            class={classes!("shrink-0", "cursor-pointer", "transition-transform", "duration-200", "hover:scale-105")}
                                            onclick={{
                                                let qr_zoomed = qr_zoomed.clone();
                                                Callback::from(move |_: MouseEvent| qr_zoomed.set(true))
                                            }}
                                            role="button"
                                            tabindex="0"
                                            aria-label="放大查看 QR 码"
                                        >
                                            <img
                                                src={group_qr_url}
                                                alt="QQ group QR"
                                                class={classes!("h-20", "w-20", "rounded-lg", "border", "border-[var(--border)]", "object-cover", "bg-white")}
                                                loading="lazy"
                                            />
                                        </div>
                                    }
                                    <div class={classes!("min-w-0", "flex-1")}>
                                        <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                                            <h2 class={classes!("m-0", "font-mono", "text-lg", "font-bold", "llm-group-name")}>
                                                { group_name.clone() }
                                            </h2>
                                            <span class={classes!("font-mono", "text-[11px]", "text-[var(--muted)]")}>
                                                { group_number.clone() }
                                            </span>
                                            <button
                                                type="button"
                                                class={classes!("btn-terminal", "!py-1", "!px-2")}
                                                onclick={{
                                                    let on_copy = on_copy.clone();
                                                    let group_number = group_number.clone();
                                                    Callback::from(move |_| on_copy.emit(("群号".to_string(), group_number.clone())))
                                                }}
                                            >
                                                <i class="fas fa-copy"></i>
                                            </button>
                                        </div>
                                        <p class={classes!("mt-2", "m-0", "text-sm", "leading-relaxed", "text-[var(--muted)]")}>
                                            { "欢迎加 QQ 群一起薅羊毛、聊模型、分享 prompt，遇到问题也能快速解决。扫码或搜群号直接加入 \u{1f389}" }
                                        </p>
                                    </div>
                                </div>
                            </section>
                        }

                        // Feedback toasts from forms (shown on main page after modal closes)
                        if let Some((message, is_error)) = (*wish_feedback).clone() {
                            <div class={classes!("mt-4", "rounded-lg", "border", "px-4", "py-3", "font-mono", "text-sm",
                                if is_error { classes!("border-red-400/35", "bg-red-500/8", "text-red-700", "dark:text-red-200") }
                                else { classes!("border-emerald-400/35", "bg-emerald-500/8", "text-emerald-700", "dark:text-emerald-200") }
                            )}>{ message }</div>
                        }
                        if let Some((message, is_error)) = (*contribution_feedback).clone() {
                            <div class={classes!("mt-4", "rounded-lg", "border", "px-4", "py-3", "font-mono", "text-sm",
                                if is_error { classes!("border-red-400/35", "bg-red-500/8", "text-red-700", "dark:text-red-200") }
                                else { classes!("border-emerald-400/35", "bg-emerald-500/8", "text-emerald-700", "dark:text-emerald-200") }
                            )}>{ message }</div>
                        }
                        if let Some((message, is_error)) = (*sponsor_feedback).clone() {
                            <div class={classes!("mt-4", "rounded-lg", "border", "px-4", "py-3", "font-mono", "text-sm",
                                if is_error { classes!("border-red-400/35", "bg-red-500/8", "text-red-700", "dark:text-red-200") }
                                else { classes!("border-emerald-400/35", "bg-emerald-500/8", "text-emerald-700", "dark:text-emerald-200") }
                            )}>{ message }</div>
                        }

                        if let Some(err) = support_error_value.clone() {
                            <div class={classes!("mt-4", "llm-access-notice")}>
                                { format!("社区配置暂不可用：{}", err) }
                            </div>
                        }
        // PLACEHOLDER_THANK_YOU_WALLS

                        // Contribution thank-you wall
                        if !contributions.is_empty() || (*contribution_error).is_some() {
                            <section class={classes!("mt-6")}>
                                <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]", "before:content-['//']", "before:mr-2", "before:text-[var(--muted)]", "before:opacity-40", "before:font-normal")}>
                                    { "贡献感谢墙" }
                                </h2>
                                if let Some(err) = (*contribution_error).clone() {
                                    <div class={classes!("mt-3", "rounded-lg", "border", "border-red-400/35", "bg-red-500/8", "px-4", "py-3", "font-mono", "text-sm", "text-red-700", "dark:text-red-200")}>
                                        { err }
                                    </div>
                                } else {
                                    <div class={classes!("mt-3", "grid", "gap-3", "lg:grid-cols-2")}>
                                        { for contributions.iter().enumerate().map(|(idx, item)| {
                                            let github_id = item.github_id.clone();
                                            let avatar_url = github_id.as_deref().map(github_avatar_url).unwrap_or_default();
                                            let profile_url = github_id.as_deref().map(github_profile_url).unwrap_or_default();
                                            html! {
                                                <article
                                                    class={classes!(
                                                        "group", "relative", "overflow-hidden",
                                                        "rounded-lg", "border", "border-[var(--border)]",
                                                        "bg-[var(--surface)]", "p-4",
                                                        "transition-all", "duration-300", "ease-out",
                                                        "hover:-translate-y-1",
                                                        "hover:shadow-[0_12px_40px_rgba(var(--primary-rgb),0.08)]",
                                                        "hover:border-[rgba(var(--primary-rgb),0.3)]",
                                                        "llm-wall-card",
                                                    )}
                                                    style={format!("animation-delay: {}ms;", idx * 80)}
                                                >
                                                    <div class={classes!("flex", "items-start", "gap-3")}>
                                                        if let Some(gid) = github_id.clone() {
                                                            <a href={profile_url.clone()} target="_blank" rel="noreferrer noopener" class={classes!("shrink-0")}>
                                                                <img src={avatar_url} alt={gid.to_string()}
                                                                    class={classes!("h-11", "w-11", "rounded-full", "object-cover", "ring-2", "ring-[rgba(var(--primary-rgb),0.25)]", "ring-offset-2", "ring-offset-[var(--surface)]", "transition-all", "duration-300", "group-hover:ring-[rgba(var(--primary-rgb),0.5)]")}
                                                                    loading="lazy" />
                                                            </a>
                                                        } else {
                                                            <div class={classes!("flex", "h-11", "w-11", "shrink-0", "items-center", "justify-center", "rounded-full", "bg-[rgba(var(--primary-rgb),0.08)]", "text-[var(--primary)]", "ring-2", "ring-[rgba(var(--primary-rgb),0.15)]", "ring-offset-2", "ring-offset-[var(--surface)]", "transition-all", "duration-300", "group-hover:ring-[rgba(var(--primary-rgb),0.35)]")}>
                                                                <i class={classes!("fas", "fa-user-astronaut", "text-sm")} />
                                                            </div>
                                                        }
                                                        <div class={classes!("min-w-0", "flex-1")}>
                                                            <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                                                                <span class={classes!("font-mono", "text-xs", "font-bold", "tracking-wide", "text-[var(--primary)]", "bg-[rgba(var(--primary-rgb),0.06)]", "px-2", "py-0.5", "rounded")}>
                                                                    { item.account_name.clone() }
                                                                </span>
                                                                if let Some(gid) = item.github_id.clone() {
                                                                    <a href={profile_url.clone()} target="_blank" rel="noreferrer noopener"
                                                                        class={classes!("font-mono", "text-[11px]", "text-[var(--muted)]", "transition-colors", "hover:text-[var(--primary)]")}>
                                                                        { format!("@{}", gid) }
                                                                    </a>
                                                                }
                                                                if let Some(ts) = item.processed_at {
                                                                    <span class={classes!("font-mono", "text-[11px]", "text-[var(--muted)]", "opacity-60")}>{ format_ms(ts) }</span>
                                                                }
                                                            </div>
                                                            <p class={classes!("mt-2.5", "m-0", "whitespace-pre-wrap", "break-words", "text-sm", "leading-relaxed", "text-[var(--text)]", "opacity-85")}>
                                                                { item.contributor_message.clone() }
                                                            </p>
                                                        </div>
                                                    </div>
                                                </article>
                                            }
                                        }) }
                                    </div>
                                }
                            </section>
                        }

                        // Sponsor thank-you wall
                        if !sponsors.is_empty() || (*sponsor_error).is_some() {
                            <section class={classes!("mt-6")}>
                                <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]", "before:content-['//']", "before:mr-2", "before:text-[var(--muted)]", "before:opacity-40", "before:font-normal")}>
                                    { "Sponsor 感谢墙" }
                                </h2>
                                if let Some(err) = (*sponsor_error).clone() {
                                    <div class={classes!("mt-3", "rounded-lg", "border", "border-red-400/35", "bg-red-500/8", "px-4", "py-3", "font-mono", "text-sm", "text-red-700", "dark:text-red-200")}>
                                        { err }
                                    </div>
                                } else {
                                    <div class={classes!("mt-3", "grid", "gap-3", "lg:grid-cols-2")}>
                                        { for sponsors.iter().enumerate().map(|(idx, item)| {
                                            let github_id = item.github_id.clone();
                                            let avatar_url = github_id.as_deref().map(github_avatar_url).unwrap_or_default();
                                            let profile_url = github_id.as_deref().map(github_profile_url).unwrap_or_default();
                                            let display_name = item.display_name.clone()
                                                .filter(|v| !v.trim().is_empty())
                                                .unwrap_or_else(|| github_id.as_ref().map(|id| format!("@{}", id)).unwrap_or_else(|| "匿名".to_string()));
                                            html! {
                                                <article
                                                    class={classes!(
                                                        "group", "relative", "overflow-hidden",
                                                        "rounded-lg", "border", "border-[var(--border)]",
                                                        "bg-[var(--surface)]", "p-4",
                                                        "transition-all", "duration-300", "ease-out",
                                                        "hover:-translate-y-1",
                                                        "hover:shadow-[0_12px_40px_rgba(245,158,11,0.1)]",
                                                        "hover:border-amber-500/30",
                                                        "llm-wall-card", "llm-wall-card-sponsor",
                                                    )}
                                                    style={format!("animation-delay: {}ms;", idx * 80)}
                                                >
                                                    <div class={classes!("flex", "items-start", "gap-3")}>
                                                        if let Some(gid) = github_id.clone() {
                                                            <a href={profile_url.clone()} target="_blank" rel="noreferrer noopener" class={classes!("shrink-0")}>
                                                                <img src={avatar_url} alt={gid.to_string()}
                                                                    class={classes!("h-11", "w-11", "rounded-full", "object-cover", "ring-2", "ring-amber-500/25", "ring-offset-2", "ring-offset-[var(--surface)]", "transition-all", "duration-300", "group-hover:ring-amber-500/50")}
                                                                    loading="lazy" />
                                                            </a>
                                                        } else {
                                                            <div class={classes!("flex", "h-11", "w-11", "shrink-0", "items-center", "justify-center", "rounded-full", "bg-amber-500/8", "text-amber-600", "ring-2", "ring-amber-500/15", "ring-offset-2", "ring-offset-[var(--surface)]", "transition-all", "duration-300", "group-hover:ring-amber-500/35")}>
                                                                <i class={classes!("fas", "fa-mug-hot", "text-sm")} />
                                                            </div>
                                                        }
                                                        <div class={classes!("min-w-0", "flex-1")}>
                                                            <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                                                                <span class={classes!("font-mono", "text-sm", "font-bold", "text-[var(--text)]")}>
                                                                    { display_name }
                                                                </span>
                                                                if let Some(ts) = item.processed_at {
                                                                    <span class={classes!("font-mono", "text-[11px]", "text-[var(--muted)]", "opacity-60")}>{ format_ms(ts) }</span>
                                                                }
                                                            </div>
                                                            <p class={classes!("mt-2.5", "m-0", "whitespace-pre-wrap", "break-words", "text-sm", "leading-relaxed", "text-[var(--text)]", "opacity-85")}>
                                                                { item.sponsor_message.clone() }
                                                            </p>
                                                        </div>
                                                    </div>
                                                </article>
                                            }
                                        }) }
                                    </div>
                                }
                            </section>
                        }
                    </>
                }
    } else {
        Html::default()
    };
    // PLACEHOLDER_OUTER_HTML

    // Shared input class
    let ic = "mt-1 w-full rounded-lg border border-[var(--border)] bg-[var(--surface)] px-3 py-2 \
              text-[var(--text)] font-mono text-sm llm-access-input";
    let ic_mono_xs = "mt-1 w-full rounded-lg border border-[var(--border)] bg-[var(--surface)] \
                      px-3 py-2 font-mono text-xs text-[var(--text)] resize-y llm-access-input";

    html! {
            <main class={classes!("relative", "min-h-screen", "bg-[var(--bg)]")}>
                <div class={classes!("relative", "mx-auto", "max-w-5xl", "px-4", "pb-16", "pt-8", "lg:px-6")}>
                    { content }
                </div>

                // QR lightbox overlay
                if *qr_zoomed {
                    if let Some(ref lightbox_url) = group_qr_url_for_lightbox {
                        <div
                            class={classes!("fixed", "inset-0", "z-[100]", "flex", "items-center", "justify-center", "bg-black/70", "backdrop-blur-sm", "cursor-pointer")}
                            role="dialog" aria-modal="true"
                            onclick={{ let qr_zoomed = qr_zoomed.clone(); Callback::from(move |_: MouseEvent| qr_zoomed.set(false)) }}
                        >
                            <img
                                src={lightbox_url.clone()}
                                alt="QQ group QR code"
                                class={classes!("max-h-[80vh]", "max-w-[90vw]", "rounded-2xl", "border-2", "border-white/20", "shadow-[0_20px_60px_rgba(0,0,0,0.5)]", "bg-white", "llm-modal-enter")}
                                onclick={Callback::from(|e: MouseEvent| e.stop_propagation())}
                            />
                        </div>
                    }
                }

                // --- Modal: Token Wish ---
                if *active_modal == ActiveModal::TokenWish {
                    <div
                        class={classes!("fixed", "inset-0", "z-[100]", "flex", "items-center", "justify-center", "bg-black/60", "backdrop-blur-sm", "p-4")}
                        role="dialog" aria-modal="true"
                        onclick={{ let m = active_modal.clone(); Callback::from(move |_: MouseEvent| m.set(ActiveModal::None)) }}
                    >
                        <div
                            class={classes!("w-full", "max-w-lg", "rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-6", "shadow-[0_20px_60px_rgba(0,0,0,0.3)]", "llm-modal-enter")}
                            onclick={Callback::from(|e: MouseEvent| e.stop_propagation())}
                        >
                            <div class={classes!("flex", "items-center", "justify-between", "gap-3")}>
                                <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "许愿 Token" }</h2>
                                <button type="button" class={classes!("btn-terminal")}
                                    onclick={{ let m = active_modal.clone(); Callback::from(move |_| m.set(ActiveModal::None)) }}>
                                    <i class="fas fa-xmark"></i>
                                </button>
                            </div>
                            <p class={classes!("mt-2", "m-0", "font-mono", "text-xs", "text-[var(--muted)]")}>
                                { "提交额度申请，审核通过后 token 会发到你的邮箱。" }
                            </p>
                            <form class={classes!("mt-4", "grid", "gap-3")} onsubmit={on_submit_token_wish}>
                                <div class={classes!("grid", "gap-3", "sm:grid-cols-2")}>
                                    <label class={classes!("text-sm")}>
                                        <span class={classes!("font-mono", "text-xs", "text-[var(--muted)]")}>{ "token 量" }</span>
                                        <input type="number" min="1" step="1" placeholder="500000" class={ic}
                                            value={(*wish_quota).clone()} required=true
                                            oninput={{ let s = wish_quota.clone(); Callback::from(move |e: InputEvent| { if let Some(t) = e.target_dyn_into::<HtmlInputElement>() { s.set(t.value()); } }) }} />
                                    </label>
                                    <label class={classes!("text-sm")}>
                                        <span class={classes!("font-mono", "text-xs", "text-[var(--muted)]")}>{ "邮箱" }</span>
                                        <input type="email" placeholder="you@example.com" class={ic}
                                            value={(*wish_email).clone()} required=true
                                            oninput={{ let s = wish_email.clone(); Callback::from(move |e: InputEvent| { if let Some(t) = e.target_dyn_into::<HtmlInputElement>() { s.set(t.value()); } }) }} />
                                    </label>
                                </div>
                                <label class={classes!("text-sm")}>
                                    <span class={classes!("font-mono", "text-xs", "text-[var(--muted)]")}>{ "缘由" }</span>
                                    <textarea rows="3" placeholder="用途和需求量说明" class={ic_mono_xs}
                                        value={(*wish_reason).clone()} required=true
                                        oninput={{ let s = wish_reason.clone(); Callback::from(move |e: InputEvent| { if let Some(t) = e.target_dyn_into::<HtmlTextAreaElement>() { s.set(t.value()); } }) }} />
                                </label>
                                <div class={classes!("flex", "justify-end")}>
                                    <button type="submit" class={classes!("btn-terminal", "btn-terminal-primary")} disabled={*wish_submitting}>
                                        <i class={classes!("fas", if *wish_submitting { "fa-spinner animate-spin" } else { "fa-paper-plane" })}></i>
                                        { if *wish_submitting { "提交中..." } else { "提交" } }
                                    </button>
                                </div>
                                if let Some((msg, is_err)) = (*wish_feedback).clone() {
                                    if is_err {
                                        <div class={classes!("rounded-lg", "border", "border-red-400/35", "bg-red-500/8", "px-3", "py-2", "font-mono", "text-xs", "text-red-700", "dark:text-red-200")}>{ msg }</div>
                                    }
                                }
                            </form>
                        </div>
                    </div>
                }
    // PLACEHOLDER_MODAL_CONTRIBUTION

                // --- Modal: Account Contribution ---
                if *active_modal == ActiveModal::AccountContribution {
                    <div
                        class={classes!("fixed", "inset-0", "z-[100]", "flex", "items-center", "justify-center", "bg-black/60", "backdrop-blur-sm", "p-4", "overflow-y-auto")}
                        role="dialog" aria-modal="true"
                        onclick={{ let m = active_modal.clone(); Callback::from(move |_: MouseEvent| m.set(ActiveModal::None)) }}
                    >
                        <div
                            class={classes!("w-full", "max-w-lg", "my-8", "rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-6", "shadow-[0_20px_60px_rgba(0,0,0,0.3)]", "llm-modal-enter")}
                            onclick={Callback::from(|e: MouseEvent| e.stop_propagation())}
                        >
                            <div class={classes!("flex", "items-center", "justify-between", "gap-3")}>
                                <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "贡献账号" }</h2>
                                <button type="button" class={classes!("btn-terminal")}
                                    onclick={{ let m = active_modal.clone(); Callback::from(move |_| m.set(ActiveModal::None)) }}>
                                    <i class="fas fa-xmark"></i>
                                </button>
                            </div>
                            <p class={classes!("mt-2", "m-0", "font-mono", "text-xs", "text-[var(--muted)]")}>
                                { "贡献 Codex 账号到池里，审核通过后会发一把绑定该账号的 token 到你的邮箱。" }
                            </p>
                            <form class={classes!("mt-4", "grid", "gap-3")} onsubmit={on_submit_account_contribution}>
                                <div class={classes!("grid", "gap-3", "sm:grid-cols-2")}>
                                    <label class={classes!("text-sm")}>
                                        <span class={classes!("font-mono", "text-xs", "text-[var(--muted)]")}>{ "账号名" }</span>
                                        <input type="text" placeholder="my-pro-account" class={ic}
                                            value={(*contribution_account_name).clone()} required=true
                                            oninput={{ let s = contribution_account_name.clone(); Callback::from(move |e: InputEvent| { if let Some(t) = e.target_dyn_into::<HtmlInputElement>() { s.set(t.value()); } }) }} />
                                    </label>
                                    <label class={classes!("text-sm")}>
                                        <span class={classes!("font-mono", "text-xs", "text-[var(--muted)]")}>{ "邮箱" }</span>
                                        <input type="email" placeholder="you@example.com" class={ic}
                                            value={(*contribution_email).clone()} required=true
                                            oninput={{ let s = contribution_email.clone(); Callback::from(move |e: InputEvent| { if let Some(t) = e.target_dyn_into::<HtmlInputElement>() { s.set(t.value()); } }) }} />
                                    </label>
                                </div>
                                <label class={classes!("text-sm")}>
                                    <div class={classes!("flex", "items-center", "justify-between", "gap-2")}>
                                        <span class={classes!("font-mono", "text-xs", "text-[var(--muted)]")}>{ "auth.json（可选，粘贴后自动回填）" }</span>
                                    </div>
                                    <textarea rows="4" placeholder="{\"tokens\":{...}}" class={ic_mono_xs}
                                        value={(*contribution_raw_auth_json).clone()}
                                        oninput={{
                                            let raw_s = contribution_raw_auth_json.clone();
                                            let fb = contribution_raw_auth_feedback.clone();
                                            let aid = contribution_account_id.clone();
                                            let idt = contribution_id_token.clone();
                                            let act = contribution_access_token.clone();
                                            let rft = contribution_refresh_token.clone();
                                            Callback::from(move |e: InputEvent| {
                                                if let Some(t) = e.target_dyn_into::<HtmlTextAreaElement>() {
                                                    let raw = t.value();
                                                    let trimmed = raw.trim().to_string();
                                                    raw_s.set(raw);
                                                    if trimmed.is_empty() { fb.set(None); return; }
                                                    match parse_imported_auth_json(&trimmed) {
                                                        Ok(p) => {
                                                            aid.set(p.account_id.unwrap_or_default());
                                                            idt.set(p.id_token); act.set(p.access_token); rft.set(p.refresh_token);
                                                            fb.set(Some(("已自动回填 token".to_string(), false)));
                                                        },
                                                        Err(err) => {
                                                            if trimmed.ends_with('}') || trimmed.contains('\n') { fb.set(Some((err, true))); }
                                                            else { fb.set(None); }
                                                        },
                                                    }
                                                }
                                            })
                                        }} />
                                    if let Some((msg, is_err)) = (*contribution_raw_auth_feedback).clone() {
                                        <div class={classes!("mt-1", "font-mono", "text-[11px]", if is_err { "text-red-600 dark:text-red-300" } else { "text-emerald-600 dark:text-emerald-300" })}>
                                            { msg }
                                        </div>
                                    }
                                </label>
                                <div class={classes!("grid", "gap-3", "sm:grid-cols-2")}>
                                    <label class={classes!("text-sm")}>
                                        <span class={classes!("font-mono", "text-xs", "text-[var(--muted)]")}>{ "GitHub ID（可选）" }</span>
                                        <input type="text" placeholder="ackingliu" class={ic}
                                            value={(*contribution_github_id).clone()}
                                            oninput={{ let s = contribution_github_id.clone(); Callback::from(move |e: InputEvent| { if let Some(t) = e.target_dyn_into::<HtmlInputElement>() { s.set(t.value()); } }) }} />
                                    </label>
                                    <label class={classes!("text-sm")}>
                                        <span class={classes!("font-mono", "text-xs", "text-[var(--muted)]")}>{ "account_id（可选）" }</span>
                                        <input type="text" class={ic}
                                            value={(*contribution_account_id).clone()}
                                            oninput={{ let s = contribution_account_id.clone(); Callback::from(move |e: InputEvent| { if let Some(t) = e.target_dyn_into::<HtmlInputElement>() { s.set(t.value()); } }) }} />
                                    </label>
                                </div>
                                <label class={classes!("text-sm")}>
                                    <span class={classes!("font-mono", "text-xs", "text-[var(--muted)]")}>{ "access_token" }</span>
                                    <textarea rows="2" class={ic_mono_xs} value={(*contribution_access_token).clone()} required=true
                                        oninput={{ let s = contribution_access_token.clone(); Callback::from(move |e: InputEvent| { if let Some(t) = e.target_dyn_into::<HtmlTextAreaElement>() { s.set(t.value()); } }) }} />
                                </label>
                                <div class={classes!("grid", "gap-3", "sm:grid-cols-2")}>
                                    <label class={classes!("text-sm")}>
                                        <span class={classes!("font-mono", "text-xs", "text-[var(--muted)]")}>{ "id_token" }</span>
                                        <textarea rows="2" class={ic_mono_xs} value={(*contribution_id_token).clone()} required=true
                                            oninput={{ let s = contribution_id_token.clone(); Callback::from(move |e: InputEvent| { if let Some(t) = e.target_dyn_into::<HtmlTextAreaElement>() { s.set(t.value()); } }) }} />
                                    </label>
                                    <label class={classes!("text-sm")}>
                                        <span class={classes!("font-mono", "text-xs", "text-[var(--muted)]")}>{ "refresh_token" }</span>
                                        <textarea rows="2" class={ic_mono_xs} value={(*contribution_refresh_token).clone()} required=true
                                            oninput={{ let s = contribution_refresh_token.clone(); Callback::from(move |e: InputEvent| { if let Some(t) = e.target_dyn_into::<HtmlTextAreaElement>() { s.set(t.value()); } }) }} />
                                    </label>
                                </div>
                                <label class={classes!("text-sm")}>
                                    <span class={classes!("font-mono", "text-xs", "text-[var(--muted)]")}>{ "留言" }</span>
                                    <textarea rows="3" placeholder="为什么愿意贡献这个账号" class={ic_mono_xs}
                                        value={(*contribution_message).clone()} required=true
                                        oninput={{ let s = contribution_message.clone(); Callback::from(move |e: InputEvent| { if let Some(t) = e.target_dyn_into::<HtmlTextAreaElement>() { s.set(t.value()); } }) }} />
                                </label>
                                <div class={classes!("flex", "justify-end")}>
                                    <button type="submit" class={classes!("btn-terminal", "btn-terminal-primary")} disabled={*contribution_submitting}>
                                        <i class={classes!("fas", if *contribution_submitting { "fa-spinner animate-spin" } else { "fa-heart-circle-plus" })}></i>
                                        { if *contribution_submitting { "提交中..." } else { "提交" } }
                                    </button>
                                </div>
                                if let Some((msg, is_err)) = (*contribution_feedback).clone() {
                                    if is_err {
                                        <div class={classes!("rounded-lg", "border", "border-red-400/35", "bg-red-500/8", "px-3", "py-2", "font-mono", "text-xs", "text-red-700", "dark:text-red-200")}>{ msg }</div>
                                    }
                                }
                            </form>
                        </div>
                    </div>
                }
    // PLACEHOLDER_MODAL_SPONSOR

                // --- Modal: Sponsor ---
                if *active_modal == ActiveModal::Sponsor {
                    <div
                        class={classes!("fixed", "inset-0", "z-[100]", "flex", "items-center", "justify-center", "bg-black/60", "backdrop-blur-sm", "p-4")}
                        role="dialog" aria-modal="true"
                        onclick={{ let m = active_modal.clone(); Callback::from(move |_: MouseEvent| m.set(ActiveModal::None)) }}
                    >
                        <div
                            class={classes!("w-full", "max-w-lg", "rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-6", "shadow-[0_20px_60px_rgba(0,0,0,0.3)]", "llm-modal-enter")}
                            onclick={Callback::from(|e: MouseEvent| e.stop_propagation())}
                        >
                            <div class={classes!("flex", "items-center", "justify-between", "gap-3")}>
                                <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "请站长喝杯咖啡" }</h2>
                                <button type="button" class={classes!("btn-terminal")}
                                    onclick={{ let m = active_modal.clone(); Callback::from(move |_| m.set(ActiveModal::None)) }}>
                                    <i class="fas fa-xmark"></i>
                                </button>
                            </div>
                            <p class={classes!("mt-2", "m-0", "text-sm", "leading-relaxed", "text-[var(--muted)]")}>
                                { "这是一个公益站点，服务器和带宽都需要持续投入。一杯咖啡的支持就能帮站长分担运维成本，让大家继续免费用下去。" }
                            </p>
                            <p class={classes!("mt-2", "m-0", "font-mono", "text-[11px]", "text-[var(--muted)]", "opacity-60")}>
                                { "填写邮箱 → 收到付款说明 → 付款后回复邮件 → 上墙" }
                            </p>
                            <form class={classes!("mt-4", "grid", "gap-3")} onsubmit={on_submit_sponsor}>
                                <div class={classes!("grid", "gap-3", "sm:grid-cols-2")}>
                                    <label class={classes!("text-sm")}>
                                        <span class={classes!("font-mono", "text-xs", "text-[var(--muted)]")}>{ "邮箱" }</span>
                                        <input type="email" placeholder="you@example.com" class={ic}
                                            value={(*sponsor_email).clone()} required=true
                                            oninput={{ let s = sponsor_email.clone(); Callback::from(move |e: InputEvent| { if let Some(t) = e.target_dyn_into::<HtmlInputElement>() { s.set(t.value()); } }) }} />
                                    </label>
                                    <label class={classes!("text-sm")}>
                                        <span class={classes!("font-mono", "text-xs", "text-[var(--muted)]")}>{ "名字（可选）" }</span>
                                        <input type="text" placeholder="显示名字" class={ic}
                                            value={(*sponsor_display_name).clone()}
                                            oninput={{ let s = sponsor_display_name.clone(); Callback::from(move |e: InputEvent| { if let Some(t) = e.target_dyn_into::<HtmlInputElement>() { s.set(t.value()); } }) }} />
                                    </label>
                                </div>
                                <label class={classes!("text-sm")}>
                                    <span class={classes!("font-mono", "text-xs", "text-[var(--muted)]")}>{ "GitHub ID（可选）" }</span>
                                    <input type="text" placeholder="ackingliu" class={ic}
                                        value={(*sponsor_github_id).clone()}
                                        oninput={{ let s = sponsor_github_id.clone(); Callback::from(move |e: InputEvent| { if let Some(t) = e.target_dyn_into::<HtmlInputElement>() { s.set(t.value()); } }) }} />
                                </label>
                                <label class={classes!("text-sm")}>
                                    <span class={classes!("font-mono", "text-xs", "text-[var(--muted)]")}>{ "留言" }</span>
                                    <textarea rows="3" placeholder="想对站点说的话" class={ic_mono_xs}
                                        value={(*sponsor_message).clone()} required=true
                                        oninput={{ let s = sponsor_message.clone(); Callback::from(move |e: InputEvent| { if let Some(t) = e.target_dyn_into::<HtmlTextAreaElement>() { s.set(t.value()); } }) }} />
                                </label>
                                <div class={classes!("flex", "items-center", "justify-between", "gap-3")}>
                                    <span class={classes!("font-mono", "text-[11px]", "text-[var(--muted)]")}>{ "邮箱不会公开" }</span>
                                    <button type="submit" class={classes!("btn-terminal", "btn-terminal-primary")} disabled={*sponsor_submitting}>
                                        <i class={classes!("fas", if *sponsor_submitting { "fa-spinner animate-spin" } else { "fa-mug-hot" })}></i>
                                        { if *sponsor_submitting { "提交中..." } else { "提交" } }
                                    </button>
                                </div>
                                if let Some((msg, is_err)) = (*sponsor_feedback).clone() {
                                    if is_err {
                                        <div class={classes!("rounded-lg", "border", "border-red-400/35", "bg-red-500/8", "px-3", "py-2", "font-mono", "text-xs", "text-red-700", "dark:text-red-200")}>{ msg }</div>
                                    }
                                }
                            </form>
                        </div>
                    </div>
                }

                // Toast
                if let Some((message, is_error)) = (*toast).clone() {
                    <div class={classes!(
                        "fixed", "bottom-5", "right-5", "z-[90]",
                        "rounded-full", "border", "px-4", "py-2.5",
                        "font-mono", "text-sm", "font-semibold",
                        "shadow-[0_8px_24px_rgba(0,0,0,0.15)]",
                        if is_error { classes!("border-red-400/35", "bg-red-500/92", "text-white") }
                        else { classes!("border-emerald-400/35", "bg-emerald-500/92", "text-white") }
                    )}>
                        { message }
                    </div>
                }
            </main>
        }
}
