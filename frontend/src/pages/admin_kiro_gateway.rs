//! Admin UI for managing Kiro accounts, keys, usage, and proxy bindings.

use std::collections::{BTreeMap, HashSet};

use gloo_timers::callback::Timeout;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;
use web_sys::{HtmlInputElement, HtmlSelectElement, HtmlTextAreaElement};
use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{
    api::{
        create_admin_kiro_account_group, create_admin_kiro_key, create_admin_kiro_manual_account,
        delete_admin_kiro_account, delete_admin_kiro_account_group, delete_admin_kiro_key,
        fetch_admin_kiro_account_groups, fetch_admin_kiro_accounts, fetch_admin_kiro_keys,
        fetch_admin_kiro_usage_events, fetch_admin_llm_gateway_config,
        fetch_admin_llm_gateway_proxy_bindings, fetch_admin_llm_gateway_proxy_configs,
        fetch_kiro_models, import_admin_kiro_account, patch_admin_kiro_account,
        patch_admin_kiro_account_group, patch_admin_kiro_key, refresh_admin_kiro_account_balance,
        update_admin_llm_gateway_config, AdminAccountGroupView, AdminLlmGatewayKeyView,
        AdminLlmGatewayUsageEventView, AdminLlmGatewayUsageEventsQuery,
        AdminUpstreamProxyBindingView, AdminUpstreamProxyConfigView, CreateAdminAccountGroupInput,
        CreateManualKiroAccountInput, KiroAccountView, KiroBalanceView, KiroModelView,
        LlmGatewayRuntimeConfig, PatchAdminAccountGroupInput, PatchAdminLlmGatewayKeyRequest,
        PatchKiroAccountInput,
    },
    pages::llm_access_shared::{
        format_float2, format_kiro_disabled_reason, format_ms, format_number_i64,
        format_number_u64, format_reset_hint, kiro_credit_ratio, kiro_key_usage_ratio,
        MaskedSecretCode,
    },
    router::Route,
};

const TAB_OVERVIEW: &str = "overview";
const TAB_ACCOUNTS: &str = "accounts";
const TAB_KEYS: &str = "keys";
const TAB_GROUPS: &str = "groups";
const TAB_USAGE: &str = "usage";

fn kiro_account_status_route() -> Route {
    Route::AdminKiroAccountStatus
}

fn kiro_account_status_cta_text() -> &'static str {
    "Open Account Status Page"
}

fn should_load_kiro_usage_preview(active_tab: &str) -> bool {
    active_tab == TAB_USAGE
}

/// Shared Tailwind classes for the dark "Kiro" pill badge.
fn kiro_badge() -> Classes {
    classes!(
        "inline-flex",
        "items-center",
        "rounded-full",
        "bg-slate-900",
        "px-2.5",
        "py-1",
        "font-mono",
        "text-[11px]",
        "font-semibold",
        "uppercase",
        "tracking-[0.16em]",
        "text-emerald-300"
    )
}

/// Render a horizontal tab bar. Each `(id, label)` pair becomes a button;
/// the one matching `active` gets the primary style.
fn render_tab_bar(active: &str, tabs: &[(&str, &str)], on_click: &Callback<String>) -> Html {
    html! {
        <nav class={classes!(
            "flex", "items-center", "gap-1.5", "flex-wrap",
            "rounded-xl", "border", "border-[var(--border)]",
            "bg-[var(--surface)]", "p-1.5"
        )} role="tablist">
            { for tabs.iter().map(|(id, label)| {
                let is_active = active == *id;
                let id_owned = id.to_string();
                let on_click = on_click.clone();
                html! {
                    <button
                        type="button"
                        role="tab"
                        aria-selected={is_active.to_string()}
                        class={classes!(
                            "btn-terminal",
                            if is_active { "btn-terminal-primary" } else { "" }
                        )}
                        onclick={Callback::from(move |_| on_click.emit(id_owned.clone()))}
                    >
                        { *label }
                    </button>
                }
            }) }
        </nav>
    }
}

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

fn format_timestamp_opt(ts: Option<i64>) -> String {
    ts.map(format_ms).unwrap_or_else(|| "-".to_string())
}

fn format_float4(value: f64) -> String {
    format!("{value:.4}")
}

fn format_json_for_textarea(raw: &str) -> String {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| raw.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct KiroCachePolicyBandForm {
    credit_start: String,
    credit_end: String,
    cache_ratio_start: String,
    cache_ratio_end: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct KiroCachePolicyForm {
    target_input_tokens: String,
    credit_start: String,
    credit_end: String,
    high_credit_diagnostic_threshold: String,
    bands: Vec<KiroCachePolicyBandForm>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct KiroCachePolicyJson {
    small_input_high_credit_boost: KiroSmallInputHighCreditBoostJson,
    prefix_tree_credit_ratio_bands: Vec<KiroCachePolicyBandJson>,
    high_credit_diagnostic_threshold: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct KiroSmallInputHighCreditBoostJson {
    target_input_tokens: u64,
    credit_start: f64,
    credit_end: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct KiroCachePolicyBandJson {
    credit_start: f64,
    credit_end: f64,
    cache_ratio_start: f64,
    cache_ratio_end: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct KiroCachePolicyOverrideJson {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    small_input_high_credit_boost: Option<KiroSmallInputHighCreditBoostOverrideJson>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prefix_tree_credit_ratio_bands: Option<Vec<KiroCachePolicyBandJson>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    high_credit_diagnostic_threshold: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct KiroSmallInputHighCreditBoostOverrideJson {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    credit_start: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    credit_end: Option<f64>,
}

fn format_kiro_cache_policy_number(value: f64) -> String {
    let mut text = value.to_string();
    if !text.contains(['.', 'e', 'E']) {
        text.push_str(".0");
    }
    text
}

fn parse_kiro_cache_policy_u64(label: &str, raw: &str) -> Result<u64, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} must not be empty."));
    }
    trimmed
        .parse::<u64>()
        .map_err(|_| format!("{label} must be a valid integer."))
}

fn parse_kiro_cache_policy_f64(label: &str, raw: &str) -> Result<f64, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} must not be empty."));
    }
    let value = trimmed
        .parse::<f64>()
        .map_err(|_| format!("{label} must be a valid number."))?;
    if !value.is_finite() {
        return Err(format!("{label} must be finite."));
    }
    Ok(value)
}

fn kiro_cache_policy_form_from_json(policy: &KiroCachePolicyJson) -> KiroCachePolicyForm {
    KiroCachePolicyForm {
        target_input_tokens: policy
            .small_input_high_credit_boost
            .target_input_tokens
            .to_string(),
        credit_start: format_kiro_cache_policy_number(
            policy.small_input_high_credit_boost.credit_start,
        ),
        credit_end: format_kiro_cache_policy_number(
            policy.small_input_high_credit_boost.credit_end,
        ),
        high_credit_diagnostic_threshold: format_kiro_cache_policy_number(
            policy.high_credit_diagnostic_threshold,
        ),
        bands: policy
            .prefix_tree_credit_ratio_bands
            .iter()
            .map(|band| KiroCachePolicyBandForm {
                credit_start: format_kiro_cache_policy_number(band.credit_start),
                credit_end: format_kiro_cache_policy_number(band.credit_end),
                cache_ratio_start: format_kiro_cache_policy_number(band.cache_ratio_start),
                cache_ratio_end: format_kiro_cache_policy_number(band.cache_ratio_end),
            })
            .collect(),
    }
}

fn kiro_cache_policy_json_from_form(
    form: &KiroCachePolicyForm,
) -> Result<KiroCachePolicyJson, String> {
    let bands = form
        .bands
        .iter()
        .enumerate()
        .map(|(index, band)| {
            Ok(KiroCachePolicyBandJson {
                credit_start: parse_kiro_cache_policy_f64(
                    &format!("Band {} credit start", index + 1),
                    &band.credit_start,
                )?,
                credit_end: parse_kiro_cache_policy_f64(
                    &format!("Band {} credit end", index + 1),
                    &band.credit_end,
                )?,
                cache_ratio_start: parse_kiro_cache_policy_f64(
                    &format!("Band {} cache ratio start", index + 1),
                    &band.cache_ratio_start,
                )?,
                cache_ratio_end: parse_kiro_cache_policy_f64(
                    &format!("Band {} cache ratio end", index + 1),
                    &band.cache_ratio_end,
                )?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let policy = KiroCachePolicyJson {
        small_input_high_credit_boost: KiroSmallInputHighCreditBoostJson {
            target_input_tokens: parse_kiro_cache_policy_u64(
                "Small-input target input tokens",
                &form.target_input_tokens,
            )?,
            credit_start: parse_kiro_cache_policy_f64(
                "Small-input credit start",
                &form.credit_start,
            )?,
            credit_end: parse_kiro_cache_policy_f64("Small-input credit end", &form.credit_end)?,
        },
        prefix_tree_credit_ratio_bands: bands,
        high_credit_diagnostic_threshold: parse_kiro_cache_policy_f64(
            "High-credit diagnostic threshold",
            &form.high_credit_diagnostic_threshold,
        )?,
    };
    Ok(policy)
}

fn parse_kiro_cache_policy_form_json(raw: &str) -> Result<KiroCachePolicyForm, String> {
    let policy = serde_json::from_str::<KiroCachePolicyJson>(raw)
        .map_err(|err| format!("Failed to parse kiro cache policy JSON: {err}"))?;
    Ok(kiro_cache_policy_form_from_json(&policy))
}

fn serialize_kiro_cache_policy_form_json(form: &KiroCachePolicyForm) -> Result<String, String> {
    let policy = kiro_cache_policy_json_from_form(form)?;
    serde_json::to_string(&policy)
        .map_err(|err| format!("Failed to serialize kiro cache policy: {err}"))
}

fn build_kiro_cache_policy_override_json(
    global: &KiroCachePolicyForm,
    edited: &KiroCachePolicyForm,
) -> Result<Option<String>, String> {
    let global_policy = kiro_cache_policy_json_from_form(global)?;
    let edited_policy = kiro_cache_policy_json_from_form(edited)?;
    let mut override_policy = KiroCachePolicyOverrideJson::default();
    let mut boost_override = KiroSmallInputHighCreditBoostOverrideJson::default();

    if edited_policy
        .small_input_high_credit_boost
        .target_input_tokens
        != global_policy
            .small_input_high_credit_boost
            .target_input_tokens
    {
        boost_override.target_input_tokens = Some(
            edited_policy
                .small_input_high_credit_boost
                .target_input_tokens,
        );
    }
    if edited_policy.small_input_high_credit_boost.credit_start
        != global_policy.small_input_high_credit_boost.credit_start
    {
        boost_override.credit_start =
            Some(edited_policy.small_input_high_credit_boost.credit_start);
    }
    if edited_policy.small_input_high_credit_boost.credit_end
        != global_policy.small_input_high_credit_boost.credit_end
    {
        boost_override.credit_end = Some(edited_policy.small_input_high_credit_boost.credit_end);
    }
    if boost_override != KiroSmallInputHighCreditBoostOverrideJson::default() {
        override_policy.small_input_high_credit_boost = Some(boost_override);
    }
    if edited_policy.prefix_tree_credit_ratio_bands != global_policy.prefix_tree_credit_ratio_bands
    {
        override_policy.prefix_tree_credit_ratio_bands =
            Some(edited_policy.prefix_tree_credit_ratio_bands.clone());
    }
    if edited_policy.high_credit_diagnostic_threshold
        != global_policy.high_credit_diagnostic_threshold
    {
        override_policy.high_credit_diagnostic_threshold =
            Some(edited_policy.high_credit_diagnostic_threshold);
    }
    if override_policy == KiroCachePolicyOverrideJson::default() {
        Ok(None)
    } else {
        serde_json::to_string(&override_policy)
            .map(Some)
            .map_err(|err| format!("Failed to serialize kiro cache policy override: {err}"))
    }
}

fn build_kiro_cache_policy_override_patch(
    persisted_global: &KiroCachePolicyForm,
    initial_override_enabled: bool,
    initial_effective: &KiroCachePolicyForm,
    edited_override_enabled: bool,
    edited_effective: &KiroCachePolicyForm,
) -> Result<Option<Option<String>>, String> {
    let editor_state_changed = edited_override_enabled != initial_override_enabled
        || (edited_override_enabled && edited_effective != initial_effective);
    if !editor_state_changed {
        return Ok(None);
    }

    if !edited_override_enabled {
        return Ok(initial_override_enabled.then_some(None));
    }

    match build_kiro_cache_policy_override_json(persisted_global, edited_effective)? {
        Some(json) => Ok(Some(Some(json))),
        None if initial_override_enabled => Ok(Some(None)),
        None => Ok(None),
    }
}

fn should_reset_kiro_cache_policy_editor(
    is_initial_load: bool,
    editable_form: &KiroCachePolicyForm,
    persisted_form: &KiroCachePolicyForm,
) -> bool {
    is_initial_load || editable_form == persisted_form
}

fn format_kiro_cache_policy_summary(
    global: &KiroCachePolicyForm,
    effective: &KiroCachePolicyForm,
) -> String {
    format_kiro_cache_policy_summary_with_scope(
        if global == effective { "inherit global" } else { "override" },
        effective,
    )
}

fn format_effective_kiro_cache_policy_summary(
    uses_global: bool,
    effective: &KiroCachePolicyForm,
) -> String {
    format_kiro_cache_policy_summary_with_scope(
        if uses_global { "inherit global" } else { "override" },
        effective,
    )
}

fn format_kiro_cache_policy_summary_with_scope(
    scope: &str,
    effective: &KiroCachePolicyForm,
) -> String {
    let scope = if scope.is_empty() { "inherit global" } else { scope };
    format!(
        "{scope} · boost {} -> {} => {} · diag {} · bands {}",
        effective.credit_start.trim(),
        effective.credit_end.trim(),
        effective.target_input_tokens.trim(),
        effective.high_credit_diagnostic_threshold.trim(),
        effective.bands.len(),
    )
}

fn format_cache_summary(account: &KiroAccountView) -> String {
    let status = account.cache.status.trim();
    if status.is_empty() {
        return "cache loading".to_string();
    }
    match account.cache.last_checked_at {
        Some(ts) => format!("cache {status} · checked {}", format_ms(ts)),
        None => format!("cache {status}"),
    }
}

fn kiro_account_proxy_select_value(account: &KiroAccountView) -> String {
    match account.proxy_mode.as_str() {
        "direct" => "direct".to_string(),
        "fixed" => account
            .proxy_config_id
            .as_deref()
            .map(|id| format!("fixed:{id}"))
            .unwrap_or_else(|| "inherit".to_string()),
        _ => "inherit".to_string(),
    }
}

fn sanitize_kiro_account_group_id(
    value: Option<&str>,
    groups: &[AdminAccountGroupView],
    _allow_empty: bool,
) -> String {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return String::new();
    };
    if groups.iter().any(|group| group.id == value) {
        value.to_string()
    } else {
        String::new()
    }
}

fn kiro_group_name_for_id(groups: &[AdminAccountGroupView], group_id: &str) -> String {
    groups
        .iter()
        .find(|group| group.id == group_id)
        .map(|group| group.name.clone())
        .unwrap_or_else(|| group_id.to_string())
}

fn kiro_key_route_summary(
    route_strategy: &str,
    account_group_id: &str,
    account_groups: &[AdminAccountGroupView],
) -> String {
    if route_strategy == "fixed" {
        if account_group_id.is_empty() {
            "固定组：未选择".to_string()
        } else {
            format!("固定组：{}", kiro_group_name_for_id(account_groups, account_group_id))
        }
    } else if account_group_id.is_empty() {
        "全账号池自动择优；如果某个账号不可用，会继续尝试其他账号。".to_string()
    } else {
        format!(
            "仅在账号组 `{}` 中自动择优；如果组内没有可用账号，请求会直接报错。",
            kiro_group_name_for_id(account_groups, account_group_id)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
struct KiroKeyCandidateCreditSummary {
    candidate_count: usize,
    loaded_balance_count: usize,
    missing_balance_count: usize,
    total_limit: f64,
    total_remaining: f64,
}

fn kiro_key_candidate_credit_summary(
    route_strategy: &str,
    account_group_id: &str,
    accounts: &[KiroAccountView],
    account_groups: &[AdminAccountGroupView],
) -> KiroKeyCandidateCreditSummary {
    let selected_accounts = if route_strategy == "fixed" {
        if let Some(group) = account_groups
            .iter()
            .find(|group| group.id == account_group_id)
        {
            let allowed_names = group.account_names.iter().collect::<HashSet<_>>();
            accounts
                .iter()
                .filter(|account| allowed_names.contains(&account.name))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        }
    } else if account_group_id.is_empty() {
        accounts.iter().collect::<Vec<_>>()
    } else if let Some(group) = account_groups
        .iter()
        .find(|group| group.id == account_group_id)
    {
        let allowed_names = group.account_names.iter().collect::<HashSet<_>>();
        accounts
            .iter()
            .filter(|account| allowed_names.contains(&account.name))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    let mut summary = KiroKeyCandidateCreditSummary {
        candidate_count: selected_accounts.len(),
        ..KiroKeyCandidateCreditSummary::default()
    };
    for account in selected_accounts {
        if let Some(balance) = account.balance.as_ref() {
            summary.loaded_balance_count += 1;
            summary.total_limit += balance.usage_limit.max(0.0);
            summary.total_remaining += balance.remaining.max(0.0);
        } else {
            summary.missing_balance_count += 1;
        }
    }
    summary
}

fn format_kiro_key_candidate_credit_summary(summary: &KiroKeyCandidateCreditSummary) -> String {
    if summary.candidate_count == 0 {
        return "候选账号额度: 当前没有命中任何账号。".to_string();
    }

    format!(
        "候选账号额度: 剩余 {} / 总额 {} · {}/{} 已加载{}",
        format_float4(summary.total_remaining),
        format_float4(summary.total_limit),
        summary.loaded_balance_count,
        summary.candidate_count,
        if summary.missing_balance_count == 0 {
            String::new()
        } else {
            format!(" · {} 个账号余额未加载", summary.missing_balance_count)
        }
    )
}

#[derive(Properties, PartialEq)]
struct KiroCachePolicyEditorProps {
    form: UseStateHandle<KiroCachePolicyForm>,
}

#[function_component(KiroCachePolicyEditor)]
fn kiro_cache_policy_editor(props: &KiroCachePolicyEditorProps) -> Html {
    let on_add_band = {
        let form = props.form.clone();
        Callback::from(move |_| {
            let mut next = (*form).clone();
            next.bands.push(KiroCachePolicyBandForm::default());
            form.set(next);
        })
    };

    html! {
        <div class={classes!("space-y-3")}>
            <div class={classes!("grid", "gap-3", "md:grid-cols-2")}>
                <label class={classes!("block", "text-sm")}>
                    <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Boost Target Tokens" }</div>
                    <input
                        class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "font-mono", "text-sm")}
                        value={props.form.target_input_tokens.clone()}
                        oninput={{
                            let form = props.form.clone();
                            Callback::from(move |event: InputEvent| {
                                let input: HtmlInputElement = event.target_unchecked_into();
                                let mut next = (*form).clone();
                                next.target_input_tokens = input.value();
                                form.set(next);
                            })
                        }}
                    />
                </label>
                <label class={classes!("block", "text-sm")}>
                    <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Diagnostic Threshold" }</div>
                    <input
                        class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "font-mono", "text-sm")}
                        value={props.form.high_credit_diagnostic_threshold.clone()}
                        oninput={{
                            let form = props.form.clone();
                            Callback::from(move |event: InputEvent| {
                                let input: HtmlInputElement = event.target_unchecked_into();
                                let mut next = (*form).clone();
                                next.high_credit_diagnostic_threshold = input.value();
                                form.set(next);
                            })
                        }}
                    />
                </label>
                <label class={classes!("block", "text-sm")}>
                    <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Boost Credit Start" }</div>
                    <input
                        class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "font-mono", "text-sm")}
                        value={props.form.credit_start.clone()}
                        oninput={{
                            let form = props.form.clone();
                            Callback::from(move |event: InputEvent| {
                                let input: HtmlInputElement = event.target_unchecked_into();
                                let mut next = (*form).clone();
                                next.credit_start = input.value();
                                form.set(next);
                            })
                        }}
                    />
                </label>
                <label class={classes!("block", "text-sm")}>
                    <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Boost Credit End" }</div>
                    <input
                        class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "font-mono", "text-sm")}
                        value={props.form.credit_end.clone()}
                        oninput={{
                            let form = props.form.clone();
                            Callback::from(move |event: InputEvent| {
                                let input: HtmlInputElement = event.target_unchecked_into();
                                let mut next = (*form).clone();
                                next.credit_end = input.value();
                                form.set(next);
                            })
                        }}
                    />
                </label>
            </div>

            <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-3")}>
                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                    <div>
                        <div class={classes!("text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Prefix Tree Bands" }</div>
                        <div class={classes!("mt-1", "text-xs", "text-[var(--muted)]")}>
                            { "Bands replace as a whole when any row changes. Keep credit/rate continuity between adjacent rows." }
                        </div>
                    </div>
                    <button type="button" class={classes!("btn-terminal", "text-xs")} onclick={on_add_band}>
                        { "Add Band" }
                    </button>
                </div>
                <div class={classes!("mt-3", "space-y-2")}>
                    { for props.form.bands.iter().enumerate().map(|(index, band)| {
                        let remove_disabled = props.form.bands.len() <= 1;
                        html! {
                            <div class={classes!("grid", "gap-2", "md:grid-cols-[repeat(4,minmax(0,1fr))_auto]", "items-end")}>
                                <label class={classes!("block", "text-sm")}>
                                    <div class={classes!("mb-1", "text-[11px]", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ format!("Band {} Credit Start", index + 1) }</div>
                                    <input
                                        class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "font-mono", "text-sm")}
                                        value={band.credit_start.clone()}
                                        oninput={{
                                            let form = props.form.clone();
                                            Callback::from(move |event: InputEvent| {
                                                let input: HtmlInputElement = event.target_unchecked_into();
                                                let mut next = (*form).clone();
                                                if let Some(target_band) = next.bands.get_mut(index) {
                                                    target_band.credit_start = input.value();
                                                }
                                                form.set(next);
                                            })
                                        }}
                                    />
                                </label>
                                <label class={classes!("block", "text-sm")}>
                                    <div class={classes!("mb-1", "text-[11px]", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ format!("Band {} Credit End", index + 1) }</div>
                                    <input
                                        class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "font-mono", "text-sm")}
                                        value={band.credit_end.clone()}
                                        oninput={{
                                            let form = props.form.clone();
                                            Callback::from(move |event: InputEvent| {
                                                let input: HtmlInputElement = event.target_unchecked_into();
                                                let mut next = (*form).clone();
                                                if let Some(target_band) = next.bands.get_mut(index) {
                                                    target_band.credit_end = input.value();
                                                }
                                                form.set(next);
                                            })
                                        }}
                                    />
                                </label>
                                <label class={classes!("block", "text-sm")}>
                                    <div class={classes!("mb-1", "text-[11px]", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ format!("Band {} Ratio Start", index + 1) }</div>
                                    <input
                                        class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "font-mono", "text-sm")}
                                        value={band.cache_ratio_start.clone()}
                                        oninput={{
                                            let form = props.form.clone();
                                            Callback::from(move |event: InputEvent| {
                                                let input: HtmlInputElement = event.target_unchecked_into();
                                                let mut next = (*form).clone();
                                                if let Some(target_band) = next.bands.get_mut(index) {
                                                    target_band.cache_ratio_start = input.value();
                                                }
                                                form.set(next);
                                            })
                                        }}
                                    />
                                </label>
                                <label class={classes!("block", "text-sm")}>
                                    <div class={classes!("mb-1", "text-[11px]", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ format!("Band {} Ratio End", index + 1) }</div>
                                    <input
                                        class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "font-mono", "text-sm")}
                                        value={band.cache_ratio_end.clone()}
                                        oninput={{
                                            let form = props.form.clone();
                                            Callback::from(move |event: InputEvent| {
                                                let input: HtmlInputElement = event.target_unchecked_into();
                                                let mut next = (*form).clone();
                                                if let Some(target_band) = next.bands.get_mut(index) {
                                                    target_band.cache_ratio_end = input.value();
                                                }
                                                form.set(next);
                                            })
                                        }}
                                    />
                                </label>
                                <button
                                    type="button"
                                    class={classes!("btn-terminal", "text-xs")}
                                    disabled={remove_disabled}
                                    onclick={{
                                        let form = props.form.clone();
                                        Callback::from(move |_| {
                                            let mut next = (*form).clone();
                                            if next.bands.len() > 1 {
                                                next.bands.remove(index);
                                                form.set(next);
                                            }
                                        })
                                    }}
                                >
                                    { "Remove" }
                                </button>
                            </div>
                        }
                    }) }
                </div>
            </div>
        </div>
    }
}

#[derive(Properties, PartialEq)]
pub(crate) struct KiroAccountCardProps {
    pub(crate) account: KiroAccountView,
    pub(crate) proxy_configs: Vec<AdminUpstreamProxyConfigView>,
    pub(crate) on_reload: Callback<()>,
    pub(crate) flash: UseStateHandle<Option<String>>,
    pub(crate) notify: Callback<(String, bool)>,
    pub(crate) error: UseStateHandle<Option<String>>,
}

#[function_component(KiroAccountCard)]
pub(crate) fn kiro_account_card(props: &KiroAccountCardProps) -> Html {
    let expanded = use_state(|| false);
    let scheduler_max = use_state(|| props.account.kiro_channel_max_concurrency.to_string());
    let scheduler_min = use_state(|| props.account.kiro_channel_min_start_interval_ms.to_string());
    let minimum_remaining_credits_before_block =
        use_state(|| format_float4(props.account.minimum_remaining_credits_before_block));
    let selected_proxy = use_state(|| kiro_account_proxy_select_value(&props.account));
    let feedback = use_state(|| None::<String>);
    let busy = use_state(|| false);

    {
        let account = props.account.clone();
        let scheduler_max = scheduler_max.clone();
        let scheduler_min = scheduler_min.clone();
        let minimum_remaining_credits_before_block = minimum_remaining_credits_before_block.clone();
        let selected_proxy = selected_proxy.clone();
        use_effect_with(props.account.clone(), move |_| {
            scheduler_max.set(account.kiro_channel_max_concurrency.to_string());
            scheduler_min.set(account.kiro_channel_min_start_interval_ms.to_string());
            minimum_remaining_credits_before_block
                .set(format_float4(account.minimum_remaining_credits_before_block));
            selected_proxy.set(kiro_account_proxy_select_value(&account));
            || ()
        });
    }

    let on_refresh_cache = {
        let account_name = props.account.name.clone();
        let flash = props.flash.clone();
        let notify = props.notify.clone();
        let error = props.error.clone();
        let feedback = feedback.clone();
        let busy = busy.clone();
        let on_reload = props.on_reload.clone();
        Callback::from(move |_| {
            let account_name = account_name.clone();
            let flash = flash.clone();
            let notify = notify.clone();
            let error = error.clone();
            let feedback = feedback.clone();
            let busy = busy.clone();
            let on_reload = on_reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                busy.set(true);
                error.set(None);
                match refresh_admin_kiro_account_balance(&account_name).await {
                    Ok(_) => {
                        feedback.set(Some("Cache refreshed.".to_string()));
                        let message = format!("Refreshed cached balance for `{account_name}`.");
                        flash.set(Some(message.clone()));
                        notify.emit((message, false));
                        on_reload.emit(());
                    },
                    Err(err) => {
                        error.set(Some(err.clone()));
                        notify.emit((
                            format!(
                                "Failed to refresh cached balance for `{account_name}`.\n{err}"
                            ),
                            true,
                        ));
                    },
                }
                busy.set(false);
            });
        })
    };

    let on_delete_account = {
        let account_name = props.account.name.clone();
        let flash = props.flash.clone();
        let notify = props.notify.clone();
        let error = props.error.clone();
        let feedback = feedback.clone();
        let busy = busy.clone();
        let on_reload = props.on_reload.clone();
        Callback::from(move |_| {
            let account_name = account_name.clone();
            let flash = flash.clone();
            let notify = notify.clone();
            let error = error.clone();
            let feedback = feedback.clone();
            let busy = busy.clone();
            let on_reload = on_reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                busy.set(true);
                error.set(None);
                match delete_admin_kiro_account(&account_name).await {
                    Ok(_) => {
                        feedback.set(Some("Deleted.".to_string()));
                        let message = format!("Deleted `{account_name}`.");
                        flash.set(Some(message.clone()));
                        notify.emit((message, false));
                        on_reload.emit(());
                    },
                    Err(err) => {
                        error.set(Some(err.clone()));
                        notify.emit((format!("Failed to delete `{account_name}`.\n{err}"), true));
                    },
                }
                busy.set(false);
            });
        })
    };

    let on_save_scheduler = {
        let account_name = props.account.name.clone();
        let scheduler_max = scheduler_max.clone();
        let scheduler_min = scheduler_min.clone();
        let minimum_remaining_credits_before_block = minimum_remaining_credits_before_block.clone();
        let selected_proxy = selected_proxy.clone();
        let flash = props.flash.clone();
        let notify = props.notify.clone();
        let error = props.error.clone();
        let feedback = feedback.clone();
        let busy = busy.clone();
        let on_reload = props.on_reload.clone();
        Callback::from(move |_| {
            let account_name = account_name.clone();
            let scheduler_max = scheduler_max.clone();
            let scheduler_min = scheduler_min.clone();
            let minimum_remaining_credits_before_block =
                minimum_remaining_credits_before_block.clone();
            let selected_proxy = selected_proxy.clone();
            let flash = flash.clone();
            let notify = notify.clone();
            let error = error.clone();
            let feedback = feedback.clone();
            let busy = busy.clone();
            let on_reload = on_reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let parsed_max = match (*scheduler_max).trim().parse::<u64>() {
                    Ok(value) => value,
                    Err(_) => {
                        let message = "Max concurrency must be a valid integer.".to_string();
                        error.set(Some(message.clone()));
                        notify.emit((message, true));
                        return;
                    },
                };
                let parsed_min = match (*scheduler_min).trim().parse::<u64>() {
                    Ok(value) => value,
                    Err(_) => {
                        let message = "Min start interval must be a valid integer.".to_string();
                        error.set(Some(message.clone()));
                        notify.emit((message, true));
                        return;
                    },
                };
                let parsed_minimum_remaining_credits_before_block =
                    match (*minimum_remaining_credits_before_block)
                        .trim()
                        .parse::<f64>()
                    {
                        Ok(value) if value.is_finite() && value >= 0.0 => value,
                        _ => {
                            let message = "Minimum remaining credits must be a non-negative \
                                           number."
                                .to_string();
                            error.set(Some(message.clone()));
                            notify.emit((message, true));
                            return;
                        },
                    };
                busy.set(true);
                error.set(None);
                let (proxy_mode, proxy_config_id) = if *selected_proxy == "direct" {
                    (Some("direct".to_string()), None)
                } else if let Some(proxy_config_id) = (*selected_proxy).strip_prefix("fixed:") {
                    (Some("fixed".to_string()), Some(proxy_config_id.to_string()))
                } else {
                    (Some("inherit".to_string()), None)
                };
                match patch_admin_kiro_account(&account_name, &PatchKiroAccountInput {
                    kiro_channel_max_concurrency: Some(parsed_max),
                    kiro_channel_min_start_interval_ms: Some(parsed_min),
                    minimum_remaining_credits_before_block: Some(
                        parsed_minimum_remaining_credits_before_block,
                    ),
                    proxy_mode,
                    proxy_config_id,
                })
                .await
                {
                    Ok(_) => {
                        feedback.set(Some("Account settings saved.".to_string()));
                        let message = format!("Updated account settings for `{account_name}`.");
                        flash.set(Some(message.clone()));
                        notify.emit((message, false));
                        on_reload.emit(());
                    },
                    Err(err) => {
                        error.set(Some(err.clone()));
                        notify.emit((
                            format!(
                                "Failed to update account settings for `{account_name}`.\n{err}"
                            ),
                            true,
                        ));
                    },
                }
                busy.set(false);
            });
        })
    };

    let toggle_expanded = {
        let expanded = expanded.clone();
        Callback::from(move |_| expanded.set(!*expanded))
    };

    let account = props.account.clone();
    let email = account.email.clone().unwrap_or_else(|| "-".to_string());
    let expires_at = account
        .expires_at
        .clone()
        .unwrap_or_else(|| "-".to_string());
    let profile_arn = account
        .profile_arn
        .clone()
        .unwrap_or_else(|| "-".to_string());
    let machine_id = account
        .machine_id
        .clone()
        .unwrap_or_else(|| "-".to_string());
    let region = account.region.clone().unwrap_or_else(|| "-".to_string());
    let auth_region = account
        .auth_region
        .clone()
        .unwrap_or_else(|| "-".to_string());
    let api_region = account
        .api_region
        .clone()
        .unwrap_or_else(|| "-".to_string());
    let proxy_url = account.proxy_url.clone().unwrap_or_else(|| "-".to_string());
    let effective_proxy_url = account
        .effective_proxy_url
        .clone()
        .unwrap_or_else(|| "direct".to_string());
    let source = account.source.clone().unwrap_or_else(|| "-".to_string());
    let source_db_path = account
        .source_db_path
        .clone()
        .unwrap_or_else(|| "-".to_string());
    let last_imported = format_timestamp_opt(account.last_imported_at);
    let disabled_reason = format_kiro_disabled_reason(account.disabled_reason.as_deref());

    html! {
        <article class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
            <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                <div>
                    <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                        <span class={kiro_badge()}>
                            { "Kiro" }
                        </span>
                        <h3 class={classes!("m-0", "text-lg", "font-semibold")}>{ account.name.clone() }</h3>
                        if account.disabled {
                            <span class={classes!("inline-flex", "items-center", "rounded-full", "border", "border-amber-500/20", "bg-amber-500/10", "px-2.5", "py-1", "text-[11px]", "font-semibold", "uppercase", "tracking-[0.16em]", "text-amber-700", "dark:text-amber-200")}>
                                { "disabled" }
                            </span>
                        }
                    </div>
                    <p class={classes!("mt-2", "mb-0", "text-sm", "text-[var(--muted)]")}>
                        { format!("{} · provider {} · refresh {}", account.auth_method, account.provider.clone().unwrap_or_else(|| "-".to_string()), if account.has_refresh_token { "present" } else { "missing" }) }
                    </p>
                    <p class={classes!("mt-1", "mb-0", "text-xs", "font-mono", "text-[var(--muted)]")}>
                        { format_cache_summary(&account) }
                    </p>
                    <p class={classes!("mt-1", "mb-0", "text-xs", "font-mono", "text-[var(--muted)]")}>
                        { format!(
                            "scheduler {} in-flight · {} ms spacing · credit floor {}",
                            account.kiro_channel_max_concurrency,
                            account.kiro_channel_min_start_interval_ms,
                            format_float4(account.minimum_remaining_credits_before_block)
                        ) }
                    </p>
                    if let Some(cache_error) = account.cache.error_message.clone() {
                        <p class={classes!("mt-1", "mb-0", "text-xs", "font-mono", "text-amber-700", "dark:text-amber-200")}>
                            { cache_error }
                        </p>
                    }
                    if let Some(disabled_reason) = disabled_reason {
                        <p class={classes!("mt-1", "mb-0", "text-xs", "font-mono", "text-amber-700", "dark:text-amber-200")}>
                            { disabled_reason }
                        </p>
                    }
                </div>
                <div class={classes!("flex", "gap-2", "flex-wrap")}>
                    <button type="button" class={classes!("btn-terminal")} onclick={on_refresh_cache.clone()} disabled={*busy}>
                        { "Refresh Cache" }
                    </button>
                    <button type="button" class={classes!("btn-terminal", "!text-red-600", "dark:!text-red-300")} onclick={on_delete_account.clone()} disabled={*busy}>
                        { "Delete" }
                    </button>
                </div>
            </div>

            <div class={classes!("mt-4", "rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                <div class={classes!("text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Quota Snapshot" }</div>
                if let Some(balance) = account.balance.clone() {
                    { quota_progress_bar(&balance, account.subscription_title.clone()) }
                } else {
                    <p class={classes!("mt-3", "mb-0", "text-sm", "text-[var(--muted)]")}>{ "Balance not loaded yet." }</p>
                }
            </div>

            <button
                type="button"
                class={classes!("mt-3", "btn-terminal", "text-xs")}
                onclick={toggle_expanded}
            >
                { if *expanded { "收起详情 ▲" } else { "展开详情 ▼" } }
            </button>

            if *expanded {
                <div class={classes!("mt-3", "grid", "gap-4", "lg:grid-cols-3")}>
                    <div class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                        <div class={classes!("text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Identity" }</div>
                        <dl class={classes!("mt-3", "space-y-2", "text-sm")}>
                            <div><dt class={classes!("inline", "text-[var(--muted)]")}>{ "email: " }</dt><dd class={classes!("inline", "font-mono", "break-all")}>{ email }</dd></div>
                            <div><dt class={classes!("inline", "text-[var(--muted)]")}>{ "expires_at: " }</dt><dd class={classes!("inline", "font-mono", "break-all")}>{ expires_at }</dd></div>
                            <div><dt class={classes!("inline", "text-[var(--muted)]")}>{ "profileArn: " }</dt><dd class={classes!("inline", "font-mono", "break-all")}>{ profile_arn }</dd></div>
                            <div><dt class={classes!("inline", "text-[var(--muted)]")}>{ "machineId: " }</dt><dd class={classes!("inline", "font-mono", "break-all")}>{ machine_id }</dd></div>
                        </dl>
                    </div>
                    <div class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                        <div class={classes!("text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Regions / Proxy" }</div>
                        <dl class={classes!("mt-3", "space-y-2", "text-sm")}>
                            <div><dt class={classes!("inline", "text-[var(--muted)]")}>{ "region: " }</dt><dd class={classes!("inline", "font-mono")}>{ region }</dd></div>
                            <div><dt class={classes!("inline", "text-[var(--muted)]")}>{ "auth_region: " }</dt><dd class={classes!("inline", "font-mono")}>{ auth_region }</dd></div>
                            <div><dt class={classes!("inline", "text-[var(--muted)]")}>{ "api_region: " }</dt><dd class={classes!("inline", "font-mono")}>{ api_region }</dd></div>
                            <div><dt class={classes!("inline", "text-[var(--muted)]")}>{ "effective_proxy: " }</dt><dd class={classes!("inline", "font-mono", "break-all")}>{ format!("{} · {}", account.effective_proxy_source, effective_proxy_url) }</dd></div>
                            <div><dt class={classes!("inline", "text-[var(--muted)]")}>{ "effective_proxy_config: " }</dt><dd class={classes!("inline", "font-mono", "break-all")}>{ account.effective_proxy_config_name.clone().unwrap_or_else(|| "-".to_string()) }</dd></div>
                            <div><dt class={classes!("inline", "text-[var(--muted)]")}>{ "legacy_proxy_url: " }</dt><dd class={classes!("inline", "font-mono", "break-all")}>{ proxy_url }</dd></div>
                        </dl>
                    </div>
                    <div class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                        <div class={classes!("text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Source" }</div>
                        <dl class={classes!("mt-3", "space-y-2", "text-sm")}>
                            <div><dt class={classes!("inline", "text-[var(--muted)]")}>{ "source: " }</dt><dd class={classes!("inline", "font-mono")}>{ source }</dd></div>
                            <div><dt class={classes!("inline", "text-[var(--muted)]")}>{ "source_db_path: " }</dt><dd class={classes!("inline", "font-mono", "break-all")}>{ source_db_path }</dd></div>
                            <div><dt class={classes!("inline", "text-[var(--muted)]")}>{ "last_imported_at: " }</dt><dd class={classes!("inline", "font-mono")}>{ last_imported }</dd></div>
                        </dl>
                    </div>
                </div>
                <div class={classes!("mt-4", "rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                    <div class={classes!("text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Scheduler / Proxy" }</div>
                    <div class={classes!("mt-3", "grid", "gap-3", "md:grid-cols-4")}>
                        <label class={classes!("text-sm")}>
                            <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Max Concurrency" }</div>
                            <input
                                class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "text-sm", "font-mono")}
                                value={(*scheduler_max).clone()}
                                oninput={{
                                    let scheduler_max = scheduler_max.clone();
                                    Callback::from(move |event: InputEvent| {
                                        let input: HtmlInputElement = event.target_unchecked_into();
                                        scheduler_max.set(input.value());
                                    })
                                }}
                            />
                        </label>
                        <label class={classes!("text-sm")}>
                            <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Min Start Interval Ms" }</div>
                            <input
                                class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "text-sm", "font-mono")}
                                value={(*scheduler_min).clone()}
                                oninput={{
                                    let scheduler_min = scheduler_min.clone();
                                    Callback::from(move |event: InputEvent| {
                                        let input: HtmlInputElement = event.target_unchecked_into();
                                        scheduler_min.set(input.value());
                                    })
                                }}
                            />
                        </label>
                        <label class={classes!("text-sm")}>
                            <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Min Remaining Credits" }</div>
                            <input
                                class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "text-sm", "font-mono")}
                                value={(*minimum_remaining_credits_before_block).clone()}
                                oninput={{
                                    let minimum_remaining_credits_before_block =
                                        minimum_remaining_credits_before_block.clone();
                                    Callback::from(move |event: InputEvent| {
                                        let input: HtmlInputElement = event.target_unchecked_into();
                                        minimum_remaining_credits_before_block.set(input.value());
                                    })
                                }}
                            />
                        </label>
                        <label class={classes!("text-sm")}>
                            <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Proxy Mode" }</div>
                            <select
                                class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "text-sm")}
                                value={(*selected_proxy).clone()}
                                onchange={{
                                    let selected_proxy = selected_proxy.clone();
                                    Callback::from(move |event: Event| {
                                        let input: HtmlSelectElement = event.target_unchecked_into();
                                        selected_proxy.set(input.value());
                                    })
                                }}
                            >
                                <option value="inherit" selected={*selected_proxy == "inherit"}>{ "Inherit Provider Proxy" }</option>
                                <option value="direct" selected={*selected_proxy == "direct"}>{ "Direct / No Proxy" }</option>
                                { for props.proxy_configs.iter().map(|proxy_config| {
                                    let option_value = format!("fixed:{}", proxy_config.id);
                                    html! {
                                        <option value={option_value.clone()} selected={*selected_proxy == option_value}>
                                            { format!("Fixed · {} · {}", proxy_config.name, proxy_config.proxy_url) }
                                        </option>
                                    }
                                }) }
                            </select>
                        </label>
                    </div>
                    <div class={classes!("mt-3", "flex", "items-center", "gap-3", "flex-wrap")}>
                        <button type="button" class={classes!("btn-terminal", "btn-terminal-primary")} onclick={on_save_scheduler} disabled={*busy}>
                            { if *busy { "Saving..." } else { "Save Account Settings" } }
                        </button>
                        <span class={classes!("text-xs", "text-[var(--muted)]")}>
                            { "并发、起步间隔、剩余积分阈值和账号级 proxy 选择一起保存。阈值为 0 表示保持原行为，只在 remaining <= 0 时停用账号。" }
                        </span>
                    </div>
                </div>
            }

            if let Some(message) = (*feedback).clone() {
                <div class={classes!("mt-3", "text-sm", "text-[var(--muted)]")}>{ message }</div>
            }
        </article>
    }
}

#[derive(Properties, PartialEq)]
struct KiroKeyEditorCardProps {
    key_item: AdminLlmGatewayKeyView,
    persisted_global_policy_form: KiroCachePolicyForm,
    available_models: Vec<KiroModelView>,
    accounts: Vec<KiroAccountView>,
    account_groups: Vec<AdminAccountGroupView>,
    on_reload: Callback<()>,
    on_copy: Callback<(String, String)>,
    on_flash: Callback<(String, bool)>,
}

#[function_component(KiroKeyEditorCard)]
fn kiro_key_editor_card(props: &KiroKeyEditorCardProps) -> Html {
    let initial_effective_policy_form_result =
        parse_kiro_cache_policy_form_json(&props.key_item.effective_kiro_cache_policy_json);
    let effective_policy_parse_error = initial_effective_policy_form_result.as_ref().err().cloned();
    let initial_effective_policy_form = initial_effective_policy_form_result
        .clone()
        .unwrap_or_else(|_| KiroCachePolicyForm::default());
    let initial_override_enabled = !props.key_item.uses_global_kiro_cache_policy;
    let name = use_state(|| props.key_item.name.clone());
    let quota = use_state(|| props.key_item.quota_billable_limit.to_string());
    let status = use_state(|| props.key_item.status.clone());
    let route_strategy = use_state(|| {
        props
            .key_item
            .route_strategy
            .clone()
            .unwrap_or_else(|| "auto".to_string())
    });
    let account_group_id = use_state(|| {
        sanitize_kiro_account_group_id(
            props.key_item.account_group_id.as_deref(),
            &props.account_groups,
            true,
        )
    });
    let model_name_map = use_state(|| props.key_item.model_name_map.clone().unwrap_or_default());
    let kiro_request_validation_enabled =
        use_state(|| props.key_item.kiro_request_validation_enabled);
    let kiro_cache_estimation_enabled = use_state(|| props.key_item.kiro_cache_estimation_enabled);
    let policy_override_enabled = use_state(|| initial_override_enabled);
    let key_policy_form = use_state(|| initial_effective_policy_form.clone());
    let key_policy_effective_baseline = use_state(|| initial_effective_policy_form.clone());
    let route_settings_expanded = use_state(|| false);
    let model_mapping_expanded = use_state(|| false);
    let saving = use_state(|| false);
    let feedback = use_state(|| None::<String>);

    {
        let key_item = props.key_item.clone();
        let account_groups = props.account_groups.clone();
        let initial_effective_policy_form = initial_effective_policy_form.clone();
        let name = name.clone();
        let quota = quota.clone();
        let status = status.clone();
        let route_strategy = route_strategy.clone();
        let account_group_id = account_group_id.clone();
        let model_name_map = model_name_map.clone();
        let kiro_request_validation_enabled = kiro_request_validation_enabled.clone();
        let kiro_cache_estimation_enabled = kiro_cache_estimation_enabled.clone();
        let policy_override_enabled = policy_override_enabled.clone();
        let key_policy_form = key_policy_form.clone();
        let key_policy_effective_baseline = key_policy_effective_baseline.clone();
        use_effect_with((props.key_item.clone(), props.account_groups.clone()), move |_| {
            name.set(key_item.name.clone());
            quota.set(key_item.quota_billable_limit.to_string());
            status.set(key_item.status.clone());
            route_strategy.set(
                key_item
                    .route_strategy
                    .clone()
                    .unwrap_or_else(|| "auto".to_string()),
            );
            account_group_id.set(sanitize_kiro_account_group_id(
                key_item.account_group_id.as_deref(),
                &account_groups,
                true,
            ));
            model_name_map.set(key_item.model_name_map.clone().unwrap_or_default());
            kiro_request_validation_enabled.set(key_item.kiro_request_validation_enabled);
            kiro_cache_estimation_enabled.set(key_item.kiro_cache_estimation_enabled);
            policy_override_enabled.set(initial_override_enabled);
            key_policy_form.set(initial_effective_policy_form.clone());
            key_policy_effective_baseline.set(initial_effective_policy_form.clone());
            || ()
        });
    }

    let on_save = {
        let key_id = props.key_item.id.clone();
        let key_name = props.key_item.name.clone();
        let persisted_global_policy_form = props.persisted_global_policy_form.clone();
        let initial_effective_policy_form = initial_effective_policy_form.clone();
        let has_effective_policy_parse_error = effective_policy_parse_error.is_some();
        let name = name.clone();
        let quota = quota.clone();
        let status = status.clone();
        let route_strategy = route_strategy.clone();
        let account_group_id = account_group_id.clone();
        let model_name_map = model_name_map.clone();
        let kiro_request_validation_enabled = kiro_request_validation_enabled.clone();
        let kiro_cache_estimation_enabled = kiro_cache_estimation_enabled.clone();
        let policy_override_enabled = policy_override_enabled.clone();
        let key_policy_form = key_policy_form.clone();
        let saving = saving.clone();
        let feedback = feedback.clone();
        let on_flash = props.on_flash.clone();
        let on_reload = props.on_reload.clone();
        Callback::from(move |_| {
            let key_id = key_id.clone();
            let key_name = key_name.clone();
            let persisted_global_policy_form = persisted_global_policy_form.clone();
            let initial_effective_policy_form = initial_effective_policy_form.clone();
            let name_value = (*name).clone();
            let quota_value = (*quota).clone();
            let status_value = (*status).clone();
            let route_strategy_value = (*route_strategy).clone();
            let account_group_id_value = (*account_group_id).clone();
            let model_name_map_value = (*model_name_map).clone();
            let kiro_request_validation_enabled_value = *kiro_request_validation_enabled;
            let kiro_cache_estimation_enabled_value = *kiro_cache_estimation_enabled;
            let policy_override_enabled_value = *policy_override_enabled;
            let key_policy_form_value = (*key_policy_form).clone();
            let saving = saving.clone();
            let feedback = feedback.clone();
            let on_flash = on_flash.clone();
            let on_reload = on_reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let parsed_quota = match quota_value.trim().parse::<u64>() {
                    Ok(value) => value,
                    Err(_) => {
                        let message = "Quota must be a valid integer.".to_string();
                        feedback.set(Some(message.clone()));
                        on_flash.emit((message, true));
                        return;
                    },
                };
                let policy_override_json = if has_effective_policy_parse_error {
                    None
                } else {
                    match build_kiro_cache_policy_override_patch(
                        &persisted_global_policy_form,
                        initial_override_enabled,
                        &initial_effective_policy_form,
                        policy_override_enabled_value,
                        &key_policy_form_value,
                    ) {
                        Ok(value) => value,
                        Err(err) => {
                            feedback.set(Some(err.clone()));
                            on_flash.emit((err, true));
                            return;
                        },
                    }
                };
                saving.set(true);
                feedback.set(None);
                match patch_admin_kiro_key(&key_id, PatchAdminLlmGatewayKeyRequest {
                    name: Some(name_value.trim()),
                    status: Some(status_value.trim()),
                    public_visible: None,
                    quota_billable_limit: Some(parsed_quota),
                    route_strategy: Some(route_strategy_value.as_str()),
                    account_group_id: Some(account_group_id_value.as_str()),
                    fixed_account_name: None,
                    auto_account_names: None,
                    model_name_map: Some(&model_name_map_value),
                    request_max_concurrency: None,
                    request_min_start_interval_ms: None,
                    kiro_request_validation_enabled: Some(kiro_request_validation_enabled_value),
                    kiro_cache_estimation_enabled: Some(kiro_cache_estimation_enabled_value),
                    kiro_cache_policy_override_json: policy_override_json
                        .as_ref()
                        .map(|value| value.as_deref()),
                    request_max_concurrency_unlimited: false,
                    request_min_start_interval_ms_unlimited: false,
                })
                .await
                {
                    Ok(_) => {
                        feedback.set(Some("Saved.".to_string()));
                        on_flash.emit((format!("Saved Kiro key `{key_name}`."), false));
                        on_reload.emit(());
                    },
                    Err(err) => {
                        feedback.set(Some(err.clone()));
                        on_flash
                            .emit((format!("Failed to save Kiro key `{key_name}`.\n{err}"), true));
                    },
                }
                saving.set(false);
            });
        })
    };

    let on_disable = {
        let key_id = props.key_item.id.clone();
        let key_name = props.key_item.name.clone();
        let name = name.clone();
        let quota = quota.clone();
        let route_strategy = route_strategy.clone();
        let account_group_id = account_group_id.clone();
        let model_name_map = model_name_map.clone();
        let kiro_cache_estimation_enabled = kiro_cache_estimation_enabled.clone();
        let saving = saving.clone();
        let feedback = feedback.clone();
        let on_flash = props.on_flash.clone();
        let on_reload = props.on_reload.clone();
        Callback::from(move |_| {
            let key_id = key_id.clone();
            let key_name = key_name.clone();
            let name_value = (*name).clone();
            let quota_value = (*quota).clone();
            let route_strategy_value = (*route_strategy).clone();
            let account_group_id_value = (*account_group_id).clone();
            let model_name_map_value = (*model_name_map).clone();
            let kiro_cache_estimation_enabled_value = *kiro_cache_estimation_enabled;
            let saving = saving.clone();
            let feedback = feedback.clone();
            let on_flash = on_flash.clone();
            let on_reload = on_reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let parsed_quota = match quota_value.trim().parse::<u64>() {
                    Ok(value) => value,
                    Err(_) => {
                        let message = "Quota must be a valid integer.".to_string();
                        feedback.set(Some(message.clone()));
                        on_flash.emit((message, true));
                        return;
                    },
                };
                saving.set(true);
                feedback.set(None);
                match patch_admin_kiro_key(&key_id, PatchAdminLlmGatewayKeyRequest {
                    name: Some(name_value.trim()),
                    status: Some("disabled"),
                    public_visible: None,
                    quota_billable_limit: Some(parsed_quota),
                    route_strategy: Some(route_strategy_value.as_str()),
                    account_group_id: Some(account_group_id_value.as_str()),
                    fixed_account_name: None,
                    auto_account_names: None,
                    model_name_map: Some(&model_name_map_value),
                    request_max_concurrency: None,
                    request_min_start_interval_ms: None,
                    kiro_request_validation_enabled: None,
                    kiro_cache_estimation_enabled: Some(kiro_cache_estimation_enabled_value),
                    kiro_cache_policy_override_json: None,
                    request_max_concurrency_unlimited: false,
                    request_min_start_interval_ms_unlimited: false,
                })
                .await
                {
                    Ok(_) => {
                        feedback.set(Some("Disabled.".to_string()));
                        on_flash.emit((format!("Disabled Kiro key `{key_name}`."), false));
                        on_reload.emit(());
                    },
                    Err(err) => {
                        feedback.set(Some(err.clone()));
                        on_flash.emit((
                            format!("Failed to disable Kiro key `{key_name}`.\n{err}"),
                            true,
                        ));
                    },
                }
                saving.set(false);
            });
        })
    };

    let on_delete = {
        let key_id = props.key_item.id.clone();
        let key_name = props.key_item.name.clone();
        let saving = saving.clone();
        let feedback = feedback.clone();
        let on_flash = props.on_flash.clone();
        let on_reload = props.on_reload.clone();
        Callback::from(move |_| {
            let key_id = key_id.clone();
            let key_name = key_name.clone();
            let saving = saving.clone();
            let feedback = feedback.clone();
            let on_flash = on_flash.clone();
            let on_reload = on_reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                saving.set(true);
                feedback.set(None);
                match delete_admin_kiro_key(&key_id).await {
                    Ok(_) => {
                        feedback.set(Some("Deleted.".to_string()));
                        on_flash.emit((format!("Deleted Kiro key `{key_name}`."), false));
                        on_reload.emit(());
                    },
                    Err(err) => {
                        feedback.set(Some(err.clone()));
                        on_flash.emit((
                            format!("Failed to delete Kiro key `{key_name}`.\n{err}"),
                            true,
                        ));
                    },
                }
                saving.set(false);
            });
        })
    };

    let on_reset_model_map = {
        let model_name_map = model_name_map.clone();
        Callback::from(move |_| model_name_map.set(BTreeMap::new()))
    };
    let on_restore_inherit = {
        let key_id = props.key_item.id.clone();
        let key_name = props.key_item.name.clone();
        let key_policy_form = key_policy_form.clone();
        let key_policy_effective_baseline = key_policy_effective_baseline.clone();
        let policy_override_enabled = policy_override_enabled.clone();
        let saving = saving.clone();
        let feedback = feedback.clone();
        let on_flash = props.on_flash.clone();
        let on_reload = props.on_reload.clone();
        let persisted_global_policy_form = props.persisted_global_policy_form.clone();
        Callback::from(move |_| {
            if *saving {
                return;
            }
            let key_id = key_id.clone();
            let key_name = key_name.clone();
            let key_policy_form = key_policy_form.clone();
            let key_policy_effective_baseline = key_policy_effective_baseline.clone();
            let policy_override_enabled = policy_override_enabled.clone();
            let saving = saving.clone();
            let feedback = feedback.clone();
            let on_flash = on_flash.clone();
            let on_reload = on_reload.clone();
            let persisted_global_policy_form = persisted_global_policy_form.clone();
            wasm_bindgen_futures::spawn_local(async move {
                saving.set(true);
                feedback.set(None);
                match patch_admin_kiro_key(&key_id, PatchAdminLlmGatewayKeyRequest {
                    kiro_cache_policy_override_json: Some(None),
                    ..PatchAdminLlmGatewayKeyRequest::default()
                })
                .await
                {
                    Ok(_) => {
                        key_policy_effective_baseline.set(persisted_global_policy_form.clone());
                        policy_override_enabled.set(false);
                        key_policy_form.set(persisted_global_policy_form.clone());
                        feedback.set(Some("Restored inherit.".to_string()));
                        on_flash
                            .emit((format!("Restored inherit for Kiro key `{key_name}`."), false));
                        on_reload.emit(());
                    },
                    Err(err) => {
                        feedback.set(Some(err.clone()));
                        on_flash.emit((
                            format!("Failed to restore inherit for Kiro key `{key_name}`.\n{err}"),
                            true,
                        ));
                    },
                }
                saving.set(false);
            });
        })
    };
    let fixed_route_groups = props
        .account_groups
        .iter()
        .filter(|group| group.account_names.len() == 1)
        .cloned()
        .collect::<Vec<_>>();
    let route_summary = kiro_key_route_summary(
        (*route_strategy).as_str(),
        (*account_group_id).as_str(),
        props.account_groups.as_slice(),
    );
    let candidate_credit_summary = kiro_key_candidate_credit_summary(
        (*route_strategy).as_str(),
        (*account_group_id).as_str(),
        props.accounts.as_slice(),
        props.account_groups.as_slice(),
    );
    let candidate_credit_summary_text =
        format_kiro_key_candidate_credit_summary(&candidate_credit_summary);
    let global_policy_summary = format_kiro_cache_policy_summary(
        &props.persisted_global_policy_form,
        &props.persisted_global_policy_form,
    );
    let displayed_effective_policy_form = if *policy_override_enabled {
        (*key_policy_form).clone()
    } else {
        props.persisted_global_policy_form.clone()
    };
    let effective_policy_summary = effective_policy_parse_error
        .as_ref()
        .map(|err| format!("invalid backend policy json · {err}"))
        .unwrap_or_else(|| {
            format_effective_kiro_cache_policy_summary(
                !*policy_override_enabled,
                &displayed_effective_policy_form,
            )
        });
    let policy_controls_disabled = effective_policy_parse_error.is_some() || *saving;

    let key_ratio = kiro_key_usage_ratio(
        props.key_item.remaining_billable,
        props.key_item.quota_billable_limit,
    );
    let key_pct = (key_ratio * 100.0).round() as i32;
    let model_override_count = (*model_name_map).len();
    let mapping_overrides_preview = if (*model_name_map).is_empty() {
        "identity map".to_string()
    } else {
        (*model_name_map)
            .iter()
            .map(|(source, target)| format!("{source} -> {target}"))
            .collect::<Vec<_>>()
            .join(" · ")
    };

    html! {
        <article class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-4")}>
            <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                <div>
                    <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                        <span class={kiro_badge()}>
                            { "Kiro" }
                        </span>
                        <h3 class={classes!("m-0", "text-base", "font-semibold")}>{ props.key_item.name.clone() }</h3>
                    </div>
                    <p class={classes!("mt-2", "mb-0", "text-xs", "font-mono", "text-[var(--muted)]")}>
                        { format!("{} · remaining {}", props.key_item.status, format_number_i64(props.key_item.remaining_billable)) }
                    </p>
                    <p class={classes!("mt-1", "mb-0", "text-xs", "font-mono", "text-[var(--muted)]")}>
                        { format!("credits {}", format_float4(props.key_item.usage_credit_total)) }
                        if props.key_item.usage_credit_missing_events > 0 {
                            { format!(" · partial ({} missing)", props.key_item.usage_credit_missing_events) }
                        }
                    </p>
                </div>
                <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                    <span class={classes!("text-xs", "font-mono", "text-[var(--muted)]")}>
                        { format!("created {} · used {}", format_ms(props.key_item.created_at), format_timestamp_opt(props.key_item.last_used_at)) }
                    </span>
                    <button
                        type="button"
                        class={classes!("btn-terminal", "text-xs")}
                        onclick={{
                            let on_reload = props.on_reload.clone();
                            Callback::from(move |_| on_reload.emit(()))
                        }}
                    >
                        { "Refresh" }
                    </button>
                </div>
            </div>

            <div class={classes!("mt-3")}>
                <div class={classes!("flex", "items-center", "justify-between", "font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>
                    <span>{ "用量" }</span>
                    <span>{ format!("{key_pct}%") }</span>
                </div>
                <div class={classes!("mt-1.5", "h-2", "overflow-hidden", "rounded-full", "bg-[var(--surface-alt)]")}>
                    <div class={classes!("h-full", "rounded-full", "bg-[linear-gradient(90deg,#0f766e,#2563eb)]", "transition-[width]", "duration-300")}
                         style={format!("width: {}%;", key_pct.clamp(0, 100))} />
                </div>
                <div class={classes!("mt-2", "flex", "items-center", "gap-4", "font-mono", "text-[11px]", "text-[var(--muted)]")}>
                    <span>{ format!("remaining {}", format_number_i64(props.key_item.remaining_billable)) }</span>
                    <span>{ format!("limit {}", format_number_u64(props.key_item.quota_billable_limit)) }</span>
                </div>
                <div class={classes!("mt-2", "font-mono", "text-[11px]", "text-[var(--muted)]")}>
                    { candidate_credit_summary_text }
                </div>
            </div>

            <div class={classes!("mt-4", "grid", "gap-3", "md:grid-cols-2")}>
                <div class={classes!("md:col-span-2", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-3")}>
                    <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Secret" }</div>
                    <MaskedSecretCode
                        value={props.key_item.secret.clone()}
                        copy_label={"Kiro Key"}
                        on_copy={props.on_copy.clone()}
                        code_class={classes!("leading-6", "text-[var(--text)]")}
                    />
                </div>
                <label class={classes!("text-sm")}>
                    <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Name" }</div>
                    <input
                        class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "text-sm")}
                        value={(*name).clone()}
                        oninput={{
                            let name = name.clone();
                            Callback::from(move |event: InputEvent| {
                                let input: HtmlInputElement = event.target_unchecked_into();
                                name.set(input.value());
                            })
                        }}
                    />
                </label>
                <label class={classes!("text-sm")}>
                    <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Quota" }</div>
                    <input
                        class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "text-sm", "font-mono")}
                        value={(*quota).clone()}
                        oninput={{
                            let quota = quota.clone();
                            Callback::from(move |event: InputEvent| {
                                let input: HtmlInputElement = event.target_unchecked_into();
                                quota.set(input.value());
                            })
                        }}
                    />
                </label>
                <label class={classes!("text-sm")}>
                    <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Status" }</div>
                    <select
                        key={format!("kiro-key-status-{}", props.key_item.id)}
                        class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "text-sm")}
                        value={(*status).clone()}
                        onchange={{
                            let status = status.clone();
                            Callback::from(move |event: Event| {
                                let input: HtmlSelectElement = event.target_unchecked_into();
                                status.set(input.value());
                            })
                        }}
                    >
                        <option value="active">{ "active" }</option>
                        <option value="disabled">{ "disabled" }</option>
                    </select>
                </label>
                <label class={classes!("md:col-span-2", "flex", "cursor-pointer", "items-start", "gap-3", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-3", "text-sm")}>
                    <input
                        type="checkbox"
                        checked={*kiro_request_validation_enabled}
                        onchange={{
                            let kiro_request_validation_enabled =
                                kiro_request_validation_enabled.clone();
                            Callback::from(move |event: Event| {
                                let input: HtmlInputElement = event.target_unchecked_into();
                                kiro_request_validation_enabled.set(input.checked());
                            })
                        }}
                    />
                    <span>
                        <strong>{ "请求合法性校验" }</strong>
                        <span class={classes!("block", "mt-1", "text-xs", "text-[var(--muted)]")}>
                            { "开启时会在转发前拦截明显坏掉的 Anthropic message 结构。空文本占位块现在会自动忽略；如果某个客户端仍被误伤，可以按 key 关闭这层校验。" }
                        </span>
                    </span>
                </label>
                <label class={classes!("md:col-span-2", "flex", "cursor-pointer", "items-start", "gap-3", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-3", "text-sm")}>
                    <input
                        type="checkbox"
                        checked={*kiro_cache_estimation_enabled}
                        onchange={{
                            let kiro_cache_estimation_enabled =
                                kiro_cache_estimation_enabled.clone();
                            Callback::from(move |event: Event| {
                                let input: HtmlInputElement = event.target_unchecked_into();
                                kiro_cache_estimation_enabled.set(input.checked());
                            })
                        }}
                    />
                    <span>
                        <strong>{ "Cache Token 估算" }</strong>
                        <span class={classes!("block", "mt-1", "text-xs", "text-[var(--muted)]")}>
                            { "开启时，这个 key 对外返回的 Anthropic usage 会暴露保守估算的 cache_read_input_tokens，同时 usage event 也会记入估算后的 cached / uncached split。关闭后这两处都会回到 cache=0。" }
                        </span>
                    </span>
                </label>
                <div class={classes!("md:col-span-2", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-3", "space-y-3")}>
                    <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                        <div class={classes!("space-y-1")}>
                            <div class={classes!("text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Cache Policy" }</div>
                            <div class={classes!("text-xs", "font-mono", "text-[var(--muted)]")}>
                                { format!("global: {global_policy_summary}") }
                            </div>
                            <div class={classes!("text-xs", "font-mono", "text-[var(--muted)]")}>
                                { format!("effective: {effective_policy_summary}") }
                            </div>
                        </div>
                        <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                            <label class={classes!("flex", "items-center", "gap-2", "text-sm")}>
                                <input
                                    type="checkbox"
                                    checked={*policy_override_enabled}
                                    disabled={policy_controls_disabled}
                                    onchange={{
                                        let policy_override_enabled = policy_override_enabled.clone();
                                        let key_policy_form = key_policy_form.clone();
                                        let key_policy_effective_baseline =
                                            key_policy_effective_baseline.clone();
                                        Callback::from(move |event: Event| {
                                            let input: HtmlInputElement = event.target_unchecked_into();
                                            let checked = input.checked();
                                            policy_override_enabled.set(checked);
                                            if checked {
                                                key_policy_form
                                                    .set((*key_policy_effective_baseline).clone());
                                            }
                                        })
                                    }}
                                />
                                <span>{ "Override Global Policy" }</span>
                            </label>
                            <button
                                type="button"
                                class={classes!("btn-terminal", "text-xs")}
                                disabled={!initial_override_enabled || *saving}
                                onclick={on_restore_inherit}
                            >
                                { "Restore Inherit" }
                            </button>
                        </div>
                    </div>
                    if let Some(policy_error) = effective_policy_parse_error.clone() {
                        <p class={classes!("m-0", "text-xs", "text-red-600", "dark:text-red-300")}>
                            { format!("Backend effective policy JSON is invalid: {policy_error}") }
                        </p>
                    }
                    if *policy_override_enabled {
                        if effective_policy_parse_error.is_none() {
                            <KiroCachePolicyEditor form={key_policy_form.clone()} />
                        }
                    } else {
                        <p class={classes!("m-0", "text-xs", "text-[var(--muted)]")}>
                            { "This key inherits the global cache policy. Enable override to replace only changed scalar fields or the full bands block." }
                        </p>
                    }
                </div>
                <div class={classes!("flex", "items-center", "gap-3", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "text-sm")}>
                    <span class={classes!("inline-flex", "items-center", "rounded-full", "bg-slate-900", "px-2", "py-1", "font-mono", "text-[11px]", "font-semibold", "uppercase", "tracking-[0.16em]", "text-emerald-300")}>
                        { "private" }
                    </span>
                    <span>{ "Kiro key 不会在公开页面暴露。" }</span>
                </div>
                <div class={classes!("md:col-span-2", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-3", "text-sm", "text-[var(--muted)]", "space-y-2")}>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <div>
                            <div class={classes!("text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Route Settings" }</div>
                            <div class={classes!("mt-1", "text-xs", "text-[var(--muted)]")}>{ route_summary.clone() }</div>
                        </div>
                        <button
                            type="button"
                            class={classes!("btn-terminal", "text-xs")}
                            onclick={{
                                let route_settings_expanded = route_settings_expanded.clone();
                                Callback::from(move |_| route_settings_expanded.set(!*route_settings_expanded))
                            }}
                        >
                            { if *route_settings_expanded { "Hide Route Settings" } else { "Show Route Settings" } }
                        </button>
                    </div>
                    if *route_settings_expanded {
                        <div class={classes!("flex", "items-center", "gap-3", "flex-wrap")}>
                            <label class={classes!("flex", "items-center", "gap-2", "text-sm")}>
                                <span>{ "路由" }</span>
                                    <select
                                        key={format!("{}-route-{}", props.key_item.id, (*route_strategy).clone())}
                                        class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-1.5", "text-sm")}
                                        onchange={{
                                            let route_strategy = route_strategy.clone();
                                            Callback::from(move |event: Event| {
                                                if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                                                    route_strategy.set(target.value());
                                                }
                                            })
                                        }}
                                    >
                                    <option value="auto" selected={*route_strategy == "auto"}>{ "自动 (按额度)" }</option>
                                    <option value="fixed" selected={*route_strategy == "fixed"}>{ "绑定账号" }</option>
                                </select>
                            </label>
                            if *route_strategy == "fixed" {
                                <label class={classes!("flex", "items-center", "gap-2", "text-sm")}>
                                    <span>{ "单账号组" }</span>
                                    <select
                                        key={format!("{}-group-fixed-{}", props.key_item.id, (*account_group_id).clone())}
                                        class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-1.5", "text-sm")}
                                        onchange={{
                                            let account_group_id = account_group_id.clone();
                                            Callback::from(move |event: Event| {
                                                if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                                                    account_group_id.set(target.value());
                                                }
                                            })
                                        }}
                                    >
                                        <option value="" selected={(*account_group_id).is_empty()}>{ "-- 选择组 --" }</option>
                                        { for fixed_route_groups.iter().map(|group| html! {
                                            <option value={group.id.clone()} selected={*account_group_id == group.id}>{ format!("{} ({})", group.name, group.account_names.join(", ")) }</option>
                                        }) }
                                    </select>
                                </label>
                            } else {
                                <label class={classes!("flex", "items-center", "gap-2", "text-sm")}>
                                    <span>{ "账号组" }</span>
                                    <select
                                        key={format!("{}-group-auto-{}", props.key_item.id, (*account_group_id).clone())}
                                        class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-1.5", "text-sm")}
                                        onchange={{
                                            let account_group_id = account_group_id.clone();
                                            Callback::from(move |event: Event| {
                                                if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                                                    account_group_id.set(target.value());
                                                }
                                            })
                                        }}
                                    >
                                        <option value="" selected={(*account_group_id).is_empty()}>{ "全账号池" }</option>
                                        { for props.account_groups.iter().map(|group| html! {
                                            <option value={group.id.clone()} selected={*account_group_id == group.id}>{ format!("{} ({} 个账号)", group.name, group.account_names.len()) }</option>
                                        }) }
                                    </select>
                                </label>
                            }
                        </div>
                    }
                </div>
                <div class={classes!("md:col-span-2", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-3")}>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <div>
                            <div class={classes!("text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Model Mapping" }</div>
                            <div class={classes!("mt-1", "text-xs", "text-[var(--muted)]")}>
                                { format!("{} · overrides {}", mapping_overrides_preview, model_override_count) }
                            </div>
                        </div>
                        <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                            <button
                                type="button"
                                class={classes!("btn-terminal", "text-xs")}
                                onclick={{
                                    let model_mapping_expanded = model_mapping_expanded.clone();
                                    Callback::from(move |_| model_mapping_expanded.set(!*model_mapping_expanded))
                                }}
                            >
                                { if *model_mapping_expanded { "Hide Model Mapping" } else { "Show Model Mapping" } }
                            </button>
                            if *model_mapping_expanded {
                                <button type="button" class={classes!("btn-terminal", "text-xs")} onclick={on_reset_model_map}>
                                    { "Reset To Identity" }
                                </button>
                            }
                        </div>
                    </div>
                    if *model_mapping_expanded {
                        if props.available_models.is_empty() {
                            <div class={classes!("mt-3", "text-sm", "text-[var(--muted)]")}>{ "当前没有加载到可用模型目录。" }</div>
                        } else {
                            <div class={classes!("mt-3", "space-y-2")}>
                                { for props.available_models.iter().map(|source_model| {
                                    let source_id = source_model.id.clone();
                                    let current_target = (*model_name_map)
                                        .get(&source_id)
                                        .cloned()
                                        .unwrap_or_else(|| source_id.clone());
                                    let model_name_map = model_name_map.clone();
                                    let target_models = props.available_models.clone();
                                    html! {
                                        <div class={classes!("grid", "gap-2", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-3", "lg:grid-cols-[minmax(0,1fr)_minmax(18rem,24rem)]")}>
                                            <div>
                                                <div class={classes!("text-sm", "font-semibold", "text-[var(--text)]")}>{ source_model.display_name.clone() }</div>
                                                <div class={classes!("mt-1", "font-mono", "text-[11px]", "break-all", "text-[var(--muted)]")}>{ source_model.id.clone() }</div>
                                            </div>
                                            <label class={classes!("text-sm")}>
                                                <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Map To" }</div>
                                                <select
                                                    class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "text-sm")}
                                                    value={current_target}
                                                    onchange={Callback::from(move |event: Event| {
                                                        let input: HtmlSelectElement = event.target_unchecked_into();
                                                        let selected = input.value();
                                                        let mut next = (*model_name_map).clone();
                                                        if selected == source_id {
                                                            next.remove(&source_id);
                                                        } else {
                                                            next.insert(source_id.clone(), selected);
                                                        }
                                                        model_name_map.set(next);
                                                    })}
                                                >
                                                    { for target_models.iter().map(|target_model| html! {
                                                        <option value={target_model.id.clone()}>
                                                            { format!("{} · {}", target_model.display_name, target_model.id) }
                                                        </option>
                                                    }) }
                                                </select>
                                            </label>
                                        </div>
                                    }
                                }) }
                            </div>
                            <div class={classes!("mt-3", "font-mono", "text-[11px]", "text-[var(--muted)]", "break-words")}>
                                { format!("overrides: {}", mapping_overrides_preview) }
                            </div>
                        }
                    }
                </div>
            </div>

            <div class={classes!("mt-4", "flex", "items-center", "gap-2", "flex-wrap")}>
                <button type="button" class={classes!("btn-terminal", "btn-terminal-primary")} onclick={on_save}>
                    { if *saving { "Saving..." } else { "Save" } }
                </button>
                <button type="button" class={classes!("btn-terminal")} onclick={on_disable}>
                    { "Disable" }
                </button>
                <button
                    type="button"
                    class={classes!("btn-terminal", "text-red-600", "dark:text-red-400")}
                    onclick={on_delete}
                >
                    { "Delete" }
                </button>
            </div>

            if let Some(message) = (*feedback).clone() {
                <div class={classes!("mt-3", "text-sm", "text-[var(--muted)]")}>{ message }</div>
            }
        </article>
    }
}

#[derive(Properties, PartialEq)]
struct KiroAccountGroupEditorCardProps {
    group_item: AdminAccountGroupView,
    accounts: Vec<KiroAccountView>,
    on_reload: Callback<()>,
    on_flash: Callback<(String, bool)>,
}

#[function_component(KiroAccountGroupEditorCard)]
fn kiro_account_group_editor_card(props: &KiroAccountGroupEditorCardProps) -> Html {
    let name = use_state(|| props.group_item.name.clone());
    let expanded = use_state(|| false);
    let account_names = use_state(|| {
        let valid_names = props
            .accounts
            .iter()
            .map(|account| account.name.as_str())
            .collect::<HashSet<_>>();
        let mut names = props
            .group_item
            .account_names
            .iter()
            .filter(|name| valid_names.contains(name.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        names.sort();
        names.dedup();
        names
    });
    let saving = use_state(|| false);
    let feedback = use_state(|| None::<String>);

    {
        let group_item = props.group_item.clone();
        let accounts = props.accounts.clone();
        let name = name.clone();
        let account_names = account_names.clone();
        use_effect_with((props.group_item.clone(), props.accounts.clone()), move |_| {
            let valid_names = accounts
                .iter()
                .map(|account| account.name.as_str())
                .collect::<HashSet<_>>();
            let mut names = group_item
                .account_names
                .iter()
                .filter(|member| valid_names.contains(member.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            names.sort();
            names.dedup();
            name.set(group_item.name.clone());
            account_names.set(names);
            || ()
        });
    }

    let on_toggle_account = {
        let account_names = account_names.clone();
        Callback::from(move |account_name: String| {
            let mut names = (*account_names).clone();
            if let Some(index) = names.iter().position(|name| name == &account_name) {
                names.remove(index);
            } else {
                names.push(account_name);
                names.sort();
                names.dedup();
            }
            account_names.set(names);
        })
    };

    let on_save = {
        let group_id = props.group_item.id.clone();
        let name = name.clone();
        let account_names = account_names.clone();
        let saving = saving.clone();
        let feedback = feedback.clone();
        let on_flash = props.on_flash.clone();
        let on_reload = props.on_reload.clone();
        Callback::from(move |_| {
            if *saving {
                return;
            }
            let group_id = group_id.clone();
            let name_value = (*name).trim().to_string();
            let account_names_value = (*account_names).clone();
            let saving = saving.clone();
            let feedback = feedback.clone();
            let on_flash = on_flash.clone();
            let on_reload = on_reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if name_value.is_empty() {
                    let message = "组名不能为空".to_string();
                    feedback.set(Some(message.clone()));
                    on_flash.emit((message, true));
                    return;
                }
                if account_names_value.is_empty() {
                    let message = "账号组至少需要选择一个账号".to_string();
                    feedback.set(Some(message.clone()));
                    on_flash.emit((message, true));
                    return;
                }
                saving.set(true);
                match patch_admin_kiro_account_group(&group_id, PatchAdminAccountGroupInput {
                    name: Some(&name_value),
                    account_names: Some(account_names_value.as_slice()),
                })
                .await
                {
                    Ok(_) => {
                        feedback.set(Some("Saved.".to_string()));
                        on_flash.emit((format!("已保存 Kiro 账号组 `{name_value}`"), false));
                        on_reload.emit(());
                    },
                    Err(err) => {
                        feedback.set(Some(err.clone()));
                        on_flash.emit((format!("保存 Kiro 账号组失败\n{err}"), true));
                    },
                }
                saving.set(false);
            });
        })
    };

    let on_delete = {
        let group_id = props.group_item.id.clone();
        let group_name = props.group_item.name.clone();
        let saving = saving.clone();
        let on_flash = props.on_flash.clone();
        let on_reload = props.on_reload.clone();
        Callback::from(move |_| {
            let Some(window) = web_sys::window() else {
                return;
            };
            if !window
                .confirm_with_message("确认删除这个 Kiro 账号组？")
                .ok()
                .unwrap_or(false)
            {
                return;
            }
            let group_id = group_id.clone();
            let group_name = group_name.clone();
            let saving = saving.clone();
            let on_flash = on_flash.clone();
            let on_reload = on_reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                saving.set(true);
                match delete_admin_kiro_account_group(&group_id).await {
                    Ok(_) => {
                        on_flash.emit((format!("已删除 Kiro 账号组 `{group_name}`"), false));
                        on_reload.emit(());
                    },
                    Err(err) => {
                        on_flash.emit((format!("删除 Kiro 账号组失败\n{err}"), true));
                    },
                }
                saving.set(false);
            });
        })
    };

    html! {
        <article class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-4")}>
            <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                <div>
                    <h3 class={classes!("m-0", "text-base", "font-semibold")}>{ props.group_item.name.clone() }</h3>
                    <p class={classes!("mt-1", "mb-0", "text-xs", "text-[var(--muted)]")}>
                        {
                            if props.group_item.account_names.is_empty() {
                                "没有成员账号".to_string()
                            } else {
                                format!("成员: {}", props.group_item.account_names.join(", "))
                            }
                        }
                    </p>
                </div>
                <div class={classes!("flex", "items-center", "gap-2")}>
                    <span class={classes!("text-xs", "text-[var(--muted)]")}>{ format!("{} 个账号", props.group_item.account_names.len()) }</span>
                    <button
                        type="button"
                        class={classes!("btn-terminal")}
                        onclick={{
                            let expanded = expanded.clone();
                            Callback::from(move |_| expanded.set(!*expanded))
                        }}
                    >
                        { if *expanded { "收起 ▲" } else { "展开 ▼" } }
                    </button>
                    <button class={classes!("btn-terminal", "text-red-600", "dark:text-red-300")} onclick={on_delete} disabled={*saving}>
                        { "删除" }
                    </button>
                </div>
            </div>

            if *expanded {
                <label class={classes!("mt-3", "block", "text-sm")}>
                    <span class={classes!("text-[var(--muted)]")}>{ "组名" }</span>
                    <input
                        type="text"
                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2")}
                        value={(*name).clone()}
                        oninput={{
                            let name = name.clone();
                            Callback::from(move |event: InputEvent| {
                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                    name.set(target.value());
                                }
                            })
                        }}
                    />
                </label>

                <div class={classes!("mt-3", "space-y-2")}>
                    <div class={classes!("text-sm", "text-[var(--muted)]")}>{ "成员账号" }</div>
                    <div class={classes!("grid", "gap-2", "xl:grid-cols-2")}>
                        { for props.accounts.iter().map(|account| {
                            let checked = account_names.iter().any(|name| name == &account.name);
                            let account_name = account.name.clone();
                            let on_toggle_account = on_toggle_account.clone();
                            let balance_hint = account
                                .balance
                                .as_ref()
                                .map(|balance| format!(
                                    "remaining {} / {}",
                                    format_float2(balance.remaining),
                                    format_float2(balance.usage_limit)
                                ))
                                .unwrap_or_else(|| "balance loading".to_string());
                            html! {
                                <label class={classes!(
                                    "flex", "cursor-pointer", "items-center", "gap-3", "rounded-lg", "border", "px-3", "py-2.5",
                                    if checked {
                                        "border-sky-500/30 bg-sky-500/8"
                                    } else {
                                        "border-[var(--border)] bg-[var(--surface-alt)]"
                                    }
                                )}>
                                    <input
                                        type="checkbox"
                                        checked={checked}
                                        onchange={Callback::from(move |_| on_toggle_account.emit(account_name.clone()))}
                                    />
                                    <div class={classes!("min-w-0", "flex-1")}>
                                        <div class={classes!("font-semibold", "text-[var(--text)]")}>{ account.name.clone() }</div>
                                        <div class={classes!("mt-1", "font-mono", "text-[11px]", "text-[var(--muted)]")}>
                                            { balance_hint }
                                        </div>
                                    </div>
                                </label>
                            }
                        }) }
                    </div>
                </div>

                <div class={classes!("mt-4", "flex", "items-center", "justify-between", "gap-3")}>
                    <span class={classes!("text-xs", "text-[var(--muted)]")}>
                        { format!("当前成员: {}", if account_names.is_empty() { "无".to_string() } else { account_names.join(", ") }) }
                    </span>
                    <button class={classes!("btn-terminal", "btn-terminal-primary")} onclick={on_save} disabled={*saving}>
                        { if *saving { "保存中..." } else { "保存账号组" } }
                    </button>
                </div>

                if let Some(feedback) = (*feedback).clone() {
                    <div class={classes!("mt-3", "text-sm", "text-[var(--muted)]")}>{ feedback }</div>
                }
            }
        </article>
    }
}

#[function_component(AdminKiroGatewayPage)]
/// Render the Kiro-specific admin surface.
///
/// This page owns the full CRUD workflow for Kiro accounts and private keys,
/// plus usage inspection and provider-level proxy context.
pub fn admin_kiro_gateway_page() -> Html {
    let accounts = use_state(Vec::<KiroAccountView>::new);
    let keys = use_state(Vec::<AdminLlmGatewayKeyView>::new);
    let account_groups = use_state(Vec::<AdminAccountGroupView>::new);
    let kiro_models = use_state(Vec::<KiroModelView>::new);
    let usage_events = use_state(Vec::<AdminLlmGatewayUsageEventView>::new);
    let usage_loading = use_state(|| false);
    let usage_error = use_state(|| None::<String>);
    let proxy_configs = use_state(Vec::<AdminUpstreamProxyConfigView>::new);
    let proxy_bindings = use_state(Vec::<AdminUpstreamProxyBindingView>::new);
    let runtime_config = use_state(|| None::<LlmGatewayRuntimeConfig>);
    let kiro_cache_policy_form = use_state(KiroCachePolicyForm::default);
    let persisted_kiro_cache_policy_form = use_state(KiroCachePolicyForm::default);
    let kiro_cache_kmodels_json = use_state(String::new);
    let saving_kmodel_config = use_state(|| false);
    let kiro_prefix_cache_mode = use_state(String::new);
    let kiro_prefix_cache_max_tokens = use_state(String::new);
    let kiro_prefix_cache_entry_ttl_seconds = use_state(String::new);
    let kiro_conversation_anchor_max_entries = use_state(String::new);
    let kiro_conversation_anchor_ttl_seconds = use_state(String::new);
    let loading = use_state(|| true);
    let error = use_state(|| None::<String>);
    let flash = use_state(|| None::<String>);
    let toast = use_state(|| None::<(String, bool)>);
    let toast_timeout = use_mut_ref(|| None::<Timeout>);
    let notify = {
        let flash = flash.clone();
        let toast = toast.clone();
        let toast_timeout = toast_timeout.clone();
        Callback::from(move |(message, is_error): (String, bool)| {
            flash.set(Some(message.clone()));
            toast.set(Some((message, is_error)));
            toast_timeout.borrow_mut().take();
            let toast = toast.clone();
            let clear_handle = toast_timeout.clone();
            let timeout = Timeout::new(2600, move || {
                toast.set(None);
                clear_handle.borrow_mut().take();
            });
            *toast_timeout.borrow_mut() = Some(timeout);
        })
    };
    let refresh_tick = use_state(|| 0u32);
    let active_tab = use_state(|| TAB_OVERVIEW.to_string());
    let on_tab_click = {
        let active_tab = active_tab.clone();
        Callback::from(move |tab: String| active_tab.set(tab))
    };
    let manual_form_expanded = use_state(|| false);

    let import_name = use_state(|| "default".to_string());
    let import_sqlite_path = use_state(String::new);
    let import_scheduler_max = use_state(|| "1".to_string());
    let import_scheduler_min = use_state(|| "0".to_string());

    let manual_name = use_state(String::new);
    let manual_auth_method = use_state(|| "social".to_string());
    let manual_access_token = use_state(String::new);
    let manual_refresh_token = use_state(String::new);
    let manual_profile_arn = use_state(String::new);
    let manual_expires_at = use_state(String::new);
    let manual_client_id = use_state(String::new);
    let manual_client_secret = use_state(String::new);
    let manual_region = use_state(|| "us-east-1".to_string());
    let manual_auth_region = use_state(|| "us-east-1".to_string());
    let manual_api_region = use_state(|| "us-east-1".to_string());
    let manual_machine_id = use_state(String::new);
    let manual_provider = use_state(String::new);
    let manual_email = use_state(String::new);
    let manual_subscription_title = use_state(String::new);
    let manual_scheduler_max = use_state(|| "1".to_string());
    let manual_scheduler_min = use_state(|| "0".to_string());
    let manual_minimum_remaining_credits_before_block = use_state(|| "0".to_string());
    let manual_disabled = use_state(|| false);

    let new_key_name = use_state(|| "kiro-private".to_string());
    let new_key_quota = use_state(|| "1000000".to_string());
    let create_account_group_name = use_state(String::new);
    let create_account_group_account_names = use_state(Vec::<String>::new);
    let creating_account_group = use_state(|| false);
    let account_group_form_expanded = use_state(|| false);

    let reload_usage = {
        let usage_events = usage_events.clone();
        let usage_loading = usage_loading.clone();
        let usage_error = usage_error.clone();
        Callback::from(move |_| {
            let usage_events = usage_events.clone();
            let usage_loading = usage_loading.clone();
            let usage_error = usage_error.clone();
            usage_loading.set(true);
            usage_error.set(None);
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_admin_kiro_usage_events(&AdminLlmGatewayUsageEventsQuery {
                    key_id: None,
                    limit: Some(5),
                    offset: Some(0),
                })
                .await
                {
                    Ok(usage_resp) => usage_events.set(usage_resp.events),
                    Err(err) => usage_error.set(Some(err)),
                }
                usage_loading.set(false);
            });
        })
    };

    {
        let accounts = accounts.clone();
        let keys = keys.clone();
        let account_groups = account_groups.clone();
        let kiro_models = kiro_models.clone();
        let proxy_configs = proxy_configs.clone();
        let proxy_bindings = proxy_bindings.clone();
        let runtime_config = runtime_config.clone();
        let kiro_cache_policy_form = kiro_cache_policy_form.clone();
        let persisted_kiro_cache_policy_form = persisted_kiro_cache_policy_form.clone();
        let kiro_cache_kmodels_json = kiro_cache_kmodels_json.clone();
        let kiro_prefix_cache_mode = kiro_prefix_cache_mode.clone();
        let kiro_prefix_cache_max_tokens = kiro_prefix_cache_max_tokens.clone();
        let kiro_prefix_cache_entry_ttl_seconds = kiro_prefix_cache_entry_ttl_seconds.clone();
        let kiro_conversation_anchor_max_entries = kiro_conversation_anchor_max_entries.clone();
        let kiro_conversation_anchor_ttl_seconds = kiro_conversation_anchor_ttl_seconds.clone();
        let loading = loading.clone();
        let error = error.clone();
        use_effect_with(*refresh_tick, move |_| {
            let accounts = accounts.clone();
            let keys = keys.clone();
            let account_groups = account_groups.clone();
            let kiro_models = kiro_models.clone();
            let proxy_configs = proxy_configs.clone();
            let proxy_bindings = proxy_bindings.clone();
            let runtime_config = runtime_config.clone();
            let kiro_cache_policy_form = kiro_cache_policy_form.clone();
            let persisted_kiro_cache_policy_form = persisted_kiro_cache_policy_form.clone();
            let kiro_cache_kmodels_json = kiro_cache_kmodels_json.clone();
            let kiro_prefix_cache_mode = kiro_prefix_cache_mode.clone();
            let kiro_prefix_cache_max_tokens = kiro_prefix_cache_max_tokens.clone();
            let kiro_prefix_cache_entry_ttl_seconds = kiro_prefix_cache_entry_ttl_seconds.clone();
            let kiro_conversation_anchor_max_entries = kiro_conversation_anchor_max_entries.clone();
            let kiro_conversation_anchor_ttl_seconds = kiro_conversation_anchor_ttl_seconds.clone();
            let loading = loading.clone();
            let error = error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                loading.set(true);
                error.set(None);
                let (
                    config_result,
                    accounts_result,
                    keys_result,
                    account_groups_result,
                    models_result,
                    proxy_configs_result,
                    proxy_bindings_result,
                ) = futures::join!(
                    fetch_admin_llm_gateway_config(),
                    fetch_admin_kiro_accounts(),
                    fetch_admin_kiro_keys(),
                    fetch_admin_kiro_account_groups(),
                    fetch_kiro_models(),
                    fetch_admin_llm_gateway_proxy_configs(),
                    fetch_admin_llm_gateway_proxy_bindings(),
                );
                match (
                    config_result,
                    accounts_result,
                    keys_result,
                    account_groups_result,
                    models_result,
                    proxy_configs_result,
                    proxy_bindings_result,
                ) {
                    (
                        Ok(config_resp),
                        Ok(accounts_resp),
                        Ok(keys_resp),
                        Ok(account_groups_resp),
                        Ok(models_resp),
                        Ok(proxy_configs_resp),
                        Ok(proxy_bindings_resp),
                    ) => {
                        let policy_form = match parse_kiro_cache_policy_form_json(
                            &config_resp.kiro_cache_policy_json,
                        ) {
                            Ok(policy_form) => policy_form,
                            Err(err) => {
                                error.set(Some(err));
                                loading.set(false);
                                return;
                            },
                        };
                        let should_reset_policy_editor = should_reset_kiro_cache_policy_editor(
                            (*runtime_config).is_none(),
                            &kiro_cache_policy_form,
                            &persisted_kiro_cache_policy_form,
                        );
                        if should_reset_policy_editor {
                            kiro_cache_policy_form.set(policy_form.clone());
                        }
                        persisted_kiro_cache_policy_form.set(policy_form);
                        kiro_cache_kmodels_json
                            .set(format_json_for_textarea(&config_resp.kiro_cache_kmodels_json));
                        kiro_prefix_cache_mode.set(config_resp.kiro_prefix_cache_mode.clone());
                        kiro_prefix_cache_max_tokens
                            .set(config_resp.kiro_prefix_cache_max_tokens.to_string());
                        kiro_prefix_cache_entry_ttl_seconds
                            .set(config_resp.kiro_prefix_cache_entry_ttl_seconds.to_string());
                        kiro_conversation_anchor_max_entries
                            .set(config_resp.kiro_conversation_anchor_max_entries.to_string());
                        kiro_conversation_anchor_ttl_seconds
                            .set(config_resp.kiro_conversation_anchor_ttl_seconds.to_string());
                        runtime_config.set(Some(config_resp));
                        accounts.set(accounts_resp.accounts);
                        keys.set(keys_resp.keys);
                        account_groups.set(account_groups_resp.groups);
                        kiro_models.set(models_resp.data);
                        proxy_configs.set(proxy_configs_resp.proxy_configs);
                        proxy_bindings.set(proxy_bindings_resp.bindings);
                    },
                    (Err(err), _, _, _, _, _, _)
                    | (_, Err(err), _, _, _, _, _)
                    | (_, _, Err(err), _, _, _, _)
                    | (_, _, _, Err(err), _, _, _)
                    | (_, _, _, _, Err(err), _, _)
                    | (_, _, _, _, _, Err(err), _)
                    | (_, _, _, _, _, _, Err(err)) => {
                        error.set(Some(err));
                    },
                }
                loading.set(false);
            });
            || ()
        });
    }

    {
        let reload_usage = reload_usage.clone();
        let active_tab = active_tab.clone();
        use_effect_with(((*active_tab).clone(), *refresh_tick), move |(active_tab, _)| {
            if should_load_kiro_usage_preview(active_tab) {
                reload_usage.emit(());
            }
            || ()
        });
    }

    let on_reload = {
        let refresh_tick = refresh_tick.clone();
        Callback::from(move |_| refresh_tick.set(refresh_tick.wrapping_add(1)))
    };

    let on_copy = {
        let notify = notify.clone();
        Callback::from(move |(label, value): (String, String)| {
            copy_text(&value);
            notify.emit((format!("Copied {} to clipboard.", label), false));
        })
    };

    let on_save_kiro_cache_kmodels = {
        let runtime_config = runtime_config.clone();
        let kiro_cache_policy_form_input = kiro_cache_policy_form.clone();
        let persisted_kiro_cache_policy_form_input = persisted_kiro_cache_policy_form.clone();
        let kiro_cache_kmodels_json_input = kiro_cache_kmodels_json.clone();
        let kiro_cache_kmodels_json = kiro_cache_kmodels_json.clone();
        let kiro_prefix_cache_mode_input = kiro_prefix_cache_mode.clone();
        let kiro_prefix_cache_max_tokens_input = kiro_prefix_cache_max_tokens.clone();
        let kiro_prefix_cache_entry_ttl_seconds_input = kiro_prefix_cache_entry_ttl_seconds.clone();
        let kiro_conversation_anchor_max_entries_input =
            kiro_conversation_anchor_max_entries.clone();
        let kiro_conversation_anchor_ttl_seconds_input =
            kiro_conversation_anchor_ttl_seconds.clone();
        let saving_kmodel_config = saving_kmodel_config.clone();
        let notify = notify.clone();
        let error = error.clone();
        let on_reload = on_reload.clone();
        Callback::from(move |_| {
            let runtime_config = runtime_config.clone();
            let kiro_cache_policy_form_value = (*kiro_cache_policy_form_input).clone();
            let kiro_cache_policy_form_input = kiro_cache_policy_form_input.clone();
            let persisted_kiro_cache_policy_form_input =
                persisted_kiro_cache_policy_form_input.clone();
            let kiro_cache_kmodels_json = (*kiro_cache_kmodels_json).clone();
            let kiro_cache_kmodels_json_input = kiro_cache_kmodels_json_input.clone();
            let kiro_prefix_cache_mode_value = (*kiro_prefix_cache_mode_input).clone();
            let kiro_prefix_cache_max_tokens_value = (*kiro_prefix_cache_max_tokens_input).clone();
            let kiro_prefix_cache_entry_ttl_seconds_value =
                (*kiro_prefix_cache_entry_ttl_seconds_input).clone();
            let kiro_conversation_anchor_max_entries_value =
                (*kiro_conversation_anchor_max_entries_input).clone();
            let kiro_conversation_anchor_ttl_seconds_value =
                (*kiro_conversation_anchor_ttl_seconds_input).clone();
            let kiro_prefix_cache_mode_input = kiro_prefix_cache_mode_input.clone();
            let kiro_prefix_cache_max_tokens_input = kiro_prefix_cache_max_tokens_input.clone();
            let kiro_prefix_cache_entry_ttl_seconds_input =
                kiro_prefix_cache_entry_ttl_seconds_input.clone();
            let kiro_conversation_anchor_max_entries_input =
                kiro_conversation_anchor_max_entries_input.clone();
            let kiro_conversation_anchor_ttl_seconds_input =
                kiro_conversation_anchor_ttl_seconds_input.clone();
            let saving_kmodel_config = saving_kmodel_config.clone();
            let notify = notify.clone();
            let error = error.clone();
            let on_reload = on_reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let Some(mut next_config) = (*runtime_config).clone() else {
                    let message = "Kiro runtime config is not loaded yet.".to_string();
                    error.set(Some(message.clone()));
                    notify.emit((message, true));
                    return;
                };
                let mode = kiro_prefix_cache_mode_value.trim();
                if mode != "formula" && mode != "prefix_tree" {
                    let message =
                        "Kiro prefix cache mode must be `formula` or `prefix_tree`.".to_string();
                    error.set(Some(message.clone()));
                    notify.emit((message, true));
                    return;
                }
                let Ok(prefix_cache_max_tokens) =
                    kiro_prefix_cache_max_tokens_value.trim().parse::<u64>()
                else {
                    let message =
                        "Kiro prefix cache max tokens must be a valid integer.".to_string();
                    error.set(Some(message.clone()));
                    notify.emit((message, true));
                    return;
                };
                let Ok(prefix_cache_entry_ttl_seconds) = kiro_prefix_cache_entry_ttl_seconds_value
                    .trim()
                    .parse::<u64>()
                else {
                    let message =
                        "Kiro prefix cache TTL seconds must be a valid integer.".to_string();
                    error.set(Some(message.clone()));
                    notify.emit((message, true));
                    return;
                };
                let Ok(conversation_anchor_max_entries) =
                    kiro_conversation_anchor_max_entries_value
                        .trim()
                        .parse::<u64>()
                else {
                    let message =
                        "Kiro conversation anchor max entries must be a valid integer.".to_string();
                    error.set(Some(message.clone()));
                    notify.emit((message, true));
                    return;
                };
                let Ok(conversation_anchor_ttl_seconds) =
                    kiro_conversation_anchor_ttl_seconds_value
                        .trim()
                        .parse::<u64>()
                else {
                    let message =
                        "Kiro conversation anchor TTL seconds must be a valid integer.".to_string();
                    error.set(Some(message.clone()));
                    notify.emit((message, true));
                    return;
                };
                let kiro_cache_policy_json =
                    match serialize_kiro_cache_policy_form_json(&kiro_cache_policy_form_value) {
                        Ok(value) => value,
                        Err(err) => {
                            error.set(Some(err.clone()));
                            notify.emit((err, true));
                            return;
                        },
                    };
                next_config.kiro_cache_kmodels_json = kiro_cache_kmodels_json;
                next_config.kiro_cache_policy_json = kiro_cache_policy_json;
                next_config.kiro_prefix_cache_mode = mode.to_string();
                next_config.kiro_prefix_cache_max_tokens = prefix_cache_max_tokens;
                next_config.kiro_prefix_cache_entry_ttl_seconds = prefix_cache_entry_ttl_seconds;
                next_config.kiro_conversation_anchor_max_entries = conversation_anchor_max_entries;
                next_config.kiro_conversation_anchor_ttl_seconds = conversation_anchor_ttl_seconds;
                saving_kmodel_config.set(true);
                match update_admin_llm_gateway_config(&next_config).await {
                    Ok(saved) => {
                        let saved_policy_form = match parse_kiro_cache_policy_form_json(
                            &saved.kiro_cache_policy_json,
                        ) {
                            Ok(value) => value,
                            Err(err) => {
                                error.set(Some(err.clone()));
                                notify.emit((err, true));
                                saving_kmodel_config.set(false);
                                return;
                            },
                        };
                        error.set(None);
                        kiro_cache_policy_form_input.set(saved_policy_form.clone());
                        persisted_kiro_cache_policy_form_input.set(saved_policy_form);
                        kiro_cache_kmodels_json_input
                            .set(format_json_for_textarea(&saved.kiro_cache_kmodels_json));
                        kiro_prefix_cache_mode_input.set(saved.kiro_prefix_cache_mode.clone());
                        kiro_prefix_cache_max_tokens_input
                            .set(saved.kiro_prefix_cache_max_tokens.to_string());
                        kiro_prefix_cache_entry_ttl_seconds_input
                            .set(saved.kiro_prefix_cache_entry_ttl_seconds.to_string());
                        kiro_conversation_anchor_max_entries_input
                            .set(saved.kiro_conversation_anchor_max_entries.to_string());
                        kiro_conversation_anchor_ttl_seconds_input
                            .set(saved.kiro_conversation_anchor_ttl_seconds.to_string());
                        runtime_config.set(Some(saved.clone()));
                        notify.emit(("Saved Kiro cache config.".to_string(), false));
                        on_reload.emit(());
                    },
                    Err(err) => {
                        error.set(Some(err.clone()));
                        notify.emit((format!("Failed to save Kiro cache config.\n{err}"), true));
                    },
                }
                saving_kmodel_config.set(false);
            });
        })
    };

    let on_import_local = {
        let import_name = import_name.clone();
        let import_sqlite_path = import_sqlite_path.clone();
        let import_scheduler_max = import_scheduler_max.clone();
        let import_scheduler_min = import_scheduler_min.clone();
        let flash = flash.clone();
        let notify = notify.clone();
        let error = error.clone();
        let on_reload = on_reload.clone();
        Callback::from(move |_| {
            let import_name = (*import_name).clone();
            let import_sqlite_path = (*import_sqlite_path).clone();
            let import_scheduler_max = (*import_scheduler_max).clone();
            let import_scheduler_min = (*import_scheduler_min).clone();
            let flash = flash.clone();
            let notify = notify.clone();
            let error = error.clone();
            let on_reload = on_reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let parsed_max = match import_scheduler_max.trim().parse::<u64>() {
                    Ok(value) => value,
                    Err(_) => {
                        let message = "Import max concurrency must be a valid integer.".to_string();
                        error.set(Some(message.clone()));
                        notify.emit((message, true));
                        return;
                    },
                };
                let parsed_min = match import_scheduler_min.trim().parse::<u64>() {
                    Ok(value) => value,
                    Err(_) => {
                        let message =
                            "Import min start interval must be a valid integer.".to_string();
                        error.set(Some(message.clone()));
                        notify.emit((message, true));
                        return;
                    },
                };
                error.set(None);
                match import_admin_kiro_account(
                    Some(import_name.as_str()),
                    if import_sqlite_path.trim().is_empty() {
                        None
                    } else {
                        Some(import_sqlite_path.as_str())
                    },
                    Some(parsed_max),
                    Some(parsed_min),
                )
                .await
                {
                    Ok(account) => {
                        let message = format!("Imported local Kiro auth `{}`.", account.name);
                        flash.set(Some(message.clone()));
                        notify.emit((message, false));
                        on_reload.emit(());
                    },
                    Err(err) => {
                        error.set(Some(err.clone()));
                        notify.emit((format!("Failed to import local Kiro auth.\n{err}"), true));
                    },
                }
            });
        })
    };

    let on_create_manual = {
        let manual_name = manual_name.clone();
        let manual_auth_method = manual_auth_method.clone();
        let manual_access_token = manual_access_token.clone();
        let manual_refresh_token = manual_refresh_token.clone();
        let manual_profile_arn = manual_profile_arn.clone();
        let manual_expires_at = manual_expires_at.clone();
        let manual_client_id = manual_client_id.clone();
        let manual_client_secret = manual_client_secret.clone();
        let manual_region = manual_region.clone();
        let manual_auth_region = manual_auth_region.clone();
        let manual_api_region = manual_api_region.clone();
        let manual_machine_id = manual_machine_id.clone();
        let manual_provider = manual_provider.clone();
        let manual_email = manual_email.clone();
        let manual_subscription_title = manual_subscription_title.clone();
        let manual_scheduler_max = manual_scheduler_max.clone();
        let manual_scheduler_min = manual_scheduler_min.clone();
        let manual_minimum_remaining_credits_before_block =
            manual_minimum_remaining_credits_before_block.clone();
        let manual_disabled = manual_disabled.clone();
        let flash = flash.clone();
        let notify = notify.clone();
        let error = error.clone();
        let on_reload = on_reload.clone();
        Callback::from(move |_| {
            let flash = flash.clone();
            let notify = notify.clone();
            let error = error.clone();
            let on_reload = on_reload.clone();
            let parsed_max = match (*manual_scheduler_max).trim().parse::<u64>() {
                Ok(value) => value,
                Err(_) => {
                    let message =
                        "Manual account max concurrency must be a valid integer.".to_string();
                    error.set(Some(message.clone()));
                    notify.emit((message, true));
                    return;
                },
            };
            let parsed_min = match (*manual_scheduler_min).trim().parse::<u64>() {
                Ok(value) => value,
                Err(_) => {
                    let message =
                        "Manual account min start interval must be a valid integer.".to_string();
                    error.set(Some(message.clone()));
                    notify.emit((message, true));
                    return;
                },
            };
            let parsed_minimum_remaining_credits_before_block =
                match (*manual_minimum_remaining_credits_before_block)
                    .trim()
                    .parse::<f64>()
                {
                    Ok(value) if value.is_finite() && value >= 0.0 => value,
                    _ => {
                        let message = "Manual account minimum remaining credits must be a \
                                       non-negative number."
                            .to_string();
                        error.set(Some(message.clone()));
                        notify.emit((message, true));
                        return;
                    },
                };
            let input = CreateManualKiroAccountInput {
                name: (*manual_name).trim().to_string(),
                access_token: normalized_str_option(&manual_access_token),
                refresh_token: normalized_str_option(&manual_refresh_token),
                profile_arn: normalized_str_option(&manual_profile_arn),
                expires_at: normalized_str_option(&manual_expires_at),
                auth_method: normalized_str_option(&manual_auth_method),
                client_id: normalized_str_option(&manual_client_id),
                client_secret: normalized_str_option(&manual_client_secret),
                region: normalized_str_option(&manual_region),
                auth_region: normalized_str_option(&manual_auth_region),
                api_region: normalized_str_option(&manual_api_region),
                machine_id: normalized_str_option(&manual_machine_id),
                provider: normalized_str_option(&manual_provider),
                email: normalized_str_option(&manual_email),
                subscription_title: normalized_str_option(&manual_subscription_title),
                kiro_channel_max_concurrency: Some(parsed_max),
                kiro_channel_min_start_interval_ms: Some(parsed_min),
                minimum_remaining_credits_before_block: Some(
                    parsed_minimum_remaining_credits_before_block,
                ),
                disabled: *manual_disabled,
            };
            wasm_bindgen_futures::spawn_local(async move {
                error.set(None);
                match create_admin_kiro_manual_account(&input).await {
                    Ok(account) => {
                        let message = format!("Saved manual Kiro account `{}`.", account.name);
                        flash.set(Some(message.clone()));
                        notify.emit((message, false));
                        on_reload.emit(());
                    },
                    Err(err) => {
                        error.set(Some(err.clone()));
                        notify.emit((format!("Failed to save manual Kiro account.\n{err}"), true));
                    },
                }
            });
        })
    };

    let on_create_key = {
        let new_key_name = new_key_name.clone();
        let new_key_quota = new_key_quota.clone();
        let flash = flash.clone();
        let notify = notify.clone();
        let error = error.clone();
        let on_reload = on_reload.clone();
        Callback::from(move |_| {
            let name = (*new_key_name).clone();
            let quota = (*new_key_quota).clone();
            let flash = flash.clone();
            let notify = notify.clone();
            let error = error.clone();
            let on_reload = on_reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let parsed_quota = match quota.trim().parse::<u64>() {
                    Ok(value) => value,
                    Err(_) => {
                        let message = "Quota must be a valid integer.".to_string();
                        error.set(Some(message.clone()));
                        notify.emit((message, true));
                        return;
                    },
                };
                error.set(None);
                match create_admin_kiro_key(name.trim(), parsed_quota).await {
                    Ok(key) => {
                        let message = format!("Created Kiro key `{}`.", key.name);
                        flash.set(Some(message.clone()));
                        notify.emit((message, false));
                        on_reload.emit(());
                    },
                    Err(err) => {
                        error.set(Some(err.clone()));
                        notify.emit((format!("Failed to create Kiro key.\n{err}"), true));
                    },
                }
            });
        })
    };

    let on_toggle_create_account_group_member = {
        let create_account_group_account_names = create_account_group_account_names.clone();
        Callback::from(move |account_name: String| {
            let mut names = (*create_account_group_account_names).clone();
            if let Some(index) = names.iter().position(|name| name == &account_name) {
                names.remove(index);
            } else {
                names.push(account_name);
                names.sort();
                names.dedup();
            }
            create_account_group_account_names.set(names);
        })
    };

    let on_create_account_group = {
        let create_account_group_name = create_account_group_name.clone();
        let create_account_group_account_names = create_account_group_account_names.clone();
        let creating_account_group = creating_account_group.clone();
        let notify = notify.clone();
        let error = error.clone();
        let on_reload = on_reload.clone();
        Callback::from(move |_| {
            if *creating_account_group {
                return;
            }
            let group_name = (*create_account_group_name).trim().to_string();
            let account_names = (*create_account_group_account_names).clone();
            let create_account_group_name = create_account_group_name.clone();
            let create_account_group_account_names = create_account_group_account_names.clone();
            let creating_account_group = creating_account_group.clone();
            let notify = notify.clone();
            let error = error.clone();
            let on_reload = on_reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if group_name.is_empty() {
                    let message = "账号组名称不能为空".to_string();
                    error.set(Some(message.clone()));
                    notify.emit((message, true));
                    return;
                }
                if account_names.is_empty() {
                    let message = "账号组至少需要选择一个账号".to_string();
                    error.set(Some(message.clone()));
                    notify.emit((message, true));
                    return;
                }
                creating_account_group.set(true);
                match create_admin_kiro_account_group(CreateAdminAccountGroupInput {
                    name: &group_name,
                    account_names: account_names.as_slice(),
                })
                .await
                {
                    Ok(_) => {
                        error.set(None);
                        create_account_group_name.set(String::new());
                        create_account_group_account_names.set(Vec::new());
                        notify.emit((format!("已创建 Kiro 账号组 `{group_name}`"), false));
                        on_reload.emit(());
                    },
                    Err(err) => {
                        error.set(Some(err.clone()));
                        notify.emit((format!("创建 Kiro 账号组失败\n{err}"), true));
                    },
                }
                creating_account_group.set(false);
            });
        })
    };

    let disabled_account_count = accounts.iter().filter(|a| a.disabled).count();
    let active_key_count = keys.iter().filter(|k| k.status == "active").count();

    html! {
        <main class={classes!(
            "min-h-screen",
            "bg-[var(--bg)]",
            "px-4",
            "py-8",
            "lg:px-6",
            "lg:py-10"
        )}>
            <div class={classes!("mx-auto", "max-w-6xl", "space-y-4")}>

            // ── Header (always visible) ──
            <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                <div class={classes!("flex", "items-center", "justify-between", "gap-4", "flex-wrap")}>
                    <div class={classes!("flex", "items-center", "gap-3")}>
                        <span class={kiro_badge()}>
                            { "Kiro" }
                        </span>
                        <h1 class={classes!("m-0", "font-mono", "text-xl", "font-bold", "text-[var(--text)]")}>{ "Gateway Admin" }</h1>
                    </div>
                    <div class={classes!("flex", "gap-2", "flex-wrap")}>
                        <Link<Route> to={Route::KiroAccess} classes={classes!("btn-terminal")}>{ "Kiro Access" }</Link<Route>>
                        <Link<Route> to={Route::LlmAccess} classes={classes!("btn-terminal")}>{ "LLM Access" }</Link<Route>>
                        <button
                            type="button"
                            class={classes!("btn-terminal", "btn-terminal-primary")}
                            onclick={{
                                let on_reload = on_reload.clone();
                                Callback::from(move |_| on_reload.emit(()))
                            }}
                        >
                            { if *loading { "Loading..." } else { "Refresh" } }
                        </button>
                    </div>
                </div>
                if let Some(message) = (*flash).clone() {
                    <div class={classes!("mt-4", "rounded-lg", "bg-emerald-500/10", "px-3", "py-2", "text-sm", "text-emerald-700", "dark:text-emerald-200")}>
                        { message }
                    </div>
                }
                if let Some(err) = (*error).clone() {
                    <div class={classes!("mt-4", "rounded-lg", "bg-red-500/10", "px-3", "py-2", "text-sm", "text-red-700", "dark:text-red-200")}>
                        { err }
                    </div>
                }

                <div class={classes!("mt-4", "grid", "gap-3", "grid-cols-2", "xl:grid-cols-4")}>
                    <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                        <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Accounts" }</div>
                        <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black")}>{ accounts.len() }</div>
                    </div>
                    <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                        <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Disabled" }</div>
                        <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black", if disabled_account_count > 0 { "text-amber-600" } else { "" })}>{ disabled_account_count }</div>
                    </div>
                    <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                        <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Keys" }</div>
                        <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black")}>{ keys.len() }</div>
                    </div>
                    <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                        <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Active Keys" }</div>
                        <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black")}>{ active_key_count }</div>
                    </div>
                </div>
            </section>

            // ── Tab Bar (always visible) ──
            { render_tab_bar(&active_tab, &[
                (TAB_OVERVIEW, "Overview"),
                (TAB_ACCOUNTS, "Accounts"),
                (TAB_KEYS, "Keys"),
                (TAB_GROUPS, "Groups"),
                (TAB_USAGE, "Usage"),
            ], &on_tab_click) }

            // ── Overview Tab ──
            if *active_tab == TAB_OVERVIEW {
            <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Effective Upstream Proxy" }</h2>
                {
                    if let Some(binding) = proxy_bindings.iter().find(|item| item.provider_type == "kiro") {
                        html! {
                            <div class={classes!("mt-4", "space-y-2", "text-sm")}>
                                <div class={classes!("font-mono", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>
                                    { format!("source: {}", binding.effective_source) }
                                </div>
                                <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-3")}>
                                    <div class={classes!("font-mono", "text-xs", "break-all")}>
                                        { binding.effective_proxy_url.clone().unwrap_or_else(|| "-".to_string()) }
                                    </div>
                                    if let Some(name) = binding.effective_proxy_config_name.as_deref() {
                                        <div class={classes!("mt-2", "text-xs", "text-[var(--muted)]")}>{ format!("config: {}", name) }</div>
                                    }
                                    if let Some(error_message) = binding.error_message.as_deref() {
                                        <div class={classes!("mt-2", "text-xs", "text-red-600", "dark:text-red-300")}>{ error_message }</div>
                                    }
                                </div>
                                <p class={classes!("m-0", "text-xs", "text-[var(--muted)]")}>
                                    { "这里是 Kiro 的默认 provider 级代理。账号没有单独指定时继承它；账号改成 direct/fixed 之后，会覆盖这里的默认值。" }
                                </p>
                            </div>
                        }
                    } else {
                        html! {
                            <p class={classes!("mt-4", "text-sm", "text-[var(--muted)]")}>
                                { "当前还没有拿到 Kiro provider 代理绑定状态。" }
                            </p>
                        }
                    }
                }
            </section>
            <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                    <div>
                        <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Kiro Cache Simulation" }</h2>
                        <p class={classes!("mt-2", "mb-0", "text-sm", "text-[var(--muted)]")}>
                            { "这里统一管理 Kiro 的全局 cache policy、cache 模拟模式、prefix tree 容量/TTL，以及按模型的保守 Kmodel 系数。prefix tree 模式会基于修正后的 ConversationState 做共享前缀匹配；formula 模式继续走旧的保守 credit 反推。" }
                        </p>
                    </div>
                    <button
                        type="button"
                        class={classes!("btn-terminal", "btn-terminal-primary")}
                        disabled={*saving_kmodel_config}
                        onclick={on_save_kiro_cache_kmodels}
                    >
                        { if *saving_kmodel_config { "Saving..." } else { "Save Cache Settings" } }
                    </button>
                </div>
                <div class={classes!("mt-4", "grid", "gap-3", "lg:grid-cols-2")}>
                    <div class={classes!("lg:col-span-2", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-3", "space-y-3")}>
                        <div class={classes!("space-y-1")}>
                            <div class={classes!("text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Global Cache Policy" }</div>
                            <div class={classes!("text-sm", "text-[var(--muted)]")}>
                                { "Key 级 override 只覆盖变化过的标量字段，bands 则整段替换。未覆盖的字段继续继承全局默认。" }
                            </div>
                            <div class={classes!("text-xs", "font-mono", "text-[var(--muted)]")}>
                                { format_kiro_cache_policy_summary(&kiro_cache_policy_form, &kiro_cache_policy_form) }
                            </div>
                        </div>
                        <KiroCachePolicyEditor form={kiro_cache_policy_form.clone()} />
                    </div>
                    <label class={classes!("block", "text-sm")}>
                        <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Simulation Mode" }</div>
                        <select
                            class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "text-sm")}
                            value={(*kiro_prefix_cache_mode).clone()}
                            onchange={{
                                let kiro_prefix_cache_mode = kiro_prefix_cache_mode.clone();
                                Callback::from(move |event: Event| {
                                    let input: HtmlSelectElement = event.target_unchecked_into();
                                    kiro_prefix_cache_mode.set(input.value());
                                })
                            }}
                        >
                            <option value="formula">{ "formula" }</option>
                            <option value="prefix_tree">{ "prefix_tree" }</option>
                        </select>
                    </label>
                    <label class={classes!("block", "text-sm")}>
                        <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Prefix Tree Max Tokens" }</div>
                        <input
                            class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "font-mono", "text-sm")}
                            value={(*kiro_prefix_cache_max_tokens).clone()}
                            oninput={{
                                let kiro_prefix_cache_max_tokens = kiro_prefix_cache_max_tokens.clone();
                                Callback::from(move |event: InputEvent| {
                                    let input: HtmlInputElement = event.target_unchecked_into();
                                    kiro_prefix_cache_max_tokens.set(input.value());
                                })
                            }}
                        />
                    </label>
                    <label class={classes!("block", "text-sm")}>
                        <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Prefix Tree TTL Seconds" }</div>
                        <input
                            class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "font-mono", "text-sm")}
                            value={(*kiro_prefix_cache_entry_ttl_seconds).clone()}
                            oninput={{
                                let kiro_prefix_cache_entry_ttl_seconds = kiro_prefix_cache_entry_ttl_seconds.clone();
                                Callback::from(move |event: InputEvent| {
                                    let input: HtmlInputElement = event.target_unchecked_into();
                                    kiro_prefix_cache_entry_ttl_seconds.set(input.value());
                                })
                            }}
                        />
                    </label>
                    <label class={classes!("block", "text-sm")}>
                        <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Anchor Max Entries" }</div>
                        <input
                            class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "font-mono", "text-sm")}
                            value={(*kiro_conversation_anchor_max_entries).clone()}
                            oninput={{
                                let kiro_conversation_anchor_max_entries = kiro_conversation_anchor_max_entries.clone();
                                Callback::from(move |event: InputEvent| {
                                    let input: HtmlInputElement = event.target_unchecked_into();
                                    kiro_conversation_anchor_max_entries.set(input.value());
                                })
                            }}
                        />
                    </label>
                    <label class={classes!("block", "text-sm", "lg:col-span-2")}>
                        <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Anchor TTL Seconds" }</div>
                        <input
                            class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "font-mono", "text-sm")}
                            value={(*kiro_conversation_anchor_ttl_seconds).clone()}
                            oninput={{
                                let kiro_conversation_anchor_ttl_seconds = kiro_conversation_anchor_ttl_seconds.clone();
                                Callback::from(move |event: InputEvent| {
                                    let input: HtmlInputElement = event.target_unchecked_into();
                                    kiro_conversation_anchor_ttl_seconds.set(input.value());
                                })
                            }}
                        />
                    </label>
                </div>
                <label class={classes!("mt-4", "block", "text-sm")}>
                    <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Kmodel JSON" }</div>
                    <textarea
                        class={classes!("min-h-[18rem]", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-3", "font-mono", "text-xs", "leading-6")}
                        value={(*kiro_cache_kmodels_json).clone()}
                        oninput={{
                            let kiro_cache_kmodels_json = kiro_cache_kmodels_json.clone();
                            Callback::from(move |event: InputEvent| {
                                let input: HtmlTextAreaElement = event.target_unchecked_into();
                                kiro_cache_kmodels_json.set(input.value());
                            })
                        }}
                    />
                </label>
                if let Some(config) = (*runtime_config).clone() {
                    <div class={classes!("mt-3", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-3", "text-xs", "text-[var(--muted)]", "space-y-1")}>
                        <div class={classes!("font-mono")}>
                            { format!("current stored bytes: {}", config.kiro_cache_kmodels_json.len()) }
                        </div>
                        <div class={classes!("font-mono")}>
                            { format!("current policy bytes: {}", config.kiro_cache_policy_json.len()) }
                        </div>
                        <div>
                            { format!("mode={}, prefix_tree_max_tokens={}, prefix_tree_ttl_seconds={}, anchor_max_entries={}, anchor_ttl_seconds={}", config.kiro_prefix_cache_mode, config.kiro_prefix_cache_max_tokens, config.kiro_prefix_cache_entry_ttl_seconds, config.kiro_conversation_anchor_max_entries, config.kiro_conversation_anchor_ttl_seconds) }
                        </div>
                        <div>
                            { "prefix_tree 模式使用修正后的 stable-prefix 做共享前缀匹配；formula 模式继续保留旧的保守下界反推。" }
                        </div>
                    </div>
                }
            </section>
            } // end TAB_OVERVIEW

            // ── Accounts Tab ──
            if *active_tab == TAB_ACCOUNTS {
            <section class={classes!("grid", "gap-4", "xl:grid-cols-2")}>
                <article class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Import Local Kiro CLI Auth" }</h2>
                    <div class={classes!("mt-4", "space-y-3")}>
                        <label class={classes!("block", "text-sm")}>
                            <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Account Name" }</div>
                            <input
                                class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "text-sm")}
                                value={(*import_name).clone()}
                                oninput={{
                                    let import_name = import_name.clone();
                                    Callback::from(move |event: InputEvent| {
                                        let input: HtmlInputElement = event.target_unchecked_into();
                                        import_name.set(input.value());
                                    })
                                }}
                            />
                        </label>
                        <label class={classes!("block", "text-sm")}>
                            <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "SQLite Path Override" }</div>
                            <input
                                class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "font-mono", "text-sm")}
                                placeholder="~/.local/share/kiro-cli/data.sqlite3"
                                value={(*import_sqlite_path).clone()}
                                oninput={{
                                    let import_sqlite_path = import_sqlite_path.clone();
                                    Callback::from(move |event: InputEvent| {
                                        let input: HtmlInputElement = event.target_unchecked_into();
                                        import_sqlite_path.set(input.value());
                                    })
                                }}
                            />
                        </label>
                        <div class={classes!("grid", "gap-3", "md:grid-cols-2")}>
                            <label class={classes!("block", "text-sm")}>
                                <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Max Concurrency" }</div>
                                <input
                                    class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "font-mono", "text-sm")}
                                    value={(*import_scheduler_max).clone()}
                                    oninput={{
                                        let import_scheduler_max = import_scheduler_max.clone();
                                        Callback::from(move |event: InputEvent| {
                                            let input: HtmlInputElement = event.target_unchecked_into();
                                            import_scheduler_max.set(input.value());
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("block", "text-sm")}>
                                <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Min Start Interval Ms" }</div>
                                <input
                                    class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "font-mono", "text-sm")}
                                    value={(*import_scheduler_min).clone()}
                                    oninput={{
                                        let import_scheduler_min = import_scheduler_min.clone();
                                        Callback::from(move |event: InputEvent| {
                                            let input: HtmlInputElement = event.target_unchecked_into();
                                            import_scheduler_min.set(input.value());
                                        })
                                    }}
                                />
                            </label>
                        </div>
                        <button type="button" class={classes!("btn-terminal", "btn-terminal-primary")} onclick={on_import_local}>
                            { "Import Local Auth" }
                        </button>
                    </div>
                </article>

                <article class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3")}>
                        <div>
                            <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Create Manual Kiro Account" }</h2>
                            <p class={classes!("mt-2", "mb-0", "text-sm", "text-[var(--muted)]")}>
                                { "手动填写必要或完整字段，保存成单独 JSON 文件。适合已有 refresh token / profileArn / IDC 凭据的场景。" }
                            </p>
                        </div>
                        <button
                            type="button"
                            class={classes!("btn-terminal", "text-xs")}
                            onclick={{
                                let manual_form_expanded = manual_form_expanded.clone();
                                Callback::from(move |_| manual_form_expanded.set(!*manual_form_expanded))
                            }}
                        >
                            { if *manual_form_expanded { "收起 ▲" } else { "展开 ▼" } }
                        </button>
                    </div>
                    if *manual_form_expanded {
                    <div class={classes!("mt-4", "grid", "gap-3", "lg:grid-cols-2")}>
                        { text_input("Name", &manual_name, None) }
                        <label class={classes!("text-sm")}>
                            <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Auth Method" }</div>
                            <select
                                class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "text-sm")}
                                value={(*manual_auth_method).clone()}
                                onchange={{
                                    let manual_auth_method = manual_auth_method.clone();
                                    Callback::from(move |event: Event| {
                                        let input: HtmlSelectElement = event.target_unchecked_into();
                                        manual_auth_method.set(input.value());
                                    })
                                }}
                            >
                                <option value="social">{ "social" }</option>
                                <option value="idc">{ "idc" }</option>
                            </select>
                        </label>
                        { text_input("Refresh Token", &manual_refresh_token, Some("lg:col-span-2")) }
                        { text_input("Access Token", &manual_access_token, Some("lg:col-span-2")) }
                        { text_input("Profile ARN", &manual_profile_arn, Some("lg:col-span-2")) }
                        { text_input("Expires At (RFC3339)", &manual_expires_at, None) }
                        { text_input("Provider", &manual_provider, None) }
                        { text_input("Email", &manual_email, None) }
                        { text_input("Subscription Title", &manual_subscription_title, None) }
                        { text_input("Client ID", &manual_client_id, None) }
                        { text_input("Client Secret", &manual_client_secret, None) }
                        { text_input("Region", &manual_region, None) }
                        { text_input("Auth Region", &manual_auth_region, None) }
                        { text_input("API Region", &manual_api_region, None) }
                        { text_input("Machine ID", &manual_machine_id, None) }
                        { text_input("Max Concurrency", &manual_scheduler_max, None) }
                        { text_input("Min Start Interval Ms", &manual_scheduler_min, None) }
                        { text_input(
                            "Min Remaining Credits",
                            &manual_minimum_remaining_credits_before_block,
                            Some("0 keeps the historic zero-only behavior.")
                        ) }
                    </div>
                    <div class={classes!("mt-4", "flex", "items-center", "gap-4", "flex-wrap", "text-sm", "text-[var(--muted)]")}>
                        <label class={classes!("inline-flex", "items-center", "gap-2")}>
                            <input
                                type="checkbox"
                                checked={*manual_disabled}
                                onchange={{
                                    let manual_disabled = manual_disabled.clone();
                                    Callback::from(move |event: Event| {
                                        let input: HtmlInputElement = event.target_unchecked_into();
                                        manual_disabled.set(input.checked());
                                    })
                                }}
                            />
                            { "disabled" }
                        </label>
                    </div>
                    <button type="button" class={classes!("mt-4", "btn-terminal", "btn-terminal-primary")} onclick={on_create_manual}>
                        { "Save Manual Account" }
                    </button>
                    } // end manual_form_expanded
                </article>
            </section>

            <section>
                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                    <div>
                        <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Account Status" }</h2>
                        <p class={classes!("mt-2", "mb-0", "text-sm", "text-[var(--muted)]")}>
                            { "状态卡片已经移到独立 admin 页面。那里保留现有卡片样式，并补上分页和名称前缀搜索。" }
                        </p>
                    </div>
                    <Link<Route>
                        to={kiro_account_status_route()}
                        classes={classes!("btn-terminal", "btn-terminal-primary")}
                    >
                        { kiro_account_status_cta_text() }
                    </Link<Route>>
                </div>
                <div class={classes!("mt-4", "grid", "gap-4", "lg:grid-cols-3")}>
                    <article class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                        <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Imported Accounts" }</div>
                        <div class={classes!("mt-2", "font-mono", "text-3xl", "font-black", "text-[var(--text)]")}>{ accounts.len() }</div>
                        <p class={classes!("mt-3", "mb-0", "text-sm", "text-[var(--muted)]")}>
                            { "账号越来越多之后，状态浏览和维护入口不能继续挤在同一块。" }
                        </p>
                    </article>
                    <article class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                        <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Disabled Accounts" }</div>
                        <div class={classes!("mt-2", "font-mono", "text-3xl", "font-black", if disabled_account_count > 0 { "text-amber-600" } else { "text-[var(--text)]" })}>{ disabled_account_count }</div>
                        <p class={classes!("mt-3", "mb-0", "text-sm", "text-[var(--muted)]")}>
                            { "独立状态页支持当前页刷新，适合集中查看这些异常或停用账号。" }
                        </p>
                    </article>
                    <article class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                        <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Search / Paging" }</div>
                        <div class={classes!("mt-2", "font-mono", "text-sm", "text-[var(--text)]")}>{ "prefix match + paginated cards" }</div>
                        <p class={classes!("mt-3", "mb-0", "text-sm", "text-[var(--muted)]")}>
                            { "这里保留导入和手工创建；状态浏览改到新页面做，避免旧设计继续膨胀。" }
                        </p>
                    </article>
                </div>
                if (*accounts).is_empty() {
                    <div class={classes!("mt-4", "rounded-xl", "border", "border-dashed", "border-[var(--border)]", "bg-[var(--surface)]", "p-5", "text-sm", "text-[var(--muted)]")}>
                        { "当前还没有导入任何 Kiro 账号。可以先从上面的 SQLite 导入，或者手动填写字段生成一个账号文件。" }
                    </div>
                }
            </section>
            } // end TAB_ACCOUNTS

            // ── Keys Tab ──
            if *active_tab == TAB_KEYS {
            <section>
                <article class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Create Kiro Key" }</h2>
                    <div class={classes!("mt-4", "space-y-3")}>
                        <label class={classes!("block", "text-sm")}>
                            <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Key Name" }</div>
                            <input
                                class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "text-sm")}
                                value={(*new_key_name).clone()}
                                oninput={{
                                    let new_key_name = new_key_name.clone();
                                    Callback::from(move |event: InputEvent| {
                                        let input: HtmlInputElement = event.target_unchecked_into();
                                        new_key_name.set(input.value());
                                    })
                                }}
                            />
                        </label>
                        <label class={classes!("block", "text-sm")}>
                            <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Quota" }</div>
                            <input
                                class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "text-sm", "font-mono")}
                                value={(*new_key_quota).clone()}
                                oninput={{
                                    let new_key_quota = new_key_quota.clone();
                                    Callback::from(move |event: InputEvent| {
                                        let input: HtmlInputElement = event.target_unchecked_into();
                                        new_key_quota.set(input.value());
                                    })
                                }}
                            />
                        </label>
                        <button type="button" class={classes!("btn-terminal", "btn-terminal-primary")} onclick={on_create_key}>
                            { "Create Kiro Key" }
                        </button>
                    </div>
                </article>
            </section>

            <section>
                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                    <div>
                        <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Kiro Key Inventory" }</h2>
                    </div>
                    <button
                        type="button"
                        class={classes!("btn-terminal")}
                        onclick={{
                            let on_reload = on_reload.clone();
                            Callback::from(move |_| on_reload.emit(()))
                        }}
                    >
                        { if *loading { "Refreshing..." } else { "Refresh" } }
                    </button>
                </div>
                <div class={classes!("mt-4", "grid", "gap-4", "xl:grid-cols-2")}>
                    {
                        if (*keys).is_empty() {
                            html! {
                                <div class={classes!("rounded-xl", "border", "border-dashed", "border-[var(--border)]", "bg-[var(--surface)]", "p-5", "text-sm", "text-[var(--muted)]")}>
                                    { "还没有 Kiro key。先创建一个，然后把 base URL 和 key 发给 Claude Code 或 Anthropic SDK 使用。" }
                                </div>
                            }
                        } else {
                            html! {
                                for (*keys).iter().map(|key_item| html! {
                                    <KiroKeyEditorCard
                                        key={key_item.id.clone()}
                                        key_item={key_item.clone()}
                                        persisted_global_policy_form={
                                            (*persisted_kiro_cache_policy_form).clone()
                                        }
                                        available_models={(*kiro_models).clone()}
                                        accounts={(*accounts).clone()}
                                        account_groups={(*account_groups).clone()}
                                        on_reload={on_reload.clone()}
                                        on_copy={on_copy.clone()}
                                        on_flash={notify.clone()}
                                    />
                                })
                            }
                        }
                    }
                </div>
            </section>
            } // end TAB_KEYS

            if *active_tab == TAB_GROUPS {
            <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                    <div>
                        <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Kiro Account Groups" }</h2>
                        <p class={classes!("mt-2", "mb-0", "text-sm", "text-[var(--muted)]")}>
                            { "先维护账号组，再让 key 选择组。固定路由请选择单账号组；自动路由可以选任意组，留空则继续使用全账号池。" }
                        </p>
                    </div>
                    <button
                        type="button"
                        class={classes!("btn-terminal")}
                        onclick={{
                            let on_reload = on_reload.clone();
                            Callback::from(move |_| on_reload.emit(()))
                        }}
                    >
                        { if *loading { "Refreshing..." } else { "Refresh Groups" } }
                    </button>
                </div>

                <div class={classes!("mt-4", "rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <div>
                            <h3 class={classes!("m-0", "text-sm", "font-semibold")}>{ "Create Kiro Account Group" }</h3>
                            <p class={classes!("mt-1", "mb-0", "text-xs", "text-[var(--muted)]")}>
                                { "默认收起，需要时再展开，不和 key 列表混在一起。" }
                            </p>
                        </div>
                        <button
                            type="button"
                            class={classes!("btn-terminal")}
                            onclick={{
                                let account_group_form_expanded = account_group_form_expanded.clone();
                                Callback::from(move |_| account_group_form_expanded.set(!*account_group_form_expanded))
                            }}
                        >
                            { if *account_group_form_expanded { "收起 ▲" } else { "展开 ▼" } }
                        </button>
                    </div>
                    if *account_group_form_expanded {
                        <div class={classes!("mt-4", "grid", "gap-3")}>
                            <label class={classes!("text-sm")}>
                                <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Group Name" }</div>
                                <input
                                    class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "text-sm")}
                                    value={(*create_account_group_name).clone()}
                                    oninput={{
                                        let create_account_group_name = create_account_group_name.clone();
                                        Callback::from(move |event: InputEvent| {
                                            let input: HtmlInputElement = event.target_unchecked_into();
                                            create_account_group_name.set(input.value());
                                        })
                                    }}
                                />
                            </label>
                            <div class={classes!("space-y-2")}>
                                <div class={classes!("text-sm", "text-[var(--muted)]")}>{ "成员账号" }</div>
                                if accounts.is_empty() {
                                    <div class={classes!("rounded-lg", "border", "border-dashed", "border-[var(--border)]", "px-3", "py-3", "text-xs", "text-[var(--muted)]")}>
                                        { "当前没有可加入账号组的 Kiro 账号。" }
                                    </div>
                                } else {
                                    <div class={classes!("grid", "gap-2", "xl:grid-cols-2")}>
                                        { for accounts.iter().map(|account| {
                                            let checked = create_account_group_account_names.iter().any(|name| name == &account.name);
                                            let account_name = account.name.clone();
                                            let on_toggle_create_account_group_member =
                                                on_toggle_create_account_group_member.clone();
                                            let balance_hint = account
                                                .balance
                                                .as_ref()
                                                .map(|balance| format!(
                                                    "remaining {} / {}",
                                                    format_float2(balance.remaining),
                                                    format_float2(balance.usage_limit)
                                                ))
                                                .unwrap_or_else(|| "balance loading".to_string());
                                            html! {
                                                <label class={classes!(
                                                    "flex", "cursor-pointer", "items-center", "gap-3", "rounded-lg", "border", "px-3", "py-2.5",
                                                    if checked {
                                                        "border-sky-500/30 bg-sky-500/8"
                                                    } else {
                                                        "border-[var(--border)] bg-[var(--surface)]"
                                                    }
                                                )}>
                                                    <input
                                                        type="checkbox"
                                                        checked={checked}
                                                        onchange={Callback::from(move |_| {
                                                            on_toggle_create_account_group_member.emit(account_name.clone())
                                                        })}
                                                    />
                                                    <div class={classes!("min-w-0", "flex-1")}>
                                                        <div class={classes!("font-semibold", "text-[var(--text)]")}>{ account.name.clone() }</div>
                                                        <div class={classes!("mt-1", "font-mono", "text-[11px]", "text-[var(--muted)]")}>
                                                            { balance_hint }
                                                        </div>
                                                    </div>
                                                </label>
                                            }
                                        }) }
                                    </div>
                                }
                            </div>
                            <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                <span class={classes!("text-xs", "text-[var(--muted)]")}>
                                    { format!(
                                        "当前成员: {}",
                                        if create_account_group_account_names.is_empty() {
                                            "无".to_string()
                                        } else {
                                            create_account_group_account_names.join(", ")
                                        }
                                    ) }
                                </span>
                                <button
                                    type="button"
                                    class={classes!("btn-terminal", "btn-terminal-primary")}
                                    onclick={on_create_account_group}
                                    disabled={*creating_account_group}
                                >
                                    { if *creating_account_group { "Creating..." } else { "Create Group" } }
                                </button>
                            </div>
                        </div>
                    }
                </div>

                <div class={classes!("mt-4", "grid", "gap-4", "xl:grid-cols-2")}>
                    {
                        if (*account_groups).is_empty() {
                            html! {
                                <div class={classes!("rounded-xl", "border", "border-dashed", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-5", "text-sm", "text-[var(--muted)]")}>
                                    { "当前还没有 Kiro 账号组。" }
                                </div>
                            }
                        } else {
                            html! {
                                for (*account_groups).iter().map(|group_item| html! {
                                    <KiroAccountGroupEditorCard
                                        key={group_item.id.clone()}
                                        group_item={group_item.clone()}
                                        accounts={(*accounts).clone()}
                                        on_reload={on_reload.clone()}
                                        on_flash={notify.clone()}
                                    />
                                })
                            }
                        }
                    }
                </div>
            </section>
            } // end TAB_GROUPS

            // ── Usage Tab ──
            if *active_tab == TAB_USAGE {
            <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                    <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Recent Usage" }</h2>
                    <Link<Route> to={Route::AdminLlmGateway} classes={classes!("btn-terminal")}>
                        { "查看完整记录" }
                    </Link<Route>>
                </div>
                if *usage_loading {
                    <div class={classes!("mt-3", "inline-flex", "items-center", "gap-2", "text-xs", "text-[var(--muted)]")}>
                        <i class={classes!("fas", "fa-spinner", "animate-spin")} />
                        <span>{ "加载中" }</span>
                    </div>
                } else if let Some(err) = (*usage_error).clone() {
                    <div class={classes!("mt-3", "rounded-lg", "bg-red-500/10", "px-3", "py-2", "text-sm", "text-red-700", "dark:text-red-200")}>
                        { err }
                    </div>
                } else if (*usage_events).is_empty() {
                    <div class={classes!("mt-3", "font-mono", "text-sm", "text-[var(--muted)]")}>{ "暂无记录" }</div>
                } else {
                    <div class={classes!("mt-3", "space-y-2")}>
                        { for (*usage_events).iter().take(5).map(|event| {
                            let credit_text = event.credit_usage
                                .map(|c| format!("{c:.4}"))
                                .unwrap_or_else(|| "-".to_string());
                            html! {
                                <div class={classes!("flex", "items-center", "gap-3", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "font-mono", "text-xs", "flex-wrap")}>
                                    <span class={classes!("text-[var(--muted)]")}>{ format_ms(event.created_at) }</span>
                                    <span class={classes!("font-semibold", "text-[var(--text)]")}>{ event.key_name.clone() }</span>
                                    <span class={classes!("text-[var(--muted)]")}>{ event.model.clone().unwrap_or_else(|| "-".to_string()) }</span>
                                    <span class={classes!("ml-auto", "text-[var(--text)]")}>{ format!("credit {credit_text}") }</span>
                                </div>
                            }
                        }) }
                    </div>
                }
            </section>
            } // end TAB_USAGE

            if let Some((message, is_error)) = (*toast).clone() {
                <div class={classes!(
                    "fixed", "bottom-5", "right-5", "z-[90]",
                    "max-w-[min(34rem,calc(100vw-2.5rem))]",
                    "rounded-xl", "border", "px-4", "py-3",
                    "text-sm", "font-semibold", "leading-5", "whitespace-pre-wrap",
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
            </div>
        </main>
    }
}

fn normalized_str_option(state: &UseStateHandle<String>) -> Option<String> {
    let value = (**state).trim();
    (!value.is_empty()).then_some(value.to_string())
}

fn text_input(label: &str, state: &UseStateHandle<String>, extra_class: Option<&str>) -> Html {
    let state_handle = state.clone();
    let mut label_classes = classes!("block", "text-sm");
    if let Some(extra_class) = extra_class {
        label_classes.push(extra_class.to_string());
    }
    html! {
        <label class={label_classes}>
            <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ label }</div>
            <input
                class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "text-sm", "font-mono")}
                value={(**state).clone()}
                oninput={Callback::from(move |event: InputEvent| {
                    let input: HtmlInputElement = event.target_unchecked_into();
                    state_handle.set(input.value());
                })}
            />
        </label>
    }
}

fn quota_progress_bar(balance: &KiroBalanceView, account_sub_title: Option<String>) -> Html {
    let subscription_title = balance
        .subscription_title
        .clone()
        .unwrap_or_else(|| account_sub_title.unwrap_or_else(|| "-".to_string()));
    let ratio = kiro_credit_ratio(Some(balance.current_usage), Some(balance.usage_limit));
    let pct = (ratio * 100.0).round() as i32;
    html! { <>
        <div class={classes!("mt-3", "grid", "gap-3", "grid-cols-2")}>
            <div>
                <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "剩余" }</div>
                <div class={classes!("mt-1", "font-mono", "text-xl", "font-black", "text-[var(--text)]")}>
                    { format_float2(balance.remaining) }
                </div>
            </div>
            <div>
                <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "总额度" }</div>
                <div class={classes!("mt-1", "font-mono", "text-xl", "font-black", "text-[var(--text)]")}>
                    { format_float2(balance.usage_limit) }
                </div>
            </div>
        </div>
        <div class={classes!("mt-3")}>
            <div class={classes!("flex", "items-center", "justify-between", "font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>
                <span>{ "用量" }</span>
                <span>{ format!("{pct}%") }</span>
            </div>
            <div class={classes!("mt-1.5", "h-2", "overflow-hidden", "rounded-full", "bg-[var(--surface)]")}>
                <div class={classes!("h-full", "rounded-full", "bg-[linear-gradient(90deg,#0f766e,#2563eb)]", "transition-[width]", "duration-300")}
                     style={format!("width: {}%;", pct.clamp(0, 100))} />
            </div>
            <div class={classes!("mt-2", "flex", "items-center", "gap-4", "font-mono", "text-[11px]", "text-[var(--muted)]")}>
                <span>{ subscription_title }</span>
                <span class={classes!("ml-auto")}>{ format_reset_hint(balance.next_reset_at) }</span>
            </div>
        </div>
    </> }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        build_kiro_cache_policy_override_json, build_kiro_cache_policy_override_patch,
        format_kiro_cache_policy_summary, kiro_account_status_cta_text, kiro_account_status_route,
        kiro_key_candidate_credit_summary, kiro_key_route_summary,
        parse_kiro_cache_policy_form_json, sanitize_kiro_account_group_id,
        should_load_kiro_usage_preview, should_reset_kiro_cache_policy_editor, TAB_GROUPS,
        TAB_KEYS, TAB_OVERVIEW, TAB_USAGE,
    };
    use crate::{
        api::{AdminAccountGroupView, KiroAccountView, KiroBalanceView, KiroCacheView},
        router::Route,
    };

    #[test]
    fn sanitize_kiro_account_group_id_drops_unknown_value() {
        let groups =
            vec![test_group("group-alpha", &["alpha"]), test_group("group-beta", &["beta"])];

        assert_eq!(sanitize_kiro_account_group_id(Some("missing"), &groups, true), "");
        assert_eq!(
            sanitize_kiro_account_group_id(Some(" group-beta "), &groups, true),
            "group-beta"
        );
    }

    #[test]
    fn kiro_key_route_summary_uses_full_pool_text_when_group_is_empty() {
        let summary = kiro_key_route_summary("auto", "", &[]);
        assert!(summary.contains("全账号池自动择优"));
    }

    #[test]
    fn kiro_key_candidate_credit_summary_uses_auto_group_only() {
        let accounts = vec![
            test_account("alpha", Some((100.0, 40.0))),
            test_account("beta", Some((60.0, 10.0))),
            test_account("gamma", Some((30.0, 5.0))),
        ];
        let groups = vec![test_group("group-beta", &["beta"])];

        let summary = kiro_key_candidate_credit_summary("auto", "group-beta", &accounts, &groups);

        assert_eq!(summary.candidate_count, 1);
        assert_eq!(summary.loaded_balance_count, 1);
        assert_eq!(summary.missing_balance_count, 0);
        assert_eq!(summary.total_limit, 60.0);
        assert_eq!(summary.total_remaining, 10.0);
    }

    #[test]
    fn kiro_key_candidate_credit_summary_counts_missing_balances() {
        let accounts = vec![test_account("alpha", Some((100.0, 40.0))), test_account("beta", None)];
        let groups = vec![test_group("group-beta", &["beta"])];

        let summary = kiro_key_candidate_credit_summary("fixed", "group-beta", &accounts, &groups);

        assert_eq!(summary.candidate_count, 1);
        assert_eq!(summary.loaded_balance_count, 0);
        assert_eq!(summary.missing_balance_count, 1);
        assert_eq!(summary.total_limit, 0.0);
        assert_eq!(summary.total_remaining, 0.0);
    }

    #[test]
    fn format_kiro_cache_policy_summary_reports_inherit_global() {
        let global = parse_kiro_cache_policy_form_json(
            r#"{
                "small_input_high_credit_boost": {
                    "target_input_tokens": 100000,
                    "credit_start": 1.0,
                    "credit_end": 1.8
                },
                "prefix_tree_credit_ratio_bands": [
                    {
                        "credit_start": 0.3,
                        "credit_end": 1.0,
                        "cache_ratio_start": 0.7,
                        "cache_ratio_end": 0.2
                    },
                    {
                        "credit_start": 1.0,
                        "credit_end": 2.5,
                        "cache_ratio_start": 0.2,
                        "cache_ratio_end": 0.0
                    }
                ],
                "high_credit_diagnostic_threshold": 2.0
            }"#,
        )
        .expect("global policy should parse");

        let summary = format_kiro_cache_policy_summary(&global, &global);

        assert_eq!(summary, "inherit global · boost 1.0 -> 1.8 => 100000 · diag 2.0 · bands 2");
    }

    #[test]
    fn kiro_account_status_cta_text_is_stable() {
        assert_eq!(kiro_account_status_cta_text(), "Open Account Status Page");
    }

    #[test]
    fn kiro_account_status_route_points_to_admin_page() {
        assert_eq!(kiro_account_status_route(), Route::AdminKiroAccountStatus);
    }

    #[test]
    fn build_kiro_cache_policy_override_json_only_emits_changed_scalar_field() {
        let global = parse_kiro_cache_policy_form_json(
            r#"{
                "small_input_high_credit_boost": {
                    "target_input_tokens": 100000,
                    "credit_start": 1.0,
                    "credit_end": 1.8
                },
                "prefix_tree_credit_ratio_bands": [
                    {
                        "credit_start": 0.3,
                        "credit_end": 1.0,
                        "cache_ratio_start": 0.7,
                        "cache_ratio_end": 0.2
                    },
                    {
                        "credit_start": 1.0,
                        "credit_end": 2.5,
                        "cache_ratio_start": 0.2,
                        "cache_ratio_end": 0.0
                    }
                ],
                "high_credit_diagnostic_threshold": 2.0
            }"#,
        )
        .expect("global policy should parse");
        let mut edited = global.clone();
        edited.high_credit_diagnostic_threshold = "1.5".to_string();

        let override_json = build_kiro_cache_policy_override_json(&global, &edited)
            .expect("override json")
            .expect("changed policy should emit override json");
        let override_value: serde_json::Value =
            serde_json::from_str(&override_json).expect("override json should parse");

        assert_eq!(
            override_value,
            json!({
                "high_credit_diagnostic_threshold": 1.5
            })
        );
    }

    #[test]
    fn build_kiro_cache_policy_override_json_only_emits_changed_bands_block() {
        let global = parse_kiro_cache_policy_form_json(
            r#"{
                "small_input_high_credit_boost": {
                    "target_input_tokens": 100000,
                    "credit_start": 1.0,
                    "credit_end": 1.8
                },
                "prefix_tree_credit_ratio_bands": [
                    {
                        "credit_start": 0.3,
                        "credit_end": 1.0,
                        "cache_ratio_start": 0.7,
                        "cache_ratio_end": 0.2
                    },
                    {
                        "credit_start": 1.0,
                        "credit_end": 2.5,
                        "cache_ratio_start": 0.2,
                        "cache_ratio_end": 0.0
                    }
                ],
                "high_credit_diagnostic_threshold": 2.0
            }"#,
        )
        .expect("global policy should parse");
        let edited = parse_kiro_cache_policy_form_json(
            r#"{
                "small_input_high_credit_boost": {
                    "target_input_tokens": 100000,
                    "credit_start": 1.0,
                    "credit_end": 1.8
                },
                "prefix_tree_credit_ratio_bands": [
                    {
                        "credit_start": 0.5,
                        "credit_end": 1.5,
                        "cache_ratio_start": 0.6,
                        "cache_ratio_end": 0.25
                    },
                    {
                        "credit_start": 1.8,
                        "credit_end": 2.8,
                        "cache_ratio_start": 0.2,
                        "cache_ratio_end": 0.05
                    }
                ],
                "high_credit_diagnostic_threshold": 2.0
            }"#,
        )
        .expect("edited policy should parse");

        let override_json = build_kiro_cache_policy_override_json(&global, &edited)
            .expect("override json")
            .expect("changed policy should emit override json");
        let override_value: serde_json::Value =
            serde_json::from_str(&override_json).expect("override json should parse");

        assert_eq!(
            override_value,
            json!({
                "prefix_tree_credit_ratio_bands": [
                    {
                        "credit_start": 0.5,
                        "credit_end": 1.5,
                        "cache_ratio_start": 0.6,
                        "cache_ratio_end": 0.25
                    },
                    {
                        "credit_start": 1.8,
                        "credit_end": 2.8,
                        "cache_ratio_start": 0.2,
                        "cache_ratio_end": 0.05
                    }
                ]
            })
        );
    }

    #[test]
    fn build_kiro_cache_policy_override_patch_keeps_existing_override_when_unchanged() {
        let global = parse_kiro_cache_policy_form_json(
            r#"{
                "small_input_high_credit_boost": {
                    "target_input_tokens": 100000,
                    "credit_start": 1.0,
                    "credit_end": 1.8
                },
                "prefix_tree_credit_ratio_bands": [
                    {
                        "credit_start": 0.3,
                        "credit_end": 1.0,
                        "cache_ratio_start": 0.7,
                        "cache_ratio_end": 0.2
                    },
                    {
                        "credit_start": 1.0,
                        "credit_end": 2.5,
                        "cache_ratio_start": 0.2,
                        "cache_ratio_end": 0.0
                    }
                ],
                "high_credit_diagnostic_threshold": 2.0
            }"#,
        )
        .expect("global policy should parse");

        let patch = build_kiro_cache_policy_override_patch(&global, true, &global, true, &global)
            .expect("patch should build");

        assert_eq!(patch, None);
    }

    #[test]
    fn build_kiro_cache_policy_override_patch_clears_override_when_restoring_inherit() {
        let global = parse_kiro_cache_policy_form_json(
            r#"{
                "small_input_high_credit_boost": {
                    "target_input_tokens": 100000,
                    "credit_start": 1.0,
                    "credit_end": 1.8
                },
                "prefix_tree_credit_ratio_bands": [
                    {
                        "credit_start": 0.3,
                        "credit_end": 1.0,
                        "cache_ratio_start": 0.7,
                        "cache_ratio_end": 0.2
                    },
                    {
                        "credit_start": 1.0,
                        "credit_end": 2.5,
                        "cache_ratio_start": 0.2,
                        "cache_ratio_end": 0.0
                    }
                ],
                "high_credit_diagnostic_threshold": 2.0
            }"#,
        )
        .expect("global policy should parse");

        let patch = build_kiro_cache_policy_override_patch(&global, true, &global, false, &global)
            .expect("patch should build");

        assert_eq!(patch, Some(None));
    }

    #[test]
    fn should_reset_kiro_cache_policy_editor_resets_on_initial_load() {
        let persisted = parse_kiro_cache_policy_form_json(
            r#"{
                "small_input_high_credit_boost": {
                    "target_input_tokens": 100000,
                    "credit_start": 1.0,
                    "credit_end": 1.8
                },
                "prefix_tree_credit_ratio_bands": [
                    {
                        "credit_start": 0.3,
                        "credit_end": 1.0,
                        "cache_ratio_start": 0.7,
                        "cache_ratio_end": 0.2
                    },
                    {
                        "credit_start": 1.0,
                        "credit_end": 2.5,
                        "cache_ratio_start": 0.2,
                        "cache_ratio_end": 0.0
                    }
                ],
                "high_credit_diagnostic_threshold": 2.0
            }"#,
        )
        .expect("policy should parse");
        let mut draft = persisted.clone();
        draft.credit_end = "2.2".to_string();

        assert!(should_reset_kiro_cache_policy_editor(true, &draft, &persisted));
    }

    #[test]
    fn should_reset_kiro_cache_policy_editor_preserves_unsaved_draft() {
        let persisted = parse_kiro_cache_policy_form_json(
            r#"{
                "small_input_high_credit_boost": {
                    "target_input_tokens": 100000,
                    "credit_start": 1.0,
                    "credit_end": 1.8
                },
                "prefix_tree_credit_ratio_bands": [
                    {
                        "credit_start": 0.3,
                        "credit_end": 1.0,
                        "cache_ratio_start": 0.7,
                        "cache_ratio_end": 0.2
                    },
                    {
                        "credit_start": 1.0,
                        "credit_end": 2.5,
                        "cache_ratio_start": 0.2,
                        "cache_ratio_end": 0.0
                    }
                ],
                "high_credit_diagnostic_threshold": 2.0
            }"#,
        )
        .expect("policy should parse");
        let mut draft = persisted.clone();
        draft.credit_end = "2.2".to_string();

        assert!(!should_reset_kiro_cache_policy_editor(false, &draft, &persisted,));
    }

    #[test]
    fn parse_kiro_cache_policy_form_json_allows_backend_validated_policy_rules() {
        let form = parse_kiro_cache_policy_form_json(
            r#"{
                "small_input_high_credit_boost": {
                    "target_input_tokens": 0,
                    "credit_start": 2.0,
                    "credit_end": 1.0
                },
                "prefix_tree_credit_ratio_bands": [
                    {
                        "credit_start": 1.5,
                        "credit_end": 1.0,
                        "cache_ratio_start": 1.2,
                        "cache_ratio_end": -0.1
                    }
                ],
                "high_credit_diagnostic_threshold": -3.0
            }"#,
        )
        .expect("frontend should only require parseable numeric fields");

        assert_eq!(form.target_input_tokens, "0");
        assert_eq!(form.credit_start, "2.0");
        assert_eq!(form.credit_end, "1.0");
        assert_eq!(form.high_credit_diagnostic_threshold, "-3.0");
        assert_eq!(form.bands.len(), 1);
    }

    #[test]
    fn should_load_kiro_usage_preview_only_for_usage_tab() {
        assert!(should_load_kiro_usage_preview(TAB_USAGE));
        assert!(!should_load_kiro_usage_preview(TAB_OVERVIEW));
        assert!(!should_load_kiro_usage_preview(TAB_KEYS));
        assert!(!should_load_kiro_usage_preview(TAB_GROUPS));
    }

    fn test_account(name: &str, balance: Option<(f64, f64)>) -> KiroAccountView {
        KiroAccountView {
            name: name.to_string(),
            balance: balance.map(|(usage_limit, remaining)| KiroBalanceView {
                current_usage: (usage_limit - remaining).max(0.0),
                usage_limit,
                remaining,
                next_reset_at: None,
                subscription_title: None,
            }),
            cache: KiroCacheView {
                status: "ready".to_string(),
                refresh_interval_seconds: 300,
                last_checked_at: None,
                last_success_at: None,
                error_message: None,
            },
            ..KiroAccountView::default()
        }
    }

    fn test_group(id: &str, account_names: &[&str]) -> AdminAccountGroupView {
        AdminAccountGroupView {
            id: id.to_string(),
            provider_type: "kiro".to_string(),
            name: id.to_string(),
            account_names: account_names
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            created_at: 0,
            updated_at: 0,
        }
    }
}
