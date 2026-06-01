use std::collections::{BTreeMap, HashSet};

use gloo_timers::callback::{Interval, Timeout};
use js_sys::Date;
use wasm_bindgen::prelude::*;
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{
    api::{
        admin_approve_and_issue_llm_gateway_account_contribution_request,
        admin_approve_and_issue_llm_gateway_token_request,
        admin_approve_llm_gateway_sponsor_request,
        admin_reject_llm_gateway_account_contribution_request,
        admin_reject_llm_gateway_token_request,
        admin_validate_llm_gateway_account_contribution_request,
        check_admin_llm_gateway_proxy_config, check_admin_llm_gateway_proxy_config_full_chain,
        create_admin_llm_gateway_account_group, create_admin_llm_gateway_account_import_job,
        create_admin_llm_gateway_key, create_admin_llm_gateway_proxy_config,
        delete_admin_llm_gateway_account, delete_admin_llm_gateway_account_group,
        delete_admin_llm_gateway_key, delete_admin_llm_gateway_proxy_config,
        delete_admin_llm_gateway_sponsor_request,
        fetch_admin_llm_gateway_account_contribution_requests,
        fetch_admin_llm_gateway_account_group_options, fetch_admin_llm_gateway_account_groups_page,
        fetch_admin_llm_gateway_account_import_job, fetch_admin_llm_gateway_account_import_jobs,
        fetch_admin_llm_gateway_accounts, fetch_admin_llm_gateway_accounts_page,
        fetch_admin_llm_gateway_accounts_page_with_query, fetch_admin_llm_gateway_config,
        fetch_admin_llm_gateway_keys, fetch_admin_llm_gateway_keys_page,
        fetch_admin_llm_gateway_keys_page_with_query, fetch_admin_llm_gateway_proxy_bindings,
        fetch_admin_llm_gateway_proxy_configs, fetch_admin_llm_gateway_sponsor_requests,
        fetch_admin_llm_gateway_token_requests, fetch_admin_llm_gateway_usage_event_detail,
        fetch_admin_llm_gateway_usage_events, fetch_admin_llm_gateway_usage_filter_options,
        fetch_admin_usage_journal_preview, fetch_admin_usage_journal_status,
        import_admin_legacy_kiro_proxy_configs, import_admin_llm_gateway_account,
        patch_admin_llm_gateway_account, patch_admin_llm_gateway_account_group,
        patch_admin_llm_gateway_key, patch_admin_llm_gateway_proxy_config,
        probe_admin_llm_gateway_account_models, refresh_admin_llm_gateway_account_auth,
        refresh_admin_llm_gateway_account_usage, reset_admin_llm_gateway_proxy_config_override,
        update_admin_llm_gateway_config, update_admin_llm_gateway_proxy_binding,
        AccountSummaryView, AdminAccountGroupOptionView, AdminAccountGroupView,
        AdminAccountsSummaryView, AdminLlmGatewayAccountContributionRequestView,
        AdminLlmGatewayAccountContributionRequestsQuery, AdminLlmGatewayAccountPageQuery,
        AdminLlmGatewayKeyPageQuery, AdminLlmGatewayKeyView, AdminLlmGatewayKeysSummaryView,
        AdminLlmGatewaySponsorRequestView, AdminLlmGatewaySponsorRequestsQuery,
        AdminLlmGatewayTokenRequestView, AdminLlmGatewayTokenRequestsQuery,
        AdminLlmGatewayUsageEventDetailView, AdminLlmGatewayUsageEventView,
        AdminLlmGatewayUsageEventsQuery, AdminLlmGatewayUsageFilterOptionsResponse,
        AdminUpstreamProxyBindingView, AdminUpstreamProxyCheckResponse,
        AdminUpstreamProxyCheckTargetView, AdminUpstreamProxyConfigScopeView,
        AdminUpstreamProxyConfigView, AdminUpstreamProxyEndpointCheckView,
        AdminUsageJournalFileView, AdminUsageJournalPreviewResponse, AdminUsageJournalStatusView,
        AdminUsageTotalsView, CodexAccountImportJobDetailView, CodexAccountImportJobSummaryView,
        CreateAdminAccountGroupInput, CreateAdminUpstreamProxyConfigInput, LlmGatewayRuntimeConfig,
        PatchAdminAccountGroupInput, PatchAdminLlmGatewayAccountInput,
        PatchAdminLlmGatewayKeyRequest, PatchAdminUpstreamProxyConfigInput,
        ProcessMemoryRuntimeStats, DEFAULT_LLM_GATEWAY_CODEX_CLIENT_VERSION,
    },
    components::{
        date_range_picker::DateRangePicker, pagination::Pagination, search_box::SearchBox,
        tab_bar::render_tab_bar,
    },
    pages::llm_access_shared::{
        confirm_destructive, credit_usage_missing_label, format_ms, format_number_i64,
        format_number_u64, token_usage_missing_label, MaskedSecretCode,
    },
    router::Route,
};

const USAGE_PAGE_SIZE: usize = 20;
const JOURNAL_PREVIEW_PAGE_SIZE: usize = 20;
const DEFAULT_ADMIN_GROUP_PAGE_SIZE: usize = 24;
const USAGE_SOURCE_HOT: &str = "hot";
const USAGE_SOURCE_ARCHIVE: &str = "archive";
const USAGE_SOURCE_ALL: &str = "all";
const USAGE_STATUS_KIND_ALL: &str = "all";
const USAGE_STATUS_KIND_OK: &str = "ok";
const USAGE_STATUS_KIND_NON_OK: &str = "non_ok";
const TOKEN_REQUEST_PAGE_SIZE: usize = 20;
const ACCOUNT_CONTRIBUTION_REQUEST_PAGE_SIZE: usize = 20;
const SPONSOR_REQUEST_PAGE_SIZE: usize = 20;
const ADMIN_CODEX_IMPORT_JOB_LIST_LIMIT: usize = 10;
const ACCOUNT_PAGE_SIZE: usize = 8;
const KEY_PAGE_SIZE: usize = 8;
const ACCOUNT_ACCENT_BORDERS: &[&str] = &[
    "border-l-4 border-l-teal-500/70",
    "border-l-4 border-l-violet-500/70",
    "border-l-4 border-l-amber-500/70",
    "border-l-4 border-l-sky-500/70",
    "border-l-4 border-l-rose-500/70",
];

const TAB_OVERVIEW: &str = "overview";
const TAB_KEYS: &str = "keys";
const TAB_GROUPS: &str = "groups";
const TAB_ACCOUNTS: &str = "accounts";
const TAB_USAGE: &str = "usage";
const TAB_JOURNAL: &str = "journal";
const TAB_REQUESTS: &str = "requests";
const TAB_SETTINGS: &str = "settings";

fn should_load_usage_journal(active_tab: &str) -> bool {
    active_tab == TAB_JOURNAL
}

fn should_load_llm_gateway_keys_inventory(active_tab: &str) -> bool {
    matches!(active_tab, TAB_KEYS | TAB_USAGE)
}

fn should_load_llm_gateway_group_options(active_tab: &str) -> bool {
    active_tab == TAB_KEYS
}

fn should_load_llm_gateway_import_jobs(active_tab: &str) -> bool {
    active_tab == TAB_ACCOUNTS
}

fn admin_group_total_pages(total: usize, page_size: usize) -> usize {
    total.max(1).div_ceil(page_size.max(1))
}

/// Render a horizontal tab bar with an optional numeric badge on one tab.
/// `badge_tab` is `Some((tab_id, count))` to show a pending-count pill.
// NOTE: the implementation moved to `crate::components::tab_bar::render_tab_bar`.
// Keep this comment block to preserve git blame context for reviewers.

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

struct ParsedAdminCodexAuthJson {
    id_token: String,
    access_token: String,
    refresh_token: String,
    account_id: Option<String>,
}

fn parse_admin_codex_auth_json(raw: &str) -> Result<ParsedAdminCodexAuthJson, String> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|_| "auth.json 不是合法 JSON".to_string())?;
    if !value.is_object() {
        return Err("auth.json 必须是 JSON object".to_string());
    }
    let id_token = optional_auth_json_string(&value, &["id_token", "idToken"]).unwrap_or_default();
    let access_token =
        optional_auth_json_string(&value, &["access_token", "accessToken"]).unwrap_or_default();
    let refresh_token =
        optional_auth_json_string(&value, &["refresh_token", "refreshToken"]).unwrap_or_default();
    if id_token.is_empty() && access_token.is_empty() && refresh_token.is_empty() {
        return Err("auth.json 没有识别到可用 token 字段".to_string());
    }
    Ok(ParsedAdminCodexAuthJson {
        id_token,
        access_token,
        refresh_token,
        account_id: optional_auth_json_string(&value, &["account_id", "accountId"]),
    })
}

fn optional_auth_json_string(value: &serde_json::Value, fields: &[&str]) -> Option<String> {
    fields
        .iter()
        .find_map(|field| value.get(*field).and_then(serde_json::Value::as_str))
        .or_else(|| {
            value.get("tokens").and_then(|tokens| {
                fields
                    .iter()
                    .find_map(|field| tokens.get(*field).and_then(serde_json::Value::as_str))
            })
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn parse_admin_codex_batch_import_json(raw: &str) -> Result<Vec<serde_json::Value>, String> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|_| "批量导入内容不是合法 JSON".to_string())?;
    let items = value
        .as_array()
        .ok_or_else(|| "批量导入内容必须是 JSON array".to_string())?;
    if items.is_empty() {
        return Err("批量导入内容不能为空".to_string());
    }
    let mut normalized = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let Some(mut object) = item.as_object().cloned() else {
            return Err(format!("第 {} 项必须是 JSON object", index + 1));
        };
        let name = object
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("第 {} 项缺少有效的 name", index + 1))?;
        let auth_json = object.get("auth_json");
        let tokens = object.get("tokens");
        if auth_json.is_none() && tokens.is_none() {
            return Err(format!("第 {} 项缺少 auth_json 或 tokens", index + 1));
        }
        if let Some(value) = auth_json {
            if !value.is_object() {
                return Err(format!("第 {} 项的 auth_json 必须是 JSON object", index + 1));
            }
        }
        if let Some(value) = tokens {
            if !value.is_object() {
                return Err(format!("第 {} 项的 tokens 必须是 JSON object", index + 1));
            }
        }
        object.insert("name".to_string(), serde_json::Value::String(name.to_string()));
        normalized.push(serde_json::Value::Object(object));
    }
    Ok(normalized)
}

fn codex_import_status_tone(status: &str) -> &'static str {
    match status {
        "completed" | "imported" => "text-emerald-600 dark:text-emerald-300",
        "failed" | "conflict" => "text-red-600 dark:text-red-300",
        "running" | "queued" => "text-amber-600 dark:text-amber-300",
        "skipped" => "text-[var(--muted)]",
        _ => "text-[var(--muted)]",
    }
}

fn codex_import_job_is_terminal(status: &str) -> bool {
    matches!(status, "completed" | "failed")
}

fn upsert_codex_import_job_summary(
    jobs: &[CodexAccountImportJobSummaryView],
    summary: CodexAccountImportJobSummaryView,
) -> Vec<CodexAccountImportJobSummaryView> {
    let mut next = jobs
        .iter()
        .filter(|job| job.job_id != summary.job_id)
        .cloned()
        .collect::<Vec<_>>();
    next.insert(0, summary);
    next.truncate(ADMIN_CODEX_IMPORT_JOB_LIST_LIMIT);
    next
}

fn account_proxy_select_value(account: &AccountSummaryView) -> String {
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

fn account_configured_proxy_label(account: &AccountSummaryView) -> String {
    match account.proxy_mode.as_str() {
        "direct" => "configured: direct".to_string(),
        "fixed" => account
            .effective_proxy_config_name
            .as_deref()
            .map(|name| format!("configured: fixed ({name})"))
            .or_else(|| {
                account
                    .proxy_config_id
                    .as_deref()
                    .map(|id| format!("configured: fixed ({id})"))
            })
            .unwrap_or_else(|| "configured: fixed".to_string()),
        _ => "configured: inherit provider".to_string(),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum AccountSortMode {
    None,
    PrimaryAsc,
    PrimaryDesc,
    SecondaryAsc,
    SecondaryDesc,
}

#[derive(Clone, Copy, PartialEq)]
enum KeySortMode {
    None,
    QuotaAsc,
    QuotaDesc,
    UsageAsc,
    UsageDesc,
}

fn format_latency_ms(latency_ms: i32) -> String {
    format!("{} ms", latency_ms.max(0))
}

fn format_optional_latency_ms(latency_ms: Option<i32>) -> String {
    latency_ms
        .map(format_latency_ms)
        .unwrap_or_else(|| "-".to_string())
}

fn format_optional_latency_ms_or_na(latency_ms: Option<i32>, applicable: bool) -> String {
    if applicable {
        format_optional_latency_ms(latency_ms)
    } else {
        "n/a".to_string()
    }
}

fn usage_account_label(account_name: &Option<String>, request_url: &str, endpoint: &str) -> String {
    if let Some(account_name) = account_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return account_name.to_string();
    }
    if request_url.contains("/kiro-gateway") || endpoint.contains("generateAssistantResponse") {
        "not captured".to_string()
    } else {
        "legacy auth".to_string()
    }
}

fn routing_total_ms_from_diagnostics(raw: Option<&str>) -> Option<i32> {
    let route_total_ms = serde_json::from_str::<serde_json::Value>(raw?).ok()?;
    let route_total_ms = route_total_ms.get("route_total_ms")?.as_u64()?;
    Some(route_total_ms.min(i32::MAX as u64) as i32)
}

fn effective_routing_wait_ms(
    routing_wait_ms: Option<i32>,
    routing_diagnostics_json: Option<&str>,
) -> Option<i32> {
    routing_wait_ms.or_else(|| routing_total_ms_from_diagnostics(routing_diagnostics_json))
}

fn format_optional_bytes(bytes: Option<u64>) -> String {
    let Some(bytes) = bytes else {
        return "-".to_string();
    };
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn format_cgroup_memory_usage(memory: &ProcessMemoryRuntimeStats) -> String {
    match (memory.cgroup_current_bytes, memory.cgroup_max_bytes) {
        (Some(current), Some(max)) if max > 0 => {
            let percent = (current as f64 / max as f64 * 100.0).clamp(0.0, 999.0);
            format!(
                "{} / {} ({percent:.1}%)",
                format_optional_bytes(Some(current)),
                format_optional_bytes(Some(max))
            )
        },
        (Some(current), Some(max)) => format!(
            "{} / {}",
            format_optional_bytes(Some(current)),
            format_optional_bytes(Some(max))
        ),
        (Some(current), None) => format!("{} / limit -", format_optional_bytes(Some(current))),
        (None, Some(max)) => format!("- / {}", format_optional_bytes(Some(max))),
        (None, None) => "-".to_string(),
    }
}

fn format_optional_duration_ms(age_ms: Option<i64>) -> String {
    let Some(age_ms) = age_ms.filter(|value| *value >= 0) else {
        return "-".to_string();
    };
    if age_ms >= 3_600_000 {
        format!("{:.1} h", age_ms as f64 / 3_600_000.0)
    } else if age_ms >= 60_000 {
        format!("{:.1} min", age_ms as f64 / 60_000.0)
    } else if age_ms >= 1_000 {
        format!("{:.1} s", age_ms as f64 / 1_000.0)
    } else {
        format!("{age_ms} ms")
    }
}

fn format_relative_age_from_ms(now_ms: i64, timestamp_ms: Option<i64>) -> String {
    let age_ms = timestamp_ms.map(|timestamp| now_ms.saturating_sub(timestamp));
    format_optional_duration_ms(age_ms)
}

fn format_future_duration_ms(remaining_ms: i64) -> String {
    if remaining_ms >= 24 * 3_600_000 {
        format!("{:.1} d", remaining_ms as f64 / (24.0 * 3_600_000.0))
    } else if remaining_ms >= 3_600_000 {
        format!("{:.1} h", remaining_ms as f64 / 3_600_000.0)
    } else if remaining_ms >= 60_000 {
        format!("{:.1} min", remaining_ms as f64 / 60_000.0)
    } else if remaining_ms >= 1_000 {
        format!("{:.1} s", remaining_ms as f64 / 1_000.0)
    } else {
        format!("{remaining_ms} ms")
    }
}

fn format_access_token_expiry(now_ms: i64, expires_at_ms: Option<i64>) -> String {
    let Some(expires_at_ms) = expires_at_ms else {
        return "access token expiry -".to_string();
    };
    let absolute = format_ms(expires_at_ms);
    let remaining_ms = expires_at_ms.saturating_sub(now_ms);
    if remaining_ms > 0 {
        format!(
            "access token expires {} · ~{} left",
            absolute,
            format_future_duration_ms(remaining_ms)
        )
    } else {
        format!(
            "access token expired {} ago · {}",
            format_optional_duration_ms(Some(remaining_ms.saturating_abs())),
            absolute
        )
    }
}

fn render_usage_journal_file_list(
    title: &str,
    files: &[AdminUsageJournalFileView],
    empty_label: &str,
) -> Html {
    html! {
        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--bg)]", "p-3")}>
            <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>
                { title }
            </div>
            if files.is_empty() {
                <div class={classes!("mt-2", "text-xs", "text-[var(--muted)]")}>{ empty_label }</div>
            } else {
                <div class={classes!("mt-2", "space-y-2")}>
                    { for files.iter().map(|file| html! {
                        <div class={classes!("rounded-md", "border", "border-[var(--border)]", "px-2.5", "py-2")}>
                            <div class={classes!("flex", "items-center", "justify-between", "gap-2", "font-mono", "text-xs", "text-[var(--text)]")}>
                                <span>{ file.sequence.map(|seq| format!("#{seq}")).unwrap_or_else(|| file.file_name.clone()) }</span>
                                <span class={classes!("text-[var(--muted)]")}>{ format_optional_bytes(Some(file.bytes)) }</span>
                            </div>
                            <div class={classes!("mt-1", "break-all", "text-[11px]", "text-[var(--muted)]")}>{ file.path.clone() }</div>
                            <div class={classes!("mt-1", "text-[10px]", "text-[var(--muted)]")}>
                                { format!("age {}", format_optional_duration_ms(file.age_ms)) }
                            </div>
                        </div>
                    }) }
                </div>
            }
        </div>
    }
}

fn render_usage_journal_current_file_card(
    title: &str,
    file: Option<&AdminUsageJournalFileView>,
    empty_label: &str,
) -> Html {
    html! {
        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--bg)]", "p-3")}>
            <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ title }</div>
            if let Some(file) = file {
                <div class={classes!("mt-1", "font-mono", "text-lg", "font-bold")}>
                    { file.sequence.map(|seq| format!("#{seq}")).unwrap_or_else(|| file.file_name.clone()) }
                </div>
                <div class={classes!("text-xs", "text-[var(--muted)]")}>
                    { format_optional_bytes(Some(file.bytes)) }
                </div>
                <div class={classes!("mt-1", "break-all", "text-[10px]", "text-[var(--muted)]")}>
                    { file.path.clone() }
                </div>
            } else {
                <div class={classes!("mt-2", "text-xs", "text-[var(--muted)]")}>{ empty_label }</div>
            }
        </div>
    }
}

fn usage_worker_state_tone(state: &str) -> &'static str {
    match state {
        "idle" => "bg-emerald-500/12 text-emerald-700 dark:text-emerald-200",
        "importing" => "bg-sky-500/12 text-sky-700 dark:text-sky-200",
        "unreachable" => "bg-red-500/12 text-red-700 dark:text-red-200",
        _ => "bg-slate-500/12 text-slate-700 dark:text-slate-200",
    }
}

fn usage_stream_state_label(
    stream_completed_cleanly: Option<bool>,
    downstream_disconnect: Option<bool>,
) -> &'static str {
    if downstream_disconnect == Some(true) {
        "disconnect"
    } else if stream_completed_cleanly == Some(true) {
        "clean"
    } else if stream_completed_cleanly == Some(false) {
        "incomplete"
    } else {
        "n/a"
    }
}

fn usage_stream_state_badge_classes(
    stream_completed_cleanly: Option<bool>,
    downstream_disconnect: Option<bool>,
) -> Classes {
    let mut classes = classes!(
        "inline-flex",
        "rounded-full",
        "border",
        "px-2.5",
        "py-1",
        "text-[11px]",
        "font-semibold",
        "uppercase",
        "tracking-[0.12em]"
    );
    match usage_stream_state_label(stream_completed_cleanly, downstream_disconnect) {
        "clean" => {
            classes.push("border-emerald-500/20");
            classes.push("bg-emerald-500/10");
            classes.push("text-emerald-700");
            classes.push("dark:text-emerald-200");
        },
        "disconnect" => {
            classes.push("border-red-500/20");
            classes.push("bg-red-500/10");
            classes.push("text-red-700");
            classes.push("dark:text-red-200");
        },
        "incomplete" => {
            classes.push("border-amber-500/20");
            classes.push("bg-amber-500/10");
            classes.push("text-amber-700");
            classes.push("dark:text-amber-200");
        },
        _ => {
            classes.push("border-slate-500/20");
            classes.push("bg-slate-500/10");
            classes.push("text-slate-700");
            classes.push("dark:text-slate-200");
        },
    }
    classes
}

fn format_stream_summary(
    stream_completed_cleanly: Option<bool>,
    downstream_disconnect: Option<bool>,
    final_event_type: Option<&str>,
    bytes_streamed: Option<u64>,
) -> String {
    let final_event_type = final_event_type
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("-");
    format!(
        "state {} · final {} · bytes {}",
        usage_stream_state_label(stream_completed_cleanly, downstream_disconnect),
        final_event_type,
        format_optional_bytes(bytes_streamed),
    )
}

fn compute_other_latency_ms(
    latency_ms: i32,
    routing_wait_ms: Option<i32>,
    upstream_headers_ms: Option<i32>,
    post_headers_body_ms: Option<i32>,
) -> Option<i32> {
    if routing_wait_ms.is_none() && upstream_headers_ms.is_none() && post_headers_body_ms.is_none()
    {
        return None;
    }
    let measured_ms: i64 = [routing_wait_ms, upstream_headers_ms, post_headers_body_ms]
        .into_iter()
        .flatten()
        .map(|value| i64::from(value.max(0)))
        .sum();
    Some((i64::from(latency_ms.max(0)) - measured_ms).clamp(0, i64::from(i32::MAX)) as i32)
}

#[derive(Clone, Copy)]
struct LatencyBreakdown {
    latency_ms: i32,
    routing_wait_ms: Option<i32>,
    upstream_headers_ms: Option<i32>,
    post_headers_body_ms: Option<i32>,
    request_body_bytes: Option<u64>,
    request_body_read_ms: Option<i32>,
    request_json_parse_ms: Option<i32>,
    pre_handler_ms: Option<i32>,
    first_sse_write_ms: Option<i32>,
    stream_finish_ms: Option<i32>,
    other_latency_ms: Option<i32>,
    quota_failover_count: u64,
}

fn format_latency_breakdown(parts: LatencyBreakdown) -> String {
    let other_latency_ms = parts.other_latency_ms.or_else(|| {
        compute_other_latency_ms(
            parts.latency_ms,
            parts.routing_wait_ms,
            parts.upstream_headers_ms,
            parts.post_headers_body_ms,
        )
    });
    let sse_applicable = parts.first_sse_write_ms.is_some();
    format!(
        "total {} · ingress {} body {} parse {} pre-handler {} · route {} · upstream headers {} · \
         post-headers body {} · first SSE {} · stream finish {} · other {} · quota failover {}",
        format_latency_ms(parts.latency_ms),
        format_optional_bytes(parts.request_body_bytes),
        format_optional_latency_ms(parts.request_body_read_ms),
        format_optional_latency_ms(parts.request_json_parse_ms),
        format_optional_latency_ms(parts.pre_handler_ms),
        format_optional_latency_ms(parts.routing_wait_ms),
        format_optional_latency_ms(parts.upstream_headers_ms),
        format_optional_latency_ms(parts.post_headers_body_ms),
        format_optional_latency_ms_or_na(parts.first_sse_write_ms, sse_applicable),
        format_optional_latency_ms(parts.stream_finish_ms),
        format_optional_latency_ms(other_latency_ms),
        parts.quota_failover_count
    )
}

fn routing_diagnostics_summary(raw: &str) -> Vec<(String, String)> {
    let Some(value) = serde_json::from_str::<serde_json::Value>(raw).ok() else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    let mut push_ms = |label: &str, key: &str| {
        if let Some(value) = value.get(key).and_then(|value| value.as_u64()) {
            rows.push((label.to_string(), format!("{value} ms")));
        }
    };
    push_ms("Route total", "route_total_ms");
    push_ms("Status load", "status_load_ms");
    push_ms("Selection", "selection_ms");
    push_ms("Local queue", "local_queue_wait_ms");
    push_ms("Cooldown wait", "upstream_cooldown_wait_ms");
    for (label, key) in [
        ("Attempts", "account_attempt_count"),
        ("Skipped", "skipped_account_count"),
        ("Codex failover", "failover_count"),
        ("Quota failover", "quota_failover_count"),
        ("Rate-limit failover", "rate_limit_failover_count"),
        ("Retry next", "retry_next_count"),
    ] {
        if let Some(count) = value.get(key).and_then(|value| value.as_u64()) {
            rows.push((label.to_string(), count.to_string()));
        }
    }
    if let Some(account) = value
        .get("selected_account")
        .and_then(|value| value.as_str())
    {
        rows.push(("Selected".to_string(), account.to_string()));
    }
    rows
}

fn format_credit4(value: f64) -> String {
    format!("{value:.4}")
}

fn key_credit_display(key_item: &AdminLlmGatewayKeyView) -> String {
    if key_item.usage_credit_total > 0.0 || key_item.usage_credit_missing_events > 0 {
        format_credit4(key_item.usage_credit_total)
    } else {
        "-".to_string()
    }
}

fn gateway_key_matches_query(key_item: &AdminLlmGatewayKeyView, query_lower: &str) -> bool {
    [
        key_item.name.to_lowercase(),
        key_item.id.to_lowercase(),
        key_item.provider_type.to_lowercase(),
        key_item.status.to_lowercase(),
    ]
    .iter()
    .any(|value| value.contains(query_lower))
}

fn filter_gateway_keys_for_query(
    keys: &[AdminLlmGatewayKeyView],
    query: &str,
) -> Vec<AdminLlmGatewayKeyView> {
    let query_lower = query.trim().to_lowercase();
    if query_lower.is_empty() {
        return keys.to_vec();
    }
    keys.iter()
        .filter(|key_item| gateway_key_matches_query(key_item, &query_lower))
        .cloned()
        .collect()
}

fn usage_source_label(value: &str) -> &'static str {
    match value {
        USAGE_SOURCE_ARCHIVE => "历史归档",
        USAGE_SOURCE_ALL => "全部",
        _ => "在线",
    }
}

fn usage_status_kind_label(value: &str) -> &'static str {
    match value {
        USAGE_STATUS_KIND_OK => "正常",
        USAGE_STATUS_KIND_NON_OK => "异常",
        _ => "全部状态",
    }
}

fn parse_datetime_local_input_to_ms(value: &str) -> Option<i64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let parsed = Date::new(&JsValue::from_str(trimmed)).get_time();
    (!parsed.is_nan()).then_some(parsed as i64)
}

fn format_datetime_local_input(ms: i64) -> String {
    let date = Date::new(&JsValue::from_f64(ms as f64));
    let year = date.get_full_year();
    let month = date.get_month() + 1;
    let day = date.get_date();
    let hours = date.get_hours();
    let minutes = date.get_minutes();
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}")
}

fn usage_time_description(start_input: &str, end_input: &str) -> String {
    match (start_input.trim(), end_input.trim()) {
        ("", "") => "全部时间".to_string(),
        (start, "") => format!("{start} -> now"),
        ("", end) => format!("start -> {end}"),
        (start, end) => format!("{start} -> {end}"),
    }
}

#[derive(Clone, PartialEq)]
struct UsageReloadArgs {
    page: Option<usize>,
    key_id: Option<String>,
    start_input: Option<String>,
    end_input: Option<String>,
    source: Option<String>,
    model: Option<String>,
    account_name: Option<String>,
    endpoint: Option<String>,
    status_kind: Option<String>,
    refresh_filter_options: bool,
}

impl Default for UsageReloadArgs {
    fn default() -> Self {
        Self {
            page: None,
            key_id: None,
            start_input: None,
            end_input: None,
            source: None,
            model: None,
            account_name: None,
            endpoint: None,
            status_kind: None,
            refresh_filter_options: true,
        }
    }
}

fn normalized_usage_filter_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn normalized_usage_status_kind(value: &str) -> Option<String> {
    match value.trim() {
        USAGE_STATUS_KIND_OK => Some(USAGE_STATUS_KIND_OK.to_string()),
        USAGE_STATUS_KIND_NON_OK => Some(USAGE_STATUS_KIND_NON_OK.to_string()),
        _ => None,
    }
}

fn sanitize_auto_account_names(names: &[String], accounts: &[AccountSummaryView]) -> Vec<String> {
    let valid_names = accounts
        .iter()
        .map(|account| account.name.as_str())
        .collect::<HashSet<_>>();
    let mut sanitized = names
        .iter()
        .filter(|name| valid_names.contains(name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    sanitized.sort();
    sanitized.dedup();
    sanitized
}

fn sanitize_account_group_id(
    value: Option<&str>,
    groups: &[AdminAccountGroupOptionView],
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

fn group_name_for_id(groups: &[AdminAccountGroupOptionView], group_id: &str) -> String {
    groups
        .iter()
        .find(|group| group.id == group_id)
        .map(|group| group.name.clone())
        .unwrap_or_else(|| group_id.to_string())
}

fn format_proxy_check_target_line(target: &AdminUpstreamProxyCheckTargetView) -> String {
    if target.reachable {
        format!(
            "{}: {} in {} ms",
            target.target,
            target
                .status_code
                .map(|status| status.to_string())
                .unwrap_or_else(|| "ok".to_string()),
            target.latency_ms.max(0)
        )
    } else {
        format!(
            "{}: {}",
            target.target,
            target
                .error_message
                .clone()
                .unwrap_or_else(|| "request failed".to_string())
        )
    }
}

fn format_proxy_check_message(result: &AdminUpstreamProxyCheckResponse) -> String {
    let mut lines = vec![if result.ok {
        format!(
            "{} 代理检查成功：{}",
            result.provider_type.to_uppercase(),
            result.proxy_config_name
        )
    } else {
        format!(
            "{} 代理检查失败：{}",
            result.provider_type.to_uppercase(),
            result.proxy_config_name
        )
    }];
    lines.push(format!("使用认证：{}", result.auth_label));
    lines.extend(result.targets.iter().map(format_proxy_check_target_line));
    lines.join("\n")
}

fn format_proxy_endpoint_check_summary(
    provider_label: &str,
    check: Option<&AdminUpstreamProxyEndpointCheckView>,
) -> String {
    let Some(check) = check else {
        return format!("{provider_label}: 未检测");
    };
    let status = check
        .status_code
        .map(|status| format!("HTTP {status}"))
        .unwrap_or_else(|| {
            if check.reachable {
                "reachable".to_string()
            } else {
                "failed".to_string()
            }
        });
    format!(
        "{provider_label}: {} ms · {} · {}",
        check.latency_ms.max(0),
        status,
        format_ms(check.checked_at)
    )
}

fn proxy_endpoint_check_tone(check: Option<&AdminUpstreamProxyEndpointCheckView>) -> &'static str {
    match check {
        Some(check) if !check.reachable => {
            "border-red-500/30 bg-red-500/8 text-red-700 dark:text-red-200"
        },
        Some(_) => "border-emerald-500/30 bg-emerald-500/8 text-emerald-700 dark:text-emerald-200",
        None => "border-[var(--border)] bg-[var(--surface-alt)] text-[var(--muted)]",
    }
}

fn preview_text(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "-".to_string();
    }
    let total_chars = trimmed.chars().count();
    if total_chars <= max_chars {
        trimmed.to_string()
    } else {
        let prefix = trimmed.chars().take(max_chars).collect::<String>();
        format!("{prefix}...")
    }
}

fn is_gpt_pro_account(plan_type: Option<&str>) -> bool {
    plan_type.map(str::trim).is_some_and(|plan| {
        let normalized = plan.to_ascii_lowercase();
        normalized == "pro" || normalized == "gpt pro"
    })
}

// Render a compact status pill that matches the current key state.
fn key_status_badge(status: &str) -> Classes {
    let base = classes!(
        "inline-flex",
        "items-center",
        "rounded-full",
        "px-2.5",
        "py-1",
        "text-xs",
        "font-semibold",
        "uppercase",
        "tracking-[0.16em]"
    );
    match status {
        "active" => {
            classes!(base, "bg-emerald-500/12", "text-emerald-700", "dark:text-emerald-200")
        },
        "disabled" => classes!(base, "bg-slate-500/14", "text-slate-700", "dark:text-slate-200"),
        _ => classes!(base, "bg-[var(--surface-alt)]", "text-[var(--muted)]"),
    }
}

// Keep copy affordances visually small so dense diagnostics tables stay
// readable.
fn copy_icon_button(text: &str, on_copy: &Callback<(String, String)>) -> Html {
    let value = text.to_string();
    let on_copy = on_copy.clone();
    html! {
        <button
            type="button"
            class={classes!(
                "inline-flex",
                "h-8",
                "w-8",
                "items-center",
                "justify-center",
                "rounded-full",
                "border",
                "border-[var(--border)]",
                "bg-[var(--surface)]",
                "text-[var(--muted)]",
                "transition-colors",
                "hover:text-[var(--primary)]",
                "hover:bg-[var(--surface-alt)]"
            )}
            title="复制"
            aria-label="复制"
            onclick={Callback::from(move |_| on_copy.emit(("".to_string(), value.clone())))}
        >
            <i class={classes!("fas", "fa-copy", "text-xs")} />
        </button>
    }
}

fn copyable_token_preview(label: &str, value: &str, on_copy: &Callback<(String, String)>) -> Html {
    html! {
        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2")}>
            <div class={classes!("flex", "items-center", "justify-between", "gap-3")}>
                <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>
                    { label }
                </div>
                { copy_icon_button(value, on_copy) }
            </div>
            <code class={classes!("mt-2", "block", "break-all", "text-xs", "text-[var(--text)]")}>
                { preview_text(value, 96) }
            </code>
        </div>
    }
}

// Reformat stored header JSON before showing it in the modal dialog.
fn pretty_headers_json(raw: &str) -> String {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| raw.to_string())
}

#[cfg(test)]
fn usage_last_message_preview(event: &AdminLlmGatewayUsageEventView) -> String {
    event
        .last_message_content
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "-".to_string())
}

#[cfg(test)]
fn usage_last_message_table_preview(event: &AdminLlmGatewayUsageEventView) -> String {
    let preview = usage_last_message_preview(event);
    if preview == "-" {
        return preview;
    }
    let single_line = preview.split_whitespace().collect::<Vec<_>>().join(" ");
    preview_text(&single_line, 120)
}

fn usage_journal_preview_message(
    preview: &crate::api::AdminUsageJournalPreviewEventView,
) -> String {
    preview
        .last_message_content
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "-".to_string())
}

fn usage_journal_preview_has_full_message(
    preview: &crate::api::AdminUsageJournalPreviewEventView,
) -> bool {
    let message = usage_journal_preview_message(preview);
    message != "-"
}

async fn tokio_like_join_usage_journal(
    preview_offset: usize,
) -> Result<(AdminUsageJournalStatusView, AdminUsageJournalPreviewResponse), String> {
    let status_fut = fetch_admin_usage_journal_status();
    let preview_fut =
        fetch_admin_usage_journal_preview(Some(JOURNAL_PREVIEW_PAGE_SIZE), Some(preview_offset));
    let (status, preview) = futures::future::join(status_fut, preview_fut).await;
    Ok((status?, preview?))
}

fn pretty_json_text(raw: &str) -> String {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| raw.to_string())
}

#[derive(Properties, PartialEq)]
struct KeyEditorCardProps {
    key_item: AdminLlmGatewayKeyView,
    on_changed: Callback<()>,
    on_refresh: Callback<(String, String)>,
    on_copy: Callback<(String, String)>,
    on_flash: Callback<(String, bool)>,
    refreshing: bool,
    account_groups: Vec<AdminAccountGroupOptionView>,
}

#[function_component(KeyEditorCard)]
fn key_editor_card(props: &KeyEditorCardProps) -> Html {
    let key_item = props.key_item.clone();
    let key_name_for_actions = key_item.name.clone();
    let name = use_state(|| key_item.name.clone());
    let quota = use_state(|| key_item.quota_billable_limit.to_string());
    let public_visible = use_state(|| key_item.public_visible);
    let status = use_state(|| key_item.status.clone());
    let route_strategy = use_state(|| {
        key_item
            .route_strategy
            .clone()
            .unwrap_or_else(|| "auto".to_string())
    });
    let account_group_id = use_state(|| {
        sanitize_account_group_id(key_item.account_group_id.as_deref(), &props.account_groups, true)
    });
    let request_max_concurrency = use_state(|| {
        key_item
            .request_max_concurrency
            .map(|value| value.to_string())
            .unwrap_or_default()
    });
    let request_min_start_interval_ms = use_state(|| {
        key_item
            .request_min_start_interval_ms
            .map(|value| value.to_string())
            .unwrap_or_default()
    });
    let codex_fast_enabled = use_state(|| key_item.codex_fast_enabled);
    let saving = use_state(|| false);
    let feedback = use_state(|| None::<String>);

    {
        // Reset editor controls whenever the parent list refreshes this card.
        let key_item = props.key_item.clone();
        let account_groups = props.account_groups.clone();
        let name = name.clone();
        let quota = quota.clone();
        let public_visible = public_visible.clone();
        let status = status.clone();
        let route_strategy = route_strategy.clone();
        let account_group_id = account_group_id.clone();
        let request_max_concurrency = request_max_concurrency.clone();
        let request_min_start_interval_ms = request_min_start_interval_ms.clone();
        let codex_fast_enabled = codex_fast_enabled.clone();
        use_effect_with((props.key_item.clone(), props.account_groups.clone()), move |_| {
            name.set(key_item.name.clone());
            quota.set(key_item.quota_billable_limit.to_string());
            public_visible.set(key_item.public_visible);
            status.set(key_item.status.clone());
            route_strategy.set(
                key_item
                    .route_strategy
                    .clone()
                    .unwrap_or_else(|| "auto".to_string()),
            );
            account_group_id.set(sanitize_account_group_id(
                key_item.account_group_id.as_deref(),
                &account_groups,
                true,
            ));
            request_max_concurrency.set(
                key_item
                    .request_max_concurrency
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
            );
            request_min_start_interval_ms.set(
                key_item
                    .request_min_start_interval_ms
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
            );
            codex_fast_enabled.set(key_item.codex_fast_enabled);
            || ()
        });
    }

    if key_item.provider_type == "kiro" {
        return html! {
            <article class={classes!(
                "rounded-xl",
                "border",
                "border-[var(--border)]",
                "bg-[var(--surface)]",
                "p-5",
                "transition-all",
                "duration-200",
                "hover:shadow-lg",
                "hover:shadow-black/5"
            )}>
                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                    <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                        <span class={classes!("inline-flex", "items-center", "rounded-full", "bg-slate-900", "px-2.5", "py-1", "font-mono", "text-[11px]", "font-semibold", "uppercase", "tracking-[0.16em]", "text-emerald-300")}>
                            { "Kiro Key" }
                        </span>
                        <h3 class={classes!("m-0", "text-base", "font-bold")}>{ key_item.name.clone() }</h3>
                    </div>
                    <Link<Route> to={Route::AdminKiroGateway} classes={classes!("btn-terminal")}>
                        { "前往 /admin/kiro-gateway" }
                    </Link<Route>>
                </div>

                <div class={classes!("mt-3", "rounded-lg", "bg-slate-950", "px-3", "py-2", "text-xs", "text-emerald-200")}>
                    <MaskedSecretCode
                        value={key_item.secret.clone()}
                        copy_label={"Kiro Key"}
                        on_copy={props.on_copy.clone()}
                        code_class={classes!("text-emerald-200")}
                    />
                </div>

                <div class={classes!("mt-3", "flex", "items-center", "gap-3", "flex-wrap", "text-xs", "text-[var(--muted)]")}>
                    <span>{ format!("status {}", key_item.status) }</span>
                    <span>{ format!("created {}", format_ms(key_item.created_at)) }</span>
                    <button
                        class={classes!("btn-terminal", "ml-auto")}
                        onclick={{
                            let on_copy = props.on_copy.clone();
                            let secret = key_item.secret.clone();
                            Callback::from(move |_| on_copy.emit(("Kiro Key".to_string(), secret.clone())))
                        }}
                    >
                        { "复制" }
                    </button>
                </div>
            </article>
        };
    }

    let on_save = {
        let key_id = key_item.id.clone();
        let name = name.clone();
        let quota = quota.clone();
        let public_visible = public_visible.clone();
        let status = status.clone();
        let route_strategy = route_strategy.clone();
        let account_group_id = account_group_id.clone();
        let request_max_concurrency = request_max_concurrency.clone();
        let request_min_start_interval_ms = request_min_start_interval_ms.clone();
        let codex_fast_enabled = codex_fast_enabled.clone();
        let saving = saving.clone();
        let feedback = feedback.clone();
        let on_flash = props.on_flash.clone();
        let on_changed = props.on_changed.clone();
        let key_name_for_actions = key_name_for_actions.clone();
        Callback::from(move |_| {
            let key_id = key_id.clone();
            let key_name = key_name_for_actions.clone();
            let name_value = (*name).trim().to_string();
            let quota_value = (*quota).trim().parse::<u64>();
            let public_visible_value = *public_visible;
            let status_value = (*status).clone();
            let route_strategy_value = (*route_strategy).clone();
            let account_group_id_value = (*account_group_id).clone();
            let request_max_concurrency_value = (*request_max_concurrency).trim().to_string();
            let request_min_start_interval_ms_value =
                (*request_min_start_interval_ms).trim().to_string();
            let codex_fast_enabled_value = *codex_fast_enabled;
            let saving = saving.clone();
            let feedback = feedback.clone();
            let on_flash = on_flash.clone();
            let on_changed = on_changed.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if *saving {
                    return;
                }
                let Ok(quota_value) = quota_value else {
                    let message = "额度必须是正整数".to_string();
                    feedback.set(Some(message.clone()));
                    on_flash.emit((message, true));
                    return;
                };
                let request_max_concurrency_value = if request_max_concurrency_value.is_empty() {
                    None
                } else {
                    match request_max_concurrency_value.parse::<u64>() {
                        Ok(value) => Some(value),
                        Err(_) => {
                            let message = "并发上限必须是整数，留空表示不限制".to_string();
                            feedback.set(Some(message.clone()));
                            on_flash.emit((message, true));
                            return;
                        },
                    }
                };
                let request_min_start_interval_ms_value =
                    if request_min_start_interval_ms_value.is_empty() {
                        None
                    } else {
                        match request_min_start_interval_ms_value.parse::<u64>() {
                            Ok(value) => Some(value),
                            Err(_) => {
                                let message = "请求间隔必须是整数毫秒，留空表示不限制".to_string();
                                feedback.set(Some(message.clone()));
                                on_flash.emit((message, true));
                                return;
                            },
                        }
                    };
                saving.set(true);
                match patch_admin_llm_gateway_key(&key_id, PatchAdminLlmGatewayKeyRequest {
                    name: Some(&name_value),
                    status: Some(&status_value),
                    public_visible: Some(public_visible_value),
                    quota_billable_limit: Some(quota_value),
                    route_strategy: Some(&route_strategy_value),
                    account_group_id: Some(&account_group_id_value),
                    fixed_account_name: None,
                    auto_account_names: None,
                    model_name_map: None,
                    request_max_concurrency: request_max_concurrency_value,
                    request_min_start_interval_ms: request_min_start_interval_ms_value,
                    codex_fast_enabled: Some(codex_fast_enabled_value),
                    kiro_request_validation_enabled: None,
                    kiro_cache_estimation_enabled: None,
                    kiro_zero_cache_debug_enabled: None,
                    kiro_full_request_logging_enabled: None,
                    kiro_remote_media_resolution_enabled: None,
                    kiro_latency_routing_enabled: None,
                    kiro_cache_policy_override_json: None,
                    kiro_billable_model_multipliers_override_json: None,
                    request_max_concurrency_unlimited: request_max_concurrency_value.is_none(),
                    request_min_start_interval_ms_unlimited: request_min_start_interval_ms_value
                        .is_none(),
                })
                .await
                {
                    Ok(_) => {
                        feedback.set(Some("已保存".to_string()));
                        on_flash.emit((format!("已保存 key `{}`", key_name), false));
                        on_changed.emit(());
                    },
                    Err(err) => {
                        feedback.set(Some(err.clone()));
                        on_flash.emit((format!("保存 key `{}` 失败\n{err}", key_name), true));
                    },
                }
                saving.set(false);
            });
        })
    };

    let on_delete = {
        let key_id = key_item.id.clone();
        let on_changed = props.on_changed.clone();
        let feedback = feedback.clone();
        let saving = saving.clone();
        let on_flash = props.on_flash.clone();
        let key_name_for_actions = key_name_for_actions.clone();
        Callback::from(move |_| {
            if !confirm_destructive("确认删除这个 API key？") {
                return;
            }
            let key_id = key_id.clone();
            let key_name = key_name_for_actions.clone();
            let feedback = feedback.clone();
            let saving = saving.clone();
            let on_flash = on_flash.clone();
            let on_changed = on_changed.clone();
            wasm_bindgen_futures::spawn_local(async move {
                saving.set(true);
                match delete_admin_llm_gateway_key(&key_id).await {
                    Ok(_) => {
                        feedback.set(Some("已删除".to_string()));
                        on_flash.emit((format!("已删除 key `{}`", key_name), false));
                        on_changed.emit(());
                    },
                    Err(err) => {
                        feedback.set(Some(err.clone()));
                        on_flash.emit((format!("删除 key `{}` 失败\n{err}", key_name), true));
                    },
                }
                saving.set(false);
            });
        })
    };

    let fixed_route_groups = props
        .account_groups
        .iter()
        .filter(|group| group.account_count == 1)
        .cloned()
        .collect::<Vec<_>>();
    let current_route_summary = if *route_strategy == "fixed" {
        if (*account_group_id).is_empty() {
            "固定组：未选择".to_string()
        } else {
            format!(
                "固定组：{}",
                group_name_for_id(&props.account_groups, (*account_group_id).as_str())
            )
        }
    } else if (*account_group_id).is_empty() {
        "自动：全账号池".to_string()
    } else {
        format!("自动：{}", group_name_for_id(&props.account_groups, (*account_group_id).as_str()))
    };

    html! {
        <article class={classes!(
            "rounded-xl",
            "border",
            "border-[var(--border)]",
            "bg-[var(--surface)]",
            "p-5",
            "transition-all",
            "duration-200",
            "hover:shadow-lg",
            "hover:shadow-black/5"
        )}>
            <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                <div class={classes!("flex", "items-center", "gap-2")}>
                    <div class={key_status_badge(&key_item.status)}>{ key_item.status.clone() }</div>
                    <h3 class={classes!("m-0", "text-base", "font-bold")}>{ key_item.name.clone() }</h3>
                    <span class={classes!("text-xs", "text-[var(--muted)]")}>{ format_ms(key_item.created_at) }</span>
                </div>
                <div class={classes!("flex", "gap-2")}>
                    <button
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
                    <button
                        class={classes!("btn-terminal")}
                        onclick={{
                            let on_copy = props.on_copy.clone();
                            let secret = key_item.secret.clone();
                            Callback::from(move |_| on_copy.emit(("Key".to_string(), secret.clone())))
                        }}
                    >
                        { "复制" }
                    </button>
                    <button class={classes!("btn-terminal", "!text-red-600", "dark:!text-red-300")} onclick={on_delete} disabled={*saving}>
                        { "删除" }
                    </button>
                </div>
            </div>

            <div class={classes!("mt-3", "rounded-lg", "bg-slate-950", "px-3", "py-2", "text-xs", "text-emerald-200")}>
                <MaskedSecretCode
                    value={key_item.secret.clone()}
                    copy_label={"Key"}
                    on_copy={props.on_copy.clone()}
                    code_class={classes!("text-emerald-200")}
                />
            </div>

            <div class={classes!("mt-3", "grid", "gap-3", "xl:grid-cols-2")}>
                <label class={classes!("text-sm")}>
                    <span class={classes!("text-[var(--muted)]")}>{ "名称" }</span>
                    <input
                        type="text"
                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
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
                <label class={classes!("text-sm")}>
                    <span class={classes!("text-[var(--muted)]")}>{ "额度上限" }</span>
                    <input
                        type="number"
                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                        value={(*quota).clone()}
                        oninput={{
                            let quota = quota.clone();
                            Callback::from(move |event: InputEvent| {
                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                    quota.set(target.value());
                                }
                            })
                        }}
                    />
                </label>
            </div>

            <div class={classes!("mt-3", "grid", "gap-3", "xl:grid-cols-2")}>
                <label class={classes!("text-sm")}>
                    <span class={classes!("text-[var(--muted)]")}>{ "并发上限" }</span>
                    <input
                        type="number"
                        placeholder="留空表示不限制"
                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                        value={(*request_max_concurrency).clone()}
                        oninput={{
                            let request_max_concurrency = request_max_concurrency.clone();
                            Callback::from(move |event: InputEvent| {
                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                    request_max_concurrency.set(target.value());
                                }
                            })
                        }}
                    />
                </label>
                <label class={classes!("text-sm")}>
                    <span class={classes!("text-[var(--muted)]")}>{ "请求起始间隔 ms" }</span>
                    <input
                        type="number"
                        placeholder="留空表示不限制"
                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                        value={(*request_min_start_interval_ms).clone()}
                        oninput={{
                            let request_min_start_interval_ms = request_min_start_interval_ms.clone();
                            Callback::from(move |event: InputEvent| {
                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                    request_min_start_interval_ms.set(target.value());
                                }
                            })
                        }}
                    />
                </label>
            </div>

            <div class={classes!("mt-3", "flex", "items-center", "gap-3", "flex-wrap")}>
                <label class={classes!("flex", "items-center", "gap-2", "text-sm")}>
                    <input
                        type="checkbox"
                        checked={*public_visible}
                        onchange={{
                            let public_visible = public_visible.clone();
                            Callback::from(move |event: Event| {
                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                    public_visible.set(target.checked());
                                }
                            })
                        }}
                    />
                    <span>{ "公开" }</span>
                </label>
                <label class={classes!("flex", "items-center", "gap-2", "text-sm")}>
                    <input
                        type="checkbox"
                        checked={*codex_fast_enabled}
                        onchange={{
                            let codex_fast_enabled = codex_fast_enabled.clone();
                            Callback::from(move |event: Event| {
                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                    codex_fast_enabled.set(target.checked());
                                }
                            })
                        }}
                    />
                    <span>{ "允许 Fast（service_tier，计费 x2）" }</span>
                </label>
                <select
                    key={format!("{}-status-{}", key_item.id, (*status).clone())}
                    class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-1.5", "text-sm")}
                    onchange={{
                        let status = status.clone();
                        Callback::from(move |event: Event| {
                            if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                                status.set(target.value());
                            }
                        })
                    }}
                >
                    <option value="active" selected={*status == "active"}>{ "active" }</option>
                    <option value="disabled" selected={*status == "disabled"}>{ "disabled" }</option>
                </select>
                <button class={classes!("btn-terminal", "btn-terminal-primary", "ml-auto")} onclick={on_save} disabled={*saving}>
                    { if *saving { "保存中..." } else { "保存" } }
                </button>
            </div>

            <div class={classes!("mt-3", "flex", "items-center", "gap-3", "flex-wrap", "overflow-hidden")}>
                <label class={classes!("flex", "items-center", "gap-2", "text-sm", "min-w-0")}>
                    <span class={classes!("text-[var(--muted)]", "shrink-0")}>{ "路由" }</span>
                    <select
                        key={format!("{}-route-{}", key_item.id, (*route_strategy).clone())}
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
                    <label class={classes!("flex", "items-center", "gap-2", "text-sm", "min-w-0")}>
                        <span class={classes!("text-[var(--muted)]", "shrink-0")}>{ "单账号组" }</span>
                        <select
                            key={format!("{}-group-fixed-{}", key_item.id, (*account_group_id).clone())}
                            class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-1.5", "text-sm", "max-w-[220px]", "truncate")}
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
                                <option value={group.id.clone()} selected={*account_group_id == group.id}>
                                    { format!(
                                        "{} ({})",
                                        group.name,
                                        group
                                            .single_account_name
                                            .clone()
                                            .unwrap_or_else(|| format!("{} 个账号", group.account_count))
                                    ) }
                                </option>
                            }) }
                        </select>
                    </label>
                } else {
                    <label class={classes!("flex", "items-center", "gap-2", "text-sm", "min-w-0")}>
                        <span class={classes!("text-[var(--muted)]", "shrink-0")}>{ "账号组" }</span>
                        <select
                            key={format!("{}-group-auto-{}", key_item.id, (*account_group_id).clone())}
                            class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-1.5", "text-sm", "max-w-[220px]", "truncate")}
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
                                <option value={group.id.clone()} selected={*account_group_id == group.id}>{ format!("{} ({} 个账号)", group.name, group.account_count) }</option>
                            }) }
                        </select>
                    </label>
                }
                <span class={classes!("text-xs", "text-[var(--muted)]", "min-w-0", "break-all")}>
                    { current_route_summary }
                </span>
            </div>

            <div class={classes!("mt-3", "flex", "items-center", "gap-4", "text-xs", "text-[var(--muted)]")}>
                <span>{ format!("剩余 {}", format_number_i64(key_item.remaining_billable)) }</span>
                <span>{ format!("输入 {}", format_number_u64(key_item.usage_input_uncached_tokens)) }</span>
                <span>{ format!("缓存 {}", format_number_u64(key_item.usage_input_cached_tokens)) }</span>
                <span>{ format!("输出 {}", format_number_u64(key_item.usage_output_tokens)) }</span>
                <span>{ format!(
                    "并发 {}",
                    key_item.request_max_concurrency.map(|value| value.to_string()).unwrap_or_else(|| "∞".to_string())
                ) }</span>
                <span>{ format!(
                    "间隔 {}ms",
                    key_item.request_min_start_interval_ms.map(|value| value.to_string()).unwrap_or_else(|| "∞".to_string())
                ) }</span>
                <span>{ format!("Credit {}", key_credit_display(&key_item)) }</span>
                if key_item.usage_credit_missing_events > 0 {
                    <span>{ format!("partial {}", key_item.usage_credit_missing_events) }</span>
                }
            </div>

            if let Some(feedback) = (*feedback).clone() {
                <p class={classes!("mt-2", "m-0", "text-xs", "text-[var(--muted)]")}>{ feedback }</p>
            }
        </article>
    }
}

#[derive(Properties, PartialEq)]
struct AccountGroupEditorCardProps {
    group_item: AdminAccountGroupView,
    accounts: Vec<AccountSummaryView>,
    on_changed: Callback<()>,
    on_flash: Callback<(String, bool)>,
}

#[function_component(AccountGroupEditorCard)]
fn account_group_editor_card(props: &AccountGroupEditorCardProps) -> Html {
    let name = use_state(|| props.group_item.name.clone());
    let account_names =
        use_state(|| sanitize_auto_account_names(&props.group_item.account_names, &props.accounts));
    let expanded = use_state(|| false);
    let saving = use_state(|| false);
    let feedback = use_state(|| None::<String>);

    {
        let group_item = props.group_item.clone();
        let accounts = props.accounts.clone();
        let name = name.clone();
        let account_names = account_names.clone();
        use_effect_with((props.group_item.clone(), props.accounts.clone()), move |_| {
            name.set(group_item.name.clone());
            account_names.set(sanitize_auto_account_names(&group_item.account_names, &accounts));
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
        let on_changed = props.on_changed.clone();
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
            let on_changed = on_changed.clone();
            wasm_bindgen_futures::spawn_local(async move {
                saving.set(true);
                match patch_admin_llm_gateway_account_group(
                    &group_id,
                    PatchAdminAccountGroupInput {
                        name: Some(&name_value),
                        account_names: Some(account_names_value.as_slice()),
                    },
                )
                .await
                {
                    Ok(_) => {
                        feedback.set(Some("已保存".to_string()));
                        on_flash.emit((format!("已保存账号组 `{}`", name_value), false));
                        on_changed.emit(());
                    },
                    Err(err) => {
                        feedback.set(Some(err.clone()));
                        on_flash.emit((format!("保存账号组失败\n{err}"), true));
                    },
                }
                saving.set(false);
            });
        })
    };

    let on_delete = {
        let group_id = props.group_item.id.clone();
        let group_name = props.group_item.name.clone();
        let on_changed = props.on_changed.clone();
        let on_flash = props.on_flash.clone();
        let saving = saving.clone();
        Callback::from(move |_| {
            if !confirm_destructive("确认删除这个账号组？") {
                return;
            }
            let group_id = group_id.clone();
            let group_name = group_name.clone();
            let on_changed = on_changed.clone();
            let on_flash = on_flash.clone();
            let saving = saving.clone();
            wasm_bindgen_futures::spawn_local(async move {
                saving.set(true);
                match delete_admin_llm_gateway_account_group(&group_id).await {
                    Ok(_) => {
                        on_flash.emit((format!("已删除账号组 `{}`", group_name), false));
                        on_changed.emit(());
                    },
                    Err(err) => {
                        on_flash.emit((format!("删除账号组失败\n{err}"), true));
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
                    <h3 class={classes!("m-0", "text-base", "font-bold")}>{ props.group_item.name.clone() }</h3>
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
                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
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
                                        if account.status != "disabled" {
                                            <div class={classes!("mt-1", "font-mono", "text-[11px]", "text-[var(--muted)]")}>
                                                { format!(
                                                    "5h {} / wk {}",
                                                    account.primary_remaining_percent.map(|value| format!("{value:.0}%")).unwrap_or_else(|| "-".to_string()),
                                                    account.secondary_remaining_percent.map(|value| format!("{value:.0}%")).unwrap_or_else(|| "-".to_string())
                                                ) }
                                            </div>
                                        }
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
                    <p class={classes!("mt-2", "m-0", "text-xs", "text-[var(--muted)]")}>{ feedback }</p>
                }
            }
        </article>
    }
}

#[derive(Properties, PartialEq)]
struct ProxyConfigEditorCardProps {
    proxy_config: AdminUpstreamProxyConfigView,
    on_changed: Callback<()>,
    on_copy: Callback<(String, String)>,
    on_flash: Callback<(String, bool)>,
}

/// Editable fields for the proxy-config form. Grouped into a single struct so
/// every field update is one `form.set(next)` instead of juggling five
/// independent `UseStateHandle`s across effect + save callback.
#[derive(Clone, PartialEq)]
struct ProxyForm {
    name: String,
    proxy_url: String,
    proxy_username: String,
    proxy_password: String,
    status: String,
}

impl ProxyForm {
    fn from_config(cfg: &AdminUpstreamProxyConfigView) -> Self {
        Self {
            name: cfg.name.clone(),
            proxy_url: cfg.proxy_url.clone(),
            proxy_username: cfg.proxy_username.clone().unwrap_or_default(),
            proxy_password: cfg.proxy_password.clone().unwrap_or_default(),
            status: cfg.status.clone(),
        }
    }
}

/// Inputs for the "create new API key" panel at the top of the Keys tab.
/// Bundled so the submit callback and `.set(next)` paths read a single clone
/// of the struct.
#[derive(Clone, PartialEq)]
struct CreateKeyForm {
    name: String,
    quota: String,
    public: bool,
    request_max_concurrency: String,
    request_min_start_interval_ms: String,
}

impl Default for CreateKeyForm {
    fn default() -> Self {
        Self {
            name: String::new(),
            quota: "100000".to_string(),
            public: true,
            request_max_concurrency: String::new(),
            request_min_start_interval_ms: String::new(),
        }
    }
}

#[function_component(ProxyConfigEditorCard)]
fn proxy_config_editor_card(props: &ProxyConfigEditorCardProps) -> Html {
    let proxy_config = props.proxy_config.clone();
    let can_edit_slot_metadata = proxy_config.can_edit_slot_metadata;
    let scope_node_label = proxy_config
        .scope_node_id
        .clone()
        .unwrap_or_else(|| "core".to_string());
    let effective_source_label = match proxy_config.effective_source.as_str() {
        "node_override" => "本机覆盖",
        "core" => "继承 core",
        other => other,
    };
    let form = use_state(|| ProxyForm::from_config(&proxy_config));
    let saving = use_state(|| false);
    let checking = use_state(|| None::<String>);
    let feedback = use_state(|| None::<String>);

    {
        let form = form.clone();
        use_effect_with(props.proxy_config.clone(), move |cfg| {
            form.set(ProxyForm::from_config(cfg));
            || ()
        });
    }

    let on_save = {
        let proxy_id = proxy_config.id.clone();
        let form = form.clone();
        let saving = saving.clone();
        let feedback = feedback.clone();
        let on_changed = props.on_changed.clone();
        let on_flash = props.on_flash.clone();
        Callback::from(move |_| {
            let proxy_id = proxy_id.clone();
            let current = (*form).clone();
            let input = PatchAdminUpstreamProxyConfigInput {
                name: if can_edit_slot_metadata {
                    Some(current.name.trim().to_string())
                } else {
                    None
                },
                proxy_url: Some(current.proxy_url.trim().to_string()),
                proxy_username: {
                    let value = current.proxy_username.trim().to_string();
                    if value.is_empty() {
                        None
                    } else {
                        Some(value)
                    }
                },
                proxy_password: {
                    let value = current.proxy_password.trim().to_string();
                    if value.is_empty() {
                        None
                    } else {
                        Some(value)
                    }
                },
                status: Some(current.status.trim().to_string()),
            };
            let saving = saving.clone();
            let feedback = feedback.clone();
            let on_changed = on_changed.clone();
            let on_flash = on_flash.clone();
            wasm_bindgen_futures::spawn_local(async move {
                saving.set(true);
                match patch_admin_llm_gateway_proxy_config(&proxy_id, &input).await {
                    Ok(_) => {
                        feedback.set(Some("Saved.".to_string()));
                        on_flash.emit(("已保存代理配置".to_string(), false));
                        on_changed.emit(());
                    },
                    Err(err) => {
                        feedback.set(Some(err.clone()));
                        on_flash.emit((format!("保存代理配置失败\n{err}"), true));
                    },
                }
                saving.set(false);
            });
        })
    };

    let on_delete = {
        let proxy_id = proxy_config.id.clone();
        let saving = saving.clone();
        let feedback = feedback.clone();
        let on_changed = props.on_changed.clone();
        let on_flash = props.on_flash.clone();
        Callback::from(move |_| {
            if !confirm_destructive("确认删除这个代理配置？绑定该配置的账号会回退到默认行为。")
            {
                return;
            }
            let proxy_id = proxy_id.clone();
            let saving = saving.clone();
            let feedback = feedback.clone();
            let on_changed = on_changed.clone();
            let on_flash = on_flash.clone();
            wasm_bindgen_futures::spawn_local(async move {
                saving.set(true);
                match delete_admin_llm_gateway_proxy_config(&proxy_id).await {
                    Ok(_) => {
                        on_flash.emit(("已删除代理配置".to_string(), false));
                        on_changed.emit(());
                    },
                    Err(err) => {
                        feedback.set(Some(err.clone()));
                        on_flash.emit((format!("删除代理配置失败\n{err}"), true));
                    },
                }
                saving.set(false);
            });
        })
    };

    let on_reset_override = {
        let proxy_id = proxy_config.id.clone();
        let saving = saving.clone();
        let feedback = feedback.clone();
        let on_changed = props.on_changed.clone();
        let on_flash = props.on_flash.clone();
        Callback::from(move |_| {
            if !confirm_destructive("确认移除这个节点上的代理覆盖？移除后会继承 core 配置。")
            {
                return;
            }
            let proxy_id = proxy_id.clone();
            let saving = saving.clone();
            let feedback = feedback.clone();
            let on_changed = on_changed.clone();
            let on_flash = on_flash.clone();
            wasm_bindgen_futures::spawn_local(async move {
                saving.set(true);
                match reset_admin_llm_gateway_proxy_config_override(&proxy_id).await {
                    Ok(_) => {
                        feedback.set(Some("Override reset.".to_string()));
                        on_flash.emit(("已移除本机代理覆盖".to_string(), false));
                        on_changed.emit(());
                    },
                    Err(err) => {
                        feedback.set(Some(err.clone()));
                        on_flash.emit((format!("移除本机代理覆盖失败\n{err}"), true));
                    },
                }
                saving.set(false);
            });
        })
    };

    let on_check_provider = {
        let proxy_id = proxy_config.id.clone();
        let checking = checking.clone();
        let feedback = feedback.clone();
        let on_changed = props.on_changed.clone();
        let on_flash = props.on_flash.clone();
        Callback::from(move |(provider_type, full_chain): (String, bool)| {
            let proxy_id = proxy_id.clone();
            let checking = checking.clone();
            let feedback = feedback.clone();
            let on_changed = on_changed.clone();
            let on_flash = on_flash.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if (*checking).is_some() {
                    return;
                }
                let action_key = format!(
                    "{}-{}",
                    provider_type,
                    if full_chain { "full-chain" } else { "connectivity" }
                );
                let action_label = if full_chain {
                    format!("{} 全链路", provider_type.to_uppercase())
                } else {
                    provider_type.to_uppercase()
                };
                checking.set(Some(action_key));
                let result = if full_chain {
                    check_admin_llm_gateway_proxy_config_full_chain(&proxy_id, &provider_type).await
                } else {
                    check_admin_llm_gateway_proxy_config(&proxy_id, &provider_type).await
                };
                match result {
                    Ok(result) => {
                        let message = format_proxy_check_message(&result);
                        feedback.set(Some(if result.ok {
                            format!("{action_label} 检查完成")
                        } else {
                            format!("{action_label} 检查失败")
                        }));
                        on_flash.emit((message, !result.ok));
                        on_changed.emit(());
                    },
                    Err(err) => {
                        feedback.set(Some(err.clone()));
                        on_flash.emit((format!("{action_label} 代理检查失败\n{err}"), true));
                    },
                }
                checking.set(None);
            });
        })
    };

    html! {
        <article class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-4")}>
            <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                <div>
                    <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                        <h3 class={classes!("m-0", "text-base", "font-semibold")}>{ props.proxy_config.name.clone() }</h3>
                        <span class={classes!("inline-flex", "items-center", "rounded-full", "px-2.5", "py-1", "text-[11px]", "font-semibold", "uppercase", "tracking-[0.16em]",
                            if props.proxy_config.status == "active" { "bg-emerald-500/12 text-emerald-700 dark:text-emerald-200" } else { "bg-slate-500/12 text-slate-700 dark:text-slate-200" })}>
                            { props.proxy_config.status.clone() }
                        </span>
                        <span class={classes!("inline-flex", "items-center", "rounded-full", "bg-cyan-500/12", "px-2.5", "py-1", "text-[11px]", "font-semibold", "text-cyan-700", "dark:text-cyan-200")}>
                            { effective_source_label }
                        </span>
                        <span class={classes!("inline-flex", "items-center", "rounded-full", "bg-[var(--surface-alt)]", "px-2.5", "py-1", "text-[11px]", "font-semibold", "text-[var(--muted)]")}>
                            { format!("scope: {}", scope_node_label) }
                        </span>
                    </div>
                    <p class={classes!("mt-2", "mb-0", "text-xs", "font-mono", "text-[var(--muted)]")}>
                        { format!("created {} · updated {}", format_ms(props.proxy_config.created_at), format_ms(props.proxy_config.updated_at)) }
                    </p>
                    <div class={classes!("mt-3", "grid", "gap-2", "sm:grid-cols-2")}>
                        <div class={classes!("rounded-lg", "border", "px-3", "py-2", "text-xs", proxy_endpoint_check_tone(props.proxy_config.latest_codex_check.as_ref()))}>
                            { format_proxy_endpoint_check_summary("Codex", props.proxy_config.latest_codex_check.as_ref()) }
                        </div>
                        <div class={classes!("rounded-lg", "border", "px-3", "py-2", "text-xs", proxy_endpoint_check_tone(props.proxy_config.latest_kiro_check.as_ref()))}>
                            { format_proxy_endpoint_check_summary("Kiro", props.proxy_config.latest_kiro_check.as_ref()) }
                        </div>
                    </div>
                </div>
                <div class={classes!("flex", "items-center", "gap-2")}>
                    { copy_icon_button(&props.proxy_config.proxy_url, &props.on_copy) }
                </div>
            </div>

            <div class={classes!("mt-4", "grid", "gap-3", "md:grid-cols-2")}>
                <label class={classes!("text-sm")}>
                    <span class={classes!("text-[var(--muted)]")}>{ "Name" }</span>
                    <input
                        type="text"
                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2")}
                        value={form.name.clone()}
                        disabled={!can_edit_slot_metadata}
                        oninput={{
                            let form = form.clone();
                            Callback::from(move |event: InputEvent| {
                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                    let mut next = (*form).clone();
                                    next.name = target.value();
                                    form.set(next);
                                }
                            })
                        }}
                    />
                </label>
                <label class={classes!("text-sm")}>
                    <span class={classes!("text-[var(--muted)]")}>{ "Status" }</span>
                    <select
                        key={format!("proxy-config-status-{}-{}", proxy_config.id, form.status)}
                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2")}
                        value={form.status.clone()}
                        onchange={{
                            let form = form.clone();
                            Callback::from(move |event: Event| {
                                if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                                    let mut next = (*form).clone();
                                    next.status = target.value();
                                    form.set(next);
                                }
                            })
                        }}
                    >
                        <option value="active" selected={form.status == "active"}>{ "active" }</option>
                        <option value="disabled" selected={form.status == "disabled"}>{ "disabled" }</option>
                    </select>
                </label>
                <label class={classes!("text-sm", "md:col-span-2")}>
                    <span class={classes!("text-[var(--muted)]")}>{ "Proxy URL" }</span>
                    <input
                        type="text"
                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "font-mono")}
                        value={form.proxy_url.clone()}
                        oninput={{
                            let form = form.clone();
                            Callback::from(move |event: InputEvent| {
                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                    let mut next = (*form).clone();
                                    next.proxy_url = target.value();
                                    form.set(next);
                                }
                            })
                        }}
                    />
                </label>
                <label class={classes!("text-sm")}>
                    <span class={classes!("text-[var(--muted)]")}>{ "Proxy Username" }</span>
                    <input
                        type="text"
                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2")}
                        value={form.proxy_username.clone()}
                        oninput={{
                            let form = form.clone();
                            Callback::from(move |event: InputEvent| {
                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                    let mut next = (*form).clone();
                                    next.proxy_username = target.value();
                                    form.set(next);
                                }
                            })
                        }}
                    />
                </label>
                <label class={classes!("text-sm")}>
                    <span class={classes!("text-[var(--muted)]")}>{ "Proxy Password" }</span>
                    <input
                        type="text"
                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2")}
                        value={form.proxy_password.clone()}
                        oninput={{
                            let form = form.clone();
                            Callback::from(move |event: InputEvent| {
                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                    let mut next = (*form).clone();
                                    next.proxy_password = target.value();
                                    form.set(next);
                                }
                            })
                        }}
                    />
                </label>
            </div>

            <div class={classes!("mt-4", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-3")}>
                <div class={classes!("flex", "items-center", "justify-between", "gap-3")}>
                    <div class={classes!("min-w-0")}>
                        <div class={classes!("text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Visible Credentials" }</div>
                        <code class={classes!("mt-2", "block", "break-all", "font-mono", "text-xs")}>
                            { format!("{} @ {}", props.proxy_config.proxy_username.clone().unwrap_or_else(|| "-".to_string()), props.proxy_config.proxy_url.clone()) }
                        </code>
                        if let Some(password) = props.proxy_config.proxy_password.as_deref() {
                            <code class={classes!("mt-1", "block", "break-all", "font-mono", "text-xs")}>
                                { password }
                            </code>
                        }
                    </div>
                    <div class={classes!("flex", "items-center", "gap-2")}>
                        { copy_icon_button(&props.proxy_config.proxy_url, &props.on_copy) }
                        if let Some(username) = props.proxy_config.proxy_username.as_deref() {
                            { copy_icon_button(username, &props.on_copy) }
                        }
                        if let Some(password) = props.proxy_config.proxy_password.as_deref() {
                            { copy_icon_button(password, &props.on_copy) }
                        }
                    </div>
                </div>
            </div>

            <div class={classes!("mt-4", "flex", "items-center", "gap-2", "flex-wrap")}>
                <button
                    class={classes!("btn-terminal")}
                    onclick={{
                        let on_check_provider = on_check_provider.clone();
                        Callback::from(move |_| on_check_provider.emit(("codex".to_string(), false)))
                    }}
                    disabled={*saving || (*checking).is_some()}
                >
                    { if (*checking).as_deref() == Some("codex-connectivity") { "检查中..." } else { "检查 Codex" } }
                </button>
                <button
                    class={classes!("btn-terminal")}
                    onclick={{
                        let on_check_provider = on_check_provider.clone();
                        Callback::from(move |_| on_check_provider.emit(("kiro".to_string(), false)))
                    }}
                    disabled={*saving || (*checking).is_some()}
                >
                    { if (*checking).as_deref() == Some("kiro-connectivity") { "检查中..." } else { "检查 Kiro" } }
                </button>
                <button
                    class={classes!("btn-terminal")}
                    onclick={{
                        let on_check_provider = on_check_provider.clone();
                        Callback::from(move |_| on_check_provider.emit(("codex".to_string(), true)))
                    }}
                    disabled={*saving || (*checking).is_some()}
                >
                    { if (*checking).as_deref() == Some("codex-full-chain") { "请求中..." } else { "全链路 Codex" } }
                </button>
                <button
                    class={classes!("btn-terminal")}
                    onclick={{
                        let on_check_provider = on_check_provider.clone();
                        Callback::from(move |_| on_check_provider.emit(("kiro".to_string(), true)))
                    }}
                    disabled={*saving || (*checking).is_some()}
                >
                    { if (*checking).as_deref() == Some("kiro-full-chain") { "请求中..." } else { "全链路 Kiro" } }
                </button>
                <button class={classes!("btn-terminal", "btn-terminal-primary")} onclick={on_save.clone()} disabled={*saving}>
                    { if *saving { "保存中..." } else { "保存" } }
                </button>
                if props.proxy_config.has_node_override {
                    <button class={classes!("btn-terminal")} onclick={on_reset_override} disabled={*saving}>
                        { "移除本机覆盖" }
                    </button>
                }
                if can_edit_slot_metadata {
                    <button class={classes!("btn-terminal", "text-red-600", "dark:text-red-400")} onclick={on_delete} disabled={*saving}>
                        { "删除" }
                    </button>
                }
            </div>

            if let Some(feedback) = (*feedback).clone() {
                <p class={classes!("mt-2", "m-0", "text-xs", "text-[var(--muted)]")}>{ feedback }</p>
            }
        </article>
    }
}

#[function_component(AdminLlmGatewayPage)]
pub fn admin_llm_gateway_page() -> Html {
    let config = use_state(|| None::<LlmGatewayRuntimeConfig>);
    let keys = use_state(Vec::<AdminLlmGatewayKeyView>::new);
    let keys_summary = use_state(AdminLlmGatewayKeysSummaryView::default);
    let keys_search = use_state(String::new);
    let keys_sort_mode = use_state(|| KeySortMode::None);
    let keys_show_active_only = use_state(|| false);
    let keys_page = use_state(|| 1_usize);
    let keys_total = use_state(|| 0_usize);
    let keys_page_limit = use_state(|| KEY_PAGE_SIZE);
    let account_group_options = use_state(Vec::<AdminAccountGroupOptionView>::new);
    let account_groups_page_items = use_state(Vec::<AdminAccountGroupView>::new);
    let account_groups_total = use_state(|| 0_usize);
    let account_groups_page = use_state(|| 1_usize);
    let account_groups_page_limit = use_state(|| DEFAULT_ADMIN_GROUP_PAGE_SIZE);
    let account_groups_search = use_state(String::new);
    let account_group_candidate_accounts = use_state(Vec::<AccountSummaryView>::new);
    let account_group_candidate_loading = use_state(|| false);
    let usage_events = use_state(Vec::<AdminLlmGatewayUsageEventView>::new);
    let usage_total = use_state(|| 0_usize);
    let usage_totals = use_state(AdminUsageTotalsView::default);
    let usage_page = use_state(|| 1_usize);
    let usage_current_rpm = use_state(|| 0_u32);
    let usage_current_in_flight = use_state(|| 0_u32);
    let usage_retention_days = use_state(|| 7_u64);
    let usage_loading = use_state(|| false);
    let usage_error = use_state(|| None::<String>);
    let usage_key_filter = use_state(String::new);
    let usage_key_search = use_state(String::new);
    let usage_start_input = use_state(String::new);
    let usage_end_input = use_state(String::new);
    let usage_source = use_state(|| USAGE_SOURCE_HOT.to_string());
    let usage_model_filter = use_state(String::new);
    let usage_account_filter = use_state(String::new);
    let usage_endpoint_filter = use_state(String::new);
    let usage_filter_options = use_state(AdminLlmGatewayUsageFilterOptionsResponse::default);
    let usage_status_kind = use_state(|| USAGE_STATUS_KIND_ALL.to_string());
    let usage_journal_status = use_state(|| None::<AdminUsageJournalStatusView>);
    let usage_journal_preview = use_state(|| None::<AdminUsageJournalPreviewResponse>);
    let usage_journal_preview_page = use_state(|| 1_usize);
    let selected_usage_journal_message = use_state(|| None::<(String, String, String, String)>);
    let usage_journal_loading = use_state(|| false);
    let usage_journal_error = use_state(|| None::<String>);
    let journal_filter_model = use_state(String::new);
    let journal_filter_account = use_state(String::new);
    let journal_filter_key = use_state(String::new);
    let journal_filter_status = use_state(String::new);
    let token_requests = use_state(Vec::<AdminLlmGatewayTokenRequestView>::new);
    let token_request_total = use_state(|| 0_usize);
    let token_request_page = use_state(|| 1_usize);
    let token_request_loading = use_state(|| false);
    let token_request_status_filter = use_state(String::new);
    let token_request_action_inflight = use_state(HashSet::<String>::new);
    let account_contribution_requests =
        use_state(Vec::<AdminLlmGatewayAccountContributionRequestView>::new);
    let account_contribution_request_total = use_state(|| 0_usize);
    let account_contribution_request_page = use_state(|| 1_usize);
    let account_contribution_request_loading = use_state(|| false);
    let account_contribution_request_status_filter = use_state(String::new);
    let account_contribution_request_action_inflight = use_state(HashSet::<String>::new);
    let sponsor_requests = use_state(Vec::<AdminLlmGatewaySponsorRequestView>::new);
    let sponsor_request_total = use_state(|| 0_usize);
    let sponsor_request_page = use_state(|| 1_usize);
    let sponsor_request_loading = use_state(|| false);
    let sponsor_request_status_filter = use_state(String::new);
    let sponsor_request_action_inflight = use_state(HashSet::<String>::new);
    let selected_usage_event = use_state(|| None::<AdminLlmGatewayUsageEventDetailView>);
    let usage_detail_loading = use_state(|| false);
    let loading = use_state(|| true);
    let load_error = use_state(|| None::<String>);
    let ttl_input = use_state(|| "60".to_string());
    let max_request_body_input = use_state(|| (8 * 1024 * 1024_u64).to_string());
    let account_failure_retry_limit_input = use_state(|| "10".to_string());
    let codex_client_version_input =
        use_state(|| DEFAULT_LLM_GATEWAY_CODEX_CLIENT_VERSION.to_string());
    let codex_refresh_min_input = use_state(|| "240".to_string());
    let codex_refresh_max_input = use_state(|| "300".to_string());
    let codex_account_jitter_max_input = use_state(|| "10".to_string());
    let codex_weight_free_input = use_state(|| "1".to_string());
    let codex_weight_plus_input = use_state(|| "10".to_string());
    let codex_weight_pro5x_input = use_state(|| "50".to_string());
    let codex_weight_pro20x_input = use_state(|| "200".to_string());
    let kiro_refresh_min_input = use_state(|| "240".to_string());
    let kiro_refresh_max_input = use_state(|| "300".to_string());
    let kiro_account_jitter_max_input = use_state(|| "10".to_string());
    let usage_flush_batch_size_input = use_state(|| "256".to_string());
    let usage_flush_interval_input = use_state(|| "15".to_string());
    let usage_flush_max_buffer_bytes_input = use_state(|| (8 * 1024 * 1024_u64).to_string());
    let duckdb_usage_memory_limit_mib_input = use_state(|| "1024".to_string());
    let duckdb_usage_checkpoint_threshold_mib_input = use_state(|| "16".to_string());
    let usage_analytics_retention_days_input = use_state(|| "7".to_string());
    let proxy_configs = use_state(Vec::<AdminUpstreamProxyConfigView>::new);
    let proxy_config_scope = use_state(AdminUpstreamProxyConfigScopeView::default);
    let proxy_bindings = use_state(Vec::<AdminUpstreamProxyBindingView>::new);
    let create_proxy_name = use_state(|| "shared-upstream".to_string());
    let create_proxy_url = use_state(|| "http://127.0.0.1:11111".to_string());
    let create_proxy_username = use_state(String::new);
    let create_proxy_password = use_state(String::new);
    let creating_proxy = use_state(|| false);
    let codex_proxy_binding_input = use_state(String::new);
    let kiro_proxy_binding_input = use_state(String::new);
    let saving_proxy_binding_provider = use_state(|| None::<String>);
    let migrating_legacy_kiro_proxy = use_state(|| false);
    let proxy_config_search = use_state(String::new);
    let proxy_config_active_query = use_state(String::new);
    let proxy_config_show_active_only = use_state(|| false);
    let saving_runtime_config = use_state(|| false);
    // Group the create-key inputs into a single state. Saves juggling five
    // separate `use_state` clones across the form and the submit callback.
    let create_key = use_state(CreateKeyForm::default);
    let creating = use_state(|| false);
    let create_account_group_name = use_state(String::new);
    let create_account_group_account_names = use_state(Vec::<String>::new);
    let creating_account_group = use_state(|| false);
    let account_group_form_expanded = use_state(|| false);
    let refreshing_key_id = use_state(|| None::<String>);
    let toast = use_state(|| None::<(String, bool)>);
    let toast_timeout = use_mut_ref(|| None::<Timeout>);
    let flash = {
        let toast = toast.clone();
        let toast_timeout = toast_timeout.clone();
        Callback::from(move |(message, is_error): (String, bool)| {
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
    let open_usage_detail = {
        let selected_usage_event = selected_usage_event.clone();
        let usage_detail_loading = usage_detail_loading.clone();
        let flash = flash.clone();
        Callback::from(move |event_id: String| {
            let selected_usage_event = selected_usage_event.clone();
            let usage_detail_loading = usage_detail_loading.clone();
            let flash = flash.clone();
            selected_usage_event.set(None);
            usage_detail_loading.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_admin_llm_gateway_usage_event_detail(&event_id).await {
                    Ok(detail) => selected_usage_event.set(Some(detail)),
                    Err(err) => flash.emit((err, true)),
                }
                usage_detail_loading.set(false);
            });
        })
    };
    let accounts = use_state(Vec::<AccountSummaryView>::new);
    let accounts_summary = use_state(AdminAccountsSummaryView::default);
    let import_name = use_state(String::new);
    let import_id_token = use_state(String::new);
    let import_access_token = use_state(String::new);
    let import_refresh_token = use_state(String::new);
    let import_account_id = use_state(String::new);
    let import_raw_auth_json = use_state(String::new);
    let import_raw_auth_feedback = use_state(|| None::<(String, bool)>);
    let importing = use_state(|| false);
    let show_batch_import_form = use_state(|| false);
    let batch_import_raw_json = use_state(String::new);
    let batch_import_feedback = use_state(|| None::<(String, bool)>);
    let batch_import_validate_before_import = use_state(|| true);
    let batch_importing = use_state(|| false);
    let recent_import_jobs = use_state(Vec::<CodexAccountImportJobSummaryView>::new);
    let active_import_job = use_state(|| None::<CodexAccountImportJobDetailView>);
    let account_action_inflight = use_state(HashSet::<String>::new);
    let account_proxy_inputs = use_state(BTreeMap::<String, String>::new);
    let account_route_weight_tier_inputs = use_state(BTreeMap::<String, String>::new);
    let account_request_max_inputs = use_state(BTreeMap::<String, String>::new);
    let account_request_min_inputs = use_state(BTreeMap::<String, String>::new);
    let show_import_form = use_state(|| false);
    let account_search = use_state(String::new);
    let account_active_query = use_state(String::new);
    let account_sort_mode = use_state(|| AccountSortMode::None);
    let account_show_unhealthy = use_state(|| false);
    let account_show_active_only = use_state(|| false);
    let account_page = use_state(|| 1_usize);
    let accounts_total = use_state(|| 0_usize);
    let account_page_limit = use_state(|| ACCOUNT_PAGE_SIZE);
    let active_tab = use_state(|| TAB_OVERVIEW.to_string());
    let on_tab_click = {
        let active_tab = active_tab.clone();
        Callback::from(move |tab: String| active_tab.set(tab))
    };

    // Usage events are fetched independently so paging and key filters do not
    // need to re-fetch the rest of the admin page chrome.
    let reload_usage = {
        let usage_events = usage_events.clone();
        let usage_total = usage_total.clone();
        let usage_totals = usage_totals.clone();
        let usage_filter_options = usage_filter_options.clone();
        let usage_page = usage_page.clone();
        let usage_current_rpm = usage_current_rpm.clone();
        let usage_current_in_flight = usage_current_in_flight.clone();
        let usage_retention_days = usage_retention_days.clone();
        let usage_loading = usage_loading.clone();
        let usage_error = usage_error.clone();
        let usage_key_filter = usage_key_filter.clone();
        let usage_start_input = usage_start_input.clone();
        let usage_end_input = usage_end_input.clone();
        let usage_source = usage_source.clone();
        let usage_model_filter = usage_model_filter.clone();
        let usage_account_filter = usage_account_filter.clone();
        let usage_endpoint_filter = usage_endpoint_filter.clone();
        let usage_status_kind = usage_status_kind.clone();
        Callback::from(move |args: UsageReloadArgs| {
            let usage_events = usage_events.clone();
            let usage_total = usage_total.clone();
            let usage_totals = usage_totals.clone();
            let usage_filter_options = usage_filter_options.clone();
            let usage_page = usage_page.clone();
            let usage_current_rpm = usage_current_rpm.clone();
            let usage_current_in_flight = usage_current_in_flight.clone();
            let usage_retention_days = usage_retention_days.clone();
            let usage_loading = usage_loading.clone();
            let usage_error = usage_error.clone();
            let usage_key_filter = usage_key_filter.clone();
            let usage_start_input = usage_start_input.clone();
            let usage_end_input = usage_end_input.clone();
            let usage_source = usage_source.clone();
            let usage_model_filter = usage_model_filter.clone();
            let usage_account_filter = usage_account_filter.clone();
            let usage_endpoint_filter = usage_endpoint_filter.clone();
            let usage_status_kind = usage_status_kind.clone();
            let page = args.page.unwrap_or(*usage_page).max(1);
            let selected_key_id = args.key_id.unwrap_or_else(|| (*usage_key_filter).clone());
            let selected_start_input = args
                .start_input
                .unwrap_or_else(|| (*usage_start_input).clone());
            let selected_end_input = args.end_input.unwrap_or_else(|| (*usage_end_input).clone());
            let selected_source = args.source.unwrap_or_else(|| (*usage_source).clone());
            let selected_model = args.model.unwrap_or_else(|| (*usage_model_filter).clone());
            let selected_account = args
                .account_name
                .unwrap_or_else(|| (*usage_account_filter).clone());
            let selected_endpoint = args
                .endpoint
                .unwrap_or_else(|| (*usage_endpoint_filter).clone());
            let selected_status_kind = args
                .status_kind
                .unwrap_or_else(|| (*usage_status_kind).clone());
            let start_ms = parse_datetime_local_input_to_ms(&selected_start_input);
            let end_ms = parse_datetime_local_input_to_ms(&selected_end_input);
            usage_loading.set(true);
            usage_error.set(None);
            wasm_bindgen_futures::spawn_local(async move {
                let query = AdminLlmGatewayUsageEventsQuery {
                    key_id: (!selected_key_id.is_empty()).then_some(selected_key_id),
                    start_ms,
                    end_ms,
                    source: Some(selected_source),
                    model: normalized_usage_filter_text(&selected_model),
                    account_name: normalized_usage_filter_text(&selected_account),
                    endpoint: normalized_usage_filter_text(&selected_endpoint),
                    status_code: None,
                    status_kind: normalized_usage_status_kind(&selected_status_kind),
                    limit: Some(USAGE_PAGE_SIZE),
                    offset: Some((page - 1) * USAGE_PAGE_SIZE),
                };
                if args.refresh_filter_options {
                    let filter_options_query = AdminLlmGatewayUsageEventsQuery {
                        offset: None,
                        limit: None,
                        ..query.clone()
                    };
                    if let Ok(options) =
                        fetch_admin_llm_gateway_usage_filter_options(&filter_options_query).await
                    {
                        usage_filter_options.set(options);
                    }
                }
                match fetch_admin_llm_gateway_usage_events(&query).await {
                    Ok(resp) => {
                        usage_total.set(resp.total);
                        usage_totals.set(resp.totals);
                        usage_current_rpm.set(resp.current_rpm);
                        usage_current_in_flight.set(resp.current_in_flight);
                        usage_retention_days.set(resp.retention_days);
                        usage_events.set(resp.events);
                        let actual_page = (resp.offset / resp.limit.max(1)).saturating_add(1);
                        usage_page.set(actual_page.max(1));
                    },
                    Err(err) => {
                        usage_totals.set(AdminUsageTotalsView::default());
                        usage_current_rpm.set(0);
                        usage_current_in_flight.set(0);
                        usage_error.set(Some(err));
                    },
                }
                usage_loading.set(false);
            });
        })
    };

    let reload_usage_journal_status = {
        let usage_journal_status = usage_journal_status.clone();
        let usage_journal_preview = usage_journal_preview.clone();
        let usage_journal_preview_page = usage_journal_preview_page.clone();
        let usage_journal_loading = usage_journal_loading.clone();
        let usage_journal_error = usage_journal_error.clone();
        Callback::from(move |requested_page: Option<usize>| {
            let usage_journal_status = usage_journal_status.clone();
            let usage_journal_preview = usage_journal_preview.clone();
            let usage_journal_preview_page = usage_journal_preview_page.clone();
            let usage_journal_loading = usage_journal_loading.clone();
            let usage_journal_error = usage_journal_error.clone();
            let page = requested_page.unwrap_or(*usage_journal_preview_page).max(1);
            let offset = (page - 1) * JOURNAL_PREVIEW_PAGE_SIZE;
            usage_journal_loading.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                match tokio_like_join_usage_journal(offset).await {
                    Ok((status, preview)) => {
                        let actual_page = (preview.offset / preview.limit.max(1))
                            .saturating_add(1)
                            .max(1);
                        usage_journal_status.set(Some(status));
                        usage_journal_preview.set(Some(preview));
                        usage_journal_preview_page.set(actual_page);
                        usage_journal_error.set(None);
                    },
                    Err(err) => usage_journal_error.set(Some(err)),
                }
                usage_journal_loading.set(false);
            });
        })
    };

    let reload_token_requests = {
        let token_requests = token_requests.clone();
        let token_request_total = token_request_total.clone();
        let token_request_page = token_request_page.clone();
        let token_request_loading = token_request_loading.clone();
        let token_request_status_filter = token_request_status_filter.clone();
        let load_error = load_error.clone();
        Callback::from(move |(requested_page, override_status): (Option<usize>, Option<String>)| {
            let token_requests = token_requests.clone();
            let token_request_total = token_request_total.clone();
            let token_request_page = token_request_page.clone();
            let token_request_loading = token_request_loading.clone();
            let token_request_status_filter = token_request_status_filter.clone();
            let load_error = load_error.clone();
            let page = requested_page.unwrap_or(*token_request_page).max(1);
            let selected_status =
                override_status.unwrap_or_else(|| (*token_request_status_filter).clone());
            token_request_loading.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                let query = AdminLlmGatewayTokenRequestsQuery {
                    status: (!selected_status.is_empty()).then_some(selected_status),
                    limit: Some(TOKEN_REQUEST_PAGE_SIZE),
                    offset: Some((page - 1) * TOKEN_REQUEST_PAGE_SIZE),
                };
                match fetch_admin_llm_gateway_token_requests(&query).await {
                    Ok(resp) => {
                        token_request_total.set(resp.total);
                        token_requests.set(resp.requests);
                        token_request_page.set(page);
                        load_error.set(None);
                    },
                    Err(err) => load_error.set(Some(err)),
                }
                token_request_loading.set(false);
            });
        })
    };

    let reload_account_contribution_requests = {
        let account_contribution_requests = account_contribution_requests.clone();
        let account_contribution_request_total = account_contribution_request_total.clone();
        let account_contribution_request_page = account_contribution_request_page.clone();
        let account_contribution_request_loading = account_contribution_request_loading.clone();
        let account_contribution_request_status_filter =
            account_contribution_request_status_filter.clone();
        let load_error = load_error.clone();
        Callback::from(move |(requested_page, override_status): (Option<usize>, Option<String>)| {
            let account_contribution_requests = account_contribution_requests.clone();
            let account_contribution_request_total = account_contribution_request_total.clone();
            let account_contribution_request_page = account_contribution_request_page.clone();
            let account_contribution_request_loading = account_contribution_request_loading.clone();
            let account_contribution_request_status_filter =
                account_contribution_request_status_filter.clone();
            let load_error = load_error.clone();
            let page = requested_page
                .unwrap_or(*account_contribution_request_page)
                .max(1);
            let selected_status = override_status
                .unwrap_or_else(|| (*account_contribution_request_status_filter).clone());
            account_contribution_request_loading.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                let query = AdminLlmGatewayAccountContributionRequestsQuery {
                    status: (!selected_status.is_empty()).then_some(selected_status),
                    limit: Some(ACCOUNT_CONTRIBUTION_REQUEST_PAGE_SIZE),
                    offset: Some((page - 1) * ACCOUNT_CONTRIBUTION_REQUEST_PAGE_SIZE),
                };
                match fetch_admin_llm_gateway_account_contribution_requests(&query).await {
                    Ok(resp) => {
                        account_contribution_request_total.set(resp.total);
                        account_contribution_requests.set(resp.requests);
                        account_contribution_request_page.set(page);
                        load_error.set(None);
                    },
                    Err(err) => load_error.set(Some(err)),
                }
                account_contribution_request_loading.set(false);
            });
        })
    };

    let reload_sponsor_requests = {
        let sponsor_requests = sponsor_requests.clone();
        let sponsor_request_total = sponsor_request_total.clone();
        let sponsor_request_page = sponsor_request_page.clone();
        let sponsor_request_loading = sponsor_request_loading.clone();
        let sponsor_request_status_filter = sponsor_request_status_filter.clone();
        let load_error = load_error.clone();
        Callback::from(move |(requested_page, override_status): (Option<usize>, Option<String>)| {
            let sponsor_requests = sponsor_requests.clone();
            let sponsor_request_total = sponsor_request_total.clone();
            let sponsor_request_page = sponsor_request_page.clone();
            let sponsor_request_loading = sponsor_request_loading.clone();
            let sponsor_request_status_filter = sponsor_request_status_filter.clone();
            let load_error = load_error.clone();
            let page = requested_page.unwrap_or(*sponsor_request_page).max(1);
            let selected_status =
                override_status.unwrap_or_else(|| (*sponsor_request_status_filter).clone());
            sponsor_request_loading.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                let query = AdminLlmGatewaySponsorRequestsQuery {
                    status: (!selected_status.is_empty()).then_some(selected_status),
                    limit: Some(SPONSOR_REQUEST_PAGE_SIZE),
                    offset: Some((page - 1) * SPONSOR_REQUEST_PAGE_SIZE),
                };
                match fetch_admin_llm_gateway_sponsor_requests(&query).await {
                    Ok(resp) => {
                        sponsor_request_total.set(resp.total);
                        sponsor_requests.set(resp.requests);
                        sponsor_request_page.set(page);
                        load_error.set(None);
                    },
                    Err(err) => load_error.set(Some(err)),
                }
                sponsor_request_loading.set(false);
            });
        })
    };

    // This reload keeps the inventory, runtime config, and the current usage
    // page in sync after any admin write operation.
    let reload = {
        let config = config.clone();
        let keys = keys.clone();
        let keys_summary = keys_summary.clone();
        let keys_total = keys_total.clone();
        let keys_page_limit = keys_page_limit.clone();
        let account_group_options = account_group_options.clone();
        let proxy_configs = proxy_configs.clone();
        let proxy_config_scope = proxy_config_scope.clone();
        let proxy_bindings = proxy_bindings.clone();
        let account_groups_page_items = account_groups_page_items.clone();
        let account_groups_total = account_groups_total.clone();
        let account_groups_page = account_groups_page.clone();
        let account_groups_page_limit = account_groups_page_limit.clone();
        let loading = loading.clone();
        let load_error = load_error.clone();
        let ttl_input = ttl_input.clone();
        let max_request_body_input = max_request_body_input.clone();
        let account_failure_retry_limit_input = account_failure_retry_limit_input.clone();
        let codex_client_version_input = codex_client_version_input.clone();
        let codex_refresh_min_input = codex_refresh_min_input.clone();
        let codex_refresh_max_input = codex_refresh_max_input.clone();
        let codex_account_jitter_max_input = codex_account_jitter_max_input.clone();
        let codex_weight_free_input = codex_weight_free_input.clone();
        let codex_weight_plus_input = codex_weight_plus_input.clone();
        let codex_weight_pro5x_input = codex_weight_pro5x_input.clone();
        let codex_weight_pro20x_input = codex_weight_pro20x_input.clone();
        let kiro_refresh_min_input = kiro_refresh_min_input.clone();
        let kiro_refresh_max_input = kiro_refresh_max_input.clone();
        let kiro_account_jitter_max_input = kiro_account_jitter_max_input.clone();
        let usage_flush_batch_size_input = usage_flush_batch_size_input.clone();
        let usage_flush_interval_input = usage_flush_interval_input.clone();
        let usage_flush_max_buffer_bytes_input = usage_flush_max_buffer_bytes_input.clone();
        let duckdb_usage_memory_limit_mib_input = duckdb_usage_memory_limit_mib_input.clone();
        let duckdb_usage_checkpoint_threshold_mib_input =
            duckdb_usage_checkpoint_threshold_mib_input.clone();
        let usage_analytics_retention_days_input = usage_analytics_retention_days_input.clone();
        let codex_proxy_binding_input = codex_proxy_binding_input.clone();
        let kiro_proxy_binding_input = kiro_proxy_binding_input.clone();
        let usage_page = usage_page.clone();
        let usage_key_filter = usage_key_filter.clone();
        let accounts = accounts.clone();
        let accounts_summary = accounts_summary.clone();
        let accounts_total = accounts_total.clone();
        let account_page_limit = account_page_limit.clone();
        let active_tab = active_tab.clone();
        let recent_import_jobs = recent_import_jobs.clone();
        let account_proxy_inputs = account_proxy_inputs.clone();
        let account_route_weight_tier_inputs = account_route_weight_tier_inputs.clone();
        let account_request_max_inputs = account_request_max_inputs.clone();
        let account_request_min_inputs = account_request_min_inputs.clone();
        let account_group_candidate_accounts = account_group_candidate_accounts.clone();
        let account_group_candidate_loading = account_group_candidate_loading.clone();
        let keys_search = keys_search.clone();
        let keys_sort_mode = keys_sort_mode.clone();
        let keys_show_active_only = keys_show_active_only.clone();
        let keys_page = keys_page.clone();
        let account_active_query = account_active_query.clone();
        let account_sort_mode = account_sort_mode.clone();
        let account_show_unhealthy = account_show_unhealthy.clone();
        let account_show_active_only = account_show_active_only.clone();
        let account_page = account_page.clone();
        let reload_usage = reload_usage.clone();
        Callback::from(move |_| {
            let config = config.clone();
            let keys = keys.clone();
            let keys_summary = keys_summary.clone();
            let keys_total = keys_total.clone();
            let keys_page_limit = keys_page_limit.clone();
            let account_group_options = account_group_options.clone();
            let proxy_configs = proxy_configs.clone();
            let proxy_config_scope = proxy_config_scope.clone();
            let proxy_bindings = proxy_bindings.clone();
            let account_groups_page_items = account_groups_page_items.clone();
            let account_groups_total = account_groups_total.clone();
            let account_groups_page = account_groups_page.clone();
            let account_groups_page_limit = account_groups_page_limit.clone();
            let loading = loading.clone();
            let load_error = load_error.clone();
            let ttl_input = ttl_input.clone();
            let max_request_body_input = max_request_body_input.clone();
            let account_failure_retry_limit_input = account_failure_retry_limit_input.clone();
            let codex_client_version_input = codex_client_version_input.clone();
            let codex_refresh_min_input = codex_refresh_min_input.clone();
            let codex_refresh_max_input = codex_refresh_max_input.clone();
            let codex_account_jitter_max_input = codex_account_jitter_max_input.clone();
            let codex_weight_free_input = codex_weight_free_input.clone();
            let codex_weight_plus_input = codex_weight_plus_input.clone();
            let codex_weight_pro5x_input = codex_weight_pro5x_input.clone();
            let codex_weight_pro20x_input = codex_weight_pro20x_input.clone();
            let kiro_refresh_min_input = kiro_refresh_min_input.clone();
            let kiro_refresh_max_input = kiro_refresh_max_input.clone();
            let kiro_account_jitter_max_input = kiro_account_jitter_max_input.clone();
            let usage_flush_batch_size_input = usage_flush_batch_size_input.clone();
            let usage_flush_interval_input = usage_flush_interval_input.clone();
            let usage_flush_max_buffer_bytes_input = usage_flush_max_buffer_bytes_input.clone();
            let duckdb_usage_memory_limit_mib_input = duckdb_usage_memory_limit_mib_input.clone();
            let duckdb_usage_checkpoint_threshold_mib_input =
                duckdb_usage_checkpoint_threshold_mib_input.clone();
            let usage_analytics_retention_days_input = usage_analytics_retention_days_input.clone();
            let codex_proxy_binding_input = codex_proxy_binding_input.clone();
            let kiro_proxy_binding_input = kiro_proxy_binding_input.clone();
            let usage_page = usage_page.clone();
            let usage_key_filter = usage_key_filter.clone();
            let accounts = accounts.clone();
            let accounts_summary = accounts_summary.clone();
            let accounts_total = accounts_total.clone();
            let account_page_limit = account_page_limit.clone();
            let active_tab = active_tab.clone();
            let recent_import_jobs = recent_import_jobs.clone();
            let account_proxy_inputs = account_proxy_inputs.clone();
            let account_route_weight_tier_inputs = account_route_weight_tier_inputs.clone();
            let account_request_max_inputs = account_request_max_inputs.clone();
            let account_request_min_inputs = account_request_min_inputs.clone();
            let account_group_candidate_accounts = account_group_candidate_accounts.clone();
            let account_group_candidate_loading = account_group_candidate_loading.clone();
            let keys_search = keys_search.clone();
            let keys_sort_mode = keys_sort_mode.clone();
            let keys_show_active_only = keys_show_active_only.clone();
            let keys_page = keys_page.clone();
            let account_active_query = account_active_query.clone();
            let account_sort_mode = account_sort_mode.clone();
            let account_show_unhealthy = account_show_unhealthy.clone();
            let account_show_active_only = account_show_active_only.clone();
            let account_page = account_page.clone();
            let reload_usage = reload_usage.clone();
            loading.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                let active_tab_value = (*active_tab).clone();
                let current_key_filter = (*usage_key_filter).clone();
                let current_page = (*usage_page).max(1);
                let current_group_page = (*account_groups_page).max(1);
                let current_keys_page = (*keys_page).max(1);
                let current_account_page = (*account_page).max(1);
                let key_query = AdminLlmGatewayKeyPageQuery {
                    q: Some((*keys_search).clone()),
                    active_only: *keys_show_active_only,
                    sort: Some(
                        match *keys_sort_mode {
                            KeySortMode::QuotaAsc => "quota_asc",
                            KeySortMode::QuotaDesc => "quota_desc",
                            KeySortMode::UsageAsc => "usage_asc",
                            KeySortMode::UsageDesc => "usage_desc",
                            KeySortMode::None => "",
                        }
                        .to_string(),
                    ),
                };
                let account_query = AdminLlmGatewayAccountPageQuery {
                    q: Some((*account_active_query).clone()),
                    active_only: *account_show_active_only,
                    unhealthy_only: *account_show_unhealthy,
                    sort: Some(
                        match *account_sort_mode {
                            AccountSortMode::PrimaryAsc => "primary_asc",
                            AccountSortMode::PrimaryDesc => "primary_desc",
                            AccountSortMode::SecondaryAsc => "secondary_asc",
                            AccountSortMode::SecondaryDesc => "secondary_desc",
                            AccountSortMode::None => "",
                        }
                        .to_string(),
                    ),
                };
                let result = async {
                    let (
                        cfg_result,
                        key_summary_result,
                        account_summary_result,
                        proxy_configs_result,
                        proxy_bindings_result,
                    ) = futures::join!(
                        fetch_admin_llm_gateway_config(),
                        fetch_admin_llm_gateway_keys_page(1, 0),
                        fetch_admin_llm_gateway_accounts_page(1, 0),
                        fetch_admin_llm_gateway_proxy_configs(),
                        fetch_admin_llm_gateway_proxy_bindings(),
                    );
                    let cfg = cfg_result?;
                    let key_summary_resp = key_summary_result?;
                    let account_summary_resp = account_summary_result?;
                    let proxy_configs_resp = proxy_configs_result?;
                    let proxy_bindings_resp = proxy_bindings_result?;
                    let keys_resp = if should_load_llm_gateway_keys_inventory(&active_tab_value) {
                        if active_tab_value == TAB_KEYS {
                            let limit = KEY_PAGE_SIZE.max(1);
                            let offset = current_keys_page.saturating_sub(1) * limit;
                            Some(
                                fetch_admin_llm_gateway_keys_page_with_query(
                                    limit, offset, &key_query,
                                )
                                .await?,
                            )
                        } else {
                            Some(fetch_admin_llm_gateway_keys().await?)
                        }
                    } else {
                        None
                    };
                    let account_group_options_resp =
                        if should_load_llm_gateway_group_options(&active_tab_value) {
                            Some(fetch_admin_llm_gateway_account_group_options().await?)
                        } else {
                            None
                        };
                    let account_groups_page_resp = if active_tab_value == TAB_GROUPS {
                        let limit = *account_groups_page_limit;
                        let offset = current_group_page.saturating_sub(1) * limit.max(1);
                        Some(fetch_admin_llm_gateway_account_groups_page(limit, offset).await?)
                    } else {
                        None
                    };
                    let accounts_resp = if active_tab_value == TAB_ACCOUNTS {
                        let limit = ACCOUNT_PAGE_SIZE.max(1);
                        let offset = current_account_page.saturating_sub(1) * limit;
                        Some(
                            fetch_admin_llm_gateway_accounts_page_with_query(
                                limit,
                                offset,
                                &account_query,
                            )
                            .await?,
                        )
                    } else {
                        None
                    };
                    let import_jobs = if should_load_llm_gateway_import_jobs(&active_tab_value) {
                        Some(
                            fetch_admin_llm_gateway_account_import_jobs(Some(
                                ADMIN_CODEX_IMPORT_JOB_LIST_LIMIT,
                            ))
                            .await?,
                        )
                    } else {
                        None
                    };
                    let effective_key_filter = if let Some(keys_resp) = keys_resp.as_ref() {
                        if current_key_filter.is_empty()
                            || keys_resp
                                .keys
                                .iter()
                                .any(|item| item.id == current_key_filter)
                        {
                            current_key_filter
                        } else {
                            String::new()
                        }
                    } else {
                        current_key_filter
                    };
                    Ok::<_, String>((
                        cfg,
                        key_summary_resp,
                        account_summary_resp,
                        proxy_configs_resp.proxy_config_scope,
                        proxy_configs_resp.proxy_configs,
                        proxy_bindings_resp.bindings,
                        keys_resp,
                        account_group_options_resp,
                        account_groups_page_resp,
                        accounts_resp,
                        import_jobs,
                        effective_key_filter,
                    ))
                }
                .await;

                match result {
                    Ok((
                        cfg,
                        key_summary_resp,
                        account_summary_resp,
                        proxy_config_scope_resp,
                        proxy_config_items,
                        proxy_binding_items,
                        keys_resp,
                        account_group_options_resp,
                        account_groups_page_resp,
                        accounts_resp,
                        import_jobs,
                        effective_key_filter,
                    )) => {
                        let usage_filter_for_reload = effective_key_filter.clone();
                        ttl_input.set(cfg.auth_cache_ttl_seconds.to_string());
                        max_request_body_input.set(cfg.max_request_body_bytes.to_string());
                        account_failure_retry_limit_input
                            .set(cfg.account_failure_retry_limit.to_string());
                        codex_client_version_input.set(cfg.codex_client_version.clone());
                        codex_refresh_min_input
                            .set(cfg.codex_status_refresh_min_interval_seconds.to_string());
                        codex_refresh_max_input
                            .set(cfg.codex_status_refresh_max_interval_seconds.to_string());
                        codex_account_jitter_max_input
                            .set(cfg.codex_status_account_jitter_max_seconds.to_string());
                        codex_weight_free_input.set(cfg.codex_weight_free.to_string());
                        codex_weight_plus_input.set(cfg.codex_weight_plus.to_string());
                        codex_weight_pro5x_input.set(cfg.codex_weight_pro5x.to_string());
                        codex_weight_pro20x_input.set(cfg.codex_weight_pro20x.to_string());
                        kiro_refresh_min_input
                            .set(cfg.kiro_status_refresh_min_interval_seconds.to_string());
                        kiro_refresh_max_input
                            .set(cfg.kiro_status_refresh_max_interval_seconds.to_string());
                        kiro_account_jitter_max_input
                            .set(cfg.kiro_status_account_jitter_max_seconds.to_string());
                        usage_flush_batch_size_input
                            .set(cfg.usage_event_flush_batch_size.to_string());
                        usage_flush_interval_input
                            .set(cfg.usage_event_flush_interval_seconds.to_string());
                        usage_flush_max_buffer_bytes_input
                            .set(cfg.usage_event_flush_max_buffer_bytes.to_string());
                        duckdb_usage_memory_limit_mib_input
                            .set(cfg.duckdb_usage_memory_limit_mib.to_string());
                        duckdb_usage_checkpoint_threshold_mib_input
                            .set(cfg.duckdb_usage_checkpoint_threshold_mib.to_string());
                        usage_analytics_retention_days_input
                            .set(cfg.usage_analytics_retention_days.to_string());
                        config.set(Some(cfg));
                        keys_summary.set(key_summary_resp.summary);
                        accounts_summary.set(account_summary_resp.summary);
                        proxy_config_scope.set(proxy_config_scope_resp);
                        let codex_bound = proxy_binding_items
                            .iter()
                            .find(|item| item.provider_type == "codex")
                            .and_then(|item| item.bound_proxy_config_id.clone())
                            .unwrap_or_default();
                        let kiro_bound = proxy_binding_items
                            .iter()
                            .find(|item| item.provider_type == "kiro")
                            .and_then(|item| item.bound_proxy_config_id.clone())
                            .unwrap_or_default();
                        proxy_configs.set(proxy_config_items);
                        proxy_bindings.set(proxy_binding_items);
                        codex_proxy_binding_input.set(codex_bound);
                        kiro_proxy_binding_input.set(kiro_bound);
                        if let Some(keys_resp) = keys_resp {
                            keys_total.set(keys_resp.total);
                            keys_page_limit.set(keys_resp.limit.max(1));
                            keys.set(keys_resp.keys);
                            usage_key_filter.set(effective_key_filter);
                        }
                        if let Some(account_group_options_resp) = account_group_options_resp {
                            account_group_options.set(account_group_options_resp);
                        }
                        if let Some(account_groups_page_resp) = account_groups_page_resp {
                            let effective_limit = account_groups_page_resp.limit.max(1);
                            let total_pages = admin_group_total_pages(
                                account_groups_page_resp.total,
                                effective_limit,
                            );
                            account_groups_total.set(account_groups_page_resp.total);
                            account_groups_page_limit.set(effective_limit);
                            if current_group_page > total_pages {
                                account_groups_page.set(total_pages);
                            } else {
                                account_groups_page_items.set(account_groups_page_resp.groups);
                            }
                        }
                        if let Some(accounts_resp) = accounts_resp {
                            let next_proxy_inputs = accounts_resp
                                .accounts
                                .iter()
                                .map(|account| {
                                    (account.name.clone(), account_proxy_select_value(account))
                                })
                                .collect::<BTreeMap<_, _>>();
                            let next_route_weight_tier_inputs = accounts_resp
                                .accounts
                                .iter()
                                .map(|account| {
                                    (
                                        account.name.clone(),
                                        if account.route_weight_tier.trim().is_empty() {
                                            "auto".to_string()
                                        } else {
                                            account.route_weight_tier.clone()
                                        },
                                    )
                                })
                                .collect::<BTreeMap<_, _>>();
                            let next_request_max_inputs = accounts_resp
                                .accounts
                                .iter()
                                .map(|account| {
                                    (
                                        account.name.clone(),
                                        account
                                            .request_max_concurrency
                                            .map(|value| value.to_string())
                                            .unwrap_or_default(),
                                    )
                                })
                                .collect::<BTreeMap<_, _>>();
                            let next_request_min_inputs = accounts_resp
                                .accounts
                                .iter()
                                .map(|account| {
                                    (
                                        account.name.clone(),
                                        account
                                            .request_min_start_interval_ms
                                            .map(|value| value.to_string())
                                            .unwrap_or_default(),
                                    )
                                })
                                .collect::<BTreeMap<_, _>>();
                            accounts_total.set(accounts_resp.total);
                            account_page_limit.set(accounts_resp.limit.max(1));
                            accounts.set(accounts_resp.accounts);
                            account_proxy_inputs.set(next_proxy_inputs);
                            account_route_weight_tier_inputs.set(next_route_weight_tier_inputs);
                            account_request_max_inputs.set(next_request_max_inputs);
                            account_request_min_inputs.set(next_request_min_inputs);
                        } else if active_tab_value != TAB_GROUPS {
                            accounts_total.set(0);
                            accounts.set(Vec::new());
                            account_proxy_inputs.set(BTreeMap::new());
                            account_route_weight_tier_inputs.set(BTreeMap::new());
                            account_request_max_inputs.set(BTreeMap::new());
                            account_request_min_inputs.set(BTreeMap::new());
                        }
                        if active_tab_value != TAB_GROUPS {
                            account_group_candidate_accounts.set(Vec::new());
                            account_group_candidate_loading.set(false);
                        }
                        if let Some(import_jobs) = import_jobs {
                            recent_import_jobs.set(import_jobs);
                        }
                        load_error.set(None);
                        if *active_tab == TAB_USAGE {
                            reload_usage.emit(UsageReloadArgs {
                                page: Some(current_page),
                                key_id: Some(usage_filter_for_reload),
                                ..UsageReloadArgs::default()
                            });
                        }
                    },
                    Err(err) => load_error.set(Some(err)),
                }
                loading.set(false);
            });
        })
    };

    let load_account_group_candidates = {
        let account_group_candidate_accounts = account_group_candidate_accounts.clone();
        let account_group_candidate_loading = account_group_candidate_loading.clone();
        let load_error = load_error.clone();
        Callback::from(move |_| {
            account_group_candidate_loading.set(true);
            let account_group_candidate_accounts = account_group_candidate_accounts.clone();
            let account_group_candidate_loading = account_group_candidate_loading.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_admin_llm_gateway_accounts().await {
                    Ok(resp) => {
                        account_group_candidate_accounts.set(resp.accounts);
                        load_error.set(None);
                    },
                    Err(err) => load_error.set(Some(err)),
                }
                account_group_candidate_loading.set(false);
            });
        })
    };

    {
        let reload = reload.clone();
        let active_tab = active_tab.clone();
        use_effect_with(((*active_tab).clone(),), move |_| {
            reload.emit(());
            || ()
        });
    }

    {
        let reload = reload.clone();
        let active_tab = active_tab.clone();
        let keys_page = keys_page.clone();
        let keys_search = keys_search.clone();
        let keys_sort_mode = keys_sort_mode.clone();
        let keys_show_active_only = keys_show_active_only.clone();
        use_effect_with(
            (*keys_page, (*keys_search).clone(), *keys_sort_mode, *keys_show_active_only),
            move |_| {
                if *active_tab == TAB_KEYS {
                    reload.emit(());
                }
                || ()
            },
        );
    }

    {
        let reload = reload.clone();
        let active_tab = active_tab.clone();
        let account_page = account_page.clone();
        let account_active_query = account_active_query.clone();
        let account_sort_mode = account_sort_mode.clone();
        let account_show_unhealthy = account_show_unhealthy.clone();
        let account_show_active_only = account_show_active_only.clone();
        use_effect_with(
            (
                *account_page,
                (*account_active_query).clone(),
                *account_sort_mode,
                *account_show_unhealthy,
                *account_show_active_only,
            ),
            move |_| {
                if *active_tab == TAB_ACCOUNTS {
                    reload.emit(());
                }
                || ()
            },
        );
    }

    {
        let reload_token_requests = reload_token_requests.clone();
        let reload_account_contribution_requests = reload_account_contribution_requests.clone();
        let reload_sponsor_requests = reload_sponsor_requests.clone();
        use_effect_with((), move |_| {
            reload_token_requests.emit((Some(1), Some(String::new())));
            reload_account_contribution_requests.emit((Some(1), Some(String::new())));
            reload_sponsor_requests.emit((Some(1), Some(String::new())));
            || ()
        });
    }

    {
        let active_tab = active_tab.clone();
        let reload_usage_journal_status = reload_usage_journal_status.clone();
        use_effect_with(((*active_tab).clone(),), move |(active_tab,)| {
            let interval = if should_load_usage_journal(active_tab) {
                reload_usage_journal_status.emit(None);
                Some(Interval::new(5_000, move || {
                    reload_usage_journal_status.emit(None);
                }))
            } else {
                None
            };
            move || drop(interval)
        });
    }

    {
        let active_import_job = active_import_job.clone();
        let recent_import_jobs = recent_import_jobs.clone();
        let reload = reload.clone();
        let load_error = load_error.clone();
        use_effect_with((*active_import_job).clone(), move |job_detail| {
            let interval = job_detail.clone().and_then(|job_detail| {
                if codex_import_job_is_terminal(&job_detail.summary.status) {
                    return None;
                }
                let job_id = job_detail.summary.job_id.clone();
                Some(Interval::new(1500, move || {
                    let active_import_job = active_import_job.clone();
                    let recent_import_jobs = recent_import_jobs.clone();
                    let reload = reload.clone();
                    let load_error = load_error.clone();
                    let job_id = job_id.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        match fetch_admin_llm_gateway_account_import_job(&job_id).await {
                            Ok(detail) => {
                                let summary = detail.summary.clone();
                                let is_terminal = codex_import_job_is_terminal(&summary.status);
                                active_import_job.set(Some(detail));
                                recent_import_jobs.set(upsert_codex_import_job_summary(
                                    &recent_import_jobs,
                                    summary,
                                ));
                                if is_terminal {
                                    reload.emit(());
                                }
                            },
                            Err(err) => load_error.set(Some(err)),
                        }
                    });
                }))
            });
            move || drop(interval)
        });
    }

    let on_save_runtime_config = {
        let config = config.clone();
        let ttl_input = ttl_input.clone();
        let max_request_body_input = max_request_body_input.clone();
        let account_failure_retry_limit_input = account_failure_retry_limit_input.clone();
        let codex_client_version_input = codex_client_version_input.clone();
        let codex_refresh_min_input = codex_refresh_min_input.clone();
        let codex_refresh_max_input = codex_refresh_max_input.clone();
        let codex_account_jitter_max_input = codex_account_jitter_max_input.clone();
        let codex_weight_free_input = codex_weight_free_input.clone();
        let codex_weight_plus_input = codex_weight_plus_input.clone();
        let codex_weight_pro5x_input = codex_weight_pro5x_input.clone();
        let codex_weight_pro20x_input = codex_weight_pro20x_input.clone();
        let kiro_refresh_min_input = kiro_refresh_min_input.clone();
        let kiro_refresh_max_input = kiro_refresh_max_input.clone();
        let kiro_account_jitter_max_input = kiro_account_jitter_max_input.clone();
        let usage_flush_batch_size_input = usage_flush_batch_size_input.clone();
        let usage_flush_interval_input = usage_flush_interval_input.clone();
        let usage_flush_max_buffer_bytes_input = usage_flush_max_buffer_bytes_input.clone();
        let duckdb_usage_memory_limit_mib_input = duckdb_usage_memory_limit_mib_input.clone();
        let duckdb_usage_checkpoint_threshold_mib_input =
            duckdb_usage_checkpoint_threshold_mib_input.clone();
        let usage_analytics_retention_days_input = usage_analytics_retention_days_input.clone();
        let saving_runtime_config = saving_runtime_config.clone();
        let load_error = load_error.clone();
        let reload = reload.clone();
        Callback::from(move |_| {
            let config = config.clone();
            let ttl = (*ttl_input).trim().parse::<u64>();
            let max_request_body_bytes = (*max_request_body_input).trim().parse::<u64>();
            let account_failure_retry_limit =
                (*account_failure_retry_limit_input).trim().parse::<u64>();
            let codex_client_version = (*codex_client_version_input).trim().to_string();
            let codex_status_refresh_min_interval_seconds =
                (*codex_refresh_min_input).trim().parse::<u64>();
            let codex_status_refresh_max_interval_seconds =
                (*codex_refresh_max_input).trim().parse::<u64>();
            let codex_status_account_jitter_max_seconds =
                (*codex_account_jitter_max_input).trim().parse::<u64>();
            let codex_weight_free = (*codex_weight_free_input).trim().parse::<u64>();
            let codex_weight_plus = (*codex_weight_plus_input).trim().parse::<u64>();
            let codex_weight_pro5x = (*codex_weight_pro5x_input).trim().parse::<u64>();
            let codex_weight_pro20x = (*codex_weight_pro20x_input).trim().parse::<u64>();
            let kiro_status_refresh_min_interval_seconds =
                (*kiro_refresh_min_input).trim().parse::<u64>();
            let kiro_status_refresh_max_interval_seconds =
                (*kiro_refresh_max_input).trim().parse::<u64>();
            let kiro_status_account_jitter_max_seconds =
                (*kiro_account_jitter_max_input).trim().parse::<u64>();
            let usage_event_flush_batch_size =
                (*usage_flush_batch_size_input).trim().parse::<u64>();
            let usage_event_flush_interval_seconds =
                (*usage_flush_interval_input).trim().parse::<u64>();
            let usage_event_flush_max_buffer_bytes =
                (*usage_flush_max_buffer_bytes_input).trim().parse::<u64>();
            let duckdb_usage_memory_limit_mib =
                (*duckdb_usage_memory_limit_mib_input).trim().parse::<u64>();
            let duckdb_usage_checkpoint_threshold_mib =
                (*duckdb_usage_checkpoint_threshold_mib_input)
                    .trim()
                    .parse::<u64>();
            let usage_analytics_retention_days = (*usage_analytics_retention_days_input)
                .trim()
                .parse::<u64>();
            let saving_runtime_config = saving_runtime_config.clone();
            let load_error = load_error.clone();
            let reload = reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let Ok(ttl) = ttl else {
                    load_error.set(Some("TTL 必须是正整数".to_string()));
                    return;
                };
                let Ok(max_request_body_bytes) = max_request_body_bytes else {
                    load_error.set(Some("请求体上限必须是正整数".to_string()));
                    return;
                };
                let Ok(account_failure_retry_limit) = account_failure_retry_limit else {
                    load_error.set(Some("账号失败重试次数必须是非负整数".to_string()));
                    return;
                };
                if codex_client_version.is_empty() {
                    load_error.set(Some("Codex client version 不能为空".to_string()));
                    return;
                }
                let Ok(codex_status_refresh_min_interval_seconds) =
                    codex_status_refresh_min_interval_seconds
                else {
                    load_error.set(Some("Codex 最小轮询间隔必须是非负整数".to_string()));
                    return;
                };
                let Ok(codex_status_refresh_max_interval_seconds) =
                    codex_status_refresh_max_interval_seconds
                else {
                    load_error.set(Some("Codex 最大轮询间隔必须是非负整数".to_string()));
                    return;
                };
                let Ok(codex_status_account_jitter_max_seconds) =
                    codex_status_account_jitter_max_seconds
                else {
                    load_error.set(Some("Codex 单账号抖动上限必须是非负整数".to_string()));
                    return;
                };
                let Ok(codex_weight_free) = codex_weight_free else {
                    load_error.set(Some("Codex free 权重必须是非负整数".to_string()));
                    return;
                };
                let Ok(codex_weight_plus) = codex_weight_plus else {
                    load_error.set(Some("Codex plus 权重必须是非负整数".to_string()));
                    return;
                };
                let Ok(codex_weight_pro5x) = codex_weight_pro5x else {
                    load_error.set(Some("Codex pro5x 权重必须是非负整数".to_string()));
                    return;
                };
                let Ok(codex_weight_pro20x) = codex_weight_pro20x else {
                    load_error.set(Some("Codex pro20x 权重必须是非负整数".to_string()));
                    return;
                };
                let Ok(kiro_status_refresh_min_interval_seconds) =
                    kiro_status_refresh_min_interval_seconds
                else {
                    load_error.set(Some("Kiro 最小轮询间隔必须是非负整数".to_string()));
                    return;
                };
                let Ok(kiro_status_refresh_max_interval_seconds) =
                    kiro_status_refresh_max_interval_seconds
                else {
                    load_error.set(Some("Kiro 最大轮询间隔必须是非负整数".to_string()));
                    return;
                };
                let Ok(kiro_status_account_jitter_max_seconds) =
                    kiro_status_account_jitter_max_seconds
                else {
                    load_error.set(Some("Kiro 单账号抖动上限必须是非负整数".to_string()));
                    return;
                };
                let Ok(usage_event_flush_batch_size) = usage_event_flush_batch_size else {
                    load_error.set(Some("usage flush 批大小必须是非负整数".to_string()));
                    return;
                };
                let Ok(usage_event_flush_interval_seconds) = usage_event_flush_interval_seconds
                else {
                    load_error.set(Some("usage flush 间隔必须是非负整数".to_string()));
                    return;
                };
                let Ok(usage_event_flush_max_buffer_bytes) = usage_event_flush_max_buffer_bytes
                else {
                    load_error.set(Some("usage flush 缓冲上限必须是非负整数".to_string()));
                    return;
                };
                let Ok(duckdb_usage_memory_limit_mib) = duckdb_usage_memory_limit_mib else {
                    load_error.set(Some("DuckDB memory_limit 必须是正整数 MiB".to_string()));
                    return;
                };
                let Ok(duckdb_usage_checkpoint_threshold_mib) =
                    duckdb_usage_checkpoint_threshold_mib
                else {
                    load_error
                        .set(Some("DuckDB checkpoint threshold 必须是正整数 MiB".to_string()));
                    return;
                };
                let Ok(usage_analytics_retention_days) = usage_analytics_retention_days else {
                    load_error.set(Some("Usage analytics retention 必须是正整数天数".to_string()));
                    return;
                };
                let runtime_config = LlmGatewayRuntimeConfig {
                    auth_cache_ttl_seconds: ttl,
                    max_request_body_bytes,
                    account_failure_retry_limit,
                    codex_client_version,
                    codex_status_refresh_min_interval_seconds,
                    codex_status_refresh_max_interval_seconds,
                    codex_status_account_jitter_max_seconds,
                    codex_weight_free,
                    codex_weight_plus,
                    codex_weight_pro5x,
                    codex_weight_pro20x,
                    kiro_status_refresh_min_interval_seconds,
                    kiro_status_refresh_max_interval_seconds,
                    kiro_status_account_jitter_max_seconds,
                    usage_event_flush_batch_size,
                    usage_event_flush_interval_seconds,
                    usage_event_flush_max_buffer_bytes,
                    duckdb_usage_memory_limit_mib,
                    duckdb_usage_checkpoint_threshold_mib,
                    usage_analytics_retention_days,
                    usage_journal_enabled: config
                        .as_ref()
                        .map(|current| current.usage_journal_enabled)
                        .unwrap_or(true),
                    usage_journal_max_file_bytes: config
                        .as_ref()
                        .map(|current| current.usage_journal_max_file_bytes)
                        .unwrap_or(64 * 1024 * 1024),
                    usage_journal_max_file_age_ms: config
                        .as_ref()
                        .map(|current| current.usage_journal_max_file_age_ms)
                        .unwrap_or(300_000),
                    usage_journal_max_files: config
                        .as_ref()
                        .map(|current| current.usage_journal_max_files)
                        .unwrap_or(128),
                    usage_journal_block_target_uncompressed_bytes: config
                        .as_ref()
                        .map(|current| current.usage_journal_block_target_uncompressed_bytes)
                        .unwrap_or(1024 * 1024),
                    usage_journal_block_max_events: config
                        .as_ref()
                        .map(|current| current.usage_journal_block_max_events)
                        .unwrap_or(1024),
                    usage_journal_fsync_interval_ms: config
                        .as_ref()
                        .map(|current| current.usage_journal_fsync_interval_ms)
                        .unwrap_or(250),
                    usage_journal_zstd_level: config
                        .as_ref()
                        .map(|current| current.usage_journal_zstd_level)
                        .unwrap_or(3),
                    usage_journal_consumer_lease_ms: config
                        .as_ref()
                        .map(|current| current.usage_journal_consumer_lease_ms)
                        .unwrap_or(300_000),
                    usage_journal_delete_bad_files: config
                        .as_ref()
                        .map(|current| current.usage_journal_delete_bad_files)
                        .unwrap_or(false),
                    usage_query_bind_addr: config
                        .as_ref()
                        .map(|current| current.usage_query_bind_addr.clone())
                        .unwrap_or_else(|| "127.0.0.1:19081".to_string()),
                    usage_query_base_url: config
                        .as_ref()
                        .map(|current| current.usage_query_base_url.clone())
                        .unwrap_or_else(|| "http://127.0.0.1:19081".to_string()),
                    kiro_cache_kmodels_json: config
                        .as_ref()
                        .map(|current| current.kiro_cache_kmodels_json.clone())
                        .unwrap_or_default(),
                    kiro_billable_model_multipliers_json: config
                        .as_ref()
                        .map(|current| current.kiro_billable_model_multipliers_json.clone())
                        .unwrap_or_else(|| "{}".to_string()),
                    kiro_cache_policy_json: config
                        .as_ref()
                        .map(|current| current.kiro_cache_policy_json.clone())
                        .unwrap_or_default(),
                    kiro_context_usage_min_request_tokens: config
                        .as_ref()
                        .map(|current| current.kiro_context_usage_min_request_tokens)
                        .unwrap_or(15_000),
                    kiro_compact_trigger_tokens: config
                        .as_ref()
                        .map(|current| current.kiro_compact_trigger_tokens)
                        .unwrap_or(780_000),
                    kiro_prefix_cache_mode: config
                        .as_ref()
                        .map(|current| current.kiro_prefix_cache_mode.clone())
                        .unwrap_or_else(|| "prefix_tree".to_string()),
                    kiro_prefix_cache_max_tokens: config
                        .as_ref()
                        .map(|current| current.kiro_prefix_cache_max_tokens)
                        .unwrap_or(4_000_000),
                    kiro_prefix_cache_entry_ttl_seconds: config
                        .as_ref()
                        .map(|current| current.kiro_prefix_cache_entry_ttl_seconds)
                        .unwrap_or(21_600),
                    kiro_conversation_anchor_max_entries: config
                        .as_ref()
                        .map(|current| current.kiro_conversation_anchor_max_entries)
                        .unwrap_or(20_000),
                    kiro_conversation_anchor_ttl_seconds: config
                        .as_ref()
                        .map(|current| current.kiro_conversation_anchor_ttl_seconds)
                        .unwrap_or(86_400),
                };
                saving_runtime_config.set(true);
                match update_admin_llm_gateway_config(&runtime_config).await {
                    Ok(_) => {
                        load_error.set(None);
                        reload.emit(());
                    },
                    Err(err) => load_error.set(Some(err)),
                }
                saving_runtime_config.set(false);
            });
        })
    };

    let on_create_proxy_config = {
        let create_proxy_name = create_proxy_name.clone();
        let create_proxy_url = create_proxy_url.clone();
        let create_proxy_username = create_proxy_username.clone();
        let create_proxy_password = create_proxy_password.clone();
        let creating_proxy = creating_proxy.clone();
        let proxy_config_scope = proxy_config_scope.clone();
        let load_error = load_error.clone();
        let flash = flash.clone();
        let reload = reload.clone();
        Callback::from(move |_| {
            if !proxy_config_scope.can_edit_slot_metadata {
                flash.emit(("只有 core 节点可以创建代理槽位".to_string(), true));
                return;
            }
            let input = CreateAdminUpstreamProxyConfigInput {
                name: (*create_proxy_name).trim().to_string(),
                proxy_url: (*create_proxy_url).trim().to_string(),
                proxy_username: {
                    let value = (*create_proxy_username).trim().to_string();
                    if value.is_empty() {
                        None
                    } else {
                        Some(value)
                    }
                },
                proxy_password: {
                    let value = (*create_proxy_password).trim().to_string();
                    if value.is_empty() {
                        None
                    } else {
                        Some(value)
                    }
                },
            };
            let create_proxy_name = create_proxy_name.clone();
            let create_proxy_username = create_proxy_username.clone();
            let create_proxy_password = create_proxy_password.clone();
            let creating_proxy = creating_proxy.clone();
            let load_error = load_error.clone();
            let flash = flash.clone();
            let reload = reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                creating_proxy.set(true);
                match create_admin_llm_gateway_proxy_config(&input).await {
                    Ok(_) => {
                        create_proxy_name.set(String::new());
                        create_proxy_username.set(String::new());
                        create_proxy_password.set(String::new());
                        load_error.set(None);
                        flash.emit(("已创建代理配置".to_string(), false));
                        reload.emit(());
                    },
                    Err(err) => {
                        load_error.set(Some(err.clone()));
                        flash.emit((format!("创建代理配置失败\n{err}"), true));
                    },
                }
                creating_proxy.set(false);
            });
        })
    };

    let on_save_proxy_binding = {
        let proxy_bindings = proxy_bindings.clone();
        let codex_proxy_binding_input = codex_proxy_binding_input.clone();
        let kiro_proxy_binding_input = kiro_proxy_binding_input.clone();
        let saving_proxy_binding_provider = saving_proxy_binding_provider.clone();
        let load_error = load_error.clone();
        let flash = flash.clone();
        Callback::from(move |provider_type: String| {
            let proxy_config_id = match provider_type.as_str() {
                "codex" => (*codex_proxy_binding_input).clone(),
                "kiro" => (*kiro_proxy_binding_input).clone(),
                _ => String::new(),
            };
            let proxy_bindings = proxy_bindings.clone();
            let codex_proxy_binding_input = codex_proxy_binding_input.clone();
            let kiro_proxy_binding_input = kiro_proxy_binding_input.clone();
            let saving_proxy_binding_provider = saving_proxy_binding_provider.clone();
            let load_error = load_error.clone();
            let flash = flash.clone();
            wasm_bindgen_futures::spawn_local(async move {
                saving_proxy_binding_provider.set(Some(provider_type.clone()));
                match update_admin_llm_gateway_proxy_binding(
                    &provider_type,
                    if proxy_config_id.trim().is_empty() {
                        None
                    } else {
                        Some(proxy_config_id.trim())
                    },
                )
                .await
                {
                    Ok(updated) => {
                        let mut items = (*proxy_bindings).clone();
                        if let Some(existing) = items
                            .iter_mut()
                            .find(|item| item.provider_type == updated.provider_type)
                        {
                            *existing = updated.clone();
                        } else {
                            items.push(updated.clone());
                            items.sort_by(|left, right| {
                                left.provider_type.cmp(&right.provider_type)
                            });
                        }
                        proxy_bindings.set(items);
                        let bound_value = updated.bound_proxy_config_id.clone().unwrap_or_default();
                        match provider_type.as_str() {
                            "codex" => codex_proxy_binding_input.set(bound_value),
                            "kiro" => kiro_proxy_binding_input.set(bound_value),
                            _ => {},
                        }
                        load_error.set(None);
                        flash.emit((
                            format!("已更新 {} 代理绑定", provider_type.to_uppercase()),
                            false,
                        ));
                    },
                    Err(err) => {
                        load_error.set(Some(err.clone()));
                        flash.emit((
                            format!("保存 {} 代理绑定失败\n{err}", provider_type.to_uppercase()),
                            true,
                        ));
                    },
                }
                saving_proxy_binding_provider.set(None);
            });
        })
    };

    let on_import_legacy_kiro_proxy = {
        let migrating_legacy_kiro_proxy = migrating_legacy_kiro_proxy.clone();
        let load_error = load_error.clone();
        let flash = flash.clone();
        let reload = reload.clone();
        Callback::from(move |_| {
            let migrating_legacy_kiro_proxy = migrating_legacy_kiro_proxy.clone();
            let load_error = load_error.clone();
            let flash = flash.clone();
            let reload = reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                migrating_legacy_kiro_proxy.set(true);
                match import_admin_legacy_kiro_proxy_configs().await {
                    Ok(_) => {
                        load_error.set(None);
                        flash.emit(("已导入 legacy Kiro 代理配置".to_string(), false));
                        reload.emit(());
                    },
                    Err(err) => {
                        load_error.set(Some(err.clone()));
                        flash.emit((format!("导入 legacy Kiro 代理配置失败\n{err}"), true));
                    },
                }
                migrating_legacy_kiro_proxy.set(false);
            });
        })
    };

    let on_create = {
        let create_key = create_key.clone();
        let creating = creating.clone();
        let load_error = load_error.clone();
        let flash = flash.clone();
        let reload = reload.clone();
        let usage_page = usage_page.clone();
        Callback::from(move |_| {
            let current = (*create_key).clone();
            let name = current.name.trim().to_string();
            let quota = current.quota.trim().parse::<u64>();
            let public_visible = current.public;
            let request_max_concurrency = current.request_max_concurrency.trim().to_string();
            let request_min_start_interval_ms =
                current.request_min_start_interval_ms.trim().to_string();
            let creating = creating.clone();
            let load_error = load_error.clone();
            let flash = flash.clone();
            let reload = reload.clone();
            let create_key = create_key.clone();
            let usage_page = usage_page.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let Ok(quota) = quota else {
                    let message = "主额度必须是正整数".to_string();
                    load_error.set(Some(message.clone()));
                    flash.emit((message, true));
                    return;
                };
                let request_max_concurrency = if request_max_concurrency.is_empty() {
                    None
                } else {
                    match request_max_concurrency.parse::<u64>() {
                        Ok(value) => Some(value),
                        Err(_) => {
                            let message = "并发上限必须是整数，留空表示不限制".to_string();
                            load_error.set(Some(message.clone()));
                            flash.emit((message, true));
                            return;
                        },
                    }
                };
                let request_min_start_interval_ms = if request_min_start_interval_ms.is_empty() {
                    None
                } else {
                    match request_min_start_interval_ms.parse::<u64>() {
                        Ok(value) => Some(value),
                        Err(_) => {
                            let message = "请求间隔必须是整数毫秒，留空表示不限制".to_string();
                            load_error.set(Some(message.clone()));
                            flash.emit((message, true));
                            return;
                        },
                    }
                };
                creating.set(true);
                match create_admin_llm_gateway_key(
                    &name,
                    quota,
                    public_visible,
                    request_max_concurrency,
                    request_min_start_interval_ms,
                )
                .await
                {
                    Ok(_) => {
                        // Reset the form inputs after a successful create;
                        // leave `public` / `quota` defaults as-is so the next
                        // create has the same baseline.
                        let mut next = (*create_key).clone();
                        next.name = String::new();
                        next.request_max_concurrency = String::new();
                        next.request_min_start_interval_ms = String::new();
                        create_key.set(next);
                        usage_page.set(1);
                        load_error.set(None);
                        flash.emit((format!("已创建 key `{}`", name), false));
                        reload.emit(());
                    },
                    Err(err) => {
                        load_error.set(Some(err.clone()));
                        flash.emit((format!("创建 key `{}` 失败\n{err}", name), true));
                    },
                }
                creating.set(false);
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

    let on_toggle_account_group_form = {
        let account_group_form_expanded = account_group_form_expanded.clone();
        let load_account_group_candidates = load_account_group_candidates.clone();
        Callback::from(move |_| {
            let next_expanded = !*account_group_form_expanded;
            account_group_form_expanded.set(next_expanded);
            if next_expanded {
                load_account_group_candidates.emit(());
            }
        })
    };

    let on_create_account_group = {
        let create_account_group_name = create_account_group_name.clone();
        let create_account_group_account_names = create_account_group_account_names.clone();
        let creating_account_group = creating_account_group.clone();
        let flash = flash.clone();
        let load_error = load_error.clone();
        let reload = reload.clone();
        Callback::from(move |_| {
            if *creating_account_group {
                return;
            }
            let group_name = (*create_account_group_name).trim().to_string();
            let account_names = (*create_account_group_account_names).clone();
            let create_account_group_name = create_account_group_name.clone();
            let create_account_group_account_names = create_account_group_account_names.clone();
            let creating_account_group = creating_account_group.clone();
            let flash = flash.clone();
            let load_error = load_error.clone();
            let reload = reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if group_name.is_empty() {
                    let message = "账号组名称不能为空".to_string();
                    load_error.set(Some(message.clone()));
                    flash.emit((message, true));
                    return;
                }
                if account_names.is_empty() {
                    let message = "账号组至少需要选择一个账号".to_string();
                    load_error.set(Some(message.clone()));
                    flash.emit((message, true));
                    return;
                }
                creating_account_group.set(true);
                match create_admin_llm_gateway_account_group(CreateAdminAccountGroupInput {
                    name: &group_name,
                    account_names: account_names.as_slice(),
                })
                .await
                {
                    Ok(_) => {
                        create_account_group_name.set(String::new());
                        create_account_group_account_names.set(Vec::new());
                        load_error.set(None);
                        flash.emit((format!("已创建账号组 `{group_name}`"), false));
                        reload.emit(());
                    },
                    Err(err) => {
                        load_error.set(Some(err.clone()));
                        flash.emit((format!("创建账号组失败\n{err}"), true));
                    },
                }
                creating_account_group.set(false);
            });
        })
    };

    // A per-card refresh avoids reloading unrelated state while re-reading the
    // latest counters for a single key.
    let on_refresh_key = {
        let reload = reload.clone();
        let flash = flash.clone();
        let refreshing_key_id = refreshing_key_id.clone();
        Callback::from(move |(key_id, key_name): (String, String)| {
            refreshing_key_id.set(Some(key_id.clone()));
            let reload = reload.clone();
            let flash = flash.clone();
            let refreshing_key_id = refreshing_key_id.clone();
            wasm_bindgen_futures::spawn_local(async move {
                reload.emit(());
                flash.emit((format!("已触发 key `{}` 刷新", key_name), false));
                refreshing_key_id.set(None);
            });
        })
    };

    let on_usage_key_pick = {
        let usage_key_filter = usage_key_filter.clone();
        let usage_key_search = usage_key_search.clone();
        let usage_page = usage_page.clone();
        Callback::from(move |selected_key_id: String| {
            if selected_key_id.is_empty() {
                usage_key_search.set(String::new());
            }
            usage_key_filter.set(selected_key_id.clone());
            usage_page.set(1);
        })
    };

    let on_usage_key_filter_change = {
        let on_usage_key_pick = on_usage_key_pick.clone();
        Callback::from(move |event: Event| {
            if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                on_usage_key_pick.emit(target.value());
            }
        })
    };

    let on_usage_key_search_change = {
        let usage_key_search = usage_key_search.clone();
        Callback::from(move |value: String| usage_key_search.set(value))
    };

    let on_usage_source_change = {
        let usage_source = usage_source.clone();
        Callback::from(move |event: Event| {
            if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                usage_source.set(target.value());
            }
        })
    };

    let on_usage_model_filter_input = {
        let usage_model_filter = usage_model_filter.clone();
        Callback::from(move |event: InputEvent| {
            let value = event.target_unchecked_into::<HtmlInputElement>().value();
            usage_model_filter.set(value.clone());
        })
    };

    let on_usage_account_filter_input = {
        let usage_account_filter = usage_account_filter.clone();
        Callback::from(move |event: InputEvent| {
            let value = event.target_unchecked_into::<HtmlInputElement>().value();
            usage_account_filter.set(value.clone());
        })
    };

    let on_usage_endpoint_filter_input = {
        let usage_endpoint_filter = usage_endpoint_filter.clone();
        Callback::from(move |event: InputEvent| {
            let value = event.target_unchecked_into::<HtmlInputElement>().value();
            usage_endpoint_filter.set(value.clone());
        })
    };

    let on_usage_status_kind_change = {
        let usage_status_kind = usage_status_kind.clone();
        Callback::from(move |event: Event| {
            if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                usage_status_kind.set(target.value());
            }
        })
    };

    let on_apply_usage_filters = {
        let reload_usage = reload_usage.clone();
        Callback::from(move |_| {
            reload_usage.emit(UsageReloadArgs {
                page: Some(1),
                refresh_filter_options: true,
                ..UsageReloadArgs::default()
            });
        })
    };

    let on_clear_usage_filters = {
        let usage_key_filter = usage_key_filter.clone();
        let usage_key_search = usage_key_search.clone();
        let usage_start_input = usage_start_input.clone();
        let usage_end_input = usage_end_input.clone();
        let usage_source = usage_source.clone();
        let usage_model_filter = usage_model_filter.clone();
        let usage_account_filter = usage_account_filter.clone();
        let usage_endpoint_filter = usage_endpoint_filter.clone();
        let usage_status_kind = usage_status_kind.clone();
        let reload_usage = reload_usage.clone();
        Callback::from(move |_| {
            usage_key_filter.set(String::new());
            usage_key_search.set(String::new());
            usage_start_input.set(String::new());
            usage_end_input.set(String::new());
            usage_source.set(USAGE_SOURCE_HOT.to_string());
            usage_model_filter.set(String::new());
            usage_account_filter.set(String::new());
            usage_endpoint_filter.set(String::new());
            usage_status_kind.set(USAGE_STATUS_KIND_ALL.to_string());
            reload_usage.emit(UsageReloadArgs {
                page: Some(1),
                key_id: Some(String::new()),
                start_input: Some(String::new()),
                end_input: Some(String::new()),
                source: Some(USAGE_SOURCE_HOT.to_string()),
                model: Some(String::new()),
                account_name: Some(String::new()),
                endpoint: Some(String::new()),
                status_kind: Some(String::new()),
                refresh_filter_options: true,
            });
        })
    };

    let on_usage_page_change = {
        let usage_page = usage_page.clone();
        let reload_usage = reload_usage.clone();
        Callback::from(move |page: usize| {
            usage_page.set(page);
            reload_usage.emit(UsageReloadArgs {
                page: Some(page),
                refresh_filter_options: false,
                ..UsageReloadArgs::default()
            });
        })
    };

    let on_usage_journal_preview_page_change = {
        let usage_journal_preview_page = usage_journal_preview_page.clone();
        let reload_usage_journal_status = reload_usage_journal_status.clone();
        Callback::from(move |page: usize| {
            usage_journal_preview_page.set(page);
            reload_usage_journal_status.emit(Some(page));
        })
    };


    let usage_total_pages = (*usage_total).max(1).div_ceil(USAGE_PAGE_SIZE);
    let usage_journal_preview_total_pages = (*usage_journal_preview)
        .as_ref()
        .map(|resp| resp.total.max(1).div_ceil(resp.limit.max(1)))
        .unwrap_or(1);
    let token_request_total_pages = (*token_request_total)
        .max(1)
        .div_ceil(TOKEN_REQUEST_PAGE_SIZE);
    let account_contribution_request_total_pages = (*account_contribution_request_total)
        .max(1)
        .div_ceil(ACCOUNT_CONTRIBUTION_REQUEST_PAGE_SIZE);
    let sponsor_request_total_pages = (*sponsor_request_total)
        .max(1)
        .div_ceil(SPONSOR_REQUEST_PAGE_SIZE);

    let on_token_request_status_filter_change = {
        let token_request_status_filter = token_request_status_filter.clone();
        let token_request_page = token_request_page.clone();
        let reload_token_requests = reload_token_requests.clone();
        Callback::from(move |event: Event| {
            if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                let status = target.value();
                token_request_status_filter.set(status.clone());
                token_request_page.set(1);
                reload_token_requests.emit((Some(1), Some(status)));
            }
        })
    };

    let on_token_request_page_change = {
        let token_request_page = token_request_page.clone();
        let reload_token_requests = reload_token_requests.clone();
        Callback::from(move |page: usize| {
            token_request_page.set(page);
            reload_token_requests.emit((Some(page), None));
        })
    };

    let on_approve_token_request = {
        let token_request_action_inflight = token_request_action_inflight.clone();
        let token_requests = token_requests.clone();
        let reload = reload.clone();
        let reload_token_requests = reload_token_requests.clone();
        let load_error = load_error.clone();
        Callback::from(move |request_id: String| {
            let token_request_action_inflight = token_request_action_inflight.clone();
            let token_requests = token_requests.clone();
            let reload = reload.clone();
            let reload_token_requests = reload_token_requests.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let mut inflight = (*token_request_action_inflight).clone();
                inflight.insert(request_id.clone());
                token_request_action_inflight.set(inflight);

                match admin_approve_and_issue_llm_gateway_token_request(&request_id, None).await {
                    Ok(updated) => {
                        let mut list = (*token_requests).clone();
                        if let Some(item) = list
                            .iter_mut()
                            .find(|item| item.request_id == updated.request_id)
                        {
                            *item = updated;
                        }
                        token_requests.set(list);
                        load_error.set(None);
                        reload.emit(());
                        reload_token_requests.emit((None, None));
                    },
                    Err(err) => load_error.set(Some(err)),
                }

                let mut inflight = (*token_request_action_inflight).clone();
                inflight.remove(&request_id);
                token_request_action_inflight.set(inflight);
            });
        })
    };

    let on_reject_token_request = {
        let token_request_action_inflight = token_request_action_inflight.clone();
        let token_requests = token_requests.clone();
        let reload_token_requests = reload_token_requests.clone();
        let load_error = load_error.clone();
        Callback::from(move |request_id: String| {
            let token_request_action_inflight = token_request_action_inflight.clone();
            let token_requests = token_requests.clone();
            let reload_token_requests = reload_token_requests.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let mut inflight = (*token_request_action_inflight).clone();
                inflight.insert(request_id.clone());
                token_request_action_inflight.set(inflight);

                match admin_reject_llm_gateway_token_request(&request_id, None).await {
                    Ok(updated) => {
                        let mut list = (*token_requests).clone();
                        if let Some(item) = list
                            .iter_mut()
                            .find(|item| item.request_id == updated.request_id)
                        {
                            *item = updated;
                        }
                        token_requests.set(list);
                        load_error.set(None);
                        reload_token_requests.emit((None, None));
                    },
                    Err(err) => load_error.set(Some(err)),
                }

                let mut inflight = (*token_request_action_inflight).clone();
                inflight.remove(&request_id);
                token_request_action_inflight.set(inflight);
            });
        })
    };

    let on_account_contribution_status_filter_change = {
        let account_contribution_request_status_filter =
            account_contribution_request_status_filter.clone();
        let account_contribution_request_page = account_contribution_request_page.clone();
        let reload_account_contribution_requests = reload_account_contribution_requests.clone();
        Callback::from(move |event: Event| {
            if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                let status = target.value();
                account_contribution_request_status_filter.set(status.clone());
                account_contribution_request_page.set(1);
                reload_account_contribution_requests.emit((Some(1), Some(status)));
            }
        })
    };

    let on_account_contribution_page_change = {
        let account_contribution_request_page = account_contribution_request_page.clone();
        let reload_account_contribution_requests = reload_account_contribution_requests.clone();
        Callback::from(move |page: usize| {
            account_contribution_request_page.set(page);
            reload_account_contribution_requests.emit((Some(page), None));
        })
    };

    let on_validate_account_contribution_request = {
        let account_contribution_request_action_inflight =
            account_contribution_request_action_inflight.clone();
        let account_contribution_requests = account_contribution_requests.clone();
        let reload_account_contribution_requests = reload_account_contribution_requests.clone();
        let load_error = load_error.clone();
        Callback::from(move |request_id: String| {
            let account_contribution_request_action_inflight =
                account_contribution_request_action_inflight.clone();
            let account_contribution_requests = account_contribution_requests.clone();
            let reload_account_contribution_requests = reload_account_contribution_requests.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let mut inflight = (*account_contribution_request_action_inflight).clone();
                inflight.insert(request_id.clone());
                account_contribution_request_action_inflight.set(inflight);

                match admin_validate_llm_gateway_account_contribution_request(&request_id, None)
                    .await
                {
                    Ok(updated) => {
                        let mut list = (*account_contribution_requests).clone();
                        if let Some(item) = list
                            .iter_mut()
                            .find(|item| item.request_id == updated.request_id)
                        {
                            *item = updated;
                        }
                        account_contribution_requests.set(list);
                        load_error.set(None);
                        reload_account_contribution_requests.emit((None, None));
                    },
                    Err(err) => load_error.set(Some(err)),
                }

                let mut inflight = (*account_contribution_request_action_inflight).clone();
                inflight.remove(&request_id);
                account_contribution_request_action_inflight.set(inflight);
            });
        })
    };

    let on_approve_account_contribution_request = {
        let account_contribution_request_action_inflight =
            account_contribution_request_action_inflight.clone();
        let account_contribution_requests = account_contribution_requests.clone();
        let reload = reload.clone();
        let reload_account_contribution_requests = reload_account_contribution_requests.clone();
        let load_error = load_error.clone();
        Callback::from(move |request_id: String| {
            let account_contribution_request_action_inflight =
                account_contribution_request_action_inflight.clone();
            let account_contribution_requests = account_contribution_requests.clone();
            let reload = reload.clone();
            let reload_account_contribution_requests = reload_account_contribution_requests.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let mut inflight = (*account_contribution_request_action_inflight).clone();
                inflight.insert(request_id.clone());
                account_contribution_request_action_inflight.set(inflight);

                match admin_approve_and_issue_llm_gateway_account_contribution_request(
                    &request_id,
                    None,
                )
                .await
                {
                    Ok(updated) => {
                        let mut list = (*account_contribution_requests).clone();
                        if let Some(item) = list
                            .iter_mut()
                            .find(|item| item.request_id == updated.request_id)
                        {
                            *item = updated;
                        }
                        account_contribution_requests.set(list);
                        load_error.set(None);
                        reload.emit(());
                        reload_account_contribution_requests.emit((None, None));
                    },
                    Err(err) => load_error.set(Some(err)),
                }

                let mut inflight = (*account_contribution_request_action_inflight).clone();
                inflight.remove(&request_id);
                account_contribution_request_action_inflight.set(inflight);
            });
        })
    };

    let on_reject_account_contribution_request = {
        let account_contribution_request_action_inflight =
            account_contribution_request_action_inflight.clone();
        let account_contribution_requests = account_contribution_requests.clone();
        let reload_account_contribution_requests = reload_account_contribution_requests.clone();
        let load_error = load_error.clone();
        Callback::from(move |request_id: String| {
            let account_contribution_request_action_inflight =
                account_contribution_request_action_inflight.clone();
            let account_contribution_requests = account_contribution_requests.clone();
            let reload_account_contribution_requests = reload_account_contribution_requests.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let mut inflight = (*account_contribution_request_action_inflight).clone();
                inflight.insert(request_id.clone());
                account_contribution_request_action_inflight.set(inflight);

                match admin_reject_llm_gateway_account_contribution_request(&request_id, None).await
                {
                    Ok(updated) => {
                        let mut list = (*account_contribution_requests).clone();
                        if let Some(item) = list
                            .iter_mut()
                            .find(|item| item.request_id == updated.request_id)
                        {
                            *item = updated;
                        }
                        account_contribution_requests.set(list);
                        load_error.set(None);
                        reload_account_contribution_requests.emit((None, None));
                    },
                    Err(err) => load_error.set(Some(err)),
                }

                let mut inflight = (*account_contribution_request_action_inflight).clone();
                inflight.remove(&request_id);
                account_contribution_request_action_inflight.set(inflight);
            });
        })
    };

    let on_sponsor_request_status_filter_change = {
        let sponsor_request_status_filter = sponsor_request_status_filter.clone();
        let sponsor_request_page = sponsor_request_page.clone();
        let reload_sponsor_requests = reload_sponsor_requests.clone();
        Callback::from(move |event: Event| {
            if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                let status = target.value();
                sponsor_request_status_filter.set(status.clone());
                sponsor_request_page.set(1);
                reload_sponsor_requests.emit((Some(1), Some(status)));
            }
        })
    };

    let on_sponsor_request_page_change = {
        let sponsor_request_page = sponsor_request_page.clone();
        let reload_sponsor_requests = reload_sponsor_requests.clone();
        Callback::from(move |page: usize| {
            sponsor_request_page.set(page);
            reload_sponsor_requests.emit((Some(page), None));
        })
    };

    let on_approve_sponsor_request = {
        let sponsor_request_action_inflight = sponsor_request_action_inflight.clone();
        let sponsor_requests = sponsor_requests.clone();
        let reload_sponsor_requests = reload_sponsor_requests.clone();
        let load_error = load_error.clone();
        Callback::from(move |request_id: String| {
            let sponsor_request_action_inflight = sponsor_request_action_inflight.clone();
            let sponsor_requests = sponsor_requests.clone();
            let reload_sponsor_requests = reload_sponsor_requests.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let mut inflight = (*sponsor_request_action_inflight).clone();
                inflight.insert(request_id.clone());
                sponsor_request_action_inflight.set(inflight);

                match admin_approve_llm_gateway_sponsor_request(&request_id, None).await {
                    Ok(updated) => {
                        let mut list = (*sponsor_requests).clone();
                        if let Some(item) = list
                            .iter_mut()
                            .find(|item| item.request_id == updated.request_id)
                        {
                            *item = updated;
                        }
                        sponsor_requests.set(list);
                        load_error.set(None);
                        reload_sponsor_requests.emit((None, None));
                    },
                    Err(err) => load_error.set(Some(err)),
                }

                let mut inflight = (*sponsor_request_action_inflight).clone();
                inflight.remove(&request_id);
                sponsor_request_action_inflight.set(inflight);
            });
        })
    };

    let on_delete_sponsor_request = {
        let sponsor_request_action_inflight = sponsor_request_action_inflight.clone();
        let sponsor_requests = sponsor_requests.clone();
        let sponsor_request_total = sponsor_request_total.clone();
        let reload_sponsor_requests = reload_sponsor_requests.clone();
        let load_error = load_error.clone();
        Callback::from(move |request_id: String| {
            if !confirm_destructive("确认删除这条 Sponsor 请求？") {
                return;
            }

            let sponsor_request_action_inflight = sponsor_request_action_inflight.clone();
            let sponsor_requests = sponsor_requests.clone();
            let sponsor_request_total = sponsor_request_total.clone();
            let reload_sponsor_requests = reload_sponsor_requests.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let mut inflight = (*sponsor_request_action_inflight).clone();
                inflight.insert(request_id.clone());
                sponsor_request_action_inflight.set(inflight);

                match delete_admin_llm_gateway_sponsor_request(&request_id).await {
                    Ok(_) => {
                        let filtered = (*sponsor_requests)
                            .iter()
                            .filter(|item| item.request_id != request_id)
                            .cloned()
                            .collect::<Vec<_>>();
                        sponsor_requests.set(filtered);
                        sponsor_request_total.set((*sponsor_request_total).saturating_sub(1));
                        load_error.set(None);
                        reload_sponsor_requests.emit((None, None));
                    },
                    Err(err) => load_error.set(Some(err)),
                }

                let mut inflight = (*sponsor_request_action_inflight).clone();
                inflight.remove(&request_id);
                sponsor_request_action_inflight.set(inflight);
            });
        })
    };

    let on_toggle_account_spark_mapping = {
        let account_action_inflight = account_action_inflight.clone();
        let accounts = accounts.clone();
        let load_error = load_error.clone();
        Callback::from(move |(account_name, enabled): (String, bool)| {
            let account_action_inflight = account_action_inflight.clone();
            let accounts = accounts.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let mut inflight = (*account_action_inflight).clone();
                inflight.insert(account_name.clone());
                account_action_inflight.set(inflight);

                match patch_admin_llm_gateway_account(
                    &account_name,
                    &PatchAdminLlmGatewayAccountInput {
                        status: None,
                        map_gpt53_codex_to_spark: Some(enabled),
                        auto_refresh_enabled: None,
                        route_weight_tier: None,
                        proxy_mode: None,
                        proxy_config_id: None,
                        request_max_concurrency: None,
                        request_min_start_interval_ms: None,
                        request_max_concurrency_unlimited: false,
                        request_min_start_interval_ms_unlimited: false,
                    },
                )
                .await
                {
                    Ok(updated) => {
                        let mut items = (*accounts).clone();
                        if let Some(item) = items.iter_mut().find(|item| item.name == updated.name)
                        {
                            *item = updated;
                        }
                        accounts.set(items);
                        load_error.set(None);
                    },
                    Err(err) => load_error.set(Some(err)),
                }

                let mut inflight = (*account_action_inflight).clone();
                inflight.remove(&account_name);
                account_action_inflight.set(inflight);
            });
        })
    };

    let on_toggle_account_auto_refresh = {
        let account_action_inflight = account_action_inflight.clone();
        let accounts = accounts.clone();
        let load_error = load_error.clone();
        Callback::from(move |(account_name, enabled): (String, bool)| {
            let account_action_inflight = account_action_inflight.clone();
            let accounts = accounts.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let mut inflight = (*account_action_inflight).clone();
                inflight.insert(account_name.clone());
                account_action_inflight.set(inflight);

                match patch_admin_llm_gateway_account(
                    &account_name,
                    &PatchAdminLlmGatewayAccountInput {
                        status: None,
                        map_gpt53_codex_to_spark: None,
                        auto_refresh_enabled: Some(enabled),
                        route_weight_tier: None,
                        proxy_mode: None,
                        proxy_config_id: None,
                        request_max_concurrency: None,
                        request_min_start_interval_ms: None,
                        request_max_concurrency_unlimited: false,
                        request_min_start_interval_ms_unlimited: false,
                    },
                )
                .await
                {
                    Ok(updated) => {
                        let mut items = (*accounts).clone();
                        if let Some(item) = items.iter_mut().find(|item| item.name == updated.name)
                        {
                            *item = updated;
                        }
                        accounts.set(items);
                        load_error.set(None);
                    },
                    Err(err) => load_error.set(Some(err)),
                }

                let mut inflight = (*account_action_inflight).clone();
                inflight.remove(&account_name);
                account_action_inflight.set(inflight);
            });
        })
    };

    let on_toggle_account_status = {
        let account_action_inflight = account_action_inflight.clone();
        let accounts = accounts.clone();
        let load_error = load_error.clone();
        Callback::from(move |(account_name, status): (String, String)| {
            let account_action_inflight = account_action_inflight.clone();
            let accounts = accounts.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let mut inflight = (*account_action_inflight).clone();
                inflight.insert(account_name.clone());
                account_action_inflight.set(inflight);

                match patch_admin_llm_gateway_account(
                    &account_name,
                    &PatchAdminLlmGatewayAccountInput {
                        status: Some(status),
                        map_gpt53_codex_to_spark: None,
                        auto_refresh_enabled: None,
                        route_weight_tier: None,
                        proxy_mode: None,
                        proxy_config_id: None,
                        request_max_concurrency: None,
                        request_min_start_interval_ms: None,
                        request_max_concurrency_unlimited: false,
                        request_min_start_interval_ms_unlimited: false,
                    },
                )
                .await
                {
                    Ok(updated) => {
                        let mut items = (*accounts).clone();
                        if let Some(item) = items.iter_mut().find(|item| item.name == updated.name)
                        {
                            *item = updated;
                        }
                        accounts.set(items);
                        load_error.set(None);
                    },
                    Err(err) => load_error.set(Some(err)),
                }

                let mut inflight = (*account_action_inflight).clone();
                inflight.remove(&account_name);
                account_action_inflight.set(inflight);
            });
        })
    };

    let on_save_account_settings = {
        let account_action_inflight = account_action_inflight.clone();
        let account_proxy_inputs = account_proxy_inputs.clone();
        let account_route_weight_tier_inputs = account_route_weight_tier_inputs.clone();
        let account_request_max_inputs = account_request_max_inputs.clone();
        let account_request_min_inputs = account_request_min_inputs.clone();
        let accounts = accounts.clone();
        let load_error = load_error.clone();
        Callback::from(move |account_name: String| {
            let account_action_inflight = account_action_inflight.clone();
            let account_proxy_inputs = account_proxy_inputs.clone();
            let account_route_weight_tier_inputs = account_route_weight_tier_inputs.clone();
            let account_request_max_inputs = account_request_max_inputs.clone();
            let account_request_min_inputs = account_request_min_inputs.clone();
            let accounts = accounts.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let selection = (*account_proxy_inputs)
                    .get(&account_name)
                    .cloned()
                    .unwrap_or_else(|| "inherit".to_string());
                let route_weight_tier = (*account_route_weight_tier_inputs)
                    .get(&account_name)
                    .cloned()
                    .unwrap_or_else(|| "auto".to_string());
                let request_max_raw = (*account_request_max_inputs)
                    .get(&account_name)
                    .cloned()
                    .unwrap_or_default();
                let request_min_raw = (*account_request_min_inputs)
                    .get(&account_name)
                    .cloned()
                    .unwrap_or_default();
                let (proxy_mode, proxy_config_id) = if selection == "direct" {
                    (Some("direct".to_string()), None)
                } else if let Some(proxy_config_id) = selection.strip_prefix("fixed:") {
                    (Some("fixed".to_string()), Some(proxy_config_id.to_string()))
                } else {
                    (Some("inherit".to_string()), None)
                };
                let request_max_concurrency = if request_max_raw.trim().is_empty() {
                    None
                } else {
                    match request_max_raw.trim().parse::<u64>() {
                        Ok(value) => Some(value),
                        Err(_) => {
                            load_error
                                .set(Some("账号并发上限必须是整数，留空表示不限制".to_string()));
                            return;
                        },
                    }
                };
                let request_min_start_interval_ms = if request_min_raw.trim().is_empty() {
                    None
                } else {
                    match request_min_raw.trim().parse::<u64>() {
                        Ok(value) => Some(value),
                        Err(_) => {
                            load_error.set(Some(
                                "账号请求起始间隔必须是整数毫秒，留空表示不限制".to_string(),
                            ));
                            return;
                        },
                    }
                };

                let mut inflight = (*account_action_inflight).clone();
                inflight.insert(account_name.clone());
                account_action_inflight.set(inflight);

                match patch_admin_llm_gateway_account(
                    &account_name,
                    &PatchAdminLlmGatewayAccountInput {
                        status: None,
                        map_gpt53_codex_to_spark: None,
                        auto_refresh_enabled: None,
                        route_weight_tier: Some(route_weight_tier),
                        proxy_mode,
                        proxy_config_id,
                        request_max_concurrency,
                        request_min_start_interval_ms,
                        request_max_concurrency_unlimited: request_max_concurrency.is_none(),
                        request_min_start_interval_ms_unlimited: request_min_start_interval_ms
                            .is_none(),
                    },
                )
                .await
                {
                    Ok(updated) => {
                        let mut items = (*accounts).clone();
                        if let Some(item) = items.iter_mut().find(|item| item.name == updated.name)
                        {
                            *item = updated.clone();
                        }
                        accounts.set(items);

                        let mut next_inputs = (*account_proxy_inputs).clone();
                        next_inputs
                            .insert(updated.name.clone(), account_proxy_select_value(&updated));
                        account_proxy_inputs.set(next_inputs);
                        let mut next_route_weight_tier_inputs =
                            (*account_route_weight_tier_inputs).clone();
                        next_route_weight_tier_inputs
                            .insert(updated.name.clone(), updated.route_weight_tier.clone());
                        account_route_weight_tier_inputs.set(next_route_weight_tier_inputs);
                        let mut next_request_max_inputs = (*account_request_max_inputs).clone();
                        next_request_max_inputs.insert(
                            updated.name.clone(),
                            updated
                                .request_max_concurrency
                                .map(|value| value.to_string())
                                .unwrap_or_default(),
                        );
                        account_request_max_inputs.set(next_request_max_inputs);
                        let mut next_request_min_inputs = (*account_request_min_inputs).clone();
                        next_request_min_inputs.insert(
                            updated.name.clone(),
                            updated
                                .request_min_start_interval_ms
                                .map(|value| value.to_string())
                                .unwrap_or_default(),
                        );
                        account_request_min_inputs.set(next_request_min_inputs);
                        load_error.set(None);
                    },
                    Err(err) => load_error.set(Some(err)),
                }

                let mut inflight = (*account_action_inflight).clone();
                inflight.remove(&account_name);
                account_action_inflight.set(inflight);
            });
        })
    };

    let on_refresh_account_auth = {
        let account_action_inflight = account_action_inflight.clone();
        let account_proxy_inputs = account_proxy_inputs.clone();
        let accounts = accounts.clone();
        let flash = flash.clone();
        let load_error = load_error.clone();
        Callback::from(move |account_name: String| {
            let account_action_inflight = account_action_inflight.clone();
            let account_proxy_inputs = account_proxy_inputs.clone();
            let accounts = accounts.clone();
            let flash = flash.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let mut inflight = (*account_action_inflight).clone();
                inflight.insert(account_name.clone());
                account_action_inflight.set(inflight);

                match refresh_admin_llm_gateway_account_auth(&account_name).await {
                    Ok(updated) => {
                        let mut items = (*accounts).clone();
                        if let Some(item) = items.iter_mut().find(|item| item.name == updated.name)
                        {
                            *item = updated.clone();
                        }
                        accounts.set(items);

                        let mut next_inputs = (*account_proxy_inputs).clone();
                        next_inputs
                            .insert(updated.name.clone(), account_proxy_select_value(&updated));
                        account_proxy_inputs.set(next_inputs);
                        load_error.set(None);
                        flash.emit((format!("已刷新账号 `{}` 的 token", updated.name), false));
                    },
                    Err(err) => {
                        load_error.set(Some(err.clone()));
                        flash.emit((
                            format!("刷新账号 `{}` 的 token 失败\n{err}", account_name),
                            true,
                        ));
                    },
                }

                let mut inflight = (*account_action_inflight).clone();
                inflight.remove(&account_name);
                account_action_inflight.set(inflight);
            });
        })
    };

    let on_refresh_account_usage = {
        let account_action_inflight = account_action_inflight.clone();
        let account_proxy_inputs = account_proxy_inputs.clone();
        let accounts = accounts.clone();
        let flash = flash.clone();
        let load_error = load_error.clone();
        Callback::from(move |account_name: String| {
            let account_action_inflight = account_action_inflight.clone();
            let account_proxy_inputs = account_proxy_inputs.clone();
            let accounts = accounts.clone();
            let flash = flash.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let mut inflight = (*account_action_inflight).clone();
                inflight.insert(account_name.clone());
                account_action_inflight.set(inflight);

                match refresh_admin_llm_gateway_account_usage(&account_name).await {
                    Ok(updated) => {
                        let mut items = (*accounts).clone();
                        if let Some(item) = items.iter_mut().find(|item| item.name == updated.name)
                        {
                            *item = updated.clone();
                        }
                        accounts.set(items);

                        let mut next_inputs = (*account_proxy_inputs).clone();
                        next_inputs
                            .insert(updated.name.clone(), account_proxy_select_value(&updated));
                        account_proxy_inputs.set(next_inputs);
                        load_error.set(None);
                        flash.emit((format!("已刷新账号 `{}` 的 usage", updated.name), false));
                    },
                    Err(err) => {
                        load_error.set(Some(err.clone()));
                        flash.emit((
                            format!("刷新账号 `{}` 的 usage 失败\n{err}", account_name),
                            true,
                        ));
                    },
                }

                let mut inflight = (*account_action_inflight).clone();
                inflight.remove(&account_name);
                account_action_inflight.set(inflight);
            });
        })
    };

    let on_probe_account_models = {
        let account_action_inflight = account_action_inflight.clone();
        let flash = flash.clone();
        let load_error = load_error.clone();
        Callback::from(move |account_name: String| {
            let account_action_inflight = account_action_inflight.clone();
            let flash = flash.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let mut inflight = (*account_action_inflight).clone();
                inflight.insert(account_name.clone());
                account_action_inflight.set(inflight);

                match probe_admin_llm_gateway_account_models(&account_name).await {
                    Ok(result) => {
                        load_error.set(None);
                        flash.emit((format!("账号 `{}` {}", account_name, result.message), false));
                    },
                    Err(err) => {
                        load_error.set(Some(err.clone()));
                        flash.emit((
                            format!("检查账号 `{}` 的 models 失败\n{err}", account_name),
                            true,
                        ));
                    },
                }

                let mut inflight = (*account_action_inflight).clone();
                inflight.remove(&account_name);
                account_action_inflight.set(inflight);
            });
        })
    };

    let on_copy = {
        let flash = flash.clone();
        Callback::from(move |(label, value): (String, String)| {
            copy_text(&value);
            flash.emit((format!("已复制{}", label), false));
        })
    };

    let on_import_account = {
        let import_name = import_name.clone();
        let import_id_token = import_id_token.clone();
        let import_access_token = import_access_token.clone();
        let import_refresh_token = import_refresh_token.clone();
        let import_account_id = import_account_id.clone();
        let import_raw_auth_json = import_raw_auth_json.clone();
        let import_raw_auth_feedback = import_raw_auth_feedback.clone();
        let importing = importing.clone();
        let load_error = load_error.clone();
        let reload = reload.clone();
        Callback::from(move |_| {
            let name = (*import_name).trim().to_string();
            let id_token = (*import_id_token).trim().to_string();
            let access_token = (*import_access_token).trim().to_string();
            let refresh_token = (*import_refresh_token).trim().to_string();
            let raw_auth_json = (*import_raw_auth_json).trim().to_string();
            let account_id = {
                let v = (*import_account_id).trim().to_string();
                if v.is_empty() {
                    None
                } else {
                    Some(v)
                }
            };
            let importing = importing.clone();
            let load_error = load_error.clone();
            let reload = reload.clone();
            let import_name = import_name.clone();
            let import_id_token = import_id_token.clone();
            let import_access_token = import_access_token.clone();
            let import_refresh_token = import_refresh_token.clone();
            let import_account_id = import_account_id.clone();
            let import_raw_auth_json = import_raw_auth_json.clone();
            let import_raw_auth_feedback = import_raw_auth_feedback.clone();
            wasm_bindgen_futures::spawn_local(async move {
                importing.set(true);
                let raw_auth_json_ref =
                    (!raw_auth_json.is_empty()).then_some(raw_auth_json.as_str());
                match import_admin_llm_gateway_account(
                    &name,
                    &id_token,
                    &access_token,
                    &refresh_token,
                    account_id.as_deref(),
                    raw_auth_json_ref,
                )
                .await
                {
                    Ok(_) => {
                        import_name.set(String::new());
                        import_id_token.set(String::new());
                        import_access_token.set(String::new());
                        import_refresh_token.set(String::new());
                        import_account_id.set(String::new());
                        import_raw_auth_json.set(String::new());
                        import_raw_auth_feedback.set(None);
                        load_error.set(None);
                        reload.emit(());
                    },
                    Err(err) => load_error.set(Some(err)),
                }
                importing.set(false);
            });
        })
    };

    let on_import_account_batch = {
        let batch_import_raw_json = batch_import_raw_json.clone();
        let batch_import_feedback = batch_import_feedback.clone();
        let batch_import_validate_before_import = batch_import_validate_before_import.clone();
        let batch_importing = batch_importing.clone();
        let recent_import_jobs = recent_import_jobs.clone();
        let active_import_job = active_import_job.clone();
        let load_error = load_error.clone();
        Callback::from(move |_| {
            let raw_json = (*batch_import_raw_json).trim().to_string();
            let items = match parse_admin_codex_batch_import_json(&raw_json) {
                Ok(items) => items,
                Err(err) => {
                    batch_import_feedback.set(Some((err, true)));
                    return;
                },
            };
            let validate_before_import = *batch_import_validate_before_import;
            let batch_import_raw_json = batch_import_raw_json.clone();
            let batch_import_feedback = batch_import_feedback.clone();
            let batch_importing = batch_importing.clone();
            let recent_import_jobs = recent_import_jobs.clone();
            let active_import_job = active_import_job.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                batch_importing.set(true);
                batch_import_feedback.set(None);
                match create_admin_llm_gateway_account_import_job(validate_before_import, &items)
                    .await
                {
                    Ok(detail) => {
                        let summary = detail.summary.clone();
                        let next_jobs =
                            upsert_codex_import_job_summary(&recent_import_jobs, summary.clone());
                        active_import_job.set(Some(detail));
                        recent_import_jobs.set(next_jobs);
                        batch_import_raw_json.set(String::new());
                        batch_import_feedback
                            .set(Some((format!("已创建批量导入作业 {}", summary.job_id), false)));
                        load_error.set(None);
                    },
                    Err(err) => {
                        batch_import_feedback.set(Some((err.clone(), true)));
                        load_error.set(Some(err));
                    },
                }
                batch_importing.set(false);
            });
        })
    };

    let on_load_import_job = {
        let active_import_job = active_import_job.clone();
        let load_error = load_error.clone();
        Callback::from(move |job_id: String| {
            let active_import_job = active_import_job.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_admin_llm_gateway_account_import_job(&job_id).await {
                    Ok(detail) => {
                        active_import_job.set(Some(detail));
                        load_error.set(None);
                    },
                    Err(err) => load_error.set(Some(err)),
                }
            });
        })
    };

    let on_delete_account = {
        let reload = reload.clone();
        let load_error = load_error.clone();
        Callback::from(move |name: String| {
            if !confirm_destructive(&format!("确认删除账号 {} ？", name)) {
                return;
            }
            let reload = reload.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match delete_admin_llm_gateway_account(&name).await {
                    Ok(_) => reload.emit(()),
                    Err(err) => load_error.set(Some(err)),
                }
            });
        })
    };

    let key_summary = *keys_summary;
    let account_summary = *accounts_summary;
    let total_remaining = key_summary.remaining_billable_sum;
    let public_visible_count = key_summary.public_visible_count;
    let active_key_count = key_summary.active_count;
    let total_quota = key_summary.quota_billable_limit_sum;
    let total_used = key_summary.usage_billable_tokens_sum;
    let credit_keys_present =
        key_summary.usage_credit_total > 0.0 || key_summary.usage_credit_missing_events > 0;
    let total_credit_used = key_summary.usage_credit_total;
    let total_credit_missing_events = key_summary.usage_credit_missing_events;
    // Derive usage percentage from quota and remaining (billable-token basis).
    let usage_percent = if total_quota > 0 {
        let used = total_quota as f64 - (total_remaining.max(0) as f64);
        (used / total_quota as f64 * 100.0)
            .clamp(0.0, 100.0)
            .round() as u64
    } else {
        0
    };
    let pending_token_requests = token_requests
        .iter()
        .filter(|r| r.status == "pending")
        .count();
    let pending_contribution_requests = account_contribution_requests
        .iter()
        .filter(|r| r.status == "pending" || r.status == "failed" || r.status == "validated")
        .count();
    let pending_sponsor_requests = sponsor_requests
        .iter()
        .filter(|r| r.status == "submitted" || r.status == "payment_email_sent")
        .count();
    let total_pending =
        pending_token_requests + pending_contribution_requests + pending_sponsor_requests;
    let usage_journal_message_modal = (*selected_usage_journal_message)
        .clone()
        .map(|(event_id, created_at, key_name, full_message)| {
            html! {
                <div
                    class={classes!(
                        "fixed",
                        "inset-0",
                        "z-[90]",
                        "flex",
                        "items-start",
                        "sm:items-center",
                        "justify-center",
                        "overflow-y-auto",
                        "bg-slate-950/58",
                        "backdrop-blur-sm",
                        "px-4",
                        "py-8"
                    )}
                    onclick={{
                        let selected_usage_journal_message = selected_usage_journal_message.clone();
                        Callback::from(move |_| selected_usage_journal_message.set(None))
                    }}
                >
                    <div
                        class={classes!(
                            "w-full",
                            "mx-auto",
                            "flex",
                            "max-h-[88vh]",
                            "max-w-3xl",
                            "flex-col",
                            "overflow-y-auto",
                            "rounded-xl",
                            "border",
                            "border-[var(--border)]",
                            "bg-[var(--surface)]",
                            "p-5",
                            "shadow-[0_16px_48px_rgba(0,0,0,0.2)]"
                        )}
                        onclick={Callback::from(|event: MouseEvent| event.stop_propagation())}
                    >
                        <div class={classes!("flex", "items-start", "justify-between", "gap-4", "flex-wrap", "shrink-0")}>
                            <div class={classes!("max-w-2xl")}>
                                <p class={classes!("m-0", "text-xs", "uppercase", "tracking-[0.18em]", "text-[var(--muted)]")}>{ "Journal Last Message" }</p>
                                <h2 class={classes!("mt-3", "text-xl", "font-black", "tracking-[-0.03em]")}>{ key_name.clone() }</h2>
                                <p class={classes!("mt-2", "m-0", "break-all", "text-sm", "leading-6", "text-[var(--muted)]")}>
                                    { format!("{created_at} · {event_id}") }
                                </p>
                            </div>
                            <div class={classes!("flex", "gap-2", "flex-wrap")}>
                                <button
                                    class={classes!("btn-terminal")}
                                    onclick={{
                                        let on_copy = on_copy.clone();
                                        let full_message = full_message.clone();
                                        Callback::from(move |_| on_copy.emit(("Journal Last Message".to_string(), full_message.clone())))
                                    }}
                                >
                                    { "复制全文" }
                                </button>
                                <button
                                    class={classes!("btn-terminal", "btn-terminal-primary")}
                                    onclick={{
                                        let selected_usage_journal_message = selected_usage_journal_message.clone();
                                        Callback::from(move |_| selected_usage_journal_message.set(None))
                                    }}
                                >
                                    { "关闭" }
                                </button>
                            </div>
                        </div>

                        <div class={classes!("mt-4")}>
                            <pre class={classes!(
                                "max-h-[62vh]",
                                "overflow-x-auto",
                                "overflow-y-auto",
                                "rounded-lg",
                                "bg-slate-950",
                                "p-3",
                                "text-xs",
                                "leading-6",
                                "text-amber-100",
                                "whitespace-pre-wrap",
                                "break-words"
                            )}>
                                { full_message }
                            </pre>
                        </div>
                    </div>
                </div>
            }
        });

    // Build the full-screen modal for a selected usage event (request detail,
    // headers, last message, copy buttons). Rendered outside the tab flow so
    // it overlays the entire viewport.
    let usage_detail_modal = if *usage_detail_loading {
        Some(html! {
            <div class={classes!(
                "fixed",
                "inset-0",
                "z-[90]",
                "flex",
                "items-center",
                "justify-center",
                "bg-slate-950/58",
                "backdrop-blur-sm",
                "px-4",
                "py-8"
            )}>
                <div class={classes!(
                    "rounded-xl",
                    "border",
                    "border-[var(--border)]",
                    "bg-[var(--surface)]",
                    "px-5",
                    "py-4",
                    "text-sm",
                    "text-[var(--muted)]",
                    "shadow-[0_16px_48px_rgba(0,0,0,0.2)]"
                )}>
                    { "正在加载请求详情..." }
                </div>
            </div>
        })
    } else {
        (*selected_usage_event).clone().map(|event| {
        let account_label =
            usage_account_label(&event.account_name, &event.request_url, &event.endpoint);
        let detail_routing_wait_ms = effective_routing_wait_ms(
            event.routing_wait_ms,
            event.routing_diagnostics_json.as_deref(),
        );
        let stream_summary = format_stream_summary(
            event.stream_completed_cleanly,
            event.downstream_disconnect,
            event.final_event_type.as_deref(),
            event.bytes_streamed,
        );
        let request_detail_summary = format!(
            "{} {} · {} / {} · key {} · account {} · status {} · model {} · route {} · latency {} · stream {}",
            event.request_method,
            event.request_url,
            event.client_ip,
            event.ip_region,
            event.key_name,
            account_label,
            event.status_code,
            event.model.clone().unwrap_or_else(|| "-".to_string()),
            event.endpoint,
            format_latency_breakdown(LatencyBreakdown {
                latency_ms: event.latency_ms,
                routing_wait_ms: detail_routing_wait_ms,
                upstream_headers_ms: event.upstream_headers_ms,
                post_headers_body_ms: event.post_headers_body_ms,
                request_body_bytes: event.request_body_bytes,
                request_body_read_ms: event.request_body_read_ms,
                request_json_parse_ms: event.request_json_parse_ms,
                pre_handler_ms: event.pre_handler_ms,
                first_sse_write_ms: event.first_sse_write_ms,
                stream_finish_ms: event.stream_finish_ms,
                other_latency_ms: event.other_latency_ms,
                quota_failover_count: event.quota_failover_count,
            }),
            stream_summary,
        );
        let last_message_for_copy = event
            .last_message_content
            .clone()
            .unwrap_or_else(|| "-".to_string());
        let headers_json_for_copy = pretty_headers_json(&event.request_headers_json);
        let routing_diagnostics_for_copy = event
            .routing_diagnostics_json
            .as_deref()
            .map(pretty_json_text);
        let routing_diagnostics_summary_rows = event
            .routing_diagnostics_json
            .as_deref()
            .map(routing_diagnostics_summary)
            .unwrap_or_default();
        let detail_other_latency_ms = event.other_latency_ms.or_else(|| {
            compute_other_latency_ms(
                event.latency_ms,
                detail_routing_wait_ms,
                event.upstream_headers_ms,
                event.post_headers_body_ms,
            )
        });
        let detail_sse_applicable = event.first_sse_write_ms.is_some();
        let detail_first_sse_label =
            format_optional_latency_ms_or_na(event.first_sse_write_ms, detail_sse_applicable);
        let client_request_json_for_copy = event
            .client_request_body_json
            .as_deref()
            .map(pretty_json_text);
        let full_request_json_for_copy = event
            .full_request_json
            .as_deref()
            .map(pretty_json_text);
        let upstream_request_json_for_copy = event
            .upstream_request_body_json
            .as_deref()
            .map(pretty_json_text);
        html! {
            <div
                class={classes!(
                    "fixed",
                    "inset-0",
                    "z-[90]",
                    "flex",
                    "items-start",
                    "sm:items-center",
                    "justify-center",
                    "overflow-y-auto",
                    "bg-slate-950/58",
                    "backdrop-blur-sm",
                    "px-4",
                    "py-8"
                )}
                onclick={{
                    let selected_usage_event = selected_usage_event.clone();
                    Callback::from(move |_| selected_usage_event.set(None))
                }}
            >
                <div
                    class={classes!(
                        "w-full",
                        "mx-auto",
                        "flex",
                        "max-h-[92vh]",
                        "max-w-4xl",
                        "flex-col",
                        "overflow-y-auto",
                        "rounded-xl",
                        "border",
                        "border-[var(--border)]",
                        "bg-[var(--surface)]",
                        "p-5",
                        "shadow-[0_16px_48px_rgba(0,0,0,0.2)]"
                    )}
                    onclick={Callback::from(|event: MouseEvent| event.stop_propagation())}
                >
                    <div class={classes!("flex", "items-start", "justify-between", "gap-4", "flex-wrap", "shrink-0")}>
                        <div class={classes!("max-w-3xl")}>
                            <p class={classes!("m-0", "text-xs", "uppercase", "tracking-[0.18em]", "text-[var(--muted)]")}>{ "Request Detail" }</p>
                            <h2 class={classes!("mt-3", "text-2xl", "font-black", "tracking-[-0.03em]")}>{ event.key_name.clone() }</h2>
                            <p class={classes!("mt-2", "m-0", "break-all", "text-sm", "leading-7", "text-[var(--muted)]")}>
                                { format!("{} {} · {} / {}", event.request_method, event.request_url, event.client_ip, event.ip_region) }
                            </p>
                        </div>
                        <div class={classes!("flex", "gap-2", "flex-wrap")}>
                            <button
                                class={classes!("btn-terminal")}
                                onclick={{
                                    let on_copy = on_copy.clone();
                                    let request_detail_summary = request_detail_summary.clone();
                                    Callback::from(move |_| on_copy.emit(("Request Summary".to_string(), request_detail_summary.clone())))
                                }}
                            >
                                { "复制摘要" }
                            </button>
                            <button
                                class={classes!("btn-terminal")}
                                onclick={{
                                    let on_copy = on_copy.clone();
                                    let headers_json_for_copy = headers_json_for_copy.clone();
                                    Callback::from(move |_| on_copy.emit(("Headers".to_string(), headers_json_for_copy.clone())))
                                }}
                            >
                                { "复制 Headers" }
                            </button>
                            <button
                                class={classes!("btn-terminal", "btn-terminal-primary")}
                                onclick={{
                                    let selected_usage_event = selected_usage_event.clone();
                                    Callback::from(move |_| selected_usage_event.set(None))
                                }}
                            >
                                { "关闭" }
                            </button>
                        </div>
                    </div>

                    <div class={classes!("mt-4", "grid", "gap-3", "lg:grid-cols-6")}>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Key ID" }</div>
                            <div class={classes!("mt-1", "font-mono", "text-xs", "break-all")}>{ event.key_id.clone() }</div>
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Account" }</div>
                            <div class={classes!("mt-1", "text-sm")}>{ usage_account_label(&event.account_name, &event.request_url, &event.endpoint) }</div>
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Status / Model" }</div>
                            <div class={classes!("mt-1", "text-sm")}>{ format!("{} · {}", event.status_code, event.model.clone().unwrap_or_else(|| "-".to_string())) }</div>
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Route" }</div>
                            <div class={classes!("mt-1", "font-mono", "text-xs", "break-all")}>{ event.endpoint.clone() }</div>
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Latency" }</div>
                            <div class={classes!("mt-1", "text-sm", "font-semibold")}>{ format_latency_ms(event.latency_ms) }</div>
                            <div class={classes!("mt-2", "grid", "gap-1", "font-mono", "text-[11px]", "text-[var(--muted)]")}>
                                <span>{ format!("route {}", format_optional_latency_ms(detail_routing_wait_ms)) }</span>
                                <span>{ format!("upstream headers {}", format_optional_latency_ms(event.upstream_headers_ms)) }</span>
                                <span>{ format!("post-headers body {}", format_optional_latency_ms(event.post_headers_body_ms)) }</span>
                                <span>{ format!("request body {}", format_optional_bytes(event.request_body_bytes)) }</span>
                                <span>{ format!("body read {}", format_optional_latency_ms(event.request_body_read_ms)) }</span>
                                <span>{ format!("json parse {}", format_optional_latency_ms(event.request_json_parse_ms)) }</span>
                                <span>{ format!("pre-handler {}", format_optional_latency_ms(event.pre_handler_ms)) }</span>
                                <span>{ format!("first SSE {}", detail_first_sse_label.clone()) }</span>
                                <span>{ format!("stream finish {}", format_optional_latency_ms(event.stream_finish_ms)) }</span>
                                <span>{ format!("other {}", format_optional_latency_ms(detail_other_latency_ms)) }</span>
                                <span>{ format!("quota failover {}", event.quota_failover_count) }</span>
                            </div>
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Stream" }</div>
                            <div class={classes!("mt-1", "flex", "items-center", "gap-2", "flex-wrap")}>
                                <span class={usage_stream_state_badge_classes(event.stream_completed_cleanly, event.downstream_disconnect)}>
                                    { usage_stream_state_label(event.stream_completed_cleanly, event.downstream_disconnect) }
                                </span>
                                <span class={classes!("font-mono", "text-xs", "text-[var(--muted)]")}>
                                    { format!("final {}", event.final_event_type.clone().unwrap_or_else(|| "-".to_string())) }
                                </span>
                            </div>
                            <div class={classes!("mt-2", "grid", "gap-1", "font-mono", "text-[11px]", "text-[var(--muted)]")}>
                                <span>{ format!("bytes {}", format_optional_bytes(event.bytes_streamed)) }</span>
                                <span>{ format!("disconnect {}", event.downstream_disconnect.map(|value| if value { "yes" } else { "no" }).unwrap_or("-")) }</span>
                            </div>
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Credit" }</div>
                            <div class={classes!("mt-1", "text-sm", "font-semibold")}>
                                { event.credit_usage.map(format_credit4).unwrap_or_else(|| "-".to_string()) }
                            </div>
                            if event.credit_usage_missing {
                                <div class={classes!("mt-1", "text-xs", "text-amber-700", "dark:text-amber-200")}>{ credit_usage_missing_label() }</div>
                            }
                        </div>
                    </div>

                    if let Some(routing_diagnostics_for_copy) = routing_diagnostics_for_copy {
                        <div class={classes!("mt-4")}>
                            <div class={classes!("mb-2", "flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Routing Diagnostics" }</div>
                                <button
                                    class={classes!("btn-terminal")}
                                    onclick={{
                                        let on_copy = on_copy.clone();
                                        let routing_diagnostics_for_copy = routing_diagnostics_for_copy.clone();
                                        Callback::from(move |_| on_copy.emit(("Routing Diagnostics".to_string(), routing_diagnostics_for_copy.clone())))
                                    }}
                                >
                                    { "复制 Routing Diagnostics" }
                                </button>
                            </div>
                            if !routing_diagnostics_summary_rows.is_empty() {
                                <div class={classes!("mb-3", "grid", "gap-2", "sm:grid-cols-2", "lg:grid-cols-4")}>
                                    { for routing_diagnostics_summary_rows.iter().map(|(label, value)| html! {
                                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2")}>
                                            <div class={classes!("text-[11px]", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ label.clone() }</div>
                                            <div class={classes!("mt-1", "font-mono", "text-xs", "text-[var(--text)]", "break-all")}>{ value.clone() }</div>
                                        </div>
                                    }) }
                                </div>
                            }
                            <pre class={classes!(
                                "max-h-[42vh]",
                                "overflow-x-auto",
                                "overflow-y-auto",
                                "rounded-lg",
                                "bg-slate-950",
                                "p-3",
                                "text-xs",
                                "leading-6",
                                "text-lime-100",
                                "whitespace-pre-wrap",
                                "break-words"
                            )}>
                                { routing_diagnostics_for_copy }
                            </pre>
                        </div>
                    }

                    <div class={classes!("mt-4")}>
                        <div class={classes!("mb-2", "flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                            <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Last Message" }</div>
                            <button
                                class={classes!("btn-terminal")}
                                onclick={{
                                    let on_copy = on_copy.clone();
                                    let last_message_for_copy = last_message_for_copy.clone();
                                    Callback::from(move |_| on_copy.emit(("Last Message".to_string(), last_message_for_copy.clone())))
                                }}
                            >
                                { "复制 Last Message" }
                            </button>
                        </div>
                        <pre class={classes!(
                            "max-h-[40vh]",
                            "overflow-x-auto",
                            "overflow-y-auto",
                            "rounded-lg",
                            "bg-slate-950",
                            "p-3",
                            "text-xs",
                            "leading-6",
                            "text-amber-100",
                            "whitespace-pre-wrap",
                            "break-words"
                        )}>
                            { last_message_for_copy }
                        </pre>
                    </div>

                    <div class={classes!("mt-4")}>
                        <div class={classes!("mb-2", "flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                            <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Headers" }</div>
                            <button
                                class={classes!("btn-terminal")}
                                onclick={{
                                    let on_copy = on_copy.clone();
                                    let headers_json_for_copy = headers_json_for_copy.clone();
                                    Callback::from(move |_| on_copy.emit(("Headers".to_string(), headers_json_for_copy.clone())))
                                }}
                            >
                                { "复制 Headers" }
                            </button>
                        </div>
                        <pre class={classes!(
                            "max-h-[42vh]",
                            "overflow-x-auto",
                            "overflow-y-auto",
                            "rounded-lg",
                            "bg-slate-950",
                            "p-3",
                            "text-xs",
                            "leading-6",
                            "text-emerald-200",
                            "whitespace-pre-wrap",
                            "break-words"
                        )}>
                            { headers_json_for_copy }
                        </pre>
                    </div>

                    if let Some(client_request_json_for_copy) = client_request_json_for_copy {
                        <div class={classes!("mt-4")}>
                            <div class={classes!("mb-2", "flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Client Request" }</div>
                                <button
                                    class={classes!("btn-terminal")}
                                    onclick={{
                                        let on_copy = on_copy.clone();
                                        let client_request_json_for_copy = client_request_json_for_copy.clone();
                                        Callback::from(move |_| on_copy.emit(("Client Request".to_string(), client_request_json_for_copy.clone())))
                                    }}
                                >
                                    { "复制 Client Request" }
                                </button>
                            </div>
                            <pre class={classes!(
                                "max-h-[42vh]",
                                "overflow-x-auto",
                                "overflow-y-auto",
                                "rounded-lg",
                                "bg-slate-950",
                                "p-3",
                                "text-xs",
                                "leading-6",
                                "text-sky-100",
                                "whitespace-pre-wrap",
                                "break-words"
                            )}>
                                { client_request_json_for_copy }
                            </pre>
                        </div>
                    }

                    if let Some(full_request_json_for_copy) = full_request_json_for_copy {
                        <div class={classes!("mt-4")}>
                            <div class={classes!("mb-2", "flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Full Request" }</div>
                                <button
                                    class={classes!("btn-terminal")}
                                    onclick={{
                                        let on_copy = on_copy.clone();
                                        let full_request_json_for_copy = full_request_json_for_copy.clone();
                                        Callback::from(move |_| on_copy.emit(("Full Request".to_string(), full_request_json_for_copy.clone())))
                                    }}
                                >
                                    { "复制 Full Request" }
                                </button>
                            </div>
                            <pre class={classes!(
                                "max-h-[42vh]",
                                "overflow-x-auto",
                                "overflow-y-auto",
                                "rounded-lg",
                                "bg-slate-950",
                                "p-3",
                                "text-xs",
                                "leading-6",
                                "text-cyan-100",
                                "whitespace-pre-wrap",
                                "break-words"
                            )}>
                                { full_request_json_for_copy }
                            </pre>
                        </div>
                    }

                    if let Some(upstream_request_json_for_copy) = upstream_request_json_for_copy {
                        <div class={classes!("mt-4")}>
                            <div class={classes!("mb-2", "flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Upstream Request" }</div>
                                <button
                                    class={classes!("btn-terminal")}
                                    onclick={{
                                        let on_copy = on_copy.clone();
                                        let upstream_request_json_for_copy = upstream_request_json_for_copy.clone();
                                        Callback::from(move |_| on_copy.emit(("Upstream Request".to_string(), upstream_request_json_for_copy.clone())))
                                    }}
                                >
                                    { "复制 Upstream Request" }
                                </button>
                            </div>
                            <pre class={classes!(
                                "max-h-[42vh]",
                                "overflow-x-auto",
                                "overflow-y-auto",
                                "rounded-lg",
                                "bg-slate-950",
                                "p-3",
                                "text-xs",
                                "leading-6",
                                "text-fuchsia-100",
                                "whitespace-pre-wrap",
                                "break-words"
                            )}>
                                { upstream_request_json_for_copy }
                            </pre>
                        </div>
                    }
                </div>
            </div>
        }
        })
    };

    // Client-side filters for Keys, Account Groups, and the Usage key picker.
    // Matches are case-insensitive. `use_memo` avoids re-filtering on unrelated
    // parent re-renders. These are pre-computed at component top-level because
    // the html! macro does not permit `let` bindings inside conditional branches.
    let keys_total_pages = admin_group_total_pages(*keys_total, *keys_page_limit);
    let keys_current_page = (*keys_page).clamp(1, keys_total_pages);
    let keys_page_entries: Vec<&AdminLlmGatewayKeyView> = keys.iter().collect();
    let on_keys_page_change = {
        let keys_page = keys_page.clone();
        Callback::from(move |p: usize| keys_page.set(p))
    };
    let usage_key_query_lower = (*usage_key_search).trim().to_lowercase();
    let filtered_usage_keys: Vec<AdminLlmGatewayKeyView> = {
        let q = usage_key_query_lower.clone();
        use_memo(((*keys).clone(), q.clone()), move |(items, q)| {
            filter_gateway_keys_for_query(items, q)
        })
        .as_ref()
        .clone()
    };
    let account_groups_query_lower = (*account_groups_search).trim().to_lowercase();
    let filtered_account_groups: Vec<AdminAccountGroupView> = {
        let q = account_groups_query_lower.clone();
        use_memo(((*account_groups_page_items).clone(), q.clone()), move |(items, q)| {
            if q.is_empty() {
                items.clone()
            } else {
                items
                    .iter()
                    .filter(|g| {
                        if g.name.to_lowercase().contains(q)
                            || g.id.to_lowercase().contains(q)
                            || g.provider_type.to_lowercase().contains(q)
                        {
                            return true;
                        }
                        g.account_names.iter().any(|n| n.to_lowercase().contains(q))
                    })
                    .cloned()
                    .collect()
            }
        })
        .as_ref()
        .clone()
    };
    let account_groups_total_pages =
        admin_group_total_pages(*account_groups_total, *account_groups_page_limit);
    let account_groups_current_page = (*account_groups_page).clamp(1, account_groups_total_pages);
    let on_account_groups_page_change = {
        let account_groups_page = account_groups_page.clone();
        let account_groups_page_items = account_groups_page_items.clone();
        let account_groups_total = account_groups_total.clone();
        let account_groups_page_limit = account_groups_page_limit.clone();
        let load_error = load_error.clone();
        Callback::from(move |page: usize| {
            let page = page.max(1);
            account_groups_page.set(page);
            let account_groups_page_items = account_groups_page_items.clone();
            let account_groups_total = account_groups_total.clone();
            let account_groups_page_limit = account_groups_page_limit.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let limit = (*account_groups_page_limit).max(1);
                let offset = page.saturating_sub(1) * limit;
                match fetch_admin_llm_gateway_account_groups_page(limit, offset).await {
                    Ok(resp) => {
                        account_groups_total.set(resp.total);
                        account_groups_page_limit.set(resp.limit.max(1));
                        account_groups_page_items.set(resp.groups);
                        load_error.set(None);
                    },
                    Err(err) => load_error.set(Some(err)),
                }
            });
        })
    };
    let on_keys_search_change = {
        let keys_search = keys_search.clone();
        Callback::from(move |v: String| keys_search.set(v))
    };
    let on_account_groups_search_change = {
        let account_groups_search = account_groups_search.clone();
        Callback::from(move |v: String| account_groups_search.set(v))
    };

    let account_total_pages = admin_group_total_pages(*accounts_total, *account_page_limit);
    let account_current_page = (*account_page).clamp(1, account_total_pages);
    let account_page_entries: Vec<&AccountSummaryView> = accounts.iter().collect();
    let on_account_page_change = {
        let account_page = account_page.clone();
        Callback::from(move |p: usize| account_page.set(p))
    };
    let on_account_search_submit = {
        let account_search = account_search.clone();
        let account_active_query = account_active_query.clone();
        let account_page = account_page.clone();
        Callback::from(move |_: ()| {
            account_active_query.set((*account_search).clone());
            account_page.set(1);
        })
    };
    let on_account_search_input = {
        let account_search = account_search.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
                account_search.set(target.value());
            }
        })
    };
    let on_account_search_keydown = {
        let on_account_search_submit = on_account_search_submit.clone();
        Callback::from(move |e: KeyboardEvent| {
            if e.key() == "Enter" {
                on_account_search_submit.emit(());
            }
        })
    };
    let on_account_search_clear = {
        let account_search = account_search.clone();
        let account_active_query = account_active_query.clone();
        let account_page = account_page.clone();
        Callback::from(move |_: MouseEvent| {
            account_search.set(String::new());
            account_active_query.set(String::new());
            account_page.set(1);
        })
    };

    // ── Proxy config: filter ──
    let proxy_query_lower = (*proxy_config_active_query).trim().to_lowercase();
    let proxy_configs_filtered: Vec<&AdminUpstreamProxyConfigView> = proxy_configs
        .iter()
        .filter(|pc| {
            proxy_query_lower.is_empty()
                || pc.name.to_lowercase().contains(&proxy_query_lower)
                || pc.proxy_url.to_lowercase().contains(&proxy_query_lower)
                || pc
                    .proxy_username
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&proxy_query_lower)
                || pc.id.to_lowercase().contains(&proxy_query_lower)
        })
        .filter(|pc| !*proxy_config_show_active_only || pc.status.as_str() != "disabled")
        .collect();
    let on_proxy_search_submit = {
        let proxy_config_search = proxy_config_search.clone();
        let proxy_config_active_query = proxy_config_active_query.clone();
        Callback::from(move |_: ()| {
            proxy_config_active_query.set((*proxy_config_search).clone());
        })
    };
    let on_proxy_search_input = {
        let proxy_config_search = proxy_config_search.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
                proxy_config_search.set(target.value());
            }
        })
    };
    let on_proxy_search_keydown = {
        let on_proxy_search_submit = on_proxy_search_submit.clone();
        Callback::from(move |e: KeyboardEvent| {
            if e.key() == "Enter" {
                on_proxy_search_submit.emit(());
            }
        })
    };
    let on_proxy_search_clear = {
        let proxy_config_search = proxy_config_search.clone();
        let proxy_config_active_query = proxy_config_active_query.clone();
        Callback::from(move |_: MouseEvent| {
            proxy_config_search.set(String::new());
            proxy_config_active_query.set(String::new());
        })
    };
    let proxy_scope_view = (*proxy_config_scope).clone();
    let can_create_proxy_config = proxy_scope_view.can_edit_slot_metadata;
    let proxy_scope_summary = if proxy_scope_view.is_core {
        format!(
            "当前节点 {} 使用 core 代理槽位，可创建、删除和重命名槽位。",
            proxy_scope_view.node_id
        )
    } else {
        format!(
            "当前节点 {} 继承 core 代理槽位，只能修改本机代理地址、凭据和状态。",
            proxy_scope_view.node_id
        )
    };

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
                <section class={classes!(
                    "rounded-xl",
                    "border",
                    "border-[var(--border)]",
                    "bg-[var(--surface)]",
                    "p-5"
                )}>
                    <div class={classes!("flex", "items-start", "justify-between", "gap-4", "flex-wrap")}>
                        <h1 class={classes!("m-0", "font-mono", "text-xl", "font-bold")}>
                            { "LLM Gateway Admin" }
                        </h1>
                        <div class={classes!("flex", "gap-2", "flex-wrap")}>
                            <Link<Route> to={Route::Admin} classes={classes!("btn-terminal")}>{ "Admin 首页" }</Link<Route>>
                            <Link<Route> to={Route::AdminLlmGatewayMonitor} classes={classes!("btn-terminal")}>{ "监控页" }</Link<Route>>
                            <Link<Route> to={Route::LlmAccess} classes={classes!("btn-terminal", "btn-terminal-primary")}>{ "公共页" }</Link<Route>>
                        </div>
                    </div>

                    if let Some(err) = (*load_error).clone() {
                        <div class={classes!("mt-4", "rounded-lg", "border", "border-red-400/35", "bg-red-500/8", "px-4", "py-3", "text-sm", "text-red-700", "dark:text-red-200")}>
                            { err }
                        </div>
                    }
                </section>

                // ── Tab Bar (always visible) ──
                { render_tab_bar(&active_tab, &[
                    (TAB_OVERVIEW, "Overview"),
                    (TAB_KEYS, "Keys"),
                    (TAB_GROUPS, "Groups"),
                    (TAB_ACCOUNTS, "Accounts"),
                    (TAB_USAGE, "Usage"),
                    (TAB_JOURNAL, "Journal"),
                    (TAB_REQUESTS, "Requests"),
                    (TAB_SETTINGS, "Settings"),
                ], &on_tab_click, Some((TAB_REQUESTS, total_pending))) }

                // ── Overview Tab ──
                if *active_tab == TAB_OVERVIEW {
                <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Dashboard" }</h2>
                        <button
                            class={classes!("btn-terminal")}
                            title="刷新 Dashboard"
                            aria-label="刷新 Dashboard"
                            onclick={{
                                let reload = reload.clone();
                                Callback::from(move |_| reload.emit(()))
                            }}
                            disabled={*loading}
                        >
                            <i class={classes!("fas", if *loading { "fa-spinner animate-spin" } else { "fa-rotate-right" })}></i>
                        </button>
                    </div>
                    <div class={classes!("mt-4", "grid", "gap-3", "grid-cols-2", "xl:grid-cols-4")}>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Key 总数" }</div>
                            <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black")}>{ key_summary.total }</div>
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "公开 / Active" }</div>
                            <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black")}>{ format!("{} / {}", public_visible_count, active_key_count) }</div>
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "剩余额度" }</div>
                            <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black")}>{ format_number_i64(total_remaining) }</div>
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "总额度" }</div>
                            <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black")}>{ format_number_u64(total_quota) }</div>
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "已用量" }</div>
                            <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black")}>{ format_number_u64(total_used) }</div>
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("flex", "items-center", "justify-between")}>
                                <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "使用率" }</div>
                                <div class={classes!("font-mono", "text-sm", "font-bold", "text-[var(--text)]")}>{ format!("{}%", usage_percent) }</div>
                            </div>
                            <div class={classes!("mt-2", "h-2", "w-full", "overflow-hidden", "rounded-full", "bg-[var(--surface-alt)]")}>
                                <div
                                    class={classes!(
                                        "h-full", "rounded-full",
                                        "transition-all", "duration-700", "ease-out",
                                        if usage_percent >= 90 { "bg-red-500" }
                                        else if usage_percent >= 70 { "bg-amber-500" }
                                        else { "bg-emerald-500" }
                                    )}
                                    style={format!("width: {}%", usage_percent)}
                                />
                            </div>
                            <div class={classes!("mt-1.5", "flex", "justify-between", "font-mono", "text-[10px]", "text-[var(--muted)]")}>
                                <span>{ format!("剩余 {}", format_number_i64(total_remaining)) }</span>
                                <span>{ format!("总计 {}", format_number_u64(total_quota)) }</span>
                            </div>
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Credit 已记录" }</div>
                            <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black")}>
                                { if credit_keys_present { format_credit4(total_credit_used) } else { "-".to_string() } }
                            </div>
                            if total_credit_missing_events > 0 {
                                <div class={classes!("mt-1", "text-xs", "text-amber-700", "dark:text-amber-200")}>
                                    { format!("partial · {} events missing", total_credit_missing_events) }
                                </div>
                            }
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "待审核" }</div>
                            <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black", if total_pending > 0 { "text-amber-600" } else { "" })}>{ total_pending }</div>
                        </div>
                    </div>
                </section>
                } // end TAB_OVERVIEW

                // ── Journal Tab ──
                if *active_tab == TAB_JOURNAL {
                <section class={classes!("grid", "gap-4", "min-w-0")}>
                    <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5", "min-w-0")}>
                        <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                            <div>
                                <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Usage Journal" }</h2>
                                <p class={classes!("mt-2", "mb-0", "text-sm", "text-[var(--muted)]")}>
                                    { "API writes active journal blocks locally; the worker seals and imports completed files into DuckDB. Live Preview only reads already-complete blocks from the current producer file." }
                                </p>
                            </div>
                            <button
                                class={classes!("btn-terminal", "btn-terminal-secondary")}
                                onclick={{
                                    let reload_usage_journal_status = reload_usage_journal_status.clone();
                                    Callback::from(move |_| reload_usage_journal_status.emit(None))
                                }}
                                disabled={*usage_journal_loading}
                            >
                                { if *usage_journal_loading { "刷新中..." } else { "刷新状态" } }
                            </button>
                        </div>

                        if let Some(status) = (*usage_journal_status).clone() {
                            <div class={classes!("mt-4", "grid", "gap-3", "md:grid-cols-2", "xl:grid-cols-6")}>
                                <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--bg)]", "p-3")}>
                                    <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "worker" }</div>
                                    <div class={classes!("mt-2", "flex", "items-center", "gap-2", "flex-wrap")}>
                                        <span class={classes!("rounded-full", "px-2.5", "py-1", "font-mono", "text-[11px]", "font-semibold", usage_worker_state_tone(&status.worker.state))}>
                                            { status.worker.state.clone() }
                                        </span>
                                        <span class={classes!("text-xs", "text-[var(--muted)]")}>
                                            { format!("heartbeat {}", format_optional_duration_ms(status.worker.heartbeat_age_ms)) }
                                        </span>
                                    </div>
                                </div>
                                <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--bg)]", "p-3")}>
                                    <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "worker memory" }</div>
                                    <div class={classes!("mt-1", "font-mono", "text-lg", "font-bold")}>
                                        { format_optional_bytes(status.worker.process_memory.rss_bytes) }
                                    </div>
                                    <div class={classes!("text-xs", "text-[var(--muted)]")}>
                                        { format!("cgroup {}", format_cgroup_memory_usage(&status.worker.process_memory)) }
                                    </div>
                                    <div class={classes!("mt-1", "text-[10px]", "text-[var(--muted)]")}>
                                        { format!(
                                            "peak {} · swap {} / {}",
                                            format_optional_bytes(status.worker.process_memory.cgroup_peak_bytes),
                                            format_optional_bytes(status.worker.process_memory.cgroup_swap_current_bytes),
                                            format_optional_bytes(status.worker.process_memory.cgroup_swap_max_bytes),
                                        ) }
                                    </div>
                                </div>
                                <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--bg)]", "p-3")}>
                                    <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "sealed backlog" }</div>
                                    <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black", if status.sealed_file_count > 0 { "text-amber-600" } else { "text-emerald-600" })}>
                                        { status.sealed_file_count }
                                    </div>
                                    <div class={classes!("text-xs", "text-[var(--muted)]")}>
                                        { format!("{} · oldest {}", format_optional_bytes(Some(status.sealed_bytes)), format_optional_duration_ms(status.oldest_sealed_age_ms)) }
                                    </div>
                                </div>
                                { render_usage_journal_current_file_card(
                                    "producer file",
                                    status.producer_current_file.as_ref(),
                                    "producer is not holding an active file",
                                ) }
                                { render_usage_journal_current_file_card(
                                    "worker file",
                                    status.current_consuming_file.as_ref(),
                                    "worker is not holding a consuming file",
                                ) }
                                <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--bg)]", "p-3")}>
                                    <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "import progress" }</div>
                                    <div class={classes!("mt-1", "font-mono", "text-lg", "font-bold")}>
                                        { format!("{:.1}%", status.worker.progress_percent) }
                                    </div>
                                    <div class={classes!("text-xs", "text-[var(--muted)]")}>
                                        { format!(
                                            "{} / {} events · {} / {}",
                                            format_number_u64(status.worker.processed_events),
                                            format_number_u64(status.worker.total_events),
                                            format_optional_bytes(Some(status.worker.processed_compressed_bytes)),
                                            format_optional_bytes(Some(status.worker.total_compressed_bytes)),
                                        ) }
                                    </div>
                                </div>
                            </div>
                            <div class={classes!("mt-3", "grid", "gap-2", "text-xs", "text-[var(--muted)]", "xl:grid-cols-2")}>
                                <p class={classes!("m-0")}>
                                    { format!(
                                        "last_successful_import: {} · file {}",
                                        format_relative_age_from_ms(
                                            status.generated_at,
                                            status.worker.last_successful_import_at_ms,
                                        ),
                                        status
                                            .worker
                                            .last_successful_file_sequence
                                            .map(|seq| format!("#{seq}"))
                                            .unwrap_or_else(|| "-".to_string())
                                    ) }
                                </p>
                                <p class={classes!("m-0", "break-all")}>
                                    { format!("journal_root: {}", status.journal_root) }
                                </p>
                                <p class={classes!("m-0", "break-all")}>
                                    { format!("usage_query_base_url: {}", status.usage_query_base_url) }
                                </p>
                                if let Some(cluster) = status.cluster.as_ref() {
                                    <p class={classes!("m-0", "break-all")}>
                                        { format!(
                                            "cluster: node {} · class {} · role {} · usage {}",
                                            cluster.node_id,
                                            cluster.node_class,
                                            cluster.runtime_role,
                                            cluster.usage_query_mode,
                                        ) }
                                    </p>
                                    if let Some(primary_node_id) = cluster.primary_node_id.as_deref() {
                                        <p class={classes!("m-0", "break-all")}>
                                            { format!("primary_node_id: {primary_node_id}") }
                                        </p>
                                    }
                                    if let Some(primary_worker_base_url) = cluster.primary_worker_base_url.as_deref() {
                                        <p class={classes!("m-0", "break-all")}>
                                            { format!("primary_worker_base_url: {primary_worker_base_url}") }
                                        </p>
                                    }
                                }
                                if let Some(path) = status.worker.current_file_path.as_deref() {
                                    <p class={classes!("m-0", "break-all")}>
                                        { format!("current_file: {path}") }
                                    </p>
                                }
                                if let Some(error) = status.worker.last_error.as_deref() {
                                    <p class={classes!("m-0", "break-all", "text-red-600", "dark:text-red-300")}>
                                        { format!("worker_error: {error}") }
                                    </p>
                                }
                            </div>
                            <div class={classes!("mt-4", "grid", "gap-3", "xl:grid-cols-2")}>
                                { render_usage_journal_file_list("sealed files", &status.sealed_files, "no sealed backlog") }
                                { render_usage_journal_file_list("orphan consuming files", &status.orphan_consuming_files, "no orphan consuming files") }
                                { render_usage_journal_file_list("bad files", &status.bad_files, "no quarantined files") }
                                { render_usage_journal_file_list("orphan active files", &status.orphan_active_files, "no orphan active files") }
                            </div>
                        } else if let Some(error) = (*usage_journal_error).clone() {
                            <div class={classes!("mt-4", "rounded-lg", "border", "border-red-500/30", "bg-red-500/10", "p-3", "text-sm", "text-red-700", "dark:text-red-200")}>
                                { error }
                            </div>
                        } else {
                            <div class={classes!("mt-4", "text-sm", "text-[var(--muted)]")}>
                                { "尚未加载 usage journal 状态。" }
                            </div>
                        }
                    </section>

                    <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5", "min-w-0")}>
                        <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                            <div>
                                <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Live Preview" }</h2>
                                <p class={classes!("mt-2", "mb-0", "text-sm", "text-[var(--muted)]")}>
                                    { "Only the current producer file is previewed. Trailing partial writes are ignored until the next full block is flushed." }
                                </p>
                            </div>
                            <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                                if let Some(status) = (*usage_journal_status).as_ref() {
                                    <span class={classes!("rounded-full", "border", "border-[var(--border)]", "px-3", "py-1", "text-xs", "font-semibold", "text-[var(--muted)]")}>
                                        { format!("RPM {}", status.current_rpm) }
                                    </span>
                                    <span class={classes!("rounded-full", "border", "border-[var(--border)]", "px-3", "py-1", "text-xs", "font-semibold", "text-[var(--muted)]")}>
                                        { format!("In Flight {}", status.current_in_flight) }
                                    </span>
                                }
                                if let Some(preview) = (*usage_journal_preview).as_ref().and_then(|view| view.preview.as_ref()) {
                                    <span class={classes!("rounded-full", "border", "border-[var(--border)]", "px-3", "py-1", "text-xs", "font-semibold", "text-[var(--muted)]")}>
                                        { format!("blocks {} · scanned {}", preview.complete_blocks, format_optional_bytes(Some(preview.bytes_scanned))) }
                                    </span>
                                }
                                <button
                                    class={classes!("btn-terminal")}
                                    title="刷新预览"
                                    aria-label="刷新预览"
                                    onclick={{
                                        let reload_usage_journal_status = reload_usage_journal_status.clone();
                                        Callback::from(move |_| reload_usage_journal_status.emit(None))
                                    }}
                                    disabled={*usage_journal_loading}
                                >
                                    <i class={classes!("fas", if *usage_journal_loading { "fa-spinner animate-spin" } else { "fa-rotate-right" })}></i>
                                </button>
                            </div>
                        </div>

                        if let Some(preview_response) = (*usage_journal_preview).clone() {
                            if let Some(preview) = preview_response.preview {
                                <div class={classes!("mt-3", "grid", "gap-2", "text-xs", "text-[var(--muted)]", "xl:grid-cols-2")}>
                                    <p class={classes!("m-0", "break-all")}>
                                        { format!("producer_current_file: {}", preview_response.producer_current_file.as_ref().map(|file| file.path.clone()).unwrap_or_else(|| "-".to_string())) }
                                    </p>
                                    <p class={classes!("m-0")}>
                                        { format!("truncated_tail: {}", if preview.truncated_tail { "yes" } else { "no" }) }
                                    </p>
                                </div>
                                <div class={classes!("mt-4", "flex", "flex-wrap", "items-center", "gap-2")}>
                                    <input
                                        type="text"
                                        class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-2.5", "py-1.5", "text-xs", "text-[var(--text)]", "placeholder:text-[var(--muted)]", "w-28")}
                                        placeholder="model"
                                        value={(*journal_filter_model).clone()}
                                        oninput={{
                                            let journal_filter_model = journal_filter_model.clone();
                                            Callback::from(move |e: InputEvent| {
                                                let input: web_sys::HtmlInputElement = e.target_unchecked_into();
                                                journal_filter_model.set(input.value());
                                            })
                                        }}
                                    />
                                    <input
                                        type="text"
                                        class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-2.5", "py-1.5", "text-xs", "text-[var(--text)]", "placeholder:text-[var(--muted)]", "w-28")}
                                        placeholder="account"
                                        value={(*journal_filter_account).clone()}
                                        oninput={{
                                            let journal_filter_account = journal_filter_account.clone();
                                            Callback::from(move |e: InputEvent| {
                                                let input: web_sys::HtmlInputElement = e.target_unchecked_into();
                                                journal_filter_account.set(input.value());
                                            })
                                        }}
                                    />
                                    <input
                                        type="text"
                                        class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-2.5", "py-1.5", "text-xs", "text-[var(--text)]", "placeholder:text-[var(--muted)]", "w-28")}
                                        placeholder="key"
                                        value={(*journal_filter_key).clone()}
                                        oninput={{
                                            let journal_filter_key = journal_filter_key.clone();
                                            Callback::from(move |e: InputEvent| {
                                                let input: web_sys::HtmlInputElement = e.target_unchecked_into();
                                                journal_filter_key.set(input.value());
                                            })
                                        }}
                                    />
                                    <select
                                        class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-2.5", "py-1.5", "text-xs", "text-[var(--text)]", "w-20")}
                                        onchange={{
                                            let journal_filter_status = journal_filter_status.clone();
                                            Callback::from(move |e: Event| {
                                                let select: web_sys::HtmlSelectElement = e.target_unchecked_into();
                                                journal_filter_status.set(select.value());
                                            })
                                        }}
                                    >
                                        <option value="" selected={journal_filter_status.is_empty()}>{ "All" }</option>
                                        <option value="2xx" selected={&**journal_filter_status == "2xx"}>{ "2xx" }</option>
                                        <option value="4xx" selected={&**journal_filter_status == "4xx"}>{ "4xx" }</option>
                                        <option value="5xx" selected={&**journal_filter_status == "5xx"}>{ "5xx" }</option>
                                    </select>
                                    {{
                                        let total = preview.events.len();
                                        let filtered_count = preview.events.iter().filter(|e| {
                                            (journal_filter_model.is_empty() || e.model.as_deref().unwrap_or("").contains(&**journal_filter_model))
                                            && (journal_filter_account.is_empty() || e.account_name.as_deref().unwrap_or("").contains(&**journal_filter_account))
                                            && (journal_filter_key.is_empty() || e.key_name.contains(&**journal_filter_key))
                                            && (journal_filter_status.is_empty() || match journal_filter_status.as_str() {
                                                "2xx" => e.status_code >= 200 && e.status_code < 300,
                                                "4xx" => e.status_code >= 400 && e.status_code < 500,
                                                "5xx" => e.status_code >= 500,
                                                _ => true,
                                            })
                                        }).count();
                                        html! {
                                            <span class={classes!("rounded-full", "border", "border-[var(--border)]", "px-2.5", "py-1", "text-[11px]", "font-semibold", "text-[var(--muted)]")}>
                                                { format!("{}/{}", filtered_count, total) }
                                            </span>
                                        }
                                    }}
                                </div>
                                <div class={classes!("mt-3", "min-w-0") }>
                                    <div class={classes!("overflow-x-auto", "max-w-full", "rounded-xl", "border", "border-[var(--border)]")}>
                                    <table class={classes!("min-w-[64rem]", "w-full", "text-sm")}>
                                        <thead>
                                            <tr class={classes!("text-left", "text-[var(--muted)]")}>
                                                <th class={classes!("py-2", "pl-3", "pr-3")}>{ "时间" }</th>
                                                <th class={classes!("py-2", "pr-3")}>{ "Key" }</th>
                                                <th class={classes!("py-2", "pr-3")}>{ "号池" }</th>
                                                <th class={classes!("py-2", "pr-3")}>{ "Model" }</th>
                                                <th class={classes!("py-2", "pr-3")}>{ "Status" }</th>
                                                <th class={classes!("py-2", "pr-3")}>{ "Latency" }</th>
                                                <th class={classes!("py-2", "pr-3")}>{ "Tokens" }</th>
                                                <th class={classes!("py-2", "pr-3")}>{ "" }</th>
                                            </tr>
                                        </thead>
                                        <tbody>
                                            {{
                                                let filtered_events: Vec<_> = preview.events.iter().filter(|e| {
                                                    (journal_filter_model.is_empty() || e.model.as_deref().unwrap_or("").contains(&**journal_filter_model))
                                                    && (journal_filter_account.is_empty() || e.account_name.as_deref().unwrap_or("").contains(&**journal_filter_account))
                                                    && (journal_filter_key.is_empty() || e.key_name.contains(&**journal_filter_key))
                                                    && (journal_filter_status.is_empty() || match journal_filter_status.as_str() {
                                                        "2xx" => e.status_code >= 200 && e.status_code < 300,
                                                        "4xx" => e.status_code >= 400 && e.status_code < 500,
                                                        "5xx" => e.status_code >= 500,
                                                        _ => true,
                                                    })
                                                }).collect();
                                                if filtered_events.is_empty() {
                                                    html! {
                                                        <tr class={classes!("border-t", "border-[var(--border)]")}>
                                                            <td colspan="8" class={classes!("py-8", "text-center", "text-[var(--muted)]")}>{ "当前 producer file 里还没有完整 block 可预览" }</td>
                                                        </tr>
                                                    }
                                                } else {
                                                    html! { { for filtered_events.into_iter().map(|event| {
                                                        let account_label = event.account_name.clone().unwrap_or_else(|| "not captured".to_string());
                                                        let last_message_full = usage_journal_preview_message(event);
                                                        let has_full_message = usage_journal_preview_has_full_message(event);
                                                        let open_preview_message = {
                                                            let selected_usage_journal_message = selected_usage_journal_message.clone();
                                                            let event_id = event.event_id.clone();
                                                            let created_at = format_ms(event.created_at_ms);
                                                            let key_name = event.key_name.clone();
                                                            let full_message = last_message_full.clone();
                                                            Callback::from(move |_| {
                                                                selected_usage_journal_message.set(Some((
                                                                    event_id.clone(),
                                                                    created_at.clone(),
                                                                    key_name.clone(),
                                                                    full_message.clone(),
                                                                )))
                                                            })
                                                        };
                                                        let latency_ms_val = event.latency_ms.unwrap_or(0) as i32;
                                                        let latency_color = if latency_ms_val < 3000 {
                                                            ("border-emerald-500/20", "bg-emerald-500/10", "text-emerald-700", "dark:text-emerald-200")
                                                        } else if latency_ms_val < 10000 {
                                                            ("border-amber-500/20", "bg-amber-500/10", "text-amber-700", "dark:text-amber-200")
                                                        } else {
                                                            ("border-red-500/20", "bg-red-500/10", "text-red-700", "dark:text-red-200")
                                                        };
                                                        let status_ok = event.status_code >= 200 && event.status_code < 300;
                                                        html! {
                                                            <tr class={classes!("border-t", "border-[var(--border)]", "align-top")}>
                                                                <td class={classes!("py-2.5", "pl-3", "pr-3", "whitespace-nowrap")}>
                                                                    <div class={classes!("text-xs")}>{ format_ms(event.created_at_ms) }</div>
                                                                    <div class={classes!("mt-0.5", "flex", "items-center", "gap-1")}>
                                                                        <span class={classes!("max-w-[7rem]", "truncate", "font-mono", "text-[10px]", "text-[var(--muted)]")} title={event.event_id.clone()}>
                                                                            { event.event_id.clone() }
                                                                        </span>
                                                                        { copy_icon_button(&event.event_id, &on_copy) }
                                                                    </div>
                                                                </td>
                                                                <td class={classes!("py-2.5", "pr-3")}>
                                                                    <div class={classes!("text-xs", "font-semibold", "text-[var(--text)]", "truncate", "max-w-[10rem]")} title={event.key_name.clone()}>{ event.key_name.clone() }</div>
                                                                    <div class={classes!("font-mono", "text-[10px]", "text-[var(--muted)]")}>{ event.key_id.clone() }</div>
                                                                </td>
                                                                <td class={classes!("py-2.5", "pr-3")}>
                                                                    <span class={classes!("inline-flex", "rounded-full", "border", "border-emerald-500/20", "bg-emerald-500/10", "px-2", "py-0.5", "text-[11px]", "font-semibold", "text-emerald-700", "dark:text-emerald-200")}>
                                                                        { account_label }
                                                                    </span>
                                                                </td>
                                                                <td class={classes!("py-2.5", "pr-3")}>
                                                                    <div class={classes!("text-xs", "truncate", "max-w-[10rem]")} title={event.model.clone().unwrap_or_default()}>
                                                                        { event.model.clone().unwrap_or_else(|| "-".to_string()) }
                                                                    </div>
                                                                    if event.usage_missing {
                                                                        <span class={classes!("inline-flex", "rounded-full", "border", "border-amber-500/20", "bg-amber-500/10", "px-1.5", "py-0.5", "text-[10px]", "font-semibold", "text-amber-700", "dark:text-amber-200")}>
                                                                            { token_usage_missing_label() }
                                                                        </span>
                                                                    }
                                                                </td>
                                                                <td class={classes!("py-2.5", "pr-3", "whitespace-nowrap")}>
                                                                    <span class={classes!(
                                                                        "inline-flex", "h-5", "w-5", "items-center", "justify-center", "rounded-full", "text-[10px]", "font-bold",
                                                                        if status_ok { "bg-emerald-500/15" } else { "bg-red-500/15" },
                                                                        if status_ok { "text-emerald-700" } else { "text-red-700" },
                                                                        if status_ok { "dark:text-emerald-200" } else { "dark:text-red-200" },
                                                                    )} title={format!("{}", event.status_code)}>
                                                                        { if status_ok { "" } else { "!" } }
                                                                    </span>
                                                                    <span class={classes!("ml-1", "text-xs", "font-mono")}>{ event.status_code }</span>
                                                                </td>
                                                                <td class={classes!("py-2.5", "pr-3", "whitespace-nowrap")}>
                                                                    if event.latency_ms.is_some() {
                                                                        <span class={classes!("inline-flex", "rounded-full", "border", "px-2", "py-0.5", "text-[11px]", "font-semibold", latency_color.0, latency_color.1, latency_color.2, latency_color.3)}>
                                                                            { format_latency_ms(latency_ms_val) }
                                                                        </span>
                                                                        <div class={classes!("mt-0.5", "text-[10px]", "text-[var(--muted)]")}>
                                                                            { if let Some(first_ms) = event.first_sse_write_ms {
                                                                                format!("首字 {}ms", first_ms.max(0))
                                                                            } else {
                                                                                "-".to_string()
                                                                            }}
                                                                        </div>
                                                                    } else {
                                                                        <span class={classes!("text-xs", "text-[var(--muted)]")}>{ "-" }</span>
                                                                    }
                                                                </td>
                                                                <td class={classes!("py-2.5", "pr-3", "whitespace-nowrap", "font-mono", "text-[11px]")}>
                                                                    <span class={classes!("text-[var(--muted)]")}>
                                                                        { format!("{}/{}/{}", format_number_u64(event.input_uncached_tokens), format_number_u64(event.input_cached_tokens), format_number_u64(event.output_tokens)) }
                                                                    </span>
                                                                </td>
                                                                <td class={classes!("py-2.5", "pr-3")}>
                                                                    if has_full_message {
                                                                        <button
                                                                            type="button"
                                                                            class={classes!(
                                                                                "inline-flex",
                                                                                "h-7",
                                                                                "w-7",
                                                                                "items-center",
                                                                                "justify-center",
                                                                                "rounded-lg",
                                                                                "border",
                                                                                "border-[var(--border)]",
                                                                                "bg-[var(--surface)]",
                                                                                "text-[var(--muted)]",
                                                                                "transition-colors",
                                                                                "hover:text-[var(--primary)]",
                                                                                "hover:bg-[var(--surface-alt)]"
                                                                            )}
                                                                            title="查看最后一条内容"
                                                                            aria-label="查看最后一条内容"
                                                                            onclick={open_preview_message}
                                                                        >
                                                                            <i class={classes!("fas", "fa-bars-staggered", "text-xs")}></i>
                                                                        </button>
                                                                    }
                                                                </td>
                                                            </tr>
                                                        }
                                                    }) }}
                                                }
                                            }}
                                        </tbody>
                                    </table>
                                    </div>
                                </div>
                                <div class={classes!("mt-5", "flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                    <div class={classes!("text-xs", "text-[var(--muted)]")}>
                                        { format!("第 {} 页 · {} 条", *usage_journal_preview_page, preview.total_events) }
                                    </div>
                                    <Pagination
                                        current_page={*usage_journal_preview_page}
                                        total_pages={usage_journal_preview_total_pages}
                                        on_page_change={on_usage_journal_preview_page_change.clone()}
                                    />
                                </div>
                            } else {
                                <div class={classes!("mt-4", "text-sm", "text-[var(--muted)]")}>
                                    { "当前还没有 producer file 可预览。" }
                                </div>
                            }
                        } else if let Some(error) = (*usage_journal_error).clone() {
                            <div class={classes!("mt-4", "rounded-lg", "border", "border-red-500/30", "bg-red-500/10", "p-3", "text-sm", "text-red-700", "dark:text-red-200")}>
                                { error }
                            </div>
                        } else {
                            <div class={classes!("mt-4", "text-sm", "text-[var(--muted)]")}>
                                { "尚未加载实时预览。" }
                            </div>
                        }
                    </section>
                </section>
                } // end TAB_JOURNAL

                // ── Settings Tab ──
                if *active_tab == TAB_SETTINGS {
                <div class={classes!("space-y-4")}>
                <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                        <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                            <div>
                                <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Runtime Config" }</h2>
                                <p class={classes!("mt-2", "mb-0", "text-sm", "text-[var(--muted)]")}>
                                    { "This page owns gateway-wide runtime defaults and llm usage maintenance cadence. Kiro cache simulation, prefix-tree capacity, anchor settings, and per-account scheduler overrides are managed from the Kiro Gateway page." }
                                </p>
                            </div>
                            <Link<Route> to={Route::AdminKiroGateway} classes={classes!("btn-terminal", "btn-terminal-secondary")}>
                                { "Open Kiro Gateway" }
                            </Link<Route>>
                        </div>
                        <div class={classes!("mt-3", "grid", "gap-3", "md:grid-cols-2", "xl:grid-cols-3")}>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "auth_cache_ttl_seconds" }</span>
                                <input
                                    type="number"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*ttl_input).clone()}
                                    oninput={{
                                        let ttl_input = ttl_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                ttl_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "max_request_body_bytes" }</span>
                                <input
                                    type="number"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*max_request_body_input).clone()}
                                    oninput={{
                                        let max_request_body_input = max_request_body_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                max_request_body_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "account_failure_retry_limit" }</span>
                                <input
                                    type="number"
                                    min="0"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*account_failure_retry_limit_input).clone()}
                                    oninput={{
                                        let account_failure_retry_limit_input = account_failure_retry_limit_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                account_failure_retry_limit_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <h3 class={classes!("md:col-span-2", "xl:col-span-3", "m-0", "mt-2", "text-xs", "font-semibold", "uppercase", "tracking-wider", "text-[var(--muted)]")}>{ "Codex" }</h3>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "codex_client_version" }</span>
                                <input
                                    type="text"
                                    spellcheck="false"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "font-mono")}
                                    value={(*codex_client_version_input).clone()}
                                    oninput={{
                                        let codex_client_version_input = codex_client_version_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                codex_client_version_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "codex_status_refresh_min_interval_seconds" }</span>
                                <input
                                    type="number"
                                    min="240"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*codex_refresh_min_input).clone()}
                                    oninput={{
                                        let codex_refresh_min_input = codex_refresh_min_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                codex_refresh_min_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "codex_status_refresh_max_interval_seconds" }</span>
                                <input
                                    type="number"
                                    min="240"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*codex_refresh_max_input).clone()}
                                    oninput={{
                                        let codex_refresh_max_input = codex_refresh_max_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                codex_refresh_max_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "codex_status_account_jitter_max_seconds" }</span>
                                <input
                                    type="number"
                                    min="0"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*codex_account_jitter_max_input).clone()}
                                    oninput={{
                                        let codex_account_jitter_max_input = codex_account_jitter_max_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                codex_account_jitter_max_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "codex_weight_free" }</span>
                                <input
                                    type="number"
                                    min="0"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*codex_weight_free_input).clone()}
                                    oninput={{
                                        let codex_weight_free_input = codex_weight_free_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                codex_weight_free_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "codex_weight_plus" }</span>
                                <input
                                    type="number"
                                    min="0"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*codex_weight_plus_input).clone()}
                                    oninput={{
                                        let codex_weight_plus_input = codex_weight_plus_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                codex_weight_plus_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "codex_weight_pro5x" }</span>
                                <input
                                    type="number"
                                    min="0"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*codex_weight_pro5x_input).clone()}
                                    oninput={{
                                        let codex_weight_pro5x_input = codex_weight_pro5x_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                codex_weight_pro5x_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "codex_weight_pro20x" }</span>
                                <input
                                    type="number"
                                    min="0"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*codex_weight_pro20x_input).clone()}
                                    oninput={{
                                        let codex_weight_pro20x_input = codex_weight_pro20x_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                codex_weight_pro20x_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <h3 class={classes!("md:col-span-2", "xl:col-span-3", "m-0", "mt-2", "text-xs", "font-semibold", "uppercase", "tracking-wider", "text-[var(--muted)]")}>{ "Kiro" }</h3>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "kiro_status_refresh_min_interval_seconds" }</span>
                                <input
                                    type="number"
                                    min="240"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*kiro_refresh_min_input).clone()}
                                    oninput={{
                                        let kiro_refresh_min_input = kiro_refresh_min_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                kiro_refresh_min_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "kiro_status_refresh_max_interval_seconds" }</span>
                                <input
                                    type="number"
                                    min="240"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*kiro_refresh_max_input).clone()}
                                    oninput={{
                                        let kiro_refresh_max_input = kiro_refresh_max_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                kiro_refresh_max_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "kiro_status_account_jitter_max_seconds" }</span>
                                <input
                                    type="number"
                                    min="0"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*kiro_account_jitter_max_input).clone()}
                                    oninput={{
                                        let kiro_account_jitter_max_input = kiro_account_jitter_max_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                kiro_account_jitter_max_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <h3 class={classes!("md:col-span-2", "xl:col-span-3", "m-0", "mt-2", "text-xs", "font-semibold", "uppercase", "tracking-wider", "text-[var(--muted)]")}>{ "Usage / DuckDB" }</h3>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "usage_event_flush_batch_size" }</span>
                                <input
                                    type="number"
                                    min="1"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*usage_flush_batch_size_input).clone()}
                                    oninput={{
                                        let usage_flush_batch_size_input = usage_flush_batch_size_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                usage_flush_batch_size_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "usage_event_flush_interval_seconds" }</span>
                                <input
                                    type="number"
                                    min="1"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*usage_flush_interval_input).clone()}
                                    oninput={{
                                        let usage_flush_interval_input = usage_flush_interval_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                usage_flush_interval_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "usage_event_flush_max_buffer_bytes" }</span>
                                <input
                                    type="number"
                                    min="1"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*usage_flush_max_buffer_bytes_input).clone()}
                                    oninput={{
                                        let usage_flush_max_buffer_bytes_input = usage_flush_max_buffer_bytes_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                usage_flush_max_buffer_bytes_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "duckdb_usage_memory_limit_mib" }</span>
                                <input
                                    type="number"
                                    min="512"
                                    max="2048"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*duckdb_usage_memory_limit_mib_input).clone()}
                                    oninput={{
                                        let duckdb_usage_memory_limit_mib_input = duckdb_usage_memory_limit_mib_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                duckdb_usage_memory_limit_mib_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "duckdb_usage_checkpoint_threshold_mib" }</span>
                                <input
                                    type="number"
                                    min="16"
                                    max="256"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*duckdb_usage_checkpoint_threshold_mib_input).clone()}
                                    oninput={{
                                        let duckdb_usage_checkpoint_threshold_mib_input = duckdb_usage_checkpoint_threshold_mib_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                duckdb_usage_checkpoint_threshold_mib_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "usage_analytics_retention_days" }</span>
                                <input
                                    type="number"
                                    min="1"
                                    max="365"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*usage_analytics_retention_days_input).clone()}
                                    oninput={{
                                        let usage_analytics_retention_days_input = usage_analytics_retention_days_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                usage_analytics_retention_days_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <details class={classes!("rounded-lg", "border", "border-dashed", "border-[var(--border)]", "bg-[var(--bg)]", "px-3", "py-2", "text-xs", "text-[var(--muted)]", "md:col-span-2", "xl:col-span-3")}>
                                <summary class={classes!("cursor-pointer", "font-semibold", "select-none")}>{ "配置说明" }</summary>
                                <div class={classes!("mt-2")}>
                                <p class={classes!("m-0")}>
                                    { format!("默认 Codex models catalog 版本：{}。不带 client_version 的 `/v1/models` 请求会回落到这里。", DEFAULT_LLM_GATEWAY_CODEX_CLIENT_VERSION) }
                                </p>
                                <p class={classes!("m-0", "mt-1")}>
                                    { "默认轮询窗口：Codex / Kiro 都是 240-300 秒；每个账号请求之间插入 0-10 秒随机抖动。" }
                                </p>
                                <p class={classes!("m-0", "mt-1")}>
                                    { "Codex 自动选号会按 bottleneck remaining * weight 比较；默认倍率是 free=1, plus=10, pro5x=50, pro20x=200。" }
                                </p>
                                <p class={classes!("m-0", "mt-1")}>
                                    { "默认 usage flush：256 条、15 秒、8 MiB；DuckDB writer 默认 memory_limit=1024 MiB、checkpoint_threshold=16 MiB。" }
                                </p>
                                <p class={classes!("m-0", "mt-1")}>
                                    { "llm usage 表现在和其他表共用 /admin 里的 Storage Maintenance 配置：scan interval、fragment threshold、prune 窗口和 worker 数都只有一套。" }
                                </p>
                                </div>
                            </details>
                            <div class={classes!("flex", "items-end", "md:col-span-2", "xl:col-span-3")}>
                                <button class={classes!("btn-terminal", "btn-terminal-primary", "w-full", "md:w-auto")} onclick={on_save_runtime_config} disabled={*saving_runtime_config}>
                                    { if *saving_runtime_config { "保存中..." } else { "保存" } }
                                </button>
                            </div>
                        </div>
                        if let Some(cfg) = (*config).clone() {
                            <div class={classes!("mt-3", "space-y-1", "text-xs", "text-[var(--muted)]")}>
                                <p class={classes!("m-0")}>
                                    { format!("当前 TTL：{} 秒", cfg.auth_cache_ttl_seconds) }
                                </p>
                                <p class={classes!("m-0")}>
                                    { format!("当前请求体上限：{} bytes", format_number_u64(cfg.max_request_body_bytes)) }
                                </p>
                                <p class={classes!("m-0")}>
                                    { format!("当前账号失败重试次数：{}", cfg.account_failure_retry_limit) }
                                </p>
                                <p class={classes!("m-0")}>
                                    { format!("当前 Codex client version：{}", cfg.codex_client_version) }
                                </p>
                                <p class={classes!("m-0")}>
                                    { format!(
                                        "当前 Codex 轮询窗口：{}-{} 秒，单账号抖动上限：{} 秒",
                                        cfg.codex_status_refresh_min_interval_seconds,
                                        cfg.codex_status_refresh_max_interval_seconds,
                                        cfg.codex_status_account_jitter_max_seconds
                                    ) }
                                </p>
                                <p class={classes!("m-0")}>
                                    { format!(
                                        "当前 Kiro 轮询窗口：{}-{} 秒，单账号抖动上限：{} 秒",
                                        cfg.kiro_status_refresh_min_interval_seconds,
                                        cfg.kiro_status_refresh_max_interval_seconds,
                                        cfg.kiro_status_account_jitter_max_seconds
                                    ) }
                                </p>
                                <p class={classes!("m-0")}>
                                    { format!(
                                        "当前 usage flush：{} 条 / {} 秒 / {} bytes；DuckDB：{} MiB / {} MiB；保留最近 {} 天",
                                        cfg.usage_event_flush_batch_size,
                                        cfg.usage_event_flush_interval_seconds,
                                        format_number_u64(cfg.usage_event_flush_max_buffer_bytes),
                                        cfg.duckdb_usage_memory_limit_mib,
                                        cfg.duckdb_usage_checkpoint_threshold_mib,
                                        cfg.usage_analytics_retention_days
                                    ) }
                                </p>
                            </div>
                        }
                    </section>

                    <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                        <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Create Key" }</h2>
                        <div class={classes!("mt-3", "grid", "gap-3")}>
                            <div class={classes!("grid", "gap-3", "md:grid-cols-2")}>
                                <label class={classes!("text-sm")}>
                                    <span class={classes!("text-[var(--muted)]")}>{ "名称" }</span>
                                    <input
                                        type="text"
                                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                        value={create_key.name.clone()}
                                        oninput={{
                                            let create_key = create_key.clone();
                                            Callback::from(move |event: InputEvent| {
                                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                    let mut next = (*create_key).clone();
                                                    next.name = target.value();
                                                    create_key.set(next);
                                                }
                                            })
                                        }}
                                    />
                                </label>
                                <label class={classes!("text-sm")}>
                                    <span class={classes!("text-[var(--muted)]")}>{ "主额度上限" }</span>
                                    <input
                                        type="number"
                                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                        value={create_key.quota.clone()}
                                        oninput={{
                                            let create_key = create_key.clone();
                                            Callback::from(move |event: InputEvent| {
                                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                    let mut next = (*create_key).clone();
                                                    next.quota = target.value();
                                                    create_key.set(next);
                                                }
                                            })
                                        }}
                                    />
                                </label>
                            </div>
                            <div class={classes!("grid", "gap-3", "md:grid-cols-2")}>
                                <label class={classes!("text-sm")}>
                                    <span class={classes!("text-[var(--muted)]")}>{ "并发上限" }</span>
                                    <input
                                        type="number"
                                        placeholder="留空表示不限制"
                                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                        value={create_key.request_max_concurrency.clone()}
                                        oninput={{
                                            let create_key = create_key.clone();
                                            Callback::from(move |event: InputEvent| {
                                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                    let mut next = (*create_key).clone();
                                                    next.request_max_concurrency = target.value();
                                                    create_key.set(next);
                                                }
                                            })
                                        }}
                                    />
                                </label>
                                <label class={classes!("text-sm")}>
                                    <span class={classes!("text-[var(--muted)]")}>{ "请求起始间隔 ms" }</span>
                                    <input
                                        type="number"
                                        placeholder="留空表示不限制"
                                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                        value={create_key.request_min_start_interval_ms.clone()}
                                        oninput={{
                                            let create_key = create_key.clone();
                                            Callback::from(move |event: InputEvent| {
                                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                    let mut next = (*create_key).clone();
                                                    next.request_min_start_interval_ms = target.value();
                                                    create_key.set(next);
                                                }
                                            })
                                        }}
                                    />
                                </label>
                            </div>
                            <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                <label class={classes!("flex", "items-center", "gap-2", "text-sm")}>
                                    <input
                                        type="checkbox"
                                        checked={create_key.public}
                                        onchange={{
                                            let create_key = create_key.clone();
                                            Callback::from(move |event: Event| {
                                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                    let mut next = (*create_key).clone();
                                                    next.public = target.checked();
                                                    create_key.set(next);
                                                }
                                            })
                                        }}
                                    />
                                    <span>{ "公开" }</span>
                                </label>
                                <button class={classes!("btn-terminal", "btn-terminal-primary")} onclick={on_create} disabled={*creating}>
                                    { if *creating { "创建中..." } else { "创建" } }
                                </button>
                            </div>
                        </div>
                    </section>

                    <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                        <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                            <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Provider Proxy Bindings" }</h2>
                            <button class={classes!("btn-terminal")} onclick={{
                                let reload = reload.clone();
                                Callback::from(move |_| reload.emit(()))
                            }}>
                                { if *loading { "刷新中..." } else { "刷新" } }
                            </button>
                        </div>
                        <div class={classes!("mt-4", "grid", "gap-4")}>
                            {
                                for ["codex", "kiro"].iter().map(|provider| {
                                    let binding = proxy_bindings.iter().find(|item| item.provider_type == *provider).cloned();
                                    let selected_value = if *provider == "codex" {
                                        (*codex_proxy_binding_input).clone()
                                    } else {
                                        (*kiro_proxy_binding_input).clone()
                                    };
                                    let on_change = if *provider == "codex" {
                                        let codex_proxy_binding_input = codex_proxy_binding_input.clone();
                                        Callback::from(move |event: Event| {
                                            if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                                                codex_proxy_binding_input.set(target.value());
                                            }
                                        })
                                    } else {
                                        let kiro_proxy_binding_input = kiro_proxy_binding_input.clone();
                                        Callback::from(move |event: Event| {
                                            if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                                                kiro_proxy_binding_input.set(target.value());
                                            }
                                        })
                                    };
                                    let provider_name = (*provider).to_string();
                                    let select_key = format!(
                                        "provider-proxy-binding-{}-{}",
                                        provider_name,
                                        selected_value.clone()
                                    );
                                    html! {
                                        <article class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                                            <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                                <div>
                                                    <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ provider_name.to_uppercase() }</div>
                                                    <div class={classes!("mt-1", "text-sm", "text-[var(--muted)]")}>
                                                        {
                                                            binding.as_ref()
                                                                .map(|item| format!("{} · {}", item.effective_source, item.effective_proxy_url.clone().unwrap_or_else(|| "-".to_string())))
                                                                .unwrap_or_else(|| "loading".to_string())
                                                        }
                                                    </div>
                                                </div>
                                                <button
                                                    class={classes!("btn-terminal", "btn-terminal-primary")}
                                                    onclick={{
                                                        let on_save_proxy_binding = on_save_proxy_binding.clone();
                                                        let provider_name = provider_name.clone();
                                                        Callback::from(move |_| on_save_proxy_binding.emit(provider_name.clone()))
                                                    }}
                                                    disabled={(*saving_proxy_binding_provider).as_deref() == Some(provider_name.as_str())}
                                                >
                                                    {
                                                        if (*saving_proxy_binding_provider).as_deref() == Some(provider_name.as_str()) {
                                                            "保存中..."
                                                        } else {
                                                            "保存绑定"
                                                        }
                                                    }
                                                </button>
                                            </div>
                                            <label class={classes!("mt-4", "block", "text-sm")}>
                                                <span class={classes!("text-[var(--muted)]")}>{ "绑定到代理配置" }</span>
                                                <select
                                                    key={select_key}
                                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                                    value={selected_value.clone()}
                                                    onchange={on_change}
                                                >
                                                    <option value="" selected={selected_value.is_empty()}>{ "Env fallback" }</option>
                                                    { for proxy_configs.iter().map(|proxy_config| html! {
                                                        <option value={proxy_config.id.clone()} selected={selected_value == proxy_config.id}>
                                                            { format!("{} · {}", proxy_config.name, proxy_config.proxy_url) }
                                                        </option>
                                                    }) }
                                                </select>
                                            </label>
                                            if let Some(binding) = binding {
                                                <div class={classes!("mt-3", "space-y-1", "text-xs", "text-[var(--muted)]")}>
                                                    <p class={classes!("m-0")}>
                                                        { format!("effective_source: {}", binding.effective_source) }
                                                    </p>
                                                    <p class={classes!("m-0", "font-mono", "break-all")}>
                                                        { format!("effective_proxy_url: {}", binding.effective_proxy_url.unwrap_or_else(|| "-".to_string())) }
                                                    </p>
                                                    if let Some(error_message) = binding.error_message {
                                                        <p class={classes!("m-0", "text-red-600", "dark:text-red-300")}>
                                                            { format!("error: {}", error_message) }
                                                        </p>
                                                    }
                                                </div>
                                            }
                                        </article>
                                    }
                                })
                            }
                        </div>
                        <div class={classes!("mt-4", "rounded-xl", "border", "border-dashed", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                            <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                <div>
                                    <h3 class={classes!("m-0", "text-sm", "font-semibold")}>{ "Legacy Kiro Proxy Migration" }</h3>
                                    <p class={classes!("mt-2", "mb-0", "text-xs", "text-[var(--muted)]")}>
                                        { "扫描 ~/.static-flow/auths/kiro/*.json 中遗留的账号级代理字段，导入为共享代理配置，把对应账号切到 fixed 选择，并清掉旧字段。" }
                                    </p>
                                </div>
                                <button class={classes!("btn-terminal")} onclick={on_import_legacy_kiro_proxy} disabled={*migrating_legacy_kiro_proxy}>
                                    { if *migrating_legacy_kiro_proxy { "导入中..." } else { "导入 Legacy Kiro Proxy" } }
                                </button>
                            </div>
                        </div>
                    </section>

                    <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                        <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Proxy Config Inventory" }</h2>
                        <p class={classes!("mt-2", "mb-0", "text-xs", "text-[var(--muted)]")}>
                            { proxy_scope_summary }
                        </p>
                        <div class={classes!("mt-3", "grid", "gap-3", "md:grid-cols-2")}>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "Name" }</span>
                                <input
                                    type="text"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*create_proxy_name).clone()}
                                    disabled={!can_create_proxy_config}
                                    oninput={{
                                        let create_proxy_name = create_proxy_name.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                create_proxy_name.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm", "md:col-span-2")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "Proxy URL" }</span>
                                <input
                                    type="text"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "font-mono")}
                                    value={(*create_proxy_url).clone()}
                                    disabled={!can_create_proxy_config}
                                    oninput={{
                                        let create_proxy_url = create_proxy_url.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                create_proxy_url.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "Proxy Username" }</span>
                                <input
                                    type="text"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*create_proxy_username).clone()}
                                    disabled={!can_create_proxy_config}
                                    oninput={{
                                        let create_proxy_username = create_proxy_username.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                create_proxy_username.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "Proxy Password" }</span>
                                <input
                                    type="text"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*create_proxy_password).clone()}
                                    disabled={!can_create_proxy_config}
                                    oninput={{
                                        let create_proxy_password = create_proxy_password.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                create_proxy_password.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <div class={classes!("md:col-span-2")}>
                                <button class={classes!("btn-terminal", "btn-terminal-primary")} onclick={on_create_proxy_config} disabled={*creating_proxy || !can_create_proxy_config}>
                                    { if *creating_proxy { "创建中..." } else if can_create_proxy_config { "创建代理配置" } else { "edge 节点不可创建槽位" } }
                                </button>
                            </div>
                        </div>
                        // Search & filter for proxy configs
                        <div class={classes!("mt-4", "border-t", "border-[var(--border)]", "pt-4")}>
                        <div class={classes!("flex", "items-center", "gap-2")}>
                            <div class={classes!("relative", "flex-1")}>
                                <input
                                    type="text"
                                    class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "pr-16", "text-sm", "placeholder:text-[var(--muted)]")}
                                    placeholder="搜索代理配置..."
                                    value={(*proxy_config_search).clone()}
                                    oninput={on_proxy_search_input.clone()}
                                    onkeydown={on_proxy_search_keydown.clone()}
                                />
                                if !(*proxy_config_search).is_empty() {
                                    <button
                                        type="button"
                                        class={classes!("absolute", "right-10", "top-1/2", "-translate-y-1/2", "text-[var(--muted)]", "hover:text-[var(--text)]", "text-sm", "px-1")}
                                        onclick={on_proxy_search_clear.clone()}
                                    >
                                        { "✕" }
                                    </button>
                                }
                                <button
                                    type="button"
                                    class={classes!("absolute", "right-2", "top-1/2", "-translate-y-1/2", "rounded", "bg-[var(--primary)]", "px-2", "py-0.5", "text-xs", "text-white")}
                                    onclick={{
                                        let on_proxy_search_submit = on_proxy_search_submit.clone();
                                        Callback::from(move |_: MouseEvent| on_proxy_search_submit.emit(()))
                                    }}
                                >
                                    { "搜索" }
                                </button>
                            </div>
                            <button
                                type="button"
                                class={classes!(
                                    "rounded-full", "px-3", "py-1.5", "text-xs", "font-semibold", "border", "transition-colors",
                                    if *proxy_config_show_active_only {
                                        "bg-emerald-500/15 text-emerald-700 dark:text-emerald-300 border-emerald-400/50"
                                    } else {
                                        "bg-[var(--surface)] text-[var(--muted)] border-[var(--border)] hover:text-[var(--text)]"
                                    }
                                )}
                                onclick={{
                                    let proxy_config_show_active_only = proxy_config_show_active_only.clone();
                                    Callback::from(move |_| {
                                        proxy_config_show_active_only.set(!*proxy_config_show_active_only);
                                    })
                                }}
                            >
                                { "Active" }
                            </button>
                        </div>
                        <div class={classes!("mt-2", "text-xs", "text-[var(--muted)]")}>
                            { format!("共 {} 个配置 (匹配 {})", proxy_configs.len(), proxy_configs_filtered.len()) }
                        </div>
                        <div class={classes!("mt-3", "grid", "gap-4")}>
                            if proxy_configs_filtered.is_empty() {
                                <div class={classes!("rounded-xl", "border", "border-dashed", "border-[var(--border)]", "px-4", "py-10", "text-center", "text-[var(--muted)]")}>
                                    { if (*proxy_configs).is_empty() {
                                        "当前还没有可复用的代理配置。"
                                    } else {
                                        "没有匹配的代理配置。尝试调整搜索条件或清除筛选。"
                                    }}
                                </div>
                            } else {
                                { for proxy_configs_filtered.iter().map(|proxy_config| html! {
                                    <ProxyConfigEditorCard
                                        key={proxy_config.id.clone()}
                                        proxy_config={(*proxy_config).clone()}
                                        on_changed={reload.clone()}
                                        on_copy={on_copy.clone()}
                                        on_flash={flash.clone()}
                                    />
                                }) }
                            }
                        </div>
                        </div>
                    </section>
                </div>
                } // end TAB_SETTINGS

                // ── Keys Tab ──
                if *active_tab == TAB_KEYS {
                <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Key Inventory" }</h2>
                        <button class={classes!("btn-terminal")} onclick={{
                            let reload = reload.clone();
                            Callback::from(move |_| reload.emit(()))
                        }}>
                            { if *loading { "刷新中..." } else { "刷新" } }
                        </button>
                    </div>
                    <div class={classes!("mt-4", "max-w-md")}>
                        <SearchBox
                            value={(*keys_search).clone()}
                            on_change={on_keys_search_change.clone()}
                            placeholder={AttrValue::Static("搜索 key 名称 / id / provider / 状态")}
                        />
                    </div>
                    // Sort & filter toolbar
                    <div class={classes!("mt-3", "flex", "items-center", "gap-2", "flex-wrap")}>
                        <button
                            type="button"
                            class={classes!(
                                "rounded-full", "px-3", "py-1.5", "text-xs", "font-semibold", "border", "transition-colors",
                                if *keys_show_active_only {
                                    "bg-emerald-500/15 text-emerald-700 dark:text-emerald-300 border-emerald-400/50"
                                } else {
                                    "bg-[var(--surface)] text-[var(--muted)] border-[var(--border)] hover:text-[var(--text)]"
                                }
                            )}
                            onclick={{
                                let keys_show_active_only = keys_show_active_only.clone();
                                let keys_page = keys_page.clone();
                                Callback::from(move |_| {
                                    keys_show_active_only.set(!*keys_show_active_only);
                                    keys_page.set(1);
                                })
                            }}
                        >
                            { "Active" }
                        </button>
                        <span class={classes!("w-px", "h-5", "bg-[var(--border)]")} />
                        <button
                            type="button"
                            class={classes!(
                                "rounded-full", "px-3", "py-1.5", "text-xs", "font-semibold", "border", "transition-colors",
                                if matches!(*keys_sort_mode, KeySortMode::QuotaAsc | KeySortMode::QuotaDesc) {
                                    "bg-teal-500/15 text-teal-700 dark:text-teal-300 border-teal-400/50"
                                } else {
                                    "bg-[var(--surface)] text-[var(--muted)] border-[var(--border)] hover:text-[var(--text)]"
                                }
                            )}
                            onclick={{
                                let keys_sort_mode = keys_sort_mode.clone();
                                let keys_page = keys_page.clone();
                                Callback::from(move |_| {
                                    let next = match *keys_sort_mode {
                                        KeySortMode::QuotaAsc => KeySortMode::QuotaDesc,
                                        KeySortMode::QuotaDesc => KeySortMode::None,
                                        _ => KeySortMode::QuotaAsc,
                                    };
                                    keys_sort_mode.set(next);
                                    keys_page.set(1);
                                })
                            }}
                        >
                            { match *keys_sort_mode {
                                KeySortMode::QuotaAsc => "Quota \u{2191}",
                                KeySortMode::QuotaDesc => "Quota \u{2193}",
                                _ => "Quota",
                            }}
                        </button>
                        <button
                            type="button"
                            class={classes!(
                                "rounded-full", "px-3", "py-1.5", "text-xs", "font-semibold", "border", "transition-colors",
                                if matches!(*keys_sort_mode, KeySortMode::UsageAsc | KeySortMode::UsageDesc) {
                                    "bg-violet-500/15 text-violet-700 dark:text-violet-300 border-violet-400/50"
                                } else {
                                    "bg-[var(--surface)] text-[var(--muted)] border-[var(--border)] hover:text-[var(--text)]"
                                }
                            )}
                            onclick={{
                                let keys_sort_mode = keys_sort_mode.clone();
                                let keys_page = keys_page.clone();
                                Callback::from(move |_| {
                                    let next = match *keys_sort_mode {
                                        KeySortMode::UsageAsc => KeySortMode::UsageDesc,
                                        KeySortMode::UsageDesc => KeySortMode::None,
                                        _ => KeySortMode::UsageAsc,
                                    };
                                    keys_sort_mode.set(next);
                                    keys_page.set(1);
                                })
                            }}
                        >
                            { match *keys_sort_mode {
                                KeySortMode::UsageAsc => "Usage \u{2191}",
                                KeySortMode::UsageDesc => "Usage \u{2193}",
                                _ => "Usage",
                            }}
                        </button>
                    </div>
                    <div class={classes!("mt-2", "flex", "items-center", "justify-between", "text-xs", "text-[var(--muted)]")}>
                        <span>{ format!("总数 {} · 当前筛选 {} · 本页 {}", key_summary.total, *keys_total, keys.len()) }</span>
                        if keys_total_pages > 1 {
                            <span class={classes!("font-mono")}>{ format!("{}/{}", keys_current_page, keys_total_pages) }</span>
                        }
                    </div>
                    <div class={classes!("mt-3", "grid", "gap-4", "2xl:grid-cols-2")}>
                        if keys_page_entries.is_empty() {
                            <div class={classes!("rounded-xl", "border", "border-dashed", "border-[var(--border)]", "px-4", "py-10", "text-center", "text-[var(--muted)]")}>
                                { if keys.is_empty() {
                                    "当前还没有可管理的 key。"
                                } else {
                                    "当前过滤条件下没有匹配的 key。"
                                }}
                            </div>
                        } else {
                            { for keys_page_entries.iter().map(|key_item| html! {
                                <KeyEditorCard
                                    key={key_item.id.clone()}
                                    key_item={(*key_item).clone()}
                                    on_changed={reload.clone()}
                                    on_refresh={on_refresh_key.clone()}
                                    on_copy={on_copy.clone()}
                                    on_flash={flash.clone()}
                                    refreshing={(*refreshing_key_id).as_deref() == Some(key_item.id.as_str())}
                                    account_groups={(*account_group_options).clone()}
                                />
                            }) }
                        }
                    </div>
                    <div class={classes!("mt-4")}>
                        <Pagination
                            current_page={keys_current_page}
                            total_pages={keys_total_pages}
                            on_page_change={on_keys_page_change.clone()}
                        />
                    </div>
                </section>
                } // end TAB_KEYS

                if *active_tab == TAB_GROUPS {
                <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                        <div>
                            <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Account Groups" }</h2>
                            <p class={classes!("mt-2", "mb-0", "text-sm", "text-[var(--muted)]")}>
                                { "先为账号分组，再让 key 选择组而不是直接勾账号。固定路由请选择单账号组；自动路由可以选任意组，留空则继续使用全账号池。" }
                            </p>
                        </div>
                        <button
                            class={classes!("btn-terminal")}
                            onclick={{
                                let reload = reload.clone();
                                Callback::from(move |_| reload.emit(()))
                            }}
                            disabled={*loading}
                        >
                            { if *loading { "刷新中..." } else { "刷新账号组" } }
                        </button>
                    </div>

                    <div class={classes!("mt-4", "max-w-md")}>
                        <SearchBox
                            value={(*account_groups_search).clone()}
                            on_change={on_account_groups_search_change.clone()}
                            placeholder={AttrValue::Static("搜索账号组名 / id / 成员账号")}
                        />
                    </div>
                    if !account_groups_query_lower.is_empty() {
                        <p class={classes!("mt-2", "text-xs", "text-[var(--muted)]", "font-mono")}>
                            { format!("当前页匹配 {}/{} · 总数 {}", filtered_account_groups.len(), account_groups_page_items.len(), *account_groups_total) }
                        </p>
                    }

                    <div class={classes!("mt-4", "rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                        <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                            <div>
                                <h3 class={classes!("m-0", "text-sm", "font-semibold")}>{ "创建账号组" }</h3>
                                <p class={classes!("mt-1", "mb-0", "text-xs", "text-[var(--muted)]")}>
                                    { "默认收起，只在需要新增轮询号池时展开。" }
                                </p>
                            </div>
                            <button
                                type="button"
                                class={classes!("btn-terminal")}
                                onclick={on_toggle_account_group_form.clone()}
                            >
                                { if *account_group_form_expanded { "收起 ▲" } else { "展开 ▼" } }
                            </button>
                        </div>
                        if *account_group_form_expanded {
                            <div class={classes!("mt-4", "grid", "gap-3")}>
                                <label class={classes!("text-sm")}>
                                    <span class={classes!("text-[var(--muted)]")}>{ "组名" }</span>
                                    <input
                                        type="text"
                                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                        value={(*create_account_group_name).clone()}
                                        oninput={{
                                            let create_account_group_name = create_account_group_name.clone();
                                            Callback::from(move |event: InputEvent| {
                                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                    create_account_group_name.set(target.value());
                                                }
                                            })
                                        }}
                                    />
                                </label>
                                <div class={classes!("space-y-2")}>
                                    <div class={classes!("text-sm", "text-[var(--muted)]")}>{ "成员账号" }</div>
                                    if *account_group_candidate_loading {
                                        <div class={classes!("rounded-lg", "border", "border-dashed", "border-[var(--border)]", "px-3", "py-3", "text-xs", "text-[var(--muted)]")}>
                                            { "正在加载账号候选..." }
                                        </div>
                                    } else if account_group_candidate_accounts.is_empty() {
                                        <div class={classes!("rounded-lg", "border", "border-dashed", "border-[var(--border)]", "px-3", "py-3", "text-xs", "text-[var(--muted)]")}>
                                            { "当前没有可加入账号组的账号。" }
                                        </div>
                                    } else {
                                        <div class={classes!("grid", "gap-2", "xl:grid-cols-2")}>
                                            { for account_group_candidate_accounts.iter().map(|account| {
                                                let checked = create_account_group_account_names.iter().any(|name| name == &account.name);
                                                let account_name = account.name.clone();
                                                let on_toggle_create_account_group_member =
                                                    on_toggle_create_account_group_member.clone();
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
                                                            if account.status != "disabled" {
                                                                <div class={classes!("mt-1", "font-mono", "text-[11px]", "text-[var(--muted)]")}>
                                                                    { format!(
                                                                        "5h {} / wk {}",
                                                                        account.primary_remaining_percent.map(|value| format!("{value:.0}%")).unwrap_or_else(|| "-".to_string()),
                                                                        account.secondary_remaining_percent.map(|value| format!("{value:.0}%")).unwrap_or_else(|| "-".to_string())
                                                                    ) }
                                                                </div>
                                                            }
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
                                        class={classes!("btn-terminal", "btn-terminal-primary")}
                                        onclick={on_create_account_group}
                                        disabled={*creating_account_group}
                                    >
                                        { if *creating_account_group { "创建中..." } else { "创建账号组" } }
                                    </button>
                                </div>
                            </div>
                        }
                    </div>

                    <div class={classes!("mt-5", "grid", "gap-4", "2xl:grid-cols-2")}>
                        if account_groups_page_items.is_empty() && !*loading {
                            <div class={classes!("rounded-xl", "border", "border-dashed", "border-[var(--border)]", "px-4", "py-10", "text-center", "text-[var(--muted)]")}>
                                { "当前还没有账号组。" }
                            </div>
                        } else if filtered_account_groups.is_empty() {
                            <div class={classes!("rounded-xl", "border", "border-dashed", "border-[var(--border)]", "px-4", "py-6", "text-center", "text-[var(--muted)]")}>
                                { "当前过滤条件下没有匹配的账号组。" }
                            </div>
                        } else {
                            { for filtered_account_groups.iter().map(|group_item| html! {
                                <AccountGroupEditorCard
                                    key={group_item.id.clone()}
                                    group_item={group_item.clone()}
                                    accounts={(*accounts).clone()}
                                    on_changed={reload.clone()}
                                    on_flash={flash.clone()}
                                />
                            }) }
                        }
                    </div>
                    <div class={classes!("mt-4")}>
                        <div class={classes!("mb-2", "text-xs", "text-[var(--muted)]", "font-mono")}>
                            { format!("总数 {} · 第 {}/{} 页 · 每页 {}", *account_groups_total, account_groups_current_page, account_groups_total_pages, *account_groups_page_limit) }
                        </div>
                        <Pagination
                            current_page={account_groups_current_page}
                            total_pages={account_groups_total_pages}
                            on_page_change={on_account_groups_page_change.clone()}
                        />
                    </div>
                </section>
                } // end TAB_GROUPS

                // ── Accounts Tab ──
                if *active_tab == TAB_ACCOUNTS {
                // === Codex Accounts ===
                <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                        <div>
                            <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Codex Accounts" }</h2>
                            <p class={classes!("mt-1", "m-0", "text-xs", "text-[var(--muted)]")}>
                                { format!("已导入 {} 个账号。这里会显示账号状态、usage 刷新健康度和账号级 proxy 配置。", account_summary.total) }
                            </p>
                        </div>
                        <button
                            type="button"
                            class={classes!("btn-terminal")}
                            onclick={{
                                let reload = reload.clone();
                                Callback::from(move |_| reload.emit(()))
                            }}
                            disabled={*loading}
                        >
                            <i class={classes!("fas", if *loading { "fa-spinner animate-spin" } else { "fa-rotate-right" })}></i>
                            { if *loading { "刷新中..." } else { "刷新列表" } }
                        </button>
                    </div>

                    <div class={classes!("mt-3", "flex", "gap-2", "flex-wrap")}>
                        <button
                            type="button"
                            class={classes!("btn-terminal")}
                            onclick={{
                                let show_import_form = show_import_form.clone();
                                Callback::from(move |_| show_import_form.set(!*show_import_form))
                            }}
                        >
                            <i class={classes!("fas", if *show_import_form { "fa-chevron-up" } else { "fa-plus" })}></i>
                            { if *show_import_form { "收起单账号导入" } else { "导入单账号" } }
                        </button>
                        <button
                            type="button"
                            class={classes!("btn-terminal")}
                            onclick={{
                                let show_batch_import_form = show_batch_import_form.clone();
                                Callback::from(move |_| show_batch_import_form.set(!*show_batch_import_form))
                            }}
                        >
                            <i class={classes!("fas", if *show_batch_import_form { "fa-chevron-up" } else { "fa-layer-group" })}></i>
                            { if *show_batch_import_form { "收起批量导入" } else { "批量导入" } }
                        </button>
                    </div>

                    if *show_import_form {
                    <div class={classes!("mt-3", "grid", "gap-3", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                        <div class={classes!("grid", "gap-3", "md:grid-cols-2")}>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "名称 (唯一)" }</span>
                                <input
                                    type="text"
                                    placeholder="my-pro-account"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*import_name).clone()}
                                    oninput={{
                                        let import_name = import_name.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                import_name.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "account_id (可选)" }</span>
                                <input
                                    type="text"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*import_account_id).clone()}
                                    oninput={{
                                        let import_account_id = import_account_id.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                import_account_id.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            </div>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "auth.json（可直接粘贴导入）" }</span>
                                <textarea
                                    rows="4"
                                    placeholder="{\"tokens\":{...}}"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "font-mono", "text-xs")}
                                    value={(*import_raw_auth_json).clone()}
                                    oninput={{
                                        let import_raw_auth_json = import_raw_auth_json.clone();
                                        let import_raw_auth_feedback = import_raw_auth_feedback.clone();
                                        let import_account_id = import_account_id.clone();
                                        let import_id_token = import_id_token.clone();
                                        let import_access_token = import_access_token.clone();
                                        let import_refresh_token = import_refresh_token.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<web_sys::HtmlTextAreaElement>() {
                                                let raw = target.value();
                                                let trimmed = raw.trim().to_string();
                                                import_raw_auth_json.set(raw);
                                                if trimmed.is_empty() {
                                                    import_raw_auth_feedback.set(None);
                                                    return;
                                                }
                                                match parse_admin_codex_auth_json(&trimmed) {
                                                    Ok(parsed) => {
                                                        import_account_id.set(parsed.account_id.unwrap_or_default());
                                                        import_id_token.set(parsed.id_token);
                                                        import_access_token.set(parsed.access_token);
                                                        import_refresh_token.set(parsed.refresh_token);
                                                        import_raw_auth_feedback.set(Some(("已解析并回填可识别字段；提交时会保留完整 JSON".to_string(), false)));
                                                    },
                                                    Err(err) => {
                                                        if trimmed.ends_with('}') || trimmed.contains('\n') {
                                                            import_raw_auth_feedback.set(Some((err, true)));
                                                        } else {
                                                            import_raw_auth_feedback.set(None);
                                                        }
                                                    },
                                                }
                                            }
                                        })
                                    }}
                                />
                                if let Some((message, is_error)) = (*import_raw_auth_feedback).clone() {
                                    <div class={classes!("mt-1", "font-mono", "text-[11px]", if is_error { "text-red-600 dark:text-red-300" } else { "text-emerald-600 dark:text-emerald-300" })}>
                                        { message }
                                    </div>
                                }
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "access_token" }</span>
                            <textarea
                                rows="2"
                                class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "font-mono", "text-xs")}
                                value={(*import_access_token).clone()}
                                oninput={{
                                    let import_access_token = import_access_token.clone();
                                    Callback::from(move |event: InputEvent| {
                                        if let Some(target) = event.target_dyn_into::<web_sys::HtmlTextAreaElement>() {
                                            import_access_token.set(target.value());
                                        }
                                    })
                                }}
                            />
                        </label>
                        <div class={classes!("grid", "gap-3", "md:grid-cols-2")}>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "id_token" }</span>
                                <textarea
                                    rows="2"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "font-mono", "text-xs")}
                                    value={(*import_id_token).clone()}
                                    oninput={{
                                        let import_id_token = import_id_token.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<web_sys::HtmlTextAreaElement>() {
                                                import_id_token.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "refresh_token" }</span>
                                <textarea
                                    rows="2"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "font-mono", "text-xs")}
                                    value={(*import_refresh_token).clone()}
                                    oninput={{
                                        let import_refresh_token = import_refresh_token.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<web_sys::HtmlTextAreaElement>() {
                                                import_refresh_token.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                        </div>
                        <div class={classes!("flex", "justify-end")}>
                            <button class={classes!("btn-terminal", "btn-terminal-primary")} onclick={on_import_account} disabled={*importing}>
                                { if *importing { "导入验证中..." } else { "导入账号" } }
                            </button>
                        </div>
                    </div>
                    } // end show_import_form

                    if *show_batch_import_form {
                    <div class={classes!("mt-3", "grid", "gap-3", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                        <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                            <div>
                                <h3 class={classes!("m-0", "text-sm", "font-semibold", "text-[var(--text)]")}>{ "本地 JSON 数组批量导入" }</h3>
                                <p class={classes!("mt-1", "mb-0", "text-xs", "text-[var(--muted)]")}>
                                    { "每项至少带 name 和 auth_json/tokens。开启验证后会先走默认 Codex 代理做 refresh 校验，再真正入库。" }
                                </p>
                            </div>
                            <label class={classes!("flex", "items-center", "gap-2", "text-xs", "text-[var(--muted)]")}>
                                <input
                                    type="checkbox"
                                    checked={*batch_import_validate_before_import}
                                    onchange={{
                                        let batch_import_validate_before_import =
                                            batch_import_validate_before_import.clone();
                                        Callback::from(move |event: Event| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                batch_import_validate_before_import.set(target.checked());
                                            }
                                        })
                                    }}
                                />
                                <span>{ "提交前 refresh 验证" }</span>
                            </label>
                        </div>
                        <textarea
                            rows="12"
                            placeholder={r#"[
  {
    "name": "codex-a",
    "auth_json": { "refresh_token": "rt-a", "account_id": "acct-a" }
  },
  {
    "name": "codex-b",
    "tokens": { "refresh_token": "rt-b" }
  }
]"#}
                            class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "font-mono", "text-xs")}
                            value={(*batch_import_raw_json).clone()}
                            oninput={{
                                let batch_import_raw_json = batch_import_raw_json.clone();
                                let batch_import_feedback = batch_import_feedback.clone();
                                Callback::from(move |event: InputEvent| {
                                    if let Some(target) = event.target_dyn_into::<web_sys::HtmlTextAreaElement>() {
                                        batch_import_raw_json.set(target.value());
                                        batch_import_feedback.set(None);
                                    }
                                })
                            }}
                        />
                        if let Some((message, is_error)) = (*batch_import_feedback).clone() {
                            <div class={classes!("font-mono", "text-[11px]", if is_error { "text-red-600 dark:text-red-300" } else { "text-emerald-600 dark:text-emerald-300" })}>
                                { message }
                            </div>
                        }
                        <div class={classes!("flex", "justify-end")}>
                            <button
                                class={classes!("btn-terminal", "btn-terminal-primary")}
                                onclick={on_import_account_batch}
                                disabled={*batch_importing}
                            >
                                { if *batch_importing { "创建导入作业中..." } else { "开始批量导入" } }
                            </button>
                        </div>
                    </div>
                    }

                    if !recent_import_jobs.is_empty() || active_import_job.is_some() {
                        <div class={classes!("mt-4", "grid", "gap-4", "xl:grid-cols-2")}>
                            <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                    <div>
                                        <h3 class={classes!("m-0", "text-sm", "font-semibold", "text-[var(--text)]")}>{ "最近导入作业" }</h3>
                                        <p class={classes!("mt-1", "mb-0", "text-xs", "text-[var(--muted)]")}>
                                            { format!("最多展示最近 {} 个作业。", ADMIN_CODEX_IMPORT_JOB_LIST_LIMIT) }
                                        </p>
                                    </div>
                                    if let Some(active_detail) = (*active_import_job).clone() {
                                        <span class={classes!("font-mono", "text-[11px]", codex_import_status_tone(&active_detail.summary.status))}>
                                            { format!("当前查看: {}", active_detail.summary.job_id) }
                                        </span>
                                    }
                                </div>
                                <div class={classes!("mt-3", "space-y-2")}>
                                    { for recent_import_jobs.iter().map(|job| {
                                        let job_id = job.job_id.clone();
                                        let is_selected = (*active_import_job)
                                            .as_ref()
                                            .map(|detail| detail.summary.job_id == job.job_id)
                                            .unwrap_or(false);
                                        let progress = format!(
                                            "{}/{} done · ok {} · skipped {} · failed {}",
                                            job.completed_count,
                                            job.total_count,
                                            job.succeeded_count,
                                            job.skipped_count,
                                            job.failed_count
                                        );
                                        html! {
                                            <button
                                                type="button"
                                                class={classes!(
                                                    "w-full", "rounded-lg", "border", "px-3", "py-2.5", "text-left",
                                                    if is_selected {
                                                        "border-sky-500/30 bg-sky-500/8"
                                                    } else {
                                                        "border-[var(--border)] bg-[var(--surface)]"
                                                    }
                                                )}
                                                onclick={{
                                                    let on_load_import_job = on_load_import_job.clone();
                                                    Callback::from(move |_| on_load_import_job.emit(job_id.clone()))
                                                }}
                                            >
                                                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                                    <span class={classes!("font-mono", "text-xs", "font-semibold", "text-[var(--text)]")}>{ job.job_id.clone() }</span>
                                                    <span class={classes!("font-mono", "text-[11px]", codex_import_status_tone(&job.status))}>{ job.status.clone() }</span>
                                                </div>
                                                <div class={classes!("mt-1", "text-xs", "text-[var(--muted)]")}>
                                                    { progress }
                                                </div>
                                                <div class={classes!("mt-1", "font-mono", "text-[11px]", "text-[var(--muted)]")}>
                                                    { format!("{} · {}", job.source_type, format_ms(job.created_at_ms)) }
                                                </div>
                                            </button>
                                        }
                                    }) }
                                </div>
                            </div>
                            <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                                if let Some(job_detail) = (*active_import_job).clone() {
                                    <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                                        <div>
                                            <h3 class={classes!("m-0", "text-sm", "font-semibold", "text-[var(--text)]")}>{ "导入作业详情" }</h3>
                                            <p class={classes!("mt-1", "mb-0", "font-mono", "text-[11px]", "text-[var(--muted)]")}>
                                                { format!("{} · {} · validate={}", job_detail.summary.job_id, job_detail.summary.source_type, job_detail.summary.validate_before_import) }
                                            </p>
                                        </div>
                                        <span class={classes!("font-mono", "text-[11px]", codex_import_status_tone(&job_detail.summary.status))}>
                                            { job_detail.summary.status.clone() }
                                        </span>
                                    </div>
                                    <div class={classes!("mt-3", "grid", "gap-2", "sm:grid-cols-2", "xl:grid-cols-4")}>
                                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}>
                                            <div class={classes!("text-[11px]", "text-[var(--muted)]")}>{ "总数" }</div>
                                            <div class={classes!("mt-1", "font-mono", "text-sm", "font-semibold")}>{ job_detail.summary.total_count }</div>
                                        </div>
                                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}>
                                            <div class={classes!("text-[11px]", "text-[var(--muted)]")}>{ "成功" }</div>
                                            <div class={classes!("mt-1", "font-mono", "text-sm", "font-semibold", "text-emerald-600", "dark:text-emerald-300")}>{ job_detail.summary.succeeded_count }</div>
                                        </div>
                                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}>
                                            <div class={classes!("text-[11px]", "text-[var(--muted)]")}>{ "跳过" }</div>
                                            <div class={classes!("mt-1", "font-mono", "text-sm", "font-semibold")}>{ job_detail.summary.skipped_count }</div>
                                        </div>
                                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}>
                                            <div class={classes!("text-[11px]", "text-[var(--muted)]")}>{ "失败/冲突" }</div>
                                            <div class={classes!("mt-1", "font-mono", "text-sm", "font-semibold", "text-red-600", "dark:text-red-300")}>{ job_detail.summary.failed_count }</div>
                                        </div>
                                    </div>
                                    if let Some(batch_error_message) = job_detail.summary.batch_error_message.clone() {
                                        <div class={classes!("mt-3", "rounded-lg", "border", "border-red-500/30", "bg-red-500/5", "px-3", "py-2", "font-mono", "text-[11px]", "text-red-600", "dark:text-red-300")}>
                                            { batch_error_message }
                                        </div>
                                    }
                                    <div class={classes!("mt-3", "overflow-x-auto")}>
                                        <table class={classes!("min-w-full", "text-sm")}>
                                            <thead class={classes!("text-left", "text-[11px]", "uppercase", "tracking-wide", "text-[var(--muted)]")}>
                                                <tr>
                                                    <th class={classes!("px-2", "py-2")}>{ "#" }</th>
                                                    <th class={classes!("px-2", "py-2")}>{ "name" }</th>
                                                    <th class={classes!("px-2", "py-2")}>{ "status" }</th>
                                                    <th class={classes!("px-2", "py-2")}>{ "account" }</th>
                                                    <th class={classes!("px-2", "py-2")}>{ "result" }</th>
                                                </tr>
                                            </thead>
                                            <tbody>
                                                { for job_detail.items.iter().map(|item| {
                                                    let account_line = item
                                                        .final_account_id
                                                        .clone()
                                                        .or_else(|| item.requested_account_id.clone())
                                                        .unwrap_or_else(|| "-".to_string());
                                                    let result_line = item
                                                        .imported_account_name
                                                        .clone()
                                                        .or_else(|| item.error_message.clone())
                                                        .unwrap_or_else(|| "-".to_string());
                                                    html! {
                                                        <tr class={classes!("border-t", "border-[var(--border)]", "align-top")}>
                                                            <td class={classes!("px-2", "py-2", "font-mono", "text-[11px]", "text-[var(--muted)]")}>{ item.item_index }</td>
                                                            <td class={classes!("px-2", "py-2")}>
                                                                <div class={classes!("font-mono", "text-xs", "text-[var(--text)]")}>{ item.requested_name.clone() }</div>
                                                                <div class={classes!("mt-1", "text-[11px]", "text-[var(--muted)]")}>
                                                                    { item.validated_at_ms.map(format_ms).unwrap_or_else(|| "-".to_string()) }
                                                                </div>
                                                            </td>
                                                            <td class={classes!("px-2", "py-2", "font-mono", "text-[11px]", codex_import_status_tone(&item.status))}>{ item.status.clone() }</td>
                                                            <td class={classes!("px-2", "py-2", "font-mono", "text-[11px]", "text-[var(--muted)]")}>{ account_line }</td>
                                                            <td class={classes!("px-2", "py-2", "font-mono", "text-[11px]", "text-[var(--muted)]")}>{ result_line }</td>
                                                        </tr>
                                                    }
                                                }) }
                                            </tbody>
                                        </table>
                                    </div>
                                } else {
                                    <div class={classes!("rounded-lg", "border", "border-dashed", "border-[var(--border)]", "px-4", "py-10", "text-center", "text-[var(--muted)]")}>
                                        { "选择一个导入作业后，这里会显示逐条处理结果。" }
                                    </div>
                                }
                            </div>
                        </div>
                    }

                    // Account search + sort + filter toolbar
                    <div class={classes!("mt-4", "space-y-4")}>
                        // Search bar
                        <div class={classes!("flex", "items-center", "gap-2")}>
                            <input
                                type="text"
                                class={classes!("flex-1", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "text-sm", "placeholder:text-[var(--muted)]", "focus:outline-none", "focus:ring-2", "focus:ring-[var(--primary)]/40")}
                                placeholder="搜索账号名称、状态、plan、ID、权重..."
                                value={(*account_search).clone()}
                                oninput={on_account_search_input.clone()}
                                onkeydown={on_account_search_keydown.clone()}
                            />
                            if !(*account_search).is_empty() {
                                <button
                                    type="button"
                                    class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "text-sm", "text-[var(--muted)]", "hover:text-[var(--text)]", "transition-colors")}
                                    onclick={on_account_search_clear.clone()}
                                >
                                    { "清除" }
                                </button>
                            }
                            <button
                                type="button"
                                class={classes!("rounded-lg", "bg-[var(--primary)]", "px-4", "py-2", "text-sm", "font-medium", "text-white", "hover:opacity-90", "transition-opacity")}
                                onclick={Callback::from({
                                    let on_account_search_submit = on_account_search_submit.clone();
                                    move |_| on_account_search_submit.emit(())
                                })}
                            >
                                { "搜索" }
                            </button>
                        </div>
                        // Sort & filter toolbar
                        <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                            <button
                                type="button"
                                class={classes!(
                                    "rounded-full", "px-3", "py-1.5", "text-xs", "font-semibold", "border", "transition-colors",
                                    if *account_show_unhealthy {
                                        "bg-red-500/15 text-red-700 dark:text-red-300 border-red-400/50"
                                    } else {
                                        "bg-[var(--surface)] text-[var(--muted)] border-[var(--border)] hover:text-[var(--text)]"
                                    }
                                )}
                                onclick={{
                                    let account_show_unhealthy = account_show_unhealthy.clone();
                                    let account_page = account_page.clone();
                                    Callback::from(move |_| {
                                        account_show_unhealthy.set(!*account_show_unhealthy);
                                        account_page.set(1);
                                    })
                                }}
                            >
                                { "异常" }
                            </button>
                            <button
                                type="button"
                                class={classes!(
                                    "rounded-full", "px-3", "py-1.5", "text-xs", "font-semibold", "border", "transition-colors",
                                    if *account_show_active_only {
                                        "bg-emerald-500/15 text-emerald-700 dark:text-emerald-300 border-emerald-400/50"
                                    } else {
                                        "bg-[var(--surface)] text-[var(--muted)] border-[var(--border)] hover:text-[var(--text)]"
                                    }
                                )}
                                onclick={{
                                    let account_show_active_only = account_show_active_only.clone();
                                    let account_page = account_page.clone();
                                    Callback::from(move |_| {
                                        account_show_active_only.set(!*account_show_active_only);
                                        account_page.set(1);
                                    })
                                }}
                            >
                                { "Active" }
                            </button>
                            <span class={classes!("w-px", "h-5", "bg-[var(--border)]")} />
                            <button
                                type="button"
                                class={classes!(
                                    "rounded-full", "px-3", "py-1.5", "text-xs", "font-semibold", "border", "transition-colors",
                                    if matches!(*account_sort_mode, AccountSortMode::PrimaryAsc | AccountSortMode::PrimaryDesc) {
                                        "bg-teal-500/15 text-teal-700 dark:text-teal-300 border-teal-400/50"
                                    } else {
                                        "bg-[var(--surface)] text-[var(--muted)] border-[var(--border)] hover:text-[var(--text)]"
                                    }
                                )}
                                onclick={{
                                    let account_sort_mode = account_sort_mode.clone();
                                    let account_page = account_page.clone();
                                    Callback::from(move |_| {
                                        let next = match *account_sort_mode {
                                            AccountSortMode::PrimaryAsc => AccountSortMode::PrimaryDesc,
                                            AccountSortMode::PrimaryDesc => AccountSortMode::None,
                                            _ => AccountSortMode::PrimaryAsc,
                                        };
                                        account_sort_mode.set(next);
                                        account_page.set(1);
                                    })
                                }}
                            >
                                { match *account_sort_mode {
                                    AccountSortMode::PrimaryAsc => "5h ↑",
                                    AccountSortMode::PrimaryDesc => "5h ↓",
                                    _ => "5h",
                                }}
                            </button>
                            <button
                                type="button"
                                class={classes!(
                                    "rounded-full", "px-3", "py-1.5", "text-xs", "font-semibold", "border", "transition-colors",
                                    if matches!(*account_sort_mode, AccountSortMode::SecondaryAsc | AccountSortMode::SecondaryDesc) {
                                        "bg-violet-500/15 text-violet-700 dark:text-violet-300 border-violet-400/50"
                                    } else {
                                        "bg-[var(--surface)] text-[var(--muted)] border-[var(--border)] hover:text-[var(--text)]"
                                    }
                                )}
                                onclick={{
                                    let account_sort_mode = account_sort_mode.clone();
                                    let account_page = account_page.clone();
                                    Callback::from(move |_| {
                                        let next = match *account_sort_mode {
                                            AccountSortMode::SecondaryAsc => AccountSortMode::SecondaryDesc,
                                            AccountSortMode::SecondaryDesc => AccountSortMode::None,
                                            _ => AccountSortMode::SecondaryAsc,
                                        };
                                        account_sort_mode.set(next);
                                        account_page.set(1);
                                    })
                                }}
                            >
                                { match *account_sort_mode {
                                    AccountSortMode::SecondaryAsc => "周限额 ↑",
                                    AccountSortMode::SecondaryDesc => "周限额 ↓",
                                    _ => "周限额",
                                }}
                            </button>
                        </div>
                        // Summary line
                        <div class={classes!("flex", "items-center", "justify-between", "text-xs", "text-[var(--muted)]")}>
                            <span>{ format!("总数 {} · 当前筛选 {} · 本页 {}", account_summary.total, *accounts_total, accounts.len()) }</span>
                            if account_total_pages > 1 {
                                <span>{ format!("第 {} / {} 页", account_current_page, account_total_pages) }</span>
                            }
                        </div>
                    </div>
                    // Account card grid
                    if account_page_entries.is_empty() {
                        <div class={classes!("mt-4", "rounded-lg", "border", "border-dashed", "border-[var(--border)]", "px-4", "py-10", "text-center", "text-sm", "text-[var(--muted)]")}>
                            { if accounts.is_empty() {
                                "当前还没有导入任何 Codex 账号。可以先导入账号，或者点击上方「刷新列表」确认后端是否已加载本地账号文件。"
                            } else {
                                "没有匹配的账号。尝试调整搜索条件或清除筛选。"
                            }}
                        </div>
                    } else {
                        <div class={classes!("mt-4", "grid", "gap-4", "sm:grid-cols-2")}>
                            { for account_page_entries.iter().enumerate().map(|(idx, acc)| {
                                let acc_name_for_toggle = acc.name.clone();
                                let acc_name_for_auto_refresh_toggle = acc.name.clone();
                                let acc_name_for_status_toggle = acc.name.clone();
                                let acc_name_for_delete = acc.name.clone();
                                let acc_name_for_auth_refresh = acc.name.clone();
                                let acc_name_for_usage_refresh = acc.name.clone();
                                let acc_name_for_models_probe = acc.name.clone();
                                let acc_name_for_proxy_change = acc.name.clone();
                                let acc_name_for_route_weight_tier_change = acc.name.clone();
                                let acc_name_for_settings_save = acc.name.clone();
                                let acc_name_for_request_max_change = acc.name.clone();
                                let acc_name_for_request_min_change = acc.name.clone();
                                let acc_name = acc.name.clone();
                                let acc_status = acc.status.clone();
                                let account_disabled = acc_status == "disabled";
                                let toggled_account_status = if account_disabled {
                                    "active".to_string()
                                } else {
                                    "disabled".to_string()
                                };
                                let acc_plan_type = acc.plan_type.clone();
                                let acc_account_id = acc.account_id.clone();
                                let spark_mapping_enabled = acc.map_gpt53_codex_to_spark;
                                let auto_refresh_enabled = acc.auto_refresh_enabled;
                                let selected_proxy_value = (*account_proxy_inputs)
                                    .get(&acc_name)
                                    .cloned()
                                    .unwrap_or_else(|| account_proxy_select_value(acc));
                                let selected_route_weight_tier = (*account_route_weight_tier_inputs)
                                    .get(&acc_name)
                                    .cloned()
                                    .unwrap_or_else(|| {
                                        if acc.route_weight_tier.trim().is_empty() {
                                            "auto".to_string()
                                        } else {
                                            acc.route_weight_tier.clone()
                                        }
                                    });
                                let selected_request_max_value = (*account_request_max_inputs)
                                    .get(&acc_name)
                                    .cloned()
                                    .unwrap_or_else(|| {
                                        acc.request_max_concurrency
                                            .map(|value| value.to_string())
                                            .unwrap_or_default()
                                    });
                                let selected_request_min_value = (*account_request_min_inputs)
                                    .get(&acc_name)
                                    .cloned()
                                    .unwrap_or_else(|| {
                                        acc.request_min_start_interval_ms
                                            .map(|value| value.to_string())
                                            .unwrap_or_default()
                                    });
                                let configured_proxy_line = account_configured_proxy_label(acc);
                                let effective_proxy_line = format!(
                                    "effective: {} · {}",
                                    acc.effective_proxy_source,
                                    acc.effective_proxy_url.clone().unwrap_or_else(|| "direct".to_string())
                                );
                                let scheduler_line = format!(
                                    "scheduler: concurrency {} · start interval {}",
                                    acc.request_max_concurrency
                                        .map(|value| value.to_string())
                                        .unwrap_or_else(|| "∞".to_string()),
                                    acc.request_min_start_interval_ms
                                        .map(|value| format!("{} ms", value))
                                        .unwrap_or_else(|| "∞".to_string())
                                );
                                let last_refresh_line = acc
                                    .last_refresh
                                    .map(format_ms)
                                    .unwrap_or_else(|| "-".to_string());
                                let access_token_expiry_line = format_access_token_expiry(
                                    Date::now() as i64,
                                    acc.access_token_expires_at,
                                );
                                let last_usage_checked_line = acc
                                    .last_usage_checked_at
                                    .map(format_ms)
                                    .unwrap_or_else(|| "-".to_string());
                                let last_usage_success_line = acc
                                    .last_usage_success_at
                                    .map(format_ms)
                                    .unwrap_or_else(|| "-".to_string());
                                let on_delete = on_delete_account.clone();
                                let on_probe_account_models = on_probe_account_models.clone();
                                let on_refresh_account_auth = on_refresh_account_auth.clone();
                                let on_refresh_account_usage = on_refresh_account_usage.clone();
                                let on_toggle_account_status = on_toggle_account_status.clone();
                                let on_toggle_account_spark_mapping =
                                    on_toggle_account_spark_mapping.clone();
                                let on_toggle_account_auto_refresh =
                                    on_toggle_account_auto_refresh.clone();
                                let on_save_account_settings = on_save_account_settings.clone();
                                let primary_pct = acc.primary_remaining_percent
                                    .map(|v| format!("{:.0}%", v))
                                    .unwrap_or_else(|| "-".to_string());
                                let secondary_pct = acc.secondary_remaining_percent
                                    .map(|v| format!("{:.0}%", v))
                                    .unwrap_or_else(|| "-".to_string());
                                let is_pro = is_gpt_pro_account(acc_plan_type.as_deref());
                                let show_spark_toggle = is_pro || spark_mapping_enabled;
                                let account_busy =
                                    (*account_action_inflight).contains(&acc_name);
                                let accent = ACCOUNT_ACCENT_BORDERS[idx % ACCOUNT_ACCENT_BORDERS.len()];
                                html! {
                                    <div class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "overflow-hidden", "transition-all", "duration-200", "hover:shadow-lg", "hover:shadow-black/5", accent)}>
                                        // Card header
                                        <div class={classes!("p-5", "pb-3")}>
                                            <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                                                <span class={classes!(
                                                    "inline-flex", "items-center", "gap-1.5", "shrink-0",
                                                    "rounded-full", "px-2", "py-0.5",
                                                    "font-mono", "text-[10px]", "font-semibold", "uppercase", "tracking-wider",
                                                    "bg-[var(--surface-alt)]",
                                                    match acc_status.as_str() {
                                                        "active" | "ready" => "text-emerald-600",
                                                        "disabled" => "text-red-600",
                                                        _ => "text-[var(--muted)]",
                                                    }
                                                )}>
                                                    <span class={classes!(
                                                        "inline-block", "h-1.5", "w-1.5", "rounded-full",
                                                        match acc_status.as_str() {
                                                            "active" | "ready" => "bg-emerald-500",
                                                            "disabled" => "bg-red-500",
                                                            _ => "bg-slate-400",
                                                        }
                                                    )} />
                                                    { acc_status.clone() }
                                                </span>
                                                <span class={classes!("font-bold", "text-sm", "break-all")}>{ acc_name.clone() }</span>
                                                if let Some(ref plan_type) = acc_plan_type {
                                                    <span class={classes!("rounded-full", "bg-[var(--surface-alt)]", "px-2.5", "py-0.5", "shrink-0", "font-mono", "text-[10px]", "font-medium", "text-[var(--muted)]")}>
                                                        { plan_type.clone() }
                                                    </span>
                                                }
                                            </div>
                                            if acc_status != "disabled" {
                                                <div class={classes!("mt-3", "space-y-2.5")}>
                                                    <div>
                                                        <div class={classes!("flex", "items-center", "justify-between", "gap-2", "mb-1")}>
                                                            <span class={classes!("font-mono", "text-[11px]", "font-semibold", "text-[var(--muted)]", "uppercase", "tracking-wider")}>{ "5H" }</span>
                                                            <span class={classes!("font-mono", "text-sm", "font-black", "text-[var(--text)]")}>{ primary_pct.clone() }</span>
                                                        </div>
                                                        <div class={classes!("h-2", "overflow-hidden", "rounded-full", "bg-[var(--surface-alt)]")}>
                                                            <div class={classes!("h-full", "rounded-full", "transition-[width]", "duration-500", "bg-[linear-gradient(90deg,#0f766e,#14b8a6)]")} style={format!("width: {:.1}%;", acc.primary_remaining_percent.unwrap_or(100.0).clamp(0.0, 100.0))} />
                                                        </div>
                                                    </div>
                                                    <div>
                                                        <div class={classes!("flex", "items-center", "justify-between", "gap-2", "mb-1")}>
                                                            <span class={classes!("font-mono", "text-[11px]", "font-semibold", "text-[var(--muted)]", "uppercase", "tracking-wider")}>{ "WEEK" }</span>
                                                            <span class={classes!("font-mono", "text-sm", "font-black", "text-[var(--text)]")}>{ secondary_pct.clone() }</span>
                                                        </div>
                                                        <div class={classes!("h-2", "overflow-hidden", "rounded-full", "bg-[var(--surface-alt)]")}>
                                                            <div class={classes!("h-full", "rounded-full", "transition-[width]", "duration-500", "bg-[linear-gradient(90deg,#2563eb,#7c3aed)]")} style={format!("width: {:.1}%;", acc.secondary_remaining_percent.unwrap_or(100.0).clamp(0.0, 100.0))} />
                                                        </div>
                                                    </div>
                                                </div>
                                            }
                                            // Info section
                                            <div class={classes!("mt-2", "space-y-0.5", "text-xs", "font-mono", "text-[var(--muted)]")}>
                                                if let Some(ref aid) = acc_account_id {
                                                    <div class={classes!("break-all")}>{ format!("id: {}", aid) }</div>
                                                }
                                                <div>{ configured_proxy_line.clone() }</div>
                                                <div>
                                                    { effective_proxy_line.clone() }
                                                    if let Some(proxy_name) = acc.effective_proxy_config_name.as_deref() {
                                                        { format!(" · {}", proxy_name) }
                                                    }
                                                </div>
                                                <div>{ scheduler_line.clone() }</div>
                                                <div>{ format!("route weight tier: {}", acc.route_weight_tier) }</div>
                                                <div class={classes!("flex", "gap-3", "flex-wrap")}>
                                                    <span>{ if auto_refresh_enabled { "auto refresh on" } else { "auto refresh off" } }</span>
                                                    <span>{ format!("token refresh {}", last_refresh_line) }</span>
                                                    <span>{ access_token_expiry_line.clone() }</span>
                                                </div>
                                                <div class={classes!("flex", "gap-3", "flex-wrap")}>
                                                    <span>{ format!("usage checked {}", last_usage_checked_line) }</span>
                                                    <span>{ format!("usage success {}", last_usage_success_line) }</span>
                                                </div>
                                            </div>
                                            if let Some(auth_error) = acc.auth_refresh_error_message.as_deref() {
                                                <div class={classes!("mt-2", "text-xs", "leading-5", "text-amber-700", "dark:text-amber-300", "break-all")}>
                                                    { format!("auth refresh error: {}", auth_error) }
                                                </div>
                                            }
                                            if let Some(usage_error) = acc.usage_error_message.as_deref() {
                                                <div class={classes!("mt-2", "text-xs", "leading-5", "text-amber-700", "dark:text-amber-300", "break-all")}>
                                                    { format!("usage refresh error: {}", usage_error) }
                                                </div>
                                            }
                                        </div>
                                        // Controls section
                                        <div class={classes!("border-t", "border-[var(--border)]", "px-5", "py-3")}>
                                            <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                                                <input
                                                    type="number"
                                                    class={classes!("w-24", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-2", "py-1.5", "text-xs")}
                                                    placeholder="并发"
                                                    value={selected_request_max_value.clone()}
                                                    oninput={{
                                                        let account_request_max_inputs = account_request_max_inputs.clone();
                                                        Callback::from(move |event: InputEvent| {
                                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                                let mut next = (*account_request_max_inputs).clone();
                                                                next.insert(acc_name_for_request_max_change.clone(), target.value());
                                                                account_request_max_inputs.set(next);
                                                            }
                                                        })
                                                    }}
                                                />
                                                <input
                                                    type="number"
                                                    class={classes!("w-28", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-2", "py-1.5", "text-xs")}
                                                    placeholder="间隔 ms"
                                                    value={selected_request_min_value.clone()}
                                                    oninput={{
                                                        let account_request_min_inputs = account_request_min_inputs.clone();
                                                        Callback::from(move |event: InputEvent| {
                                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                                let mut next = (*account_request_min_inputs).clone();
                                                                next.insert(acc_name_for_request_min_change.clone(), target.value());
                                                                account_request_min_inputs.set(next);
                                                            }
                                                        })
                                                    }}
                                                />
                                                <select
                                                    class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-2", "py-1.5", "text-xs")}
                                                    value={selected_proxy_value.clone()}
                                                    onchange={{
                                                        let account_proxy_inputs = account_proxy_inputs.clone();
                                                        Callback::from(move |event: Event| {
                                                            if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                                                                let mut next = (*account_proxy_inputs).clone();
                                                                next.insert(acc_name_for_proxy_change.clone(), target.value());
                                                                account_proxy_inputs.set(next);
                                                            }
                                                        })
                                                    }}
                                                >
                                                    <option value="inherit" selected={selected_proxy_value == "inherit"}>{ "继承 Proxy" }</option>
                                                    <option value="direct" selected={selected_proxy_value == "direct"}>{ "Direct" }</option>
                                                    { for proxy_configs.iter().map(|proxy_config| {
                                                        let option_value = format!("fixed:{}", proxy_config.id);
                                                        html! {
                                                            <option value={option_value.clone()} selected={selected_proxy_value == option_value}>
                                                                { format!("{} · {}", proxy_config.name, proxy_config.proxy_url) }
                                                            </option>
                                                        }
                                                    }) }
                                                </select>
                                                <select
                                                    class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-2", "py-1.5", "text-xs")}
                                                    value={selected_route_weight_tier.clone()}
                                                    onchange={{
                                                        let account_route_weight_tier_inputs = account_route_weight_tier_inputs.clone();
                                                        Callback::from(move |event: Event| {
                                                            if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                                                                let mut next = (*account_route_weight_tier_inputs).clone();
                                                                next.insert(acc_name_for_route_weight_tier_change.clone(), target.value());
                                                                account_route_weight_tier_inputs.set(next);
                                                            }
                                                        })
                                                    }}
                                                >
                                                    <option value="auto" selected={selected_route_weight_tier == "auto"}>{ "Auto" }</option>
                                                    <option value="free" selected={selected_route_weight_tier == "free"}>{ "Free" }</option>
                                                    <option value="plus" selected={selected_route_weight_tier == "plus"}>{ "Plus" }</option>
                                                    <option value="pro5x" selected={selected_route_weight_tier == "pro5x"}>{ "Pro5x" }</option>
                                                    <option value="pro20x" selected={selected_route_weight_tier == "pro20x"}>{ "Pro20x" }</option>
                                                </select>
                                                <button
                                                    class={classes!("btn-terminal")}
                                                    onclick={Callback::from(move |_| on_save_account_settings.emit(acc_name_for_settings_save.clone()))}
                                                    disabled={account_busy}
                                                >
                                                    { if account_busy { "..." } else { "保存" } }
                                                </button>
                                            </div>
                                            <div class={classes!("mt-2", "flex", "items-center", "gap-2", "flex-wrap")}>
                                                <button
                                                    class={classes!("btn-terminal")}
                                                    onclick={Callback::from(move |_| on_refresh_account_auth.emit(acc_name_for_auth_refresh.clone()))}
                                                    disabled={account_busy}
                                                >
                                                    { if account_busy { "..." } else { "刷新 Token" } }
                                                </button>
                                                <button
                                                    class={classes!("btn-terminal")}
                                                    onclick={Callback::from(move |_| on_refresh_account_usage.emit(acc_name_for_usage_refresh.clone()))}
                                                    disabled={account_busy}
                                                >
                                                    { if account_busy { "..." } else { "刷新 Usage" } }
                                                </button>
                                                <button
                                                    class={classes!("btn-terminal")}
                                                    onclick={Callback::from(move |_| on_probe_account_models.emit(acc_name_for_models_probe.clone()))}
                                                    disabled={account_busy}
                                                >
                                                    { if account_busy { "..." } else { "测试 Models" } }
                                                </button>
                                                <button
                                                    class={classes!(
                                                        "btn-terminal",
                                                        if auto_refresh_enabled { "btn-terminal-primary" } else { "" }
                                                    )}
                                                    onclick={Callback::from(move |_| {
                                                        on_toggle_account_auto_refresh.emit((
                                                            acc_name_for_auto_refresh_toggle.clone(),
                                                            !auto_refresh_enabled,
                                                        ))
                                                    })}
                                                    disabled={account_busy}
                                                >
                                                    { if account_busy { "..." } else if auto_refresh_enabled { "Auto ✓" } else { "Auto ✗" } }
                                                </button>
                                                <button
                                                    class={classes!("btn-terminal")}
                                                    onclick={Callback::from(move |_| {
                                                        on_toggle_account_status.emit((
                                                            acc_name_for_status_toggle.clone(),
                                                            toggled_account_status.clone(),
                                                        ))
                                                    })}
                                                    disabled={account_busy}
                                                >
                                                    { if account_busy { "..." } else if account_disabled { "启用" } else { "禁用" } }
                                                </button>
                                                if show_spark_toggle {
                                                    <button
                                                        class={classes!(
                                                            "btn-terminal",
                                                            if spark_mapping_enabled { "btn-terminal-primary" } else { "" }
                                                        )}
                                                        onclick={Callback::from(move |_| {
                                                            on_toggle_account_spark_mapping.emit((
                                                                acc_name_for_toggle.clone(),
                                                                !spark_mapping_enabled,
                                                            ))
                                                        })}
                                                        disabled={account_busy}
                                                        title="把客户端请求的 gpt-5.3-codex 映射到该账号上游的 gpt-5.3-codex-spark"
                                                    >
                                                        { if account_busy { "..." } else if spark_mapping_enabled { "Spark ✓" } else { "Spark" } }
                                                    </button>
                                                }
                                                <button
                                                    class={classes!("btn-terminal", "!text-red-600", "dark:!text-red-300")}
                                                    onclick={Callback::from(move |_| on_delete.emit(acc_name_for_delete.clone()))}
                                                >
                                                    { "删除" }
                                                </button>
                                            </div>
                                        </div>
                                    </div>
                                }
                            }) }
                        </div>
                        <div class={classes!("mt-4")}>
                            <Pagination
                                current_page={account_current_page}
                                total_pages={account_total_pages}
                                on_page_change={on_account_page_change.clone()}
                            />
                        </div>
                    }
                </section>
                } // end TAB_ACCOUNTS

                // ── Usage Tab ──
                if *active_tab == TAB_USAGE {
                <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <div>
                            <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Usage Events" }</h2>
                            <p class={classes!("m-0", "mt-1", "text-xs", "text-[var(--muted)]")}>
                                { format!("仅展示最近 {} 天的 usage events", *usage_retention_days) }
                            </p>
                        </div>
                        <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                            <span class={classes!("text-xs", "text-[var(--muted)]")}>
                                { format!("{} · {} · {} · {} 条 · p{}", usage_source_label(&usage_source), usage_status_kind_label(&usage_status_kind), usage_time_description(&usage_start_input, &usage_end_input), *usage_total, *usage_page) }
                            </span>
                            <span class={classes!("rounded-full", "border", "border-[var(--border)]", "px-2.5", "py-0.5", "text-xs", "font-semibold", "text-[var(--muted)]")}>
                                { format!("RPM {}", *usage_current_rpm) }
                            </span>
                            <span class={classes!("rounded-full", "border", "border-[var(--border)]", "px-2.5", "py-0.5", "text-xs", "font-semibold", "text-[var(--muted)]")}>
                                { format!("In Flight {}", *usage_current_in_flight) }
                            </span>
                            <button
                                class={classes!("btn-terminal")}
                                title="刷新事件"
                                aria-label="刷新事件"
                                onclick={{
                                    let reload_usage = reload_usage.clone();
                                    Callback::from(move |_| {
                                        reload_usage.emit(UsageReloadArgs::default())
                                    })
                                }}
                                disabled={*usage_loading}
                            >
                                <i class={classes!("fas", if *usage_loading { "fa-spinner animate-spin" } else { "fa-rotate-right" })}></i>
                            </button>
                        </div>
                    </div>

                    <div class={classes!("mt-3", "flex", "flex-col", "gap-2")}>
                        // Row 1: date range picker
                        <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                            <DateRangePicker
                                start_ms={parse_datetime_local_input_to_ms(&usage_start_input)}
                                end_ms={parse_datetime_local_input_to_ms(&usage_end_input)}
                                on_change={{
                                    let usage_start_input = usage_start_input.clone();
                                    let usage_end_input = usage_end_input.clone();
                                    Callback::from(move |(start, end): (Option<i64>, Option<i64>)| {
                                        usage_start_input.set(start.map(format_datetime_local_input).unwrap_or_default());
                                        usage_end_input.set(end.map(format_datetime_local_input).unwrap_or_default());
                                    })
                                }}
                            />
                        </div>
                        // Row 2: key search + key filter dropdown
                        <div class={classes!("grid", "grid-cols-1", "sm:grid-cols-2", "gap-2", "items-end")}>
                            <div class={classes!("text-xs")}>
                                <SearchBox
                                    value={(*usage_key_search).clone()}
                                    on_change={on_usage_key_search_change.clone()}
                                    placeholder={AttrValue::Static("搜索 key 名称 / id / provider")}
                                />
                            </div>
                            <select
                                key={format!("usage-filter-{}-{}", (*usage_key_filter).clone(), usage_key_query_lower)}
                                class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-2.5", "py-1.5", "text-xs")}
                                onchange={on_usage_key_filter_change}
                            >
                                <option value="" selected={(*usage_key_filter).is_empty()}>{ "全部 Key" }</option>
                                if !(*usage_key_filter).is_empty()
                                    && !filtered_usage_keys
                                        .iter()
                                        .any(|key_item| key_item.id.as_str() == (*usage_key_filter).as_str())
                                {
                                    if let Some(selected_key) = keys
                                        .iter()
                                        .find(|key_item| key_item.id.as_str() == (*usage_key_filter).as_str())
                                    {
                                        <option
                                            value={selected_key.id.clone()}
                                            selected=true
                                        >
                                            { format!("{} · {} (当前)", selected_key.name, selected_key.id) }
                                        </option>
                                    }
                                }
                                { for filtered_usage_keys.iter().map(|key_item| html! {
                                    <option
                                        value={key_item.id.clone()}
                                        selected={(*usage_key_filter).as_str() == key_item.id.as_str()}
                                    >
                                        { format!("{} · {}", key_item.name, key_item.id) }
                                    </option>
                                }) }
                            </select>
                        </div>
                        // Row 3: source, status, model, account, endpoint + action buttons
                        <div class={classes!("flex", "items-end", "gap-2", "flex-wrap")}>
                            <select
                                key={format!("usage-source-{}", (*usage_source).clone())}
                                class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-2.5", "py-1.5", "text-xs")}
                                onchange={on_usage_source_change}
                            >
                                <option value={USAGE_SOURCE_HOT} selected={*usage_source == USAGE_SOURCE_HOT}>{ "在线" }</option>
                                <option value={USAGE_SOURCE_ARCHIVE} selected={*usage_source == USAGE_SOURCE_ARCHIVE}>{ "归档" }</option>
                                <option value={USAGE_SOURCE_ALL} selected={*usage_source == USAGE_SOURCE_ALL}>{ "全部" }</option>
                            </select>
                            <select
                                key={format!("usage-status-kind-{}", (*usage_status_kind).clone())}
                                class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-2.5", "py-1.5", "text-xs")}
                                onchange={on_usage_status_kind_change}
                            >
                                <option value={USAGE_STATUS_KIND_ALL} selected={*usage_status_kind == USAGE_STATUS_KIND_ALL}>{ "全部状态" }</option>
                                <option value={USAGE_STATUS_KIND_OK} selected={*usage_status_kind == USAGE_STATUS_KIND_OK}>{ "200" }</option>
                                <option value={USAGE_STATUS_KIND_NON_OK} selected={*usage_status_kind == USAGE_STATUS_KIND_NON_OK}>{ "非200" }</option>
                            </select>
                            <input
                                type="text"
                                list="usage-filter-models"
                                class={classes!("w-28", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-2.5", "py-1.5", "font-mono", "text-xs")}
                                placeholder="model"
                                value={(*usage_model_filter).clone()}
                                oninput={on_usage_model_filter_input}
                            />
                            <datalist id="usage-filter-models">
                                { for usage_filter_options.models.iter().map(|m| html! { <option value={m.clone()} /> }) }
                            </datalist>
                            <input
                                type="text"
                                list="usage-filter-accounts"
                                class={classes!("w-28", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-2.5", "py-1.5", "font-mono", "text-xs")}
                                placeholder="account"
                                value={(*usage_account_filter).clone()}
                                oninput={on_usage_account_filter_input}
                            />
                            <datalist id="usage-filter-accounts">
                                { for usage_filter_options.accounts.iter().map(|a| html! { <option value={a.clone()} /> }) }
                            </datalist>
                            <input
                                type="text"
                                list="usage-filter-endpoints"
                                class={classes!("w-36", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-2.5", "py-1.5", "font-mono", "text-xs")}
                                placeholder="endpoint"
                                value={(*usage_endpoint_filter).clone()}
                                oninput={on_usage_endpoint_filter_input}
                            />
                            <datalist id="usage-filter-endpoints">
                                { for usage_filter_options.endpoints.iter().map(|ep| html! { <option value={ep.clone()} /> }) }
                            </datalist>
                            <button
                                type="button"
                                class={classes!("btn-terminal", "!py-1", "!px-2.5", "!text-xs")}
                                onclick={on_apply_usage_filters}
                                disabled={*usage_loading}
                            >
                                { "查询" }
                            </button>
                            <button
                                type="button"
                                class={classes!("btn-terminal", "!py-1", "!px-2.5", "!text-xs")}
                                onclick={on_clear_usage_filters}
                                disabled={*usage_loading}
                            >
                                { "重置" }
                            </button>
                        </div>
                    </div>

                    <div class={classes!("mt-3", "flex", "items-center", "gap-x-4", "gap-y-1", "flex-wrap", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-4", "py-2", "font-mono", "text-xs")}>
                        <span><span class={classes!("text-[var(--muted)]")}>{ "匹配 " }</span><span class={classes!("font-semibold")}>{ format_number_u64(usage_totals.event_count as u64) }</span></span>
                        <span class={classes!("text-[var(--border)]")}>{ "·" }</span>
                        <span><span class={classes!("text-[var(--muted)]")}>{ "In " }</span><span class={classes!("font-semibold")}>{ format_number_u64(usage_totals.input_uncached_tokens) }</span></span>
                        <span class={classes!("text-[var(--border)]")}>{ "·" }</span>
                        <span><span class={classes!("text-[var(--muted)]")}>{ "Cached " }</span><span class={classes!("font-semibold")}>{ format_number_u64(usage_totals.input_cached_tokens) }</span></span>
                        <span class={classes!("text-[var(--border)]")}>{ "·" }</span>
                        <span><span class={classes!("text-[var(--muted)]")}>{ "Out " }</span><span class={classes!("font-semibold")}>{ format_number_u64(usage_totals.output_tokens) }</span></span>
                        <span class={classes!("text-[var(--border)]")}>{ "·" }</span>
                        <span><span class={classes!("text-[var(--muted)]")}>{ "Billable " }</span><span class={classes!("font-semibold")}>{ format_number_u64(usage_totals.billable_tokens) }</span></span>
                    </div>

                    if !usage_key_query_lower.is_empty() {
                        <div class={classes!("mt-2", "flex", "items-center", "gap-2", "flex-wrap", "text-xs", "font-mono", "text-[var(--muted)]")}>
                            <span>{ format!("匹配 {}/{}", filtered_usage_keys.len(), keys.len()) }</span>
                            if filtered_usage_keys.is_empty() {
                                <span>{ "没有匹配的 key" }</span>
                            } else {
                                { for filtered_usage_keys.iter().take(8).map(|key_item| {
                                    let key_id = key_item.id.clone();
                                    let active = (*usage_key_filter).as_str() == key_item.id.as_str();
                                    let on_usage_key_pick = on_usage_key_pick.clone();
                                    html! {
                                        <button
                                            type="button"
                                            class={classes!(
                                                "rounded-full",
                                                "border",
                                                "px-2.5",
                                                "py-1",
                                                "text-xs",
                                                "font-semibold",
                                                if active { "border-emerald-500/50" } else { "border-[var(--border)]" },
                                                if active { "bg-emerald-500/12" } else { "bg-[var(--surface-alt)]" },
                                                if active { "text-emerald-700" } else { "text-[var(--text)]" },
                                                if active { "dark:text-emerald-200" } else { "dark:text-[var(--text)]" },
                                            )}
                                            onclick={Callback::from(move |_| on_usage_key_pick.emit(key_id.clone()))}
                                        >
                                            { format!("{} · {}", key_item.name, key_item.id) }
                                        </button>
                                    }
                                }) }
                                if filtered_usage_keys.len() > 8 {
                                    <span>{ format!("另有 {} 个匹配项", filtered_usage_keys.len() - 8) }</span>
                                }
                            }
                        </div>
                    }

                    if *usage_loading {
                        <div class={classes!("mt-3", "inline-flex", "items-center", "gap-2", "text-xs", "text-[var(--muted)]")}>
                            <i class={classes!("fas", "fa-spinner", "animate-spin")} />
                            <span>{ "加载中" }</span>
                        </div>
                    }
                    if let Some(err) = (*usage_error).clone() {
                        <div class={classes!("mt-3", "rounded-lg", "border", "border-red-400/35", "bg-red-500/8", "px-4", "py-3", "text-sm", "text-red-700", "dark:text-red-200")}>
                            <div class={classes!("font-semibold")}>{ "查询失败" }</div>
                            <pre class={classes!("mt-2", "m-0", "whitespace-pre-wrap", "break-all", "font-mono", "text-xs")}>{ err }</pre>
                        </div>
                    }

                    <div class={classes!("mt-4", "overflow-x-auto", "rounded-xl", "border", "border-[var(--border)]")}>
                        <table class={classes!("min-w-[64rem]", "w-full", "text-sm")}>
                            <thead>
                                <tr class={classes!("text-left", "text-[var(--muted)]")}>
                                    <th class={classes!("py-2", "pl-3", "pr-3")}>{ "时间" }</th>
                                    <th class={classes!("py-2", "pr-3")}>{ "Key" }</th>
                                    <th class={classes!("py-2", "pr-3")}>{ "号池" }</th>
                                    <th class={classes!("py-2", "pr-3")}>{ "Model" }</th>
                                    <th class={classes!("py-2", "pr-3")}>{ "Status" }</th>
                                    <th class={classes!("py-2", "pr-3")}>{ "Latency" }</th>
                                    <th class={classes!("py-2", "pr-3")}>{ "Tokens" }</th>
                                    <th class={classes!("py-2", "pr-3")}>{ "" }</th>
                                </tr>
                            </thead>
                            <tbody>
                                if usage_events.is_empty() && !*loading && !*usage_loading && (*usage_error).is_none() {
                                    <tr class={classes!("border-t", "border-[var(--border)]")}>
                                        <td colspan="8" class={classes!("py-8", "text-center", "text-[var(--muted)]")}>{ "当前筛选下还没有 usage 事件" }</td>
                                    </tr>
                                } else {
                                    { for usage_events.iter().map(|event| {
                                        let event_id_for_detail = event.id.clone();
                                        let account_label = usage_account_label(
                                            &event.account_name,
                                            &event.request_url,
                                            &event.endpoint,
                                        );
                                        let latency_ms_val = event.latency_ms;
                                        let latency_color = if latency_ms_val < 3000 {
                                            ("border-emerald-500/20", "bg-emerald-500/10", "text-emerald-700", "dark:text-emerald-200")
                                        } else if latency_ms_val < 10000 {
                                            ("border-amber-500/20", "bg-amber-500/10", "text-amber-700", "dark:text-amber-200")
                                        } else {
                                            ("border-red-500/20", "bg-red-500/10", "text-red-700", "dark:text-red-200")
                                        };
                                        let status_ok = event.status_code == 200;
                                        html! {
                                            <tr class={classes!("border-t", "border-[var(--border)]", "align-top")}>
                                                <td class={classes!("py-2.5", "pl-3", "pr-3", "whitespace-nowrap")}>
                                                    <div class={classes!("text-xs")}>{ format_ms(event.created_at) }</div>
                                                    <div class={classes!("mt-0.5", "flex", "items-center", "gap-1")}>
                                                        <span class={classes!("max-w-[7rem]", "truncate", "font-mono", "text-[10px]", "text-[var(--muted)]")} title={event.id.clone()}>
                                                            { event.id.clone() }
                                                        </span>
                                                        { copy_icon_button(&event.id, &on_copy) }
                                                    </div>
                                                </td>
                                                <td class={classes!("py-2.5", "pr-3")}>
                                                    <div class={classes!("text-xs", "font-semibold", "text-[var(--text)]", "truncate", "max-w-[10rem]")} title={event.key_name.clone()}>{ event.key_name.clone() }</div>
                                                    <div class={classes!("font-mono", "text-[10px]", "text-[var(--muted)]")}>{ event.key_id.clone() }</div>
                                                </td>
                                                <td class={classes!("py-2.5", "pr-3")}>
                                                    <span class={classes!("inline-flex", "rounded-full", "border", "border-emerald-500/20", "bg-emerald-500/10", "px-2", "py-0.5", "text-[11px]", "font-semibold", "text-emerald-700", "dark:text-emerald-200")}>
                                                        { account_label }
                                                    </span>
                                                </td>
                                                <td class={classes!("py-2.5", "pr-3")}>
                                                    <div class={classes!("text-xs", "truncate", "max-w-[10rem]")} title={event.model.clone().unwrap_or_default()}>
                                                        { event.model.clone().unwrap_or_else(|| "-".to_string()) }
                                                    </div>
                                                    if event.usage_missing {
                                                        <span class={classes!("inline-flex", "rounded-full", "border", "border-amber-500/20", "bg-amber-500/10", "px-1.5", "py-0.5", "text-[10px]", "font-semibold", "text-amber-700", "dark:text-amber-200")}>
                                                            { token_usage_missing_label() }
                                                        </span>
                                                    }
                                                </td>
                                                <td class={classes!("py-2.5", "pr-3", "whitespace-nowrap")}>
                                                    <span class={classes!(
                                                        "inline-flex", "h-5", "w-5", "items-center", "justify-center", "rounded-full", "text-[10px]", "font-bold",
                                                        if status_ok { "bg-emerald-500/15" } else { "bg-red-500/15" },
                                                        if status_ok { "text-emerald-700" } else { "text-red-700" },
                                                        if status_ok { "dark:text-emerald-200" } else { "dark:text-red-200" },
                                                    )} title={format!("{}", event.status_code)}>
                                                        { if status_ok { "" } else { "!" } }
                                                    </span>
                                                    <span class={classes!("ml-1", "text-xs", "font-mono")}>{ event.status_code }</span>
                                                </td>
                                                <td class={classes!("py-2.5", "pr-3", "whitespace-nowrap")}>
                                                    <span class={classes!("inline-flex", "rounded-full", "border", "px-2", "py-0.5", "text-[11px]", "font-semibold", latency_color.0, latency_color.1, latency_color.2, latency_color.3)}>
                                                        { format_latency_ms(event.latency_ms) }
                                                    </span>
                                                    <div class={classes!("mt-0.5", "text-[10px]", "text-[var(--muted)]")}>
                                                        { if let Some(first_ms) = event.first_sse_write_ms {
                                                            format!("首字 {}ms", first_ms.max(0))
                                                        } else {
                                                            "-".to_string()
                                                        }}
                                                    </div>
                                                </td>
                                                <td class={classes!("py-2.5", "pr-3", "whitespace-nowrap", "font-mono", "text-[11px]")}>
                                                    <span class={classes!("text-[var(--muted)]")}>
                                                        { format!("{}/{}/{}", format_number_u64(event.input_uncached_tokens), format_number_u64(event.input_cached_tokens), format_number_u64(event.output_tokens)) }
                                                    </span>
                                                </td>
                                                <td class={classes!("py-2.5", "pr-3")}>
                                                    <button
                                                        type="button"
                                                        class={classes!(
                                                            "inline-flex",
                                                            "h-7",
                                                            "w-7",
                                                            "items-center",
                                                            "justify-center",
                                                            "rounded-lg",
                                                            "border",
                                                            "border-[var(--border)]",
                                                            "bg-[var(--surface)]",
                                                            "text-[var(--muted)]",
                                                            "transition-colors",
                                                            "hover:text-[var(--primary)]",
                                                            "hover:bg-[var(--surface-alt)]"
                                                        )}
                                                        title="查看请求详情"
                                                        aria-label="查看请求详情"
                                                        onclick={{
                                                            let open_usage_detail = open_usage_detail.clone();
                                                            Callback::from(move |_| open_usage_detail.emit(event_id_for_detail.clone()))
                                                        }}
                                                    >
                                                        <i class={classes!("fas", "fa-bars-staggered", "text-xs")}></i>
                                                    </button>
                                                </td>
                                            </tr>
                                        }
                                    }) }
                                }
                            </tbody>
                        </table>
                    </div>

                    <div class={classes!("mt-5")}>
                        <Pagination current_page={*usage_page} total_pages={usage_total_pages} on_page_change={on_usage_page_change} />
                    </div>
                </section>
                } // end TAB_USAGE

                // ── Requests Tab ──
                if *active_tab == TAB_REQUESTS {
                <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <div>
                            <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Token Wishes" }</h2>
                            <p class={classes!("mt-1", "m-0", "text-xs", "text-[var(--muted)]")}>
                                { "只有在这里审核通过后，系统才会真正创建 key 并通过邮件发给申请人。" }
                            </p>
                        </div>
                        <button
                            class={classes!("btn-terminal")}
                            onclick={{
                                let reload_token_requests = reload_token_requests.clone();
                                Callback::from(move |_| reload_token_requests.emit((None, None)))
                            }}
                            disabled={*token_request_loading}
                        >
                            <i class={classes!("fas", if *token_request_loading { "fa-spinner animate-spin" } else { "fa-rotate-right" })}></i>
                        </button>
                    </div>

                    <div class={classes!("mt-3", "grid", "gap-3", "md:grid-cols-[minmax(0,16rem)_auto]")}>
                        <label class={classes!("text-sm")}>
                            <span class={classes!("text-[var(--muted)]")}>{ "状态" }</span>
                            <select
                                key={format!("token-request-filter-{}", (*token_request_status_filter).clone())}
                                class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                onchange={on_token_request_status_filter_change}
                            >
                                <option value="" selected={(*token_request_status_filter).is_empty()}>{ "全部" }</option>
                                <option value="pending" selected={*token_request_status_filter == "pending"}>{ "pending" }</option>
                                <option value="failed" selected={*token_request_status_filter == "failed"}>{ "failed" }</option>
                                <option value="issued" selected={*token_request_status_filter == "issued"}>{ "issued" }</option>
                                <option value="rejected" selected={*token_request_status_filter == "rejected"}>{ "rejected" }</option>
                            </select>
                        </label>
                    </div>

                    if token_requests.is_empty() && !*token_request_loading {
                        <div class={classes!("mt-4", "rounded-xl", "border", "border-dashed", "border-[var(--border)]", "px-4", "py-10", "text-center", "text-[var(--muted)]")}>
                            { "当前筛选下还没有 token 许愿。" }
                        </div>
                    } else {
                        <div class={classes!("mt-4", "space-y-3")}>
                            { for token_requests.iter().map(|item| {
                                let request_id = item.request_id.clone();
                                let approve_request_id = item.request_id.clone();
                                let reject_request_id = item.request_id.clone();
                                let approve_cb = on_approve_token_request.clone();
                                let reject_cb = on_reject_token_request.clone();
                                let action_busy = token_request_action_inflight.contains(&request_id);
                                let status_class = match item.status.as_str() {
                                    "pending" => classes!("bg-amber-500/10", "text-amber-700", "dark:text-amber-200", "border-amber-500/20"),
                                    "failed" => classes!("bg-red-500/10", "text-red-700", "dark:text-red-200", "border-red-500/20"),
                                    "issued" => classes!("bg-emerald-500/10", "text-emerald-700", "dark:text-emerald-200", "border-emerald-500/20"),
                                    "rejected" => classes!("bg-slate-500/10", "text-slate-700", "dark:text-slate-200", "border-slate-500/20"),
                                    _ => classes!("bg-[var(--surface-alt)]", "text-[var(--muted)]", "border-[var(--border)]"),
                                };
                                html! {
                                    <article class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-4")}>
                                        <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                                            <div class={classes!("min-w-0", "space-y-1")}>
                                                <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                                                    <span class={classes!("inline-flex", "rounded-full", "border", "px-2.5", "py-1", "text-xs", "font-semibold", status_class)}>
                                                        { item.status.clone() }
                                                    </span>
                                                    <span class={classes!("font-semibold")}>{ item.requester_email.clone() }</span>
                                                    <span class={classes!("text-xs", "font-mono", "text-[var(--muted)]")}>{ item.request_id.clone() }</span>
                                                </div>
                                                <div class={classes!("text-xs", "text-[var(--muted)]")}>
                                                    { format!("{} / {} · created {}", item.client_ip, item.ip_region, format_ms(item.created_at)) }
                                                </div>
                                            </div>
                                            <div class={classes!("text-right")}>
                                                <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "申请 token" }</div>
                                                <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black")}>{ format_number_u64(item.requested_quota_billable_limit) }</div>
                                            </div>
                                        </div>

                                        <div class={classes!("mt-4", "grid", "gap-3", "xl:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]")}>
                                            <div>
                                                <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "缘由" }</div>
                                                <div class={classes!("mt-2", "whitespace-pre-wrap", "break-words", "text-sm", "leading-6", "text-[var(--text)]")}>
                                                    { item.request_reason.clone() }
                                                </div>
                                            </div>
                                            <div class={classes!("space-y-2", "text-sm")}>
                                                if let Some(frontend_page_url) = item.frontend_page_url.clone() {
                                                    <div>
                                                        <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "页面" }</div>
                                                        <div class={classes!("mt-1", "break-all", "text-[var(--text)]")}>{ frontend_page_url }</div>
                                                    </div>
                                                }
                                                if let Some(issued_key_name) = item.issued_key_name.clone() {
                                                    <div>
                                                        <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "已发放 Key" }</div>
                                                        <div class={classes!("mt-1", "text-[var(--text)]")}>
                                                            { format!("{} ({})", issued_key_name, item.issued_key_id.clone().unwrap_or_default()) }
                                                        </div>
                                                    </div>
                                                }
                                                if let Some(admin_note) = item.admin_note.clone() {
                                                    <div>
                                                        <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Admin Note" }</div>
                                                        <div class={classes!("mt-1", "whitespace-pre-wrap", "break-words", "text-[var(--text)]")}>{ admin_note }</div>
                                                    </div>
                                                }
                                                if let Some(failure_reason) = item.failure_reason.clone() {
                                                    <div class={classes!("rounded-lg", "border", "border-red-400/25", "bg-red-500/8", "px-3", "py-2", "text-red-700", "dark:text-red-200")}>
                                                        { failure_reason }
                                                    </div>
                                                }
                                            </div>
                                        </div>

                                        <div class={classes!("mt-4", "flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                            <div class={classes!("text-xs", "text-[var(--muted)]")}>
                                                { item.processed_at.map(format_ms).map(|value| format!("processed {}", value)).unwrap_or_else(|| "尚未处理".to_string()) }
                                            </div>
                                            <div class={classes!("flex", "items-center", "gap-2")}>
                                                if item.status == "pending" || item.status == "failed" {
                                                    <button
                                                        class={classes!("btn-terminal", "btn-terminal-primary")}
                                                        onclick={Callback::from(move |_| approve_cb.emit(approve_request_id.clone()))}
                                                        disabled={action_busy}
                                                    >
                                                        { if action_busy { "处理中..." } else { "批准并发放" } }
                                                    </button>
                                                }
                                                if item.status == "pending" || item.status == "failed" {
                                                    <button
                                                        class={classes!("btn-terminal", "!text-red-600", "dark:!text-red-300")}
                                                        onclick={Callback::from(move |_| reject_cb.emit(reject_request_id.clone()))}
                                                        disabled={action_busy}
                                                    >
                                                        { "拒绝" }
                                                    </button>
                                                }
                                            </div>
                                        </div>
                                    </article>
                                }
                            }) }
                        </div>
                    }

                    <div class={classes!("mt-5")}>
                        <Pagination current_page={*token_request_page} total_pages={token_request_total_pages} on_page_change={on_token_request_page_change} />
                    </div>
                </section>

                <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <div>
                            <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Account Contributions" }</h2>
                                <p class={classes!("mt-1", "m-0", "text-xs", "text-[var(--muted)]")}>
                                    { "公开页提交的 Codex 账号贡献申请会先进入这里；先验证 auth refresh，validated 后才能入库并发放绑定该账号路由的 token。" }
                                </p>
                        </div>
                        <button
                            class={classes!("btn-terminal")}
                            onclick={{
                                let reload_account_contribution_requests = reload_account_contribution_requests.clone();
                                Callback::from(move |_| reload_account_contribution_requests.emit((None, None)))
                            }}
                            disabled={*account_contribution_request_loading}
                        >
                            <i class={classes!("fas", if *account_contribution_request_loading { "fa-spinner animate-spin" } else { "fa-rotate-right" })}></i>
                        </button>
                    </div>

                    <div class={classes!("mt-3", "grid", "gap-3", "md:grid-cols-[minmax(0,16rem)_auto]")}>
                        <label class={classes!("text-sm")}>
                            <span class={classes!("text-[var(--muted)]")}>{ "状态" }</span>
                            <select
                                key={format!("account-contribution-filter-{}", (*account_contribution_request_status_filter).clone())}
                                class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                onchange={on_account_contribution_status_filter_change}
                            >
                                <option value="" selected={(*account_contribution_request_status_filter).is_empty()}>{ "全部" }</option>
                                    <option value="pending" selected={*account_contribution_request_status_filter == "pending"}>{ "pending" }</option>
                                    <option value="validated" selected={*account_contribution_request_status_filter == "validated"}>{ "validated" }</option>
                                    <option value="failed" selected={*account_contribution_request_status_filter == "failed"}>{ "failed" }</option>
                                <option value="issued" selected={*account_contribution_request_status_filter == "issued"}>{ "issued" }</option>
                                <option value="rejected" selected={*account_contribution_request_status_filter == "rejected"}>{ "rejected" }</option>
                            </select>
                        </label>
                    </div>

                    if account_contribution_requests.is_empty() && !*account_contribution_request_loading {
                        <div class={classes!("mt-4", "rounded-xl", "border", "border-dashed", "border-[var(--border)]", "px-4", "py-10", "text-center", "text-[var(--muted)]")}>
                            { "当前筛选下还没有账号贡献申请。" }
                        </div>
                    } else {
                        <div class={classes!("mt-4", "space-y-3")}>
                            { for account_contribution_requests.iter().map(|item| {
                                    let request_id = item.request_id.clone();
                                    let validate_request_id = item.request_id.clone();
                                    let approve_request_id = item.request_id.clone();
                                    let reject_request_id = item.request_id.clone();
                                    let validate_cb = on_validate_account_contribution_request.clone();
                                    let approve_cb = on_approve_account_contribution_request.clone();
                                let reject_cb = on_reject_account_contribution_request.clone();
                                let on_copy = on_copy.clone();
                                let action_busy =
                                    account_contribution_request_action_inflight.contains(&request_id);
                                let status_class = match item.status.as_str() {
                                        "pending" => classes!("bg-amber-500/10", "text-amber-700", "dark:text-amber-200", "border-amber-500/20"),
                                        "validated" => classes!("bg-sky-500/10", "text-sky-700", "dark:text-sky-200", "border-sky-500/20"),
                                        "failed" => classes!("bg-red-500/10", "text-red-700", "dark:text-red-200", "border-red-500/20"),
                                    "issued" => classes!("bg-emerald-500/10", "text-emerald-700", "dark:text-emerald-200", "border-emerald-500/20"),
                                    "rejected" => classes!("bg-slate-500/10", "text-slate-700", "dark:text-slate-200", "border-slate-500/20"),
                                    _ => classes!("bg-[var(--surface-alt)]", "text-[var(--muted)]", "border-[var(--border)]"),
                                };
                                html! {
                                    <article class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-4")}>
                                        <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                                            <div class={classes!("min-w-0", "space-y-1")}>
                                                <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                                                    <span class={classes!("inline-flex", "rounded-full", "border", "px-2.5", "py-1", "text-xs", "font-semibold", status_class.clone())}>
                                                        { item.status.clone() }
                                                    </span>
                                                    <span class={classes!("font-semibold")}>{ item.account_name.clone() }</span>
                                                        if !item.requester_email.trim().is_empty() {
                                                            <span class={classes!("text-xs", "text-[var(--muted)]")}>{ item.requester_email.clone() }</span>
                                                        }
                                                    <span class={classes!("text-xs", "font-mono", "text-[var(--muted)]")}>{ item.request_id.clone() }</span>
                                                </div>
                                                <div class={classes!("text-xs", "text-[var(--muted)]")}>
                                                    { format!("{} / {} · created {}", item.client_ip, item.ip_region, format_ms(item.created_at)) }
                                                </div>
                                            </div>
                                            <div class={classes!("text-right", "space-y-1")}>
                                                if let Some(github_id) = item.github_id.clone() {
                                                    <div class={classes!("text-sm", "font-semibold")}>{ format!("@{}", github_id) }</div>
                                                }
                                                if let Some(account_id) = item.account_id.clone() {
                                                    <div class={classes!("text-xs", "font-mono", "text-[var(--muted)]")}>{ account_id }</div>
                                                }
                                            </div>
                                        </div>

                                        <div class={classes!("mt-4", "grid", "gap-3", "xl:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]")}>
                                            <div>
                                                <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "留言" }</div>
                                                <div class={classes!("mt-2", "whitespace-pre-wrap", "break-words", "text-sm", "leading-6", "text-[var(--text)]")}>
                                                    { item.contributor_message.clone() }
                                                </div>
                                            </div>
                                            <div class={classes!("space-y-2", "text-sm")}>
                                                if let Some(frontend_page_url) = item.frontend_page_url.clone() {
                                                    <div>
                                                        <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "页面" }</div>
                                                        <div class={classes!("mt-1", "break-all", "text-[var(--text)]")}>{ frontend_page_url }</div>
                                                    </div>
                                                }
                                                if let Some(imported_account_name) = item.imported_account_name.clone() {
                                                    <div>
                                                        <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "已导入账号" }</div>
                                                        <div class={classes!("mt-1", "text-[var(--text)]")}>{ imported_account_name }</div>
                                                    </div>
                                                }
                                                if let Some(issued_key_name) = item.issued_key_name.clone() {
                                                    <div>
                                                        <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "已发放 Key" }</div>
                                                        <div class={classes!("mt-1", "text-[var(--text)]")}>
                                                            { format!("{} ({})", issued_key_name, item.issued_key_id.clone().unwrap_or_default()) }
                                                        </div>
                                                    </div>
                                                }
                                                if let Some(admin_note) = item.admin_note.clone() {
                                                    <div>
                                                        <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Admin Note" }</div>
                                                        <div class={classes!("mt-1", "whitespace-pre-wrap", "break-words", "text-[var(--text)]")}>{ admin_note }</div>
                                                    </div>
                                                }
                                                if let Some(failure_reason) = item.failure_reason.clone() {
                                                    <div class={classes!("rounded-lg", "border", "border-red-400/25", "bg-red-500/8", "px-3", "py-2", "text-red-700", "dark:text-red-200")}>
                                                        { failure_reason }
                                                    </div>
                                                }
                                            </div>
                                        </div>

                                        <div class={classes!("mt-4", "grid", "gap-3", "xl:grid-cols-3")}>
                                            { copyable_token_preview("access_token", &item.access_token, &on_copy) }
                                            { copyable_token_preview("id_token", &item.id_token, &on_copy) }
                                            { copyable_token_preview("refresh_token", &item.refresh_token, &on_copy) }
                                        </div>

                                        <div class={classes!("mt-4", "flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                            <div class={classes!("text-xs", "text-[var(--muted)]")}>
                                                { item.processed_at.map(format_ms).map(|value| format!("processed {}", value)).unwrap_or_else(|| "尚未处理".to_string()) }
                                            </div>
                                            <div class={classes!("flex", "items-center", "gap-2")}>
                                                    if item.status == "pending" || item.status == "failed" {
                                                        <button
                                                            class={classes!("btn-terminal", "btn-terminal-primary")}
                                                            onclick={Callback::from(move |_| validate_cb.emit(validate_request_id.clone()))}
                                                            disabled={action_busy}
                                                        >
                                                            { if action_busy { "验证中..." } else { "验证" } }
                                                        </button>
                                                    }
                                                    if item.status == "validated" {
                                                        <button
                                                            class={classes!("btn-terminal", "btn-terminal-primary")}
                                                            onclick={Callback::from(move |_| approve_cb.emit(approve_request_id.clone()))}
                                                            disabled={action_busy}
                                                        >
                                                            { if action_busy { "入库中..." } else { "入库并发放" } }
                                                        </button>
                                                    }
                                                if item.status == "pending" || item.status == "failed" {
                                                    <button
                                                        class={classes!("btn-terminal", "!text-red-600", "dark:!text-red-300")}
                                                        onclick={Callback::from(move |_| reject_cb.emit(reject_request_id.clone()))}
                                                        disabled={action_busy}
                                                    >
                                                        { "拒绝" }
                                                    </button>
                                                }
                                            </div>
                                        </div>
                                    </article>
                                }
                            }) }
                        </div>
                    }

                    <div class={classes!("mt-5")}>
                        <Pagination
                            current_page={*account_contribution_request_page}
                            total_pages={account_contribution_request_total_pages}
                            on_page_change={on_account_contribution_page_change}
                        />
                    </div>
                </section>

                <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <div>
                            <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Sponsors" }</h2>
                            <p class={classes!("mt-1", "m-0", "text-xs", "text-[var(--muted)]")}>
                                { "这批请求是「先填邮箱，再发付款说明邮件」的人工确认流。你确认对方已经按邮件说明完成赞助后，再在这里标记通过。" }
                            </p>
                        </div>
                        <button
                            class={classes!("btn-terminal")}
                            onclick={{
                                let reload_sponsor_requests = reload_sponsor_requests.clone();
                                Callback::from(move |_| reload_sponsor_requests.emit((None, None)))
                            }}
                            disabled={*sponsor_request_loading}
                        >
                            <i class={classes!("fas", if *sponsor_request_loading { "fa-spinner animate-spin" } else { "fa-rotate-right" })}></i>
                        </button>
                    </div>

                    <div class={classes!("mt-3", "grid", "gap-3", "md:grid-cols-[minmax(0,16rem)_auto]")}>
                        <label class={classes!("text-sm")}>
                            <span class={classes!("text-[var(--muted)]")}>{ "状态" }</span>
                            <select
                                key={format!("sponsor-filter-{}", (*sponsor_request_status_filter).clone())}
                                class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                onchange={on_sponsor_request_status_filter_change}
                            >
                                <option value="" selected={(*sponsor_request_status_filter).is_empty()}>{ "全部" }</option>
                                <option value="submitted" selected={*sponsor_request_status_filter == "submitted"}>{ "submitted" }</option>
                                <option value="payment_email_sent" selected={*sponsor_request_status_filter == "payment_email_sent"}>{ "payment_email_sent" }</option>
                                <option value="approved" selected={*sponsor_request_status_filter == "approved"}>{ "approved" }</option>
                            </select>
                        </label>
                    </div>

                    if sponsor_requests.is_empty() && !*sponsor_request_loading {
                        <div class={classes!("mt-4", "rounded-xl", "border", "border-dashed", "border-[var(--border)]", "px-4", "py-10", "text-center", "text-[var(--muted)]")}>
                            { "当前筛选下还没有 Sponsor 请求。" }
                        </div>
                    } else {
                        <div class={classes!("mt-4", "space-y-3")}>
                            { for sponsor_requests.iter().map(|item| {
                                let request_id = item.request_id.clone();
                                let approve_request_id = item.request_id.clone();
                                let delete_request_id = item.request_id.clone();
                                let approve_cb = on_approve_sponsor_request.clone();
                                let delete_cb = on_delete_sponsor_request.clone();
                                let action_busy = sponsor_request_action_inflight.contains(&request_id);
                                let status_class = match item.status.as_str() {
                                    "submitted" => classes!("bg-amber-500/10", "text-amber-700", "dark:text-amber-200", "border-amber-500/20"),
                                    "payment_email_sent" => classes!("bg-sky-500/10", "text-sky-700", "dark:text-sky-200", "border-sky-500/20"),
                                    "approved" => classes!("bg-emerald-500/10", "text-emerald-700", "dark:text-emerald-200", "border-emerald-500/20"),
                                    _ => classes!("bg-[var(--surface-alt)]", "text-[var(--muted)]", "border-[var(--border)]"),
                                };
                                html! {
                                    <article class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-4")}>
                                        <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                                            <div class={classes!("min-w-0", "space-y-1")}>
                                                <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                                                    <span class={classes!("inline-flex", "rounded-full", "border", "px-2.5", "py-1", "text-xs", "font-semibold", status_class.clone())}>
                                                        { item.status.clone() }
                                                    </span>
                                                    <span class={classes!("font-semibold")}>{ item.requester_email.clone() }</span>
                                                    <span class={classes!("text-xs", "font-mono", "text-[var(--muted)]")}>{ item.request_id.clone() }</span>
                                                </div>
                                                <div class={classes!("text-xs", "text-[var(--muted)]")}>
                                                    { format!("{} / {} · created {}", item.client_ip, item.ip_region, format_ms(item.created_at)) }
                                                </div>
                                            </div>
                                            <div class={classes!("text-right", "space-y-1")}>
                                                if let Some(display_name) = item.display_name.clone() {
                                                    <div class={classes!("text-sm", "font-semibold")}>{ display_name }</div>
                                                }
                                                if let Some(github_id) = item.github_id.clone() {
                                                    <div class={classes!("text-xs", "font-semibold", "text-[var(--muted)]")}>{ format!("@{}", github_id) }</div>
                                                }
                                            </div>
                                        </div>

                                        <div class={classes!("mt-4", "grid", "gap-3", "xl:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]")}>
                                            <div>
                                                <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "留言" }</div>
                                                <div class={classes!("mt-2", "whitespace-pre-wrap", "break-words", "text-sm", "leading-6", "text-[var(--text)]")}>
                                                    { item.sponsor_message.clone() }
                                                </div>
                                            </div>
                                            <div class={classes!("space-y-2", "text-sm")}>
                                                if let Some(frontend_page_url) = item.frontend_page_url.clone() {
                                                    <div>
                                                        <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "页面" }</div>
                                                        <div class={classes!("mt-1", "break-all", "text-[var(--text)]")}>{ frontend_page_url }</div>
                                                    </div>
                                                }
                                                if let Some(payment_email_sent_at) = item.payment_email_sent_at {
                                                    <div>
                                                        <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "付款说明邮件" }</div>
                                                        <div class={classes!("mt-1", "text-[var(--text)]")}>{ format_ms(payment_email_sent_at) }</div>
                                                    </div>
                                                }
                                                if let Some(admin_note) = item.admin_note.clone() {
                                                    <div>
                                                        <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Admin Note" }</div>
                                                        <div class={classes!("mt-1", "whitespace-pre-wrap", "break-words", "text-[var(--text)]")}>{ admin_note }</div>
                                                    </div>
                                                }
                                                if let Some(failure_reason) = item.failure_reason.clone() {
                                                    <div class={classes!("rounded-lg", "border", "border-red-400/25", "bg-red-500/8", "px-3", "py-2", "text-red-700", "dark:text-red-200")}>
                                                        { failure_reason }
                                                    </div>
                                                }
                                            </div>
                                        </div>

                                        <div class={classes!("mt-4", "flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                            <div class={classes!("text-xs", "text-[var(--muted)]")}>
                                                { item.processed_at.map(format_ms).map(|value| format!("processed {}", value)).unwrap_or_else(|| "尚未确认".to_string()) }
                                            </div>
                                            <div class={classes!("flex", "items-center", "gap-2")}>
                                                if item.status != "approved" {
                                                    <button
                                                        class={classes!("btn-terminal", "btn-terminal-primary")}
                                                        onclick={Callback::from(move |_| approve_cb.emit(approve_request_id.clone()))}
                                                        disabled={action_busy}
                                                    >
                                                        { if action_busy { "处理中..." } else { "标记已确认" } }
                                                    </button>
                                                }
                                                <button
                                                    class={classes!("btn-terminal", "!text-red-600", "dark:!text-red-300")}
                                                    onclick={Callback::from(move |_| delete_cb.emit(delete_request_id.clone()))}
                                                    disabled={action_busy}
                                                >
                                                    { "删除" }
                                                </button>
                                            </div>
                                        </div>
                                    </article>
                                }
                            }) }
                        </div>
                    }

                    <div class={classes!("mt-5")}>
                        <Pagination
                            current_page={*sponsor_request_page}
                            total_pages={sponsor_request_total_pages}
                            on_page_change={on_sponsor_request_page_change}
                        />
                    </div>
                </section>
                } // end TAB_REQUESTS

            </div>

            { usage_detail_modal.unwrap_or_default() }
            { usage_journal_message_modal.unwrap_or_default() }

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
        </main>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_last_message_preview_prefers_summary_content() {
        let event = AdminLlmGatewayUsageEventView {
            last_message_content: Some("hello".to_string()),
            ..AdminLlmGatewayUsageEventView::default()
        };

        assert_eq!(usage_last_message_preview(&event), "hello");
    }

    #[test]
    fn usage_last_message_preview_falls_back_for_blank_content() {
        let event = AdminLlmGatewayUsageEventView {
            last_message_content: Some("   ".to_string()),
            ..AdminLlmGatewayUsageEventView::default()
        };

        assert_eq!(usage_last_message_preview(&event), "-");
    }

    #[test]
    fn usage_last_message_table_preview_collapses_whitespace_and_truncates() {
        let event = AdminLlmGatewayUsageEventView {
            last_message_content: Some(
                "first line\n\nsecond   line with   extra spaces and a very long suffix that \
                 should be truncated in the table preview because it keeps going with more and \
                 more text until the shortened variant must end with ellipsis"
                    .to_string(),
            ),
            ..AdminLlmGatewayUsageEventView::default()
        };

        let preview = usage_last_message_table_preview(&event);

        assert!(!preview.contains('\n'));
        assert!(preview.contains("first line second line with extra spaces"));
        assert!(preview.ends_with("..."));
        assert!(preview.chars().count() <= 123);
    }

    #[test]
    fn usage_last_message_table_preview_keeps_short_single_line_text() {
        let event = AdminLlmGatewayUsageEventView {
            last_message_content: Some("short text".to_string()),
            ..AdminLlmGatewayUsageEventView::default()
        };

        assert_eq!(usage_last_message_table_preview(&event), "short text");
    }

    #[test]
    fn usage_journal_should_load_only_for_journal_tab() {
        assert!(should_load_usage_journal(TAB_JOURNAL));
        assert!(!should_load_usage_journal(TAB_OVERVIEW));
        assert!(!should_load_usage_journal(TAB_USAGE));
        assert!(!should_load_usage_journal(TAB_SETTINGS));
    }

    #[test]
    fn llm_inventory_load_helpers_follow_active_tab() {
        assert!(should_load_llm_gateway_keys_inventory(TAB_KEYS));
        assert!(should_load_llm_gateway_keys_inventory(TAB_USAGE));
        assert!(!should_load_llm_gateway_keys_inventory(TAB_OVERVIEW));

        assert!(should_load_llm_gateway_group_options(TAB_KEYS));
        assert!(!should_load_llm_gateway_group_options(TAB_GROUPS));
        assert!(!should_load_llm_gateway_group_options(TAB_ACCOUNTS));

        assert!(should_load_llm_gateway_import_jobs(TAB_ACCOUNTS));
        assert!(!should_load_llm_gateway_import_jobs(TAB_OVERVIEW));
    }

    #[test]
    fn usage_journal_preview_message_prefers_summary_content() {
        let event = crate::api::AdminUsageJournalPreviewEventView {
            last_message_content: Some("hello".to_string()),
            ..crate::api::AdminUsageJournalPreviewEventView::default()
        };

        assert_eq!(usage_journal_preview_message(&event), "hello");
    }

    #[test]
    fn usage_journal_preview_message_presence_detects_real_content() {
        let with_message = crate::api::AdminUsageJournalPreviewEventView {
            last_message_content: Some("hello".to_string()),
            ..crate::api::AdminUsageJournalPreviewEventView::default()
        };
        let without_message = crate::api::AdminUsageJournalPreviewEventView {
            last_message_content: Some("   ".to_string()),
            ..crate::api::AdminUsageJournalPreviewEventView::default()
        };

        assert!(usage_journal_preview_has_full_message(&with_message));
        assert!(!usage_journal_preview_has_full_message(&without_message));
    }

    #[test]
    fn journal_preview_layout_uses_compact_table_width_and_toolbar_badges() {
        let source =
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/pages/admin_llm_gateway.rs"));
        let journal_start = source
            .find("\"Live Preview\"")
            .expect("journal preview header");
        let journal_slice = &source[journal_start..source.len().min(journal_start + 12000)];

        assert!(journal_slice.contains("min-w-[64rem]"));
        assert!(journal_slice.contains("RPM {}"));
        assert!(journal_slice.contains("In Flight {}"));
        assert!(journal_slice.contains("刷新预览"));
    }

    #[test]
    fn kiro_usage_account_label_distinguishes_uncaptured_account_from_legacy_auth() {
        assert_eq!(
            usage_account_label(
                &None,
                "https://ackingliu.top/api/kiro-gateway/v1/messages",
                "/generateAssistantResponse",
            ),
            "not captured"
        );
        assert_eq!(
            usage_account_label(
                &None,
                "https://ackingliu.top/api/llm-gateway/v1/responses",
                "/v1/responses"
            ),
            "legacy auth"
        );
    }

    #[test]
    fn latency_breakdown_marks_first_sse_not_applicable_when_stream_never_started() {
        let summary = format_latency_breakdown(LatencyBreakdown {
            latency_ms: 502,
            routing_wait_ms: Some(12),
            upstream_headers_ms: Some(34),
            post_headers_body_ms: None,
            request_body_bytes: Some(512),
            request_body_read_ms: Some(1),
            request_json_parse_ms: Some(0),
            pre_handler_ms: Some(2),
            first_sse_write_ms: None,
            stream_finish_ms: Some(502),
            other_latency_ms: None,
            quota_failover_count: 0,
        });

        assert!(summary.contains("route 12 ms"));
        assert!(summary.contains("first SSE n/a"));
    }

    #[test]
    fn stream_summary_marks_disconnect_and_formats_bytes() {
        assert_eq!(
            format_stream_summary(Some(false), Some(true), Some("message_stop"), Some(2048)),
            "state disconnect · final message_stop · bytes 2.0 KiB"
        );
        assert_eq!(usage_stream_state_label(Some(true), Some(false)), "clean");
        assert_eq!(usage_stream_state_label(None, None), "n/a");
    }

    #[test]
    fn effective_route_latency_uses_routing_diagnostics_when_column_is_missing() {
        assert_eq!(effective_routing_wait_ms(None, Some(r#"{"route_total_ms":321}"#)), Some(321));
        assert_eq!(
            effective_routing_wait_ms(Some(12), Some(r#"{"route_total_ms":321}"#)),
            Some(12)
        );
        assert_eq!(effective_routing_wait_ms(None, Some("not-json")), None);
    }

    #[test]
    fn routing_diagnostics_summary_includes_codex_failover_count() {
        let rows = routing_diagnostics_summary(
            r#"{"route_total_ms":12,"account_attempt_count":2,"failover_count":1}"#,
        );

        assert!(rows
            .iter()
            .any(|(label, value)| label == "Route total" && value == "12 ms"));
        assert!(rows
            .iter()
            .any(|(label, value)| label == "Codex failover" && value == "1"));
    }

    fn test_key(id: &str, name: &str, provider_type: &str, status: &str) -> AdminLlmGatewayKeyView {
        AdminLlmGatewayKeyView {
            id: id.to_string(),
            name: name.to_string(),
            provider_type: provider_type.to_string(),
            status: status.to_string(),
            ..AdminLlmGatewayKeyView::default()
        }
    }

    #[test]
    fn usage_key_search_matches_name_id_provider_and_status() {
        let keys = vec![
            test_key("sfk-alpha", "Default Codex", "codex", "active"),
            test_key("sfk-beta", "Kiro Pool", "kiro", "disabled"),
        ];

        let by_name = filter_gateway_keys_for_query(&keys, "default");
        let by_id = filter_gateway_keys_for_query(&keys, "BETA");
        let by_provider = filter_gateway_keys_for_query(&keys, "kiro");
        let by_status = filter_gateway_keys_for_query(&keys, "disabled");

        assert_eq!(by_name, vec![keys[0].clone()]);
        assert_eq!(by_id, vec![keys[1].clone()]);
        assert_eq!(by_provider, vec![keys[1].clone()]);
        assert_eq!(by_status, vec![keys[1].clone()]);
    }

    #[test]
    fn usage_key_search_trims_query_and_returns_all_for_blank() {
        let keys = vec![
            test_key("sfk-alpha", "Default Codex", "codex", "active"),
            test_key("sfk-beta", "Kiro Pool", "kiro", "disabled"),
        ];

        assert_eq!(filter_gateway_keys_for_query(&keys, "   "), keys);
        assert_eq!(filter_gateway_keys_for_query(&keys, "  codex  "), vec![keys[0].clone()]);
    }

    #[test]
    fn parse_admin_codex_batch_import_json_accepts_local_json_array() {
        let items = parse_admin_codex_batch_import_json(
            r#"[
                {
                    "name": "codex-a",
                    "auth_json": { "refresh_token": "rt-a", "account_id": "acct-a" }
                },
                {
                    "name": "codex-b",
                    "tokens": { "refresh_token": "rt-b" }
                }
            ]"#,
        )
        .expect("valid local batch import json");

        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["name"], "codex-a");
        assert!(items[0]["auth_json"].is_object());
        assert!(items[1]["tokens"].is_object());
    }

    #[test]
    fn parse_admin_codex_batch_import_json_rejects_missing_name() {
        let err = parse_admin_codex_batch_import_json(
            r#"[
                {
                    "auth_json": { "refresh_token": "rt-a" }
                }
            ]"#,
        )
        .expect_err("missing name must fail");

        assert!(err.contains("name"));
    }
}
