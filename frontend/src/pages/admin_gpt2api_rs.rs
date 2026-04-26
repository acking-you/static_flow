use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use wasm_bindgen::{prelude::*, JsCast};
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{File, HtmlElement, HtmlInputElement, HtmlSelectElement, HtmlTextAreaElement};
use yew::prelude::*;
use yew_router::prelude::Link;

#[wasm_bindgen(inline_js = r#"
export function gpt2api_copy_text(text) {
    if (navigator.clipboard) {
        navigator.clipboard.writeText(text).catch(function(){});
    }
}
export function gpt2api_make_draggable(panelId, handleId) {
    const panel = document.getElementById(panelId);
    const handle = document.getElementById(handleId);
    if (!panel || !handle || panel.dataset.dragBound === "1") return;
    panel.dataset.dragBound = "1";
    handle.addEventListener("pointerdown", function(event) {
        if (event.button !== 0) return;
        if (event.target && event.target.closest && event.target.closest("button,input,select,textarea,a")) return;
        event.preventDefault();
        const rect = panel.getBoundingClientRect();
        const startX = event.clientX;
        const startY = event.clientY;
        const startLeft = rect.left;
        const startTop = rect.top;
        panel.style.right = "auto";
        panel.style.left = startLeft + "px";
        panel.style.top = startTop + "px";
        handle.setPointerCapture(event.pointerId);
        function move(moveEvent) {
            const nextLeft = Math.max(8, Math.min(window.innerWidth - 80, startLeft + moveEvent.clientX - startX));
            const nextTop = Math.max(8, Math.min(window.innerHeight - 48, startTop + moveEvent.clientY - startY));
            panel.style.left = nextLeft + "px";
            panel.style.top = nextTop + "px";
        }
        function up(upEvent) {
            handle.releasePointerCapture(upEvent.pointerId);
            window.removeEventListener("pointermove", move);
            window.removeEventListener("pointerup", up);
        }
        window.addEventListener("pointermove", move);
        window.addEventListener("pointerup", up);
    });
}
"#)]
extern "C" {
    fn gpt2api_copy_text(text: &str);
    fn gpt2api_make_draggable(panel_id: &str, handle_id: &str);
}

use crate::{
    api::{
        admin_gpt2api_rs_chat_completions, admin_gpt2api_rs_edit_images,
        admin_gpt2api_rs_generate_images, admin_gpt2api_rs_responses,
        approve_admin_gpt2api_account_contribution_request, check_admin_gpt2api_rs_proxy_config,
        create_admin_gpt2api_rs_account_group, create_admin_gpt2api_rs_key,
        create_admin_gpt2api_rs_proxy_config, delete_admin_gpt2api_rs_account_group,
        delete_admin_gpt2api_rs_accounts, delete_admin_gpt2api_rs_key,
        delete_admin_gpt2api_rs_proxy_config, fetch_admin_gpt2api_account_contribution_requests,
        fetch_admin_gpt2api_rs_account_groups, fetch_admin_gpt2api_rs_accounts,
        fetch_admin_gpt2api_rs_config, fetch_admin_gpt2api_rs_keys, fetch_admin_gpt2api_rs_models,
        fetch_admin_gpt2api_rs_proxy_configs, fetch_admin_gpt2api_rs_status,
        fetch_admin_gpt2api_rs_usage_events, fetch_admin_gpt2api_rs_version,
        import_admin_gpt2api_rs_accounts, post_admin_gpt2api_rs_login,
        refresh_admin_gpt2api_rs_accounts, reject_admin_gpt2api_account_contribution_request,
        rotate_admin_gpt2api_rs_key, update_admin_gpt2api_rs_account,
        update_admin_gpt2api_rs_account_group, update_admin_gpt2api_rs_config,
        update_admin_gpt2api_rs_key, update_admin_gpt2api_rs_proxy_config,
        AdminGpt2ApiAccountContributionRequestView, AdminGpt2ApiAccountContributionRequestsQuery,
        AdminGpt2ApiRsAccountGroupView, AdminGpt2ApiRsAccountView,
        AdminGpt2ApiRsCreateAccountGroupRequest, AdminGpt2ApiRsCreateKeyRequest,
        AdminGpt2ApiRsCreateProxyConfigRequest, AdminGpt2ApiRsDeleteAccountsRequest,
        AdminGpt2ApiRsImageEditRequest, AdminGpt2ApiRsImageGenerationRequest,
        AdminGpt2ApiRsImportAccountsRequest, AdminGpt2ApiRsKeyView, AdminGpt2ApiRsProxyCheckResult,
        AdminGpt2ApiRsProxyConfigView, AdminGpt2ApiRsRefreshAccountsRequest,
        AdminGpt2ApiRsUpdateAccountGroupRequest, AdminGpt2ApiRsUpdateAccountRequest,
        AdminGpt2ApiRsUpdateKeyRequest, AdminGpt2ApiRsUpdateProxyConfigRequest,
        AdminGpt2ApiRsUsageEventView, AdminGpt2ApiRsUsageEventsQuery, Gpt2ApiRsConfig,
    },
    components::{search_box::SearchBox, tab_bar::render_tab_bar},
    pages::llm_access_shared::{confirm_destructive, format_ms, MaskedSecretCode},
    router::Route,
};

#[derive(Debug, Default, serde::Deserialize)]
struct BrowserProfileView {
    session_token: Option<String>,
    user_agent: Option<String>,
    impersonate_browser: Option<String>,
}

#[derive(Properties, PartialEq)]
struct UsageDetailFieldProps {
    label: &'static str,
    value: String,
}

#[function_component(UsageDetailField)]
fn usage_detail_field(props: &UsageDetailFieldProps) -> Html {
    html! {
        <div class={classes!("rounded", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-3")}>
            <div class={classes!("text-xs", "text-[var(--muted)]")}>{ props.label }</div>
            <div class={classes!("mt-1", "break-all", "font-mono", "text-xs")}>{ props.value.clone() }</div>
        </div>
    }
}

#[derive(Properties, PartialEq)]
struct Gpt2ApiAccountGroupEditorProps {
    group: AdminGpt2ApiRsAccountGroupView,
    accounts: Vec<AdminGpt2ApiRsAccountView>,
    on_changed: Callback<()>,
    on_error: Callback<String>,
    on_notice: Callback<String>,
}

#[function_component(Gpt2ApiAccountGroupEditor)]
fn gpt2api_account_group_editor(props: &Gpt2ApiAccountGroupEditorProps) -> Html {
    let name = use_state(|| props.group.name.clone());
    let account_names =
        use_state(|| sanitize_gpt2api_account_names(&props.group.account_names, &props.accounts));
    let expanded = use_state(|| false);
    let saving = use_state(|| false);

    {
        let group = props.group.clone();
        let accounts = props.accounts.clone();
        let name = name.clone();
        let account_names = account_names.clone();
        use_effect_with((props.group.clone(), props.accounts.clone()), move |_| {
            name.set(group.name.clone());
            account_names.set(sanitize_gpt2api_account_names(&group.account_names, &accounts));
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
        let group_id = props.group.id.clone();
        let name = name.clone();
        let account_names = account_names.clone();
        let saving = saving.clone();
        let on_changed = props.on_changed.clone();
        let on_error = props.on_error.clone();
        let on_notice = props.on_notice.clone();
        Callback::from(move |_| {
            if *saving {
                return;
            }
            let group_id = group_id.clone();
            let name_value = (*name).trim().to_string();
            let account_names_value = (*account_names).clone();
            let saving = saving.clone();
            let on_changed = on_changed.clone();
            let on_error = on_error.clone();
            let on_notice = on_notice.clone();
            spawn_local(async move {
                saving.set(true);
                match update_admin_gpt2api_rs_account_group(
                    &group_id,
                    &AdminGpt2ApiRsUpdateAccountGroupRequest {
                        name: Some(name_value.clone()),
                        account_names: Some(account_names_value),
                    },
                )
                .await
                {
                    Ok(_) => {
                        on_notice.emit(format!("Saved account group {name_value}"));
                        on_changed.emit(());
                    },
                    Err(err) => on_error.emit(err),
                }
                saving.set(false);
            });
        })
    };

    let on_delete = {
        let group_id = props.group.id.clone();
        let group_name = props.group.name.clone();
        let saving = saving.clone();
        let on_changed = props.on_changed.clone();
        let on_error = props.on_error.clone();
        let on_notice = props.on_notice.clone();
        Callback::from(move |_| {
            if !confirm_destructive(&format!("Delete account group \"{group_name}\"?")) {
                return;
            }
            let group_id = group_id.clone();
            let group_name = group_name.clone();
            let saving = saving.clone();
            let on_changed = on_changed.clone();
            let on_error = on_error.clone();
            let on_notice = on_notice.clone();
            spawn_local(async move {
                saving.set(true);
                match delete_admin_gpt2api_rs_account_group(&group_id).await {
                    Ok(_) => {
                        on_notice.emit(format!("Deleted account group {group_name}"));
                        on_changed.emit(());
                    },
                    Err(err) => on_error.emit(err),
                }
                saving.set(false);
            });
        })
    };

    html! {
        <article class={classes!("rounded-[var(--radius)]", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-4")}>
            <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                <div>
                    <h3 class={classes!("m-0", "text-base", "font-semibold")}>{ props.group.name.clone() }</h3>
                    <p class={classes!("m-0", "mt-1", "text-xs", "text-[var(--muted)]")}>
                        {
                            if props.group.account_names.is_empty() {
                                "No member accounts".to_string()
                            } else {
                                format!("Members: {}", props.group.account_names.join(", "))
                            }
                        }
                    </p>
                </div>
                <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                    <span class={classes!("text-xs", "text-[var(--muted)]")}>
                        { format!("{} accounts", props.group.account_names.len()) }
                    </span>
                    <button
                        type="button"
                        class={classes!("btn-terminal")}
                        onclick={{
                            let expanded = expanded.clone();
                            Callback::from(move |_| expanded.set(!*expanded))
                        }}
                    >
                        { if *expanded { "Collapse" } else { "Edit" } }
                    </button>
                </div>
            </div>

            if *expanded {
                <div class={classes!("mt-4", "grid", "gap-3")}>
                    <label class={classes!("text-sm")}>
                        <span class={classes!("text-[var(--muted)]")}>{ "Name" }</span>
                        <input
                            type="text"
                            class={classes!("mt-1", "w-full", "rounded", "border", "border-[var(--border)]", "bg-transparent", "px-3", "py-2")}
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
                    <div class={classes!("grid", "gap-2", "xl:grid-cols-2")}>
                        { for props.accounts.iter().map(|account| {
                            let checked = account_names.iter().any(|name| name == &account.name);
                            let account_name = account.name.clone();
                            let on_toggle_account = on_toggle_account.clone();
                            html! {
                                <label class={classes!(
                                    "flex", "cursor-pointer", "items-center", "gap-3", "rounded", "border", "px-3", "py-2",
                                    if checked { "border-sky-500/40 bg-sky-500/10" } else { "border-[var(--border)] bg-[var(--surface-alt)]" }
                                )}>
                                    <input
                                        type="checkbox"
                                        checked={checked}
                                        onchange={Callback::from(move |_| on_toggle_account.emit(account_name.clone()))}
                                    />
                                    <div class={classes!("min-w-0")}>
                                        <div class={classes!("font-medium")}>{ account.name.clone() }</div>
                                        <div class={classes!("text-xs", "text-[var(--muted)]")}>
                                            { format!("{} · quota {}", account.status, account.quota_remaining) }
                                        </div>
                                    </div>
                                </label>
                            }
                        }) }
                    </div>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <span class={classes!("text-xs", "text-[var(--muted)]")}>
                            { format!(
                                "Selected: {}",
                                if account_names.is_empty() { "none".to_string() } else { account_names.join(", ") }
                            ) }
                        </span>
                        <div class={classes!("flex", "items-center", "gap-2")}>
                            <button class={classes!("btn-terminal")} onclick={on_save} disabled={*saving}>
                                { if *saving { "Saving..." } else { "Save" } }
                            </button>
                            <button class={classes!("btn-terminal", "text-red-600")} onclick={on_delete} disabled={*saving}>
                                { "Delete" }
                            </button>
                        </div>
                    </div>
                </div>
            }
        </article>
    }
}

// Tabs on the gpt2api-rs admin page. Using &'static str to slot straight into
// the shared `render_tab_bar` helper without boxing.
const GPT2API_TAB_OVERVIEW: &str = "overview";
const GPT2API_TAB_ACCOUNTS: &str = "accounts";
const GPT2API_TAB_PROXIES: &str = "proxies";
const GPT2API_TAB_GROUPS: &str = "groups";
const GPT2API_TAB_KEYS: &str = "keys";
const GPT2API_TAB_CONTRIBUTIONS: &str = "contributions";
const GPT2API_TAB_USAGE: &str = "usage";
const GPT2API_TAB_ADVANCED: &str = "advanced";
const GPT2API_TAB_IMAGES: &str = "images";
const GPT2API_TAB_PLAYGROUND: &str = "playground";

fn pretty_json(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn pretty_json_text(raw: &str) -> String {
    serde_json::from_str::<serde_json::Value>(raw)
        .map(|value| pretty_json(&value))
        .unwrap_or_else(|_| raw.to_string())
}

fn sanitize_gpt2api_account_names(
    names: &[String],
    accounts: &[AdminGpt2ApiRsAccountView],
) -> Vec<String> {
    let valid_names = accounts
        .iter()
        .map(|account| account.name.as_str())
        .collect::<std::collections::HashSet<_>>();
    let mut sanitized = names
        .iter()
        .filter(|name| valid_names.contains(name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    sanitized.sort();
    sanitized.dedup();
    sanitized
}

fn gpt2api_account_option_label(account: &AdminGpt2ApiRsAccountView) -> String {
    let email = account.email.as_deref().unwrap_or("no email");
    let quota_suffix = if account.quota_known { "" } else { " unknown" };
    format!(
        "{} · {} · {} · quota {}{}",
        account.name, email, account.status, account.quota_remaining, quota_suffix
    )
}

fn gpt2api_group_name_for_id(groups: &[AdminGpt2ApiRsAccountGroupView], group_id: &str) -> String {
    groups
        .iter()
        .find(|group| group.id == group_id)
        .map(|group| group.name.clone())
        .unwrap_or_else(|| group_id.to_string())
}

fn gpt2api_group_matches_query(group: &AdminGpt2ApiRsAccountGroupView, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    group.id.to_lowercase().contains(query)
        || group.name.to_lowercase().contains(query)
        || group
            .account_names
            .iter()
            .any(|name| name.to_lowercase().contains(query))
}

fn parse_json_text(raw: &str) -> Result<serde_json::Value, String> {
    serde_json::from_str(raw).map_err(|err| format!("JSON parse error: {err}"))
}

fn image_size_credit_hint(size: &str) -> String {
    let Some((width, height)) =
        size.trim()
            .to_ascii_lowercase()
            .split_once('x')
            .map(|(width, height)| {
                (width.trim().parse::<u64>().ok(), height.trim().parse::<u64>().ok())
            })
    else {
        return "Use WIDTHxHEIGHT; credits = ceil(width * height / 1024^2).".to_string();
    };
    let Some(width) = width else {
        return "Use WIDTHxHEIGHT; credits = ceil(width * height / 1024^2).".to_string();
    };
    let Some(height) = height else {
        return "Use WIDTHxHEIGHT; credits = ceil(width * height / 1024^2).".to_string();
    };
    let credits = (width * height).div_ceil(1024 * 1024).max(1);
    format!("{credits} credit/image · formula ceil({width} * {height} / 1024^2)")
}

fn extract_image_data_urls(value: &serde_json::Value) -> Vec<String> {
    value
        .get("data")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let b64 = item.get("b64_json")?.as_str()?;
            Some(format!("data:image/png;base64,{b64}"))
        })
        .collect()
}

async fn read_file_as_base64(file: File) -> Result<(String, String, String), String> {
    let file_name = file.name();
    let mime_type = file.type_();
    let blob: web_sys::Blob = file.into();
    let js_value = JsFuture::from(blob.array_buffer())
        .await
        .map_err(|err| format!("{err:?}"))?;
    let bytes = js_sys::Uint8Array::new(&js_value).to_vec();
    Ok((
        BASE64.encode(bytes),
        file_name,
        if mime_type.trim().is_empty() { "image/png".to_string() } else { mime_type },
    ))
}

fn parse_browser_profile(account: &AdminGpt2ApiRsAccountView) -> BrowserProfileView {
    serde_json::from_str(&account.browser_profile_json).unwrap_or_default()
}

fn parse_required_i64_input(value: &str, field_name: &str) -> Result<i64, String> {
    value
        .trim()
        .parse::<i64>()
        .map_err(|_| format!("{field_name} must be an integer"))
}

fn parse_optional_u64_input(value: &str, field_name: &str) -> Result<Option<u64>, String> {
    match value.trim() {
        "" => Ok(None),
        raw => raw
            .parse::<u64>()
            .map(Some)
            .map_err(|_| format!("{field_name} must be an integer")),
    }
}

fn format_account_scheduler(account: &AdminGpt2ApiRsAccountView) -> String {
    let concurrency = account
        .request_max_concurrency
        .map(|value| format!("{value} in-flight"))
        .unwrap_or_else(|| "inherit concurrency".to_string());
    let spacing = account
        .request_min_start_interval_ms
        .map(|value| format!("{value} ms spacing"))
        .unwrap_or_else(|| "inherit spacing".to_string());
    format!("{concurrency} · {spacing}")
}

fn format_account_proxy_binding(account: &AdminGpt2ApiRsAccountView) -> String {
    match account.proxy_mode.as_str() {
        "direct" => "direct".to_string(),
        "fixed" => account
            .proxy_config_id
            .as_ref()
            .map(|proxy_id| format!("fixed · {proxy_id}"))
            .unwrap_or_else(|| "fixed".to_string()),
        _ => "inherit".to_string(),
    }
}

fn format_account_restore_at(account: &AdminGpt2ApiRsAccountView) -> String {
    account
        .restore_at
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| "-".to_string())
}

fn format_account_effective_proxy(account: &AdminGpt2ApiRsAccountView) -> String {
    match account.effective_proxy_url.as_deref() {
        Some(url) => match account.effective_proxy_config_name.as_deref() {
            Some(name) if !name.trim().is_empty() => {
                format!("{} · {} · {}", account.effective_proxy_source, name, url)
            },
            _ => format!("{} · {}", account.effective_proxy_source, url),
        },
        None => {
            if account.effective_proxy_source.trim().is_empty() {
                "direct".to_string()
            } else {
                format!("{} · direct", account.effective_proxy_source)
            }
        },
    }
}

fn format_proxy_check_result(result: &AdminGpt2ApiRsProxyCheckResult) -> String {
    match result.status_code {
        Some(status) => format!("{} (status {})", result.message, status),
        None => result.message.clone(),
    }
}

#[function_component(AdminGpt2ApiRsPage)]
pub fn admin_gpt2api_rs_page() -> Html {
    let active_tab = use_state(|| GPT2API_TAB_OVERVIEW.to_string());
    let loading = use_state(|| false);
    let saving_config = use_state(|| false);
    let load_error = use_state(|| None::<String>);
    let notice = use_state(|| None::<String>);

    let config = use_state(Gpt2ApiRsConfig::default);
    let config_path = use_state(String::new);
    let configured = use_state(|| false);

    let status_json = use_state(|| "{}".to_string());
    let version_json = use_state(|| "{}".to_string());
    let models_json = use_state(|| "{}".to_string());
    let login_json = use_state(|| "{}".to_string());

    let accounts = use_state(Vec::<AdminGpt2ApiRsAccountView>::new);
    let proxy_configs = use_state(Vec::<AdminGpt2ApiRsProxyConfigView>::new);
    let account_groups = use_state(Vec::<AdminGpt2ApiRsAccountGroupView>::new);
    let accounts_search = use_state(String::new);
    let account_groups_search = use_state(String::new);
    let account_group_form_expanded = use_state(|| false);
    let create_account_group_name = use_state(String::new);
    let create_account_group_account_names = use_state(Vec::<String>::new);
    let creating_account_group = use_state(|| false);
    let keys = use_state(Vec::<AdminGpt2ApiRsKeyView>::new);
    let contribution_requests = use_state(Vec::<AdminGpt2ApiAccountContributionRequestView>::new);
    let contribution_total = use_state(|| 0_usize);
    let contribution_page = use_state(|| 1_usize);
    let contribution_status_filter = use_state(String::new);
    let contribution_loading = use_state(|| false);
    let contribution_action_inflight = use_state(std::collections::HashSet::<String>::new);
    let usage = use_state(Vec::<AdminGpt2ApiRsUsageEventView>::new);
    let usage_limit = use_state(|| "50".to_string());
    let usage_search = use_state(String::new);
    let usage_key_filter = use_state(String::new);
    let usage_total = use_state(|| 0_u64);
    let usage_offset = use_state(|| 0_u64);
    let usage_current_rpm = use_state(|| 0_u32);
    let usage_current_in_flight = use_state(|| 0_u32);
    let usage_billable_total = use_state(|| 0_i64);
    let usage_has_more = use_state(|| false);
    let usage_scroll_top_ref = use_node_ref();
    let usage_scroll_bottom_ref = use_node_ref();
    let usage_scroll_width = use_state(|| 1_i32);
    let selected_usage_event = use_state(|| None::<AdminGpt2ApiRsUsageEventView>);
    let editing_key_id = use_state(|| None::<String>);
    let key_form_name = use_state(String::new);
    let key_form_status = use_state(|| "active".to_string());
    let key_form_quota_total_calls = use_state(|| "100".to_string());
    let key_form_route_strategy = use_state(|| "auto".to_string());
    let key_form_role = use_state(|| "user".to_string());
    let key_form_account_group_id = use_state(String::new);
    let key_form_fixed_account_name = use_state(String::new);
    let key_form_request_max_concurrency = use_state(String::new);
    let key_form_request_min_start_interval_ms = use_state(String::new);
    let saving_key = use_state(|| false);
    let latest_key_secret = use_state(|| None::<String>);

    let import_access_tokens = use_state(String::new);
    let import_session_jsons = use_state(String::new);

    let update_access_token = use_state(String::new);
    let update_plan_type = use_state(String::new);
    let update_status = use_state(String::new);
    let update_quota_remaining = use_state(String::new);
    let update_restore_at = use_state(String::new);
    let update_session_token = use_state(String::new);
    let update_user_agent = use_state(String::new);
    let update_impersonate_browser = use_state(String::new);
    let update_request_max_concurrency = use_state(String::new);
    let update_request_min_start_interval_ms = use_state(String::new);
    let update_proxy_mode = use_state(|| "inherit".to_string());
    let update_proxy_config_id = use_state(String::new);
    let selected_scheduler_account_name = use_state(String::new);
    let saving_account_scheduler = use_state(|| false);
    let saving_account_proxy_name = use_state(|| None::<String>);

    let editing_proxy_id = use_state(|| None::<String>);
    let proxy_form_name = use_state(String::new);
    let proxy_form_url = use_state(|| "http://127.0.0.1:11118".to_string());
    let proxy_form_username = use_state(String::new);
    let proxy_form_password = use_state(String::new);
    let proxy_form_status = use_state(|| "active".to_string());
    let saving_proxy = use_state(|| false);
    let checking_proxy = use_state(|| false);

    let generation_prompt = use_state(String::new);
    let generation_model = use_state(|| "gpt-image-2".to_string());
    let generation_n = use_state(|| "1".to_string());
    let generation_size = use_state(|| "1024x1024".to_string());
    let generation_output = use_state(|| "{}".to_string());
    let generation_images = use_state(Vec::<String>::new);

    let edit_prompt = use_state(String::new);
    let edit_model = use_state(|| "gpt-image-2".to_string());
    let edit_n = use_state(|| "1".to_string());
    let edit_size = use_state(|| "1024x1024".to_string());
    let edit_image_base64 = use_state(String::new);
    let edit_file_name = use_state(|| "image.png".to_string());
    let edit_mime_type = use_state(|| "image/png".to_string());
    let edit_output = use_state(|| "{}".to_string());
    let edit_images = use_state(Vec::<String>::new);

    let chat_request_json = use_state(|| {
        serde_json::json!({
            "model": "gpt-image-1",
            "modalities": ["image"],
            "messages": [
                {
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "Draw a cinematic anime heroine with city lights in the rain." }
                    ]
                }
            ]
        })
        .to_string()
    });
    let chat_output = use_state(|| "{}".to_string());

    let responses_request_json = use_state(|| {
        serde_json::json!({
            "model": "gpt-5",
            "input": "Generate a painterly anime-style portrait with dramatic backlight.",
            "tools": [{ "type": "image_generation" }]
        })
        .to_string()
    });
    let responses_output = use_state(|| "{}".to_string());

    {
        let selected_usage_event_id = (*selected_usage_event)
            .as_ref()
            .map(|event| event.event_id.clone());
        use_effect_with(selected_usage_event_id, move |_| {
            gpt2api_make_draggable("gpt2api-usage-detail", "gpt2api-usage-detail-handle");
            || ()
        });
    }

    // Copy a secret to the clipboard and surface a short notice. Used by
    // MaskedSecretCode's built-in copy button, so the user gets consistent
    // feedback across gpt2api / llm / kiro pages.
    let on_copy = {
        let notice = notice.clone();
        Callback::from(move |(label, value): (String, String)| {
            gpt2api_copy_text(&value);
            let text = if label.is_empty() {
                "已复制".to_string()
            } else {
                format!("已复制 {label}")
            };
            notice.set(Some(text));
        })
    };

    let reload_all = {
        let loading = loading.clone();
        let load_error = load_error.clone();
        let notice = notice.clone();
        let config = config.clone();
        let config_path = config_path.clone();
        let configured = configured.clone();
        let status_json = status_json.clone();
        let version_json = version_json.clone();
        let models_json = models_json.clone();
        let accounts = accounts.clone();
        let proxy_configs = proxy_configs.clone();
        let account_groups = account_groups.clone();
        let keys = keys.clone();
        let contribution_requests = contribution_requests.clone();
        let contribution_total = contribution_total.clone();
        let contribution_page = contribution_page.clone();
        let contribution_status_filter = contribution_status_filter.clone();
        let usage = usage.clone();
        let usage_limit = usage_limit.clone();
        let usage_search = usage_search.clone();
        let usage_key_filter = usage_key_filter.clone();
        let usage_total = usage_total.clone();
        let usage_offset = usage_offset.clone();
        let usage_current_rpm = usage_current_rpm.clone();
        let usage_current_in_flight = usage_current_in_flight.clone();
        let usage_billable_total = usage_billable_total.clone();
        let usage_has_more = usage_has_more.clone();
        Callback::from(move |_| {
            loading.set(true);
            load_error.set(None);
            notice.set(None);
            let loading = loading.clone();
            let load_error = load_error.clone();
            let config = config.clone();
            let config_path = config_path.clone();
            let configured = configured.clone();
            let status_json = status_json.clone();
            let version_json = version_json.clone();
            let models_json = models_json.clone();
            let accounts = accounts.clone();
            let proxy_configs = proxy_configs.clone();
            let account_groups = account_groups.clone();
            let keys = keys.clone();
            let contribution_requests = contribution_requests.clone();
            let contribution_total = contribution_total.clone();
            let contribution_page = contribution_page.clone();
            let contribution_status_filter = contribution_status_filter.clone();
            let usage = usage.clone();
            let usage_limit = usage_limit.clone();
            let usage_search = usage_search.clone();
            let usage_key_filter = usage_key_filter.clone();
            let usage_total = usage_total.clone();
            let usage_offset = usage_offset.clone();
            let usage_current_rpm = usage_current_rpm.clone();
            let usage_current_in_flight = usage_current_in_flight.clone();
            let usage_billable_total = usage_billable_total.clone();
            let usage_has_more = usage_has_more.clone();
            spawn_local(async move {
                let config_envelope = match fetch_admin_gpt2api_rs_config().await {
                    Ok(value) => value,
                    Err(err) => {
                        load_error.set(Some(err));
                        loading.set(false);
                        return;
                    },
                };
                config.set(config_envelope.config.clone());
                config_path.set(config_envelope.config_path);
                configured.set(config_envelope.configured);

                match fetch_admin_gpt2api_rs_status().await {
                    Ok(value) => status_json.set(pretty_json(&value)),
                    Err(err) => status_json.set(err),
                }
                match fetch_admin_gpt2api_rs_version().await {
                    Ok(value) => version_json.set(pretty_json(&value)),
                    Err(err) => version_json.set(err),
                }
                match fetch_admin_gpt2api_rs_models().await {
                    Ok(value) => models_json.set(pretty_json(&value)),
                    Err(err) => models_json.set(err),
                }
                match fetch_admin_gpt2api_rs_accounts().await {
                    Ok(value) => accounts.set(value),
                    Err(err) => load_error.set(Some(err)),
                }
                match fetch_admin_gpt2api_rs_proxy_configs().await {
                    Ok(value) => proxy_configs.set(value),
                    Err(err) => load_error.set(Some(err)),
                }
                match fetch_admin_gpt2api_rs_account_groups().await {
                    Ok(value) => account_groups.set(value.groups),
                    Err(err) => load_error.set(Some(err)),
                }
                match fetch_admin_gpt2api_rs_keys().await {
                    Ok(value) => keys.set(value),
                    Err(err) => load_error.set(Some(err)),
                }
                let contribution_limit = 25_usize;
                let contribution_offset = (*contribution_page)
                    .saturating_sub(1)
                    .saturating_mul(contribution_limit);
                match fetch_admin_gpt2api_account_contribution_requests(
                    &AdminGpt2ApiAccountContributionRequestsQuery {
                        status: Some((*contribution_status_filter).clone())
                            .filter(|value| !value.trim().is_empty()),
                        limit: Some(contribution_limit),
                        offset: Some(contribution_offset),
                    },
                )
                .await
                {
                    Ok(value) => {
                        contribution_total.set(value.total);
                        contribution_requests.set(value.requests);
                    },
                    Err(err) => load_error.set(Some(err)),
                }
                let limit = (*usage_limit).trim().parse::<u64>().unwrap_or(50).max(1);
                match fetch_admin_gpt2api_rs_usage_events(&AdminGpt2ApiRsUsageEventsQuery {
                    key_id: Some((*usage_key_filter).clone())
                        .filter(|value| !value.trim().is_empty()),
                    q: Some((*usage_search).clone()).filter(|value| !value.trim().is_empty()),
                    limit: Some(limit),
                    offset: Some(*usage_offset),
                })
                .await
                {
                    Ok(value) => {
                        usage.set(value.events);
                        usage_total.set(value.total);
                        usage_current_rpm.set(value.current_rpm);
                        usage_current_in_flight.set(value.current_in_flight);
                        usage_billable_total.set(value.billable_credit_total);
                        usage_has_more.set(value.has_more);
                    },
                    Err(err) => load_error.set(Some(err)),
                }
                loading.set(false);
            });
        })
    };

    {
        let reload_all = reload_all.clone();
        use_effect_with((), move |_| {
            reload_all.emit(());
            || ()
        });
    }

    let reload_usage_page = {
        let load_error = load_error.clone();
        let usage = usage.clone();
        let usage_limit = usage_limit.clone();
        let usage_search = usage_search.clone();
        let usage_key_filter = usage_key_filter.clone();
        let usage_total = usage_total.clone();
        let usage_offset = usage_offset.clone();
        let usage_current_rpm = usage_current_rpm.clone();
        let usage_current_in_flight = usage_current_in_flight.clone();
        let usage_billable_total = usage_billable_total.clone();
        let usage_has_more = usage_has_more.clone();
        Callback::from(move |offset: u64| {
            load_error.set(None);
            usage_offset.set(offset);
            let load_error = load_error.clone();
            let usage = usage.clone();
            let usage_limit = usage_limit.clone();
            let usage_search = usage_search.clone();
            let usage_key_filter = usage_key_filter.clone();
            let usage_total = usage_total.clone();
            let usage_current_rpm = usage_current_rpm.clone();
            let usage_current_in_flight = usage_current_in_flight.clone();
            let usage_billable_total = usage_billable_total.clone();
            let usage_has_more = usage_has_more.clone();
            spawn_local(async move {
                let limit = (*usage_limit).trim().parse::<u64>().unwrap_or(50).max(1);
                match fetch_admin_gpt2api_rs_usage_events(&AdminGpt2ApiRsUsageEventsQuery {
                    key_id: Some((*usage_key_filter).clone())
                        .filter(|value| !value.trim().is_empty()),
                    q: Some((*usage_search).clone()).filter(|value| !value.trim().is_empty()),
                    limit: Some(limit),
                    offset: Some(offset),
                })
                .await
                {
                    Ok(value) => {
                        usage.set(value.events);
                        usage_total.set(value.total);
                        usage_current_rpm.set(value.current_rpm);
                        usage_current_in_flight.set(value.current_in_flight);
                        usage_billable_total.set(value.billable_credit_total);
                        usage_has_more.set(value.has_more);
                    },
                    Err(err) => load_error.set(Some(err)),
                }
            });
        })
    };

    let reload_contribution_requests = {
        let contribution_requests = contribution_requests.clone();
        let contribution_total = contribution_total.clone();
        let contribution_page = contribution_page.clone();
        let contribution_status_filter = contribution_status_filter.clone();
        let contribution_loading = contribution_loading.clone();
        let load_error = load_error.clone();
        Callback::from(move |(page_override, status_override): (Option<usize>, Option<String>)| {
            let page = page_override.unwrap_or(*contribution_page).max(1);
            let status = status_override.unwrap_or_else(|| (*contribution_status_filter).clone());
            contribution_page.set(page);
            contribution_status_filter.set(status.clone());
            contribution_loading.set(true);
            load_error.set(None);
            let contribution_requests = contribution_requests.clone();
            let contribution_total = contribution_total.clone();
            let contribution_loading = contribution_loading.clone();
            let load_error = load_error.clone();
            spawn_local(async move {
                let limit = 25_usize;
                let offset = page.saturating_sub(1).saturating_mul(limit);
                match fetch_admin_gpt2api_account_contribution_requests(
                    &AdminGpt2ApiAccountContributionRequestsQuery {
                        status: Some(status).filter(|value| !value.trim().is_empty()),
                        limit: Some(limit),
                        offset: Some(offset),
                    },
                )
                .await
                {
                    Ok(value) => {
                        contribution_total.set(value.total);
                        contribution_requests.set(value.requests);
                    },
                    Err(err) => load_error.set(Some(err)),
                }
                contribution_loading.set(false);
            });
        })
    };

    let on_usage_scroll_top = {
        let usage_scroll_top_ref = usage_scroll_top_ref.clone();
        let usage_scroll_bottom_ref = usage_scroll_bottom_ref.clone();
        Callback::from(move |_| {
            let Some(top) = usage_scroll_top_ref.cast::<HtmlElement>() else {
                return;
            };
            let Some(bottom) = usage_scroll_bottom_ref.cast::<HtmlElement>() else {
                return;
            };
            let left = top.scroll_left();
            if bottom.scroll_left() != left {
                bottom.set_scroll_left(left);
            }
        })
    };

    let on_usage_scroll_bottom = {
        let usage_scroll_top_ref = usage_scroll_top_ref.clone();
        let usage_scroll_bottom_ref = usage_scroll_bottom_ref.clone();
        Callback::from(move |_| {
            let Some(bottom) = usage_scroll_bottom_ref.cast::<HtmlElement>() else {
                return;
            };
            let Some(top) = usage_scroll_top_ref.cast::<HtmlElement>() else {
                return;
            };
            let left = bottom.scroll_left();
            if top.scroll_left() != left {
                top.set_scroll_left(left);
            }
        })
    };

    let scroll_usage_table_by = {
        let usage_scroll_top_ref = usage_scroll_top_ref.clone();
        let usage_scroll_bottom_ref = usage_scroll_bottom_ref.clone();
        Callback::from(move |delta: i32| {
            let Some(bottom) = usage_scroll_bottom_ref.cast::<HtmlElement>() else {
                return;
            };
            let next_left = (bottom.scroll_left() + delta).max(0);
            bottom.set_scroll_left(next_left);
            if let Some(top) = usage_scroll_top_ref.cast::<HtmlElement>() {
                top.set_scroll_left(next_left);
            }
        })
    };

    {
        let usage_scroll_top_ref = usage_scroll_top_ref.clone();
        let usage_scroll_bottom_ref = usage_scroll_bottom_ref.clone();
        let usage_scroll_width = usage_scroll_width.clone();
        let event_count = usage.len();
        let offset = *usage_offset;
        let active_tab_name = (*active_tab).clone();
        use_effect_with((event_count, offset, active_tab_name), move |_| {
            if let Some(bottom) = usage_scroll_bottom_ref.cast::<HtmlElement>() {
                let measured_width = bottom.scroll_width().max(bottom.client_width()).max(1);
                usage_scroll_width.set(measured_width);
                if let Some(top) = usage_scroll_top_ref.cast::<HtmlElement>() {
                    top.set_scroll_left(bottom.scroll_left());
                }
            }
            || ()
        });
    }

    let on_save_config = {
        let config = config.clone();
        let saving_config = saving_config.clone();
        let load_error = load_error.clone();
        let notice = notice.clone();
        let reload_all = reload_all.clone();
        Callback::from(move |_| {
            saving_config.set(true);
            load_error.set(None);
            notice.set(None);
            let config = (*config).clone();
            let saving_config = saving_config.clone();
            let load_error = load_error.clone();
            let notice = notice.clone();
            let reload_all = reload_all.clone();
            spawn_local(async move {
                match update_admin_gpt2api_rs_config(&config).await {
                    Ok(_) => {
                        notice.set(Some("Saved gpt2api-rs config".to_string()));
                        reload_all.emit(());
                    },
                    Err(err) => load_error.set(Some(err)),
                }
                saving_config.set(false);
            });
        })
    };

    let on_test_login = {
        let login_json = login_json.clone();
        let load_error = load_error.clone();
        Callback::from(move |_| {
            load_error.set(None);
            let login_json = login_json.clone();
            let load_error = load_error.clone();
            spawn_local(async move {
                match post_admin_gpt2api_rs_login().await {
                    Ok(value) => login_json.set(pretty_json(&value)),
                    Err(err) => load_error.set(Some(err)),
                }
            });
        })
    };

    let on_import_accounts = {
        let import_access_tokens = import_access_tokens.clone();
        let import_session_jsons = import_session_jsons.clone();
        let load_error = load_error.clone();
        let notice = notice.clone();
        let reload_all = reload_all.clone();
        Callback::from(move |_| {
            let access_tokens = import_access_tokens
                .lines()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            let session_jsons = import_session_jsons
                .lines()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            if access_tokens.is_empty() && session_jsons.is_empty() {
                load_error
                    .set(Some("Import requires access tokens or session JSON lines".to_string()));
                return;
            }
            load_error.set(None);
            notice.set(None);
            let load_error = load_error.clone();
            let notice = notice.clone();
            let reload_all = reload_all.clone();
            let import_access_tokens = import_access_tokens.clone();
            let import_session_jsons = import_session_jsons.clone();
            spawn_local(async move {
                let request = AdminGpt2ApiRsImportAccountsRequest {
                    access_tokens,
                    session_jsons,
                };
                match import_admin_gpt2api_rs_accounts(&request).await {
                    Ok(_) => {
                        import_access_tokens.set(String::new());
                        import_session_jsons.set(String::new());
                        notice.set(Some("Imported accounts".to_string()));
                        reload_all.emit(());
                    },
                    Err(err) => load_error.set(Some(err)),
                }
            });
        })
    };

    let reset_proxy_form = {
        let editing_proxy_id = editing_proxy_id.clone();
        let proxy_form_name = proxy_form_name.clone();
        let proxy_form_url = proxy_form_url.clone();
        let proxy_form_username = proxy_form_username.clone();
        let proxy_form_password = proxy_form_password.clone();
        let proxy_form_status = proxy_form_status.clone();
        Callback::from(move |_| {
            editing_proxy_id.set(None);
            proxy_form_name.set(String::new());
            proxy_form_url.set("http://127.0.0.1:11118".to_string());
            proxy_form_username.set(String::new());
            proxy_form_password.set(String::new());
            proxy_form_status.set("active".to_string());
        })
    };

    let on_edit_proxy_config = {
        let editing_proxy_id = editing_proxy_id.clone();
        let proxy_form_name = proxy_form_name.clone();
        let proxy_form_url = proxy_form_url.clone();
        let proxy_form_username = proxy_form_username.clone();
        let proxy_form_password = proxy_form_password.clone();
        let proxy_form_status = proxy_form_status.clone();
        Callback::from(move |proxy_config: AdminGpt2ApiRsProxyConfigView| {
            editing_proxy_id.set(Some(proxy_config.id));
            proxy_form_name.set(proxy_config.name);
            proxy_form_url.set(proxy_config.proxy_url);
            proxy_form_username.set(proxy_config.proxy_username.unwrap_or_default());
            proxy_form_password.set(proxy_config.proxy_password.unwrap_or_default());
            proxy_form_status.set(proxy_config.status);
        })
    };

    let on_submit_proxy_config = {
        let editing_proxy_id = editing_proxy_id.clone();
        let proxy_form_name = proxy_form_name.clone();
        let proxy_form_url = proxy_form_url.clone();
        let proxy_form_username = proxy_form_username.clone();
        let proxy_form_password = proxy_form_password.clone();
        let proxy_form_status = proxy_form_status.clone();
        let saving_proxy = saving_proxy.clone();
        let load_error = load_error.clone();
        let notice = notice.clone();
        let reload_all = reload_all.clone();
        let reset_proxy_form = reset_proxy_form.clone();
        Callback::from(move |_| {
            let name = (*proxy_form_name).trim().to_string();
            if name.is_empty() {
                load_error.set(Some("Proxy config name is required".to_string()));
                return;
            }
            let proxy_url = (*proxy_form_url).trim().to_string();
            if proxy_url.is_empty() {
                load_error.set(Some("Proxy URL is required".to_string()));
                return;
            }
            let proxy_username = (!(*proxy_form_username).trim().is_empty())
                .then(|| (*proxy_form_username).trim().to_string());
            let proxy_password = (!(*proxy_form_password).trim().is_empty())
                .then(|| (*proxy_form_password).trim().to_string());
            let status = (*proxy_form_status).trim().to_string();
            let editing_proxy_id_value = (*editing_proxy_id).clone();
            saving_proxy.set(true);
            load_error.set(None);
            notice.set(None);
            let saving_proxy = saving_proxy.clone();
            let load_error = load_error.clone();
            let notice = notice.clone();
            let reload_all = reload_all.clone();
            let reset_proxy_form = reset_proxy_form.clone();
            spawn_local(async move {
                let result = if let Some(proxy_id) = editing_proxy_id_value {
                    let request = AdminGpt2ApiRsUpdateProxyConfigRequest {
                        name: Some(name),
                        proxy_url: Some(proxy_url),
                        proxy_username: Some(proxy_username),
                        proxy_password: Some(proxy_password),
                        status: Some(status),
                    };
                    update_admin_gpt2api_rs_proxy_config(&proxy_id, &request)
                        .await
                        .map(|_| "Updated proxy config".to_string())
                } else {
                    let request = AdminGpt2ApiRsCreateProxyConfigRequest {
                        name,
                        proxy_url,
                        proxy_username,
                        proxy_password,
                        status: Some(status),
                    };
                    create_admin_gpt2api_rs_proxy_config(&request)
                        .await
                        .map(|_| "Created proxy config".to_string())
                };
                match result {
                    Ok(message) => {
                        notice.set(Some(message));
                        reset_proxy_form.emit(());
                        reload_all.emit(());
                    },
                    Err(err) => load_error.set(Some(err)),
                }
                saving_proxy.set(false);
            });
        })
    };

    let on_check_proxy_config = {
        let checking_proxy = checking_proxy.clone();
        let load_error = load_error.clone();
        let notice = notice.clone();
        Callback::from(move |proxy_config: AdminGpt2ApiRsProxyConfigView| {
            if *checking_proxy {
                return;
            }
            checking_proxy.set(true);
            load_error.set(None);
            notice.set(None);
            let checking_proxy = checking_proxy.clone();
            let load_error = load_error.clone();
            let notice = notice.clone();
            spawn_local(async move {
                match check_admin_gpt2api_rs_proxy_config(&proxy_config.id).await {
                    Ok(result) => {
                        let message = format!(
                            "{}: {}",
                            proxy_config.name,
                            format_proxy_check_result(&result)
                        );
                        if result.ok {
                            notice.set(Some(message));
                        } else {
                            load_error.set(Some(message));
                        }
                    },
                    Err(err) => load_error.set(Some(err)),
                }
                checking_proxy.set(false);
            });
        })
    };

    let on_delete_proxy_config = {
        let editing_proxy_id = editing_proxy_id.clone();
        let load_error = load_error.clone();
        let notice = notice.clone();
        let reload_all = reload_all.clone();
        let reset_proxy_form = reset_proxy_form.clone();
        Callback::from(move |proxy_config: AdminGpt2ApiRsProxyConfigView| {
            if !confirm_destructive("确认删除这个 gpt2api-rs 代理配置？仍被账号绑定时删除会失败。")
            {
                return;
            }
            load_error.set(None);
            notice.set(None);
            let editing_proxy_id = editing_proxy_id.clone();
            let load_error = load_error.clone();
            let notice = notice.clone();
            let reload_all = reload_all.clone();
            let reset_proxy_form = reset_proxy_form.clone();
            spawn_local(async move {
                match delete_admin_gpt2api_rs_proxy_config(&proxy_config.id).await {
                    Ok(_) => {
                        if (*editing_proxy_id).as_deref() == Some(proxy_config.id.as_str()) {
                            reset_proxy_form.emit(());
                        }
                        notice.set(Some(format!("Deleted proxy config {}", proxy_config.name)));
                        reload_all.emit(());
                    },
                    Err(err) => load_error.set(Some(err)),
                }
            });
        })
    };

    let on_refresh_all_accounts = {
        let load_error = load_error.clone();
        let notice = notice.clone();
        let reload_all = reload_all.clone();
        Callback::from(move |_| {
            load_error.set(None);
            notice.set(None);
            let load_error = load_error.clone();
            let notice = notice.clone();
            let reload_all = reload_all.clone();
            spawn_local(async move {
                match refresh_admin_gpt2api_rs_accounts(&AdminGpt2ApiRsRefreshAccountsRequest {
                    access_tokens: Vec::new(),
                })
                .await
                {
                    Ok(_) => {
                        notice.set(Some("Refreshed accounts".to_string()));
                        reload_all.emit(());
                    },
                    Err(err) => load_error.set(Some(err)),
                }
            });
        })
    };

    let on_update_account = {
        let update_access_token = update_access_token.clone();
        let update_plan_type = update_plan_type.clone();
        let update_status = update_status.clone();
        let update_quota_remaining = update_quota_remaining.clone();
        let update_restore_at = update_restore_at.clone();
        let update_session_token = update_session_token.clone();
        let update_user_agent = update_user_agent.clone();
        let update_impersonate_browser = update_impersonate_browser.clone();
        let update_proxy_mode = update_proxy_mode.clone();
        let update_proxy_config_id = update_proxy_config_id.clone();
        let load_error = load_error.clone();
        let notice = notice.clone();
        let reload_all = reload_all.clone();
        Callback::from(move |_| {
            let access_token = (*update_access_token).trim().to_string();
            if access_token.is_empty() {
                load_error.set(Some("Select an account before updating".to_string()));
                return;
            }
            let quota_remaining = match (*update_quota_remaining).trim() {
                "" => None,
                value => match value.parse::<i64>() {
                    Ok(parsed) => Some(parsed),
                    Err(_) => {
                        load_error.set(Some("quota_remaining must be an integer".to_string()));
                        return;
                    },
                },
            };
            let plan_type = (!(*update_plan_type).trim().is_empty())
                .then(|| (*update_plan_type).trim().to_string());
            let status =
                (!(*update_status).trim().is_empty()).then(|| (*update_status).trim().to_string());
            let restore_at = (!(*update_restore_at).trim().is_empty())
                .then(|| (*update_restore_at).trim().to_string());
            let session_token = (!(*update_session_token).trim().is_empty())
                .then(|| (*update_session_token).trim().to_string());
            let user_agent = (!(*update_user_agent).trim().is_empty())
                .then(|| (*update_user_agent).trim().to_string());
            let impersonate_browser = (!(*update_impersonate_browser).trim().is_empty())
                .then(|| (*update_impersonate_browser).trim().to_string());
            let proxy_mode = match (*update_proxy_mode).trim() {
                "" => "inherit".to_string(),
                "inherit" | "direct" | "fixed" => (*update_proxy_mode).trim().to_string(),
                _ => {
                    load_error
                        .set(Some("proxy mode must be inherit, direct, or fixed".to_string()));
                    return;
                },
            };
            let proxy_config_id = match proxy_mode.as_str() {
                "fixed" => {
                    let value = (*update_proxy_config_id).trim().to_string();
                    if value.is_empty() {
                        load_error.set(Some(
                            "Select a proxy config when proxy mode is fixed".to_string(),
                        ));
                        return;
                    }
                    Some(Some(value))
                },
                _ => Some(None),
            };
            load_error.set(None);
            notice.set(None);
            let load_error = load_error.clone();
            let notice = notice.clone();
            let reload_all = reload_all.clone();
            spawn_local(async move {
                let request = AdminGpt2ApiRsUpdateAccountRequest {
                    access_token,
                    plan_type,
                    status,
                    quota_remaining,
                    restore_at,
                    session_token,
                    user_agent,
                    impersonate_browser,
                    request_max_concurrency: None,
                    request_min_start_interval_ms: None,
                    proxy_mode: Some(proxy_mode),
                    proxy_config_id,
                };
                match update_admin_gpt2api_rs_account(&request).await {
                    Ok(_) => {
                        notice.set(Some("Updated account".to_string()));
                        reload_all.emit(());
                    },
                    Err(err) => load_error.set(Some(err)),
                }
            });
        })
    };

    let on_set_account_proxy = {
        let saving_account_proxy_name = saving_account_proxy_name.clone();
        let load_error = load_error.clone();
        let notice = notice.clone();
        let reload_all = reload_all.clone();
        Callback::from(
            move |(access_token, account_name, proxy_value): (String, String, String)| {
                if access_token.trim().is_empty()
                    || *saving_account_proxy_name == Some(account_name.clone())
                {
                    return;
                }
                let (proxy_mode, proxy_config_id) =
                    if let Some(proxy_id) = proxy_value.strip_prefix("fixed:") {
                        ("fixed".to_string(), Some(Some(proxy_id.to_string())))
                    } else if proxy_value == "direct" {
                        ("direct".to_string(), Some(None))
                    } else {
                        ("inherit".to_string(), Some(None))
                    };
                saving_account_proxy_name.set(Some(account_name.clone()));
                load_error.set(None);
                notice.set(None);
                let saving_account_proxy_name = saving_account_proxy_name.clone();
                let load_error = load_error.clone();
                let notice = notice.clone();
                let reload_all = reload_all.clone();
                spawn_local(async move {
                    let request = AdminGpt2ApiRsUpdateAccountRequest {
                        access_token,
                        plan_type: None,
                        status: None,
                        quota_remaining: None,
                        restore_at: None,
                        session_token: None,
                        user_agent: None,
                        impersonate_browser: None,
                        request_max_concurrency: None,
                        request_min_start_interval_ms: None,
                        proxy_mode: Some(proxy_mode),
                        proxy_config_id,
                    };
                    match update_admin_gpt2api_rs_account(&request).await {
                        Ok(_) => {
                            notice.set(Some("Updated account proxy".to_string()));
                            reload_all.emit(());
                        },
                        Err(err) => load_error.set(Some(err)),
                    }
                    saving_account_proxy_name.set(None);
                });
            },
        )
    };

    let on_save_account_scheduler = {
        let update_access_token = update_access_token.clone();
        let selected_scheduler_account_name = selected_scheduler_account_name.clone();
        let update_request_max_concurrency = update_request_max_concurrency.clone();
        let update_request_min_start_interval_ms = update_request_min_start_interval_ms.clone();
        let saving_account_scheduler = saving_account_scheduler.clone();
        let load_error = load_error.clone();
        let notice = notice.clone();
        let reload_all = reload_all.clone();
        Callback::from(move |_| {
            let access_token = (*update_access_token).trim().to_string();
            if access_token.is_empty() {
                load_error
                    .set(Some("Load an account before saving scheduler controls".to_string()));
                return;
            }
            let request_max_concurrency = match (*update_request_max_concurrency).trim() {
                "" => {
                    load_error.set(Some("request_max_concurrency is required".to_string()));
                    return;
                },
                value => match value.parse::<u64>() {
                    Ok(parsed) => parsed,
                    Err(_) => {
                        load_error
                            .set(Some("request_max_concurrency must be an integer".to_string()));
                        return;
                    },
                },
            };
            let request_min_start_interval_ms = match (*update_request_min_start_interval_ms).trim()
            {
                "" => {
                    load_error.set(Some("request_min_start_interval_ms is required".to_string()));
                    return;
                },
                value => match value.parse::<u64>() {
                    Ok(parsed) => parsed,
                    Err(_) => {
                        load_error.set(Some(
                            "request_min_start_interval_ms must be an integer".to_string(),
                        ));
                        return;
                    },
                },
            };
            let account_name = if (*selected_scheduler_account_name).trim().is_empty() {
                "selected account".to_string()
            } else {
                (*selected_scheduler_account_name).trim().to_string()
            };
            saving_account_scheduler.set(true);
            load_error.set(None);
            notice.set(None);
            let saving_account_scheduler = saving_account_scheduler.clone();
            let load_error = load_error.clone();
            let notice = notice.clone();
            let reload_all = reload_all.clone();
            spawn_local(async move {
                let request = AdminGpt2ApiRsUpdateAccountRequest {
                    access_token,
                    plan_type: None,
                    status: None,
                    quota_remaining: None,
                    restore_at: None,
                    session_token: None,
                    user_agent: None,
                    impersonate_browser: None,
                    request_max_concurrency: Some(request_max_concurrency),
                    request_min_start_interval_ms: Some(request_min_start_interval_ms),
                    proxy_mode: None,
                    proxy_config_id: None,
                };
                match update_admin_gpt2api_rs_account(&request).await {
                    Ok(_) => {
                        notice.set(Some(format!("Saved scheduler controls for {account_name}")));
                        reload_all.emit(());
                    },
                    Err(err) => load_error.set(Some(err)),
                }
                saving_account_scheduler.set(false);
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
            }
            create_account_group_account_names.set(names);
        })
    };

    let on_create_account_group = {
        let create_account_group_name = create_account_group_name.clone();
        let create_account_group_account_names = create_account_group_account_names.clone();
        let creating_account_group = creating_account_group.clone();
        let load_error = load_error.clone();
        let notice = notice.clone();
        let reload_all = reload_all.clone();
        Callback::from(move |_| {
            if *creating_account_group {
                return;
            }
            let name = (*create_account_group_name).trim().to_string();
            if name.is_empty() {
                load_error.set(Some("Account group name is required".to_string()));
                return;
            }
            let account_names = (*create_account_group_account_names).clone();
            let create_account_group_name = create_account_group_name.clone();
            let create_account_group_account_names = create_account_group_account_names.clone();
            let creating_account_group = creating_account_group.clone();
            let load_error = load_error.clone();
            let notice = notice.clone();
            let reload_all = reload_all.clone();
            spawn_local(async move {
                creating_account_group.set(true);
                load_error.set(None);
                match create_admin_gpt2api_rs_account_group(
                    &AdminGpt2ApiRsCreateAccountGroupRequest {
                        name: name.clone(),
                        account_names,
                    },
                )
                .await
                {
                    Ok(_) => {
                        create_account_group_name.set(String::new());
                        create_account_group_account_names.set(Vec::new());
                        notice.set(Some(format!("Created account group {name}")));
                        reload_all.emit(());
                    },
                    Err(err) => load_error.set(Some(err)),
                }
                creating_account_group.set(false);
            });
        })
    };

    let reset_key_form = {
        let editing_key_id = editing_key_id.clone();
        let key_form_name = key_form_name.clone();
        let key_form_status = key_form_status.clone();
        let key_form_quota_total_calls = key_form_quota_total_calls.clone();
        let key_form_route_strategy = key_form_route_strategy.clone();
        let key_form_role = key_form_role.clone();
        let key_form_account_group_id = key_form_account_group_id.clone();
        let key_form_fixed_account_name = key_form_fixed_account_name.clone();
        let key_form_request_max_concurrency = key_form_request_max_concurrency.clone();
        let key_form_request_min_start_interval_ms = key_form_request_min_start_interval_ms.clone();
        let latest_key_secret = latest_key_secret.clone();
        Callback::from(move |_| {
            editing_key_id.set(None);
            key_form_name.set(String::new());
            key_form_status.set("active".to_string());
            key_form_quota_total_calls.set("100".to_string());
            key_form_route_strategy.set("auto".to_string());
            key_form_role.set("user".to_string());
            key_form_account_group_id.set(String::new());
            key_form_fixed_account_name.set(String::new());
            key_form_request_max_concurrency.set(String::new());
            key_form_request_min_start_interval_ms.set(String::new());
            latest_key_secret.set(None);
        })
    };

    let on_edit_key = {
        let editing_key_id = editing_key_id.clone();
        let key_form_name = key_form_name.clone();
        let key_form_status = key_form_status.clone();
        let key_form_quota_total_calls = key_form_quota_total_calls.clone();
        let key_form_route_strategy = key_form_route_strategy.clone();
        let key_form_role = key_form_role.clone();
        let key_form_account_group_id = key_form_account_group_id.clone();
        let key_form_fixed_account_name = key_form_fixed_account_name.clone();
        let key_form_request_max_concurrency = key_form_request_max_concurrency.clone();
        let key_form_request_min_start_interval_ms = key_form_request_min_start_interval_ms.clone();
        let latest_key_secret = latest_key_secret.clone();
        Callback::from(move |key: AdminGpt2ApiRsKeyView| {
            editing_key_id.set(Some(key.id));
            key_form_name.set(key.name);
            key_form_status.set(key.status);
            key_form_quota_total_calls.set(key.quota_total_calls.to_string());
            let route_mode = if key
                .fixed_account_name
                .as_deref()
                .unwrap_or_default()
                .is_empty()
            {
                if key.account_group_id.is_some() {
                    "group"
                } else {
                    "auto"
                }
            } else {
                "account"
            };
            key_form_route_strategy.set(route_mode.to_string());
            key_form_role.set(key.role);
            key_form_account_group_id.set(key.account_group_id.unwrap_or_default());
            key_form_fixed_account_name.set(key.fixed_account_name.unwrap_or_default());
            key_form_request_max_concurrency.set(
                key.request_max_concurrency
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
            );
            key_form_request_min_start_interval_ms.set(
                key.request_min_start_interval_ms
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
            );
            latest_key_secret.set(None);
        })
    };

    let on_submit_key = {
        let editing_key_id = editing_key_id.clone();
        let key_form_name = key_form_name.clone();
        let key_form_status = key_form_status.clone();
        let key_form_quota_total_calls = key_form_quota_total_calls.clone();
        let key_form_route_strategy = key_form_route_strategy.clone();
        let key_form_role = key_form_role.clone();
        let key_form_account_group_id = key_form_account_group_id.clone();
        let key_form_fixed_account_name = key_form_fixed_account_name.clone();
        let key_form_request_max_concurrency = key_form_request_max_concurrency.clone();
        let key_form_request_min_start_interval_ms = key_form_request_min_start_interval_ms.clone();
        let saving_key = saving_key.clone();
        let latest_key_secret = latest_key_secret.clone();
        let load_error = load_error.clone();
        let notice = notice.clone();
        let reload_all = reload_all.clone();
        Callback::from(move |_| {
            let name = (*key_form_name).trim().to_string();
            if name.is_empty() {
                load_error.set(Some("Key name is required".to_string()));
                return;
            }
            let quota_total_calls = match parse_required_i64_input(
                (*key_form_quota_total_calls).as_str(),
                "quota_total_calls",
            ) {
                Ok(value) => value,
                Err(err) => {
                    load_error.set(Some(err));
                    return;
                },
            };
            let request_max_concurrency = match parse_optional_u64_input(
                (*key_form_request_max_concurrency).as_str(),
                "request_max_concurrency",
            ) {
                Ok(value) => value,
                Err(err) => {
                    load_error.set(Some(err));
                    return;
                },
            };
            let request_min_start_interval_ms = match parse_optional_u64_input(
                (*key_form_request_min_start_interval_ms).as_str(),
                "request_min_start_interval_ms",
            ) {
                Ok(value) => value,
                Err(err) => {
                    load_error.set(Some(err));
                    return;
                },
            };
            let status = (*key_form_status).trim().to_string();
            let route_mode = (*key_form_route_strategy).trim().to_string();
            if route_mode.is_empty() {
                load_error.set(Some("Route mode is required".to_string()));
                return;
            }
            let role = (*key_form_role).trim().to_ascii_lowercase();
            if !matches!(role.as_str(), "user" | "admin") {
                load_error.set(Some("role must be user or admin".to_string()));
                return;
            }
            let mut route_strategy = "auto".to_string();
            let mut account_group_id = None::<String>;
            let mut fixed_account_name = None::<String>;
            match route_mode.as_str() {
                "auto" => {},
                "group" => {
                    let value = (*key_form_account_group_id).trim().to_string();
                    if value.is_empty() {
                        load_error
                            .set(Some("Select an account group for group routing".to_string()));
                        return;
                    }
                    account_group_id = Some(value);
                },
                "account" => {
                    let value = (*key_form_fixed_account_name).trim().to_string();
                    if value.is_empty() {
                        load_error.set(Some("Select one account to bind this key".to_string()));
                        return;
                    }
                    route_strategy = "fixed".to_string();
                    fixed_account_name = Some(value);
                },
                _ => {
                    load_error.set(Some("Route mode must be all, group, or account".to_string()));
                    return;
                },
            }
            let editing_key_id_value = (*editing_key_id).clone();
            let saving_key = saving_key.clone();
            let latest_key_secret = latest_key_secret.clone();
            let load_error = load_error.clone();
            let notice = notice.clone();
            let reload_all = reload_all.clone();
            let editing_key_id = editing_key_id.clone();
            let key_form_name = key_form_name.clone();
            let key_form_status = key_form_status.clone();
            let key_form_quota_total_calls = key_form_quota_total_calls.clone();
            let key_form_route_strategy = key_form_route_strategy.clone();
            let key_form_role = key_form_role.clone();
            let key_form_account_group_id = key_form_account_group_id.clone();
            let key_form_fixed_account_name = key_form_fixed_account_name.clone();
            let key_form_request_max_concurrency = key_form_request_max_concurrency.clone();
            let key_form_request_min_start_interval_ms =
                key_form_request_min_start_interval_ms.clone();
            saving_key.set(true);
            load_error.set(None);
            notice.set(None);
            latest_key_secret.set(None);
            spawn_local(async move {
                let result = if let Some(key_id) = editing_key_id_value.clone() {
                    let request = AdminGpt2ApiRsUpdateKeyRequest {
                        name: Some(name.clone()),
                        status: Some(status.clone()),
                        quota_total_calls: Some(quota_total_calls),
                        route_strategy: Some(route_strategy.clone()),
                        role: Some(role.clone()),
                        account_group_id: Some(account_group_id.clone()),
                        fixed_account_name: Some(fixed_account_name.clone()),
                        request_max_concurrency,
                        request_min_start_interval_ms,
                    };
                    update_admin_gpt2api_rs_key(&key_id, &request).await
                } else {
                    let request = AdminGpt2ApiRsCreateKeyRequest {
                        name: name.clone(),
                        quota_total_calls,
                        status: Some(status.clone()),
                        route_strategy: route_strategy.clone(),
                        role: Some(role.clone()),
                        account_group_id: account_group_id.clone(),
                        fixed_account_name: fixed_account_name.clone(),
                        request_max_concurrency,
                        request_min_start_interval_ms,
                    };
                    create_admin_gpt2api_rs_key(&request).await
                };

                match result {
                    Ok(key) => {
                        editing_key_id.set(Some(key.id.clone()));
                        key_form_name.set(key.name.clone());
                        key_form_status.set(key.status.clone());
                        key_form_quota_total_calls.set(key.quota_total_calls.to_string());
                        let route_mode = if key
                            .fixed_account_name
                            .as_deref()
                            .unwrap_or_default()
                            .is_empty()
                        {
                            if key.account_group_id.is_some() {
                                "group"
                            } else {
                                "auto"
                            }
                        } else {
                            "account"
                        };
                        key_form_route_strategy.set(route_mode.to_string());
                        key_form_role.set(key.role.clone());
                        key_form_account_group_id
                            .set(key.account_group_id.clone().unwrap_or_default());
                        key_form_fixed_account_name
                            .set(key.fixed_account_name.clone().unwrap_or_default());
                        key_form_request_max_concurrency.set(
                            key.request_max_concurrency
                                .map(|value| value.to_string())
                                .unwrap_or_default(),
                        );
                        key_form_request_min_start_interval_ms.set(
                            key.request_min_start_interval_ms
                                .map(|value| value.to_string())
                                .unwrap_or_default(),
                        );
                        latest_key_secret.set(key.secret_plaintext.clone());
                        notice.set(Some(if editing_key_id_value.is_some() {
                            "Updated key".to_string()
                        } else {
                            "Created key".to_string()
                        }));
                        reload_all.emit(());
                    },
                    Err(err) => load_error.set(Some(err)),
                }
                saving_key.set(false);
            });
        })
    };

    let on_rotate_key = {
        let latest_key_secret = latest_key_secret.clone();
        let load_error = load_error.clone();
        let notice = notice.clone();
        let reload_all = reload_all.clone();
        Callback::from(move |key: AdminGpt2ApiRsKeyView| {
            if !confirm_destructive(&format!("Reissue plaintext key for \"{}\"?", key.name)) {
                return;
            }
            let latest_key_secret = latest_key_secret.clone();
            let load_error = load_error.clone();
            let notice = notice.clone();
            let reload_all = reload_all.clone();
            spawn_local(async move {
                match rotate_admin_gpt2api_rs_key(&key.id).await {
                    Ok(rotated) => {
                        latest_key_secret.set(rotated.secret_plaintext.clone());
                        notice.set(Some(format!("Reissued key {}", key.name)));
                        reload_all.emit(());
                    },
                    Err(err) => load_error.set(Some(err)),
                }
            });
        })
    };

    let on_delete_key = {
        let editing_key_id = editing_key_id.clone();
        let latest_key_secret = latest_key_secret.clone();
        let load_error = load_error.clone();
        let notice = notice.clone();
        let reload_all = reload_all.clone();
        let reset_key_form = reset_key_form.clone();
        Callback::from(move |key: AdminGpt2ApiRsKeyView| {
            if !confirm_destructive(&format!("Delete key \"{}\"?", key.name)) {
                return;
            }
            let editing_key_id_value = (*editing_key_id).clone();
            let latest_key_secret = latest_key_secret.clone();
            let load_error = load_error.clone();
            let notice = notice.clone();
            let reload_all = reload_all.clone();
            let reset_key_form = reset_key_form.clone();
            spawn_local(async move {
                match delete_admin_gpt2api_rs_key(&key.id).await {
                    Ok(_) => {
                        if editing_key_id_value.as_ref() == Some(&key.id) {
                            reset_key_form.emit(());
                        }
                        latest_key_secret.set(None);
                        notice.set(Some(format!("Deleted key {}", key.name)));
                        reload_all.emit(());
                    },
                    Err(err) => load_error.set(Some(err)),
                }
            });
        })
    };

    let on_generate_images = {
        let generation_prompt = generation_prompt.clone();
        let generation_model = generation_model.clone();
        let generation_n = generation_n.clone();
        let generation_size = generation_size.clone();
        let generation_output = generation_output.clone();
        let generation_images = generation_images.clone();
        let load_error = load_error.clone();
        Callback::from(move |_| {
            let n = match (*generation_n).trim().parse::<usize>() {
                Ok(value) => value,
                Err(_) => {
                    load_error.set(Some("generation n must be an integer".to_string()));
                    return;
                },
            };
            load_error.set(None);
            let generation_output = generation_output.clone();
            let generation_images = generation_images.clone();
            let load_error = load_error.clone();
            let request = AdminGpt2ApiRsImageGenerationRequest {
                prompt: (*generation_prompt).clone(),
                model: (*generation_model).clone(),
                n,
                size: (*generation_size).clone(),
                response_format: "b64_json".to_string(),
            };
            spawn_local(async move {
                match admin_gpt2api_rs_generate_images(&request).await {
                    Ok(value) => {
                        generation_images.set(extract_image_data_urls(&value));
                        generation_output.set(pretty_json(&value));
                    },
                    Err(err) => load_error.set(Some(err)),
                }
            });
        })
    };

    let on_edit_image_file_change = {
        let edit_image_base64 = edit_image_base64.clone();
        let edit_file_name = edit_file_name.clone();
        let edit_mime_type = edit_mime_type.clone();
        let load_error = load_error.clone();
        Callback::from(move |event: Event| {
            let Some(input) = event
                .target()
                .and_then(|target| target.dyn_into::<HtmlInputElement>().ok())
            else {
                return;
            };
            let Some(files) = input.files() else {
                return;
            };
            let Some(file) = files.get(0) else {
                return;
            };
            let edit_image_base64 = edit_image_base64.clone();
            let edit_file_name = edit_file_name.clone();
            let edit_mime_type = edit_mime_type.clone();
            let load_error = load_error.clone();
            spawn_local(async move {
                match read_file_as_base64(file).await {
                    Ok((base64, file_name, mime_type)) => {
                        edit_image_base64.set(base64);
                        edit_file_name.set(file_name);
                        edit_mime_type.set(mime_type);
                    },
                    Err(err) => load_error.set(Some(err)),
                }
            });
        })
    };

    let on_edit_images = {
        let edit_prompt = edit_prompt.clone();
        let edit_model = edit_model.clone();
        let edit_n = edit_n.clone();
        let edit_size = edit_size.clone();
        let edit_image_base64 = edit_image_base64.clone();
        let edit_file_name = edit_file_name.clone();
        let edit_mime_type = edit_mime_type.clone();
        let edit_output = edit_output.clone();
        let edit_images = edit_images.clone();
        let load_error = load_error.clone();
        Callback::from(move |_| {
            if (*edit_image_base64).trim().is_empty() {
                load_error.set(Some("Choose an image before calling /images/edits".to_string()));
                return;
            }
            let n = match (*edit_n).trim().parse::<usize>() {
                Ok(value) => value,
                Err(_) => {
                    load_error.set(Some("edit n must be an integer".to_string()));
                    return;
                },
            };
            load_error.set(None);
            let request = AdminGpt2ApiRsImageEditRequest {
                prompt: (*edit_prompt).clone(),
                model: (*edit_model).clone(),
                n,
                size: (*edit_size).clone(),
                image_base64: (*edit_image_base64).clone(),
                file_name: (*edit_file_name).clone(),
                mime_type: (*edit_mime_type).clone(),
            };
            let edit_output = edit_output.clone();
            let edit_images = edit_images.clone();
            let load_error = load_error.clone();
            spawn_local(async move {
                match admin_gpt2api_rs_edit_images(&request).await {
                    Ok(value) => {
                        edit_images.set(extract_image_data_urls(&value));
                        edit_output.set(pretty_json(&value));
                    },
                    Err(err) => load_error.set(Some(err)),
                }
            });
        })
    };

    let on_run_chat_completions = {
        let chat_request_json = chat_request_json.clone();
        let chat_output = chat_output.clone();
        let load_error = load_error.clone();
        Callback::from(move |_| {
            let request = match parse_json_text((*chat_request_json).as_str()) {
                Ok(value) => value,
                Err(err) => {
                    load_error.set(Some(err));
                    return;
                },
            };
            load_error.set(None);
            let chat_output = chat_output.clone();
            let load_error = load_error.clone();
            spawn_local(async move {
                match admin_gpt2api_rs_chat_completions(&request).await {
                    Ok(value) => chat_output.set(pretty_json(&value)),
                    Err(err) => load_error.set(Some(err)),
                }
            });
        })
    };

    let on_run_responses = {
        let responses_request_json = responses_request_json.clone();
        let responses_output = responses_output.clone();
        let load_error = load_error.clone();
        Callback::from(move |_| {
            let request = match parse_json_text((*responses_request_json).as_str()) {
                Ok(value) => value,
                Err(err) => {
                    load_error.set(Some(err));
                    return;
                },
            };
            load_error.set(None);
            let responses_output = responses_output.clone();
            let load_error = load_error.clone();
            spawn_local(async move {
                match admin_gpt2api_rs_responses(&request).await {
                    Ok(value) => responses_output.set(pretty_json(&value)),
                    Err(err) => load_error.set(Some(err)),
                }
            });
        })
    };

    let on_contribution_status_filter_change = {
        let reload_contribution_requests = reload_contribution_requests.clone();
        Callback::from(move |event: Event| {
            if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                reload_contribution_requests.emit((Some(1), Some(target.value())));
            }
        })
    };

    let on_contribution_page_change = {
        let reload_contribution_requests = reload_contribution_requests.clone();
        Callback::from(move |page: usize| {
            reload_contribution_requests.emit((Some(page), None));
        })
    };

    let on_approve_contribution = {
        let contribution_action_inflight = contribution_action_inflight.clone();
        let contribution_requests = contribution_requests.clone();
        let reload_contribution_requests = reload_contribution_requests.clone();
        let load_error = load_error.clone();
        let notice = notice.clone();
        let reload_all = reload_all.clone();
        Callback::from(move |request_id: String| {
            let contribution_action_inflight = contribution_action_inflight.clone();
            let contribution_requests = contribution_requests.clone();
            let reload_contribution_requests = reload_contribution_requests.clone();
            let load_error = load_error.clone();
            let notice = notice.clone();
            let reload_all = reload_all.clone();
            spawn_local(async move {
                let mut inflight = (*contribution_action_inflight).clone();
                inflight.insert(request_id.clone());
                contribution_action_inflight.set(inflight);
                match approve_admin_gpt2api_account_contribution_request(&request_id, None).await {
                    Ok(updated) => {
                        let mut items = (*contribution_requests).clone();
                        if let Some(item) = items
                            .iter_mut()
                            .find(|item| item.request_id == updated.request_id)
                        {
                            *item = updated;
                        }
                        contribution_requests.set(items);
                        notice.set(Some("Contribution approved and key issued".to_string()));
                        load_error.set(None);
                        reload_all.emit(());
                        reload_contribution_requests.emit((None, None));
                    },
                    Err(err) => load_error.set(Some(err)),
                }
                let mut inflight = (*contribution_action_inflight).clone();
                inflight.remove(&request_id);
                contribution_action_inflight.set(inflight);
            });
        })
    };

    let on_reject_contribution = {
        let contribution_action_inflight = contribution_action_inflight.clone();
        let contribution_requests = contribution_requests.clone();
        let reload_contribution_requests = reload_contribution_requests.clone();
        let load_error = load_error.clone();
        let notice = notice.clone();
        Callback::from(move |request_id: String| {
            if !confirm_destructive("确认拒绝这个 GPT 账号贡献请求？") {
                return;
            }
            let contribution_action_inflight = contribution_action_inflight.clone();
            let contribution_requests = contribution_requests.clone();
            let reload_contribution_requests = reload_contribution_requests.clone();
            let load_error = load_error.clone();
            let notice = notice.clone();
            spawn_local(async move {
                let mut inflight = (*contribution_action_inflight).clone();
                inflight.insert(request_id.clone());
                contribution_action_inflight.set(inflight);
                match reject_admin_gpt2api_account_contribution_request(&request_id, None).await {
                    Ok(updated) => {
                        let mut items = (*contribution_requests).clone();
                        if let Some(item) = items
                            .iter_mut()
                            .find(|item| item.request_id == updated.request_id)
                        {
                            *item = updated;
                        }
                        contribution_requests.set(items);
                        notice.set(Some("Contribution rejected".to_string()));
                        load_error.set(None);
                        reload_contribution_requests.emit((None, None));
                    },
                    Err(err) => load_error.set(Some(err)),
                }
                let mut inflight = (*contribution_action_inflight).clone();
                inflight.remove(&request_id);
                contribution_action_inflight.set(inflight);
            });
        })
    };

    // Client-side account filter: matches on name / access_token prefix /
    // user_agent fragment (case-insensitive). Kept as a cloned Vec so the
    // existing `.iter().map()` rendering below still works unchanged.
    let accounts_query_lower = (*accounts_search).trim().to_lowercase();
    let filtered_accounts: Vec<AdminGpt2ApiRsAccountView> =
        use_memo(((*accounts).clone(), accounts_query_lower.clone()), |(items, q)| {
            if q.is_empty() {
                items.clone()
            } else {
                items
                    .iter()
                    .filter(|a| {
                        let ua = parse_browser_profile(a)
                            .user_agent
                            .unwrap_or_default()
                            .to_lowercase();
                        a.name.to_lowercase().contains(q.as_str())
                            || a.access_token.to_lowercase().contains(q.as_str())
                            || ua.contains(q.as_str())
                    })
                    .cloned()
                    .collect()
            }
        })
        .as_ref()
        .clone();

    let account_groups_query_lower = (*account_groups_search).trim().to_lowercase();
    let filtered_account_groups: Vec<AdminGpt2ApiRsAccountGroupView> =
        use_memo(((*account_groups).clone(), account_groups_query_lower.clone()), |(items, q)| {
            items
                .iter()
                .filter(|group| gpt2api_group_matches_query(group, q))
                .cloned()
                .collect::<Vec<_>>()
        })
        .as_ref()
        .clone();
    let on_account_groups_search_change = {
        let account_groups_search = account_groups_search.clone();
        Callback::from(move |value: String| account_groups_search.set(value))
    };
    let on_account_group_error = {
        let load_error = load_error.clone();
        Callback::from(move |value: String| load_error.set(Some(value)))
    };
    let on_account_group_notice = {
        let notice = notice.clone();
        Callback::from(move |value: String| notice.set(Some(value)))
    };

    // Tab wiring. Pure UI switch — all data is still reloaded together by
    // `reload_all`, so switching tabs does not trigger additional network.
    let on_tab_select = {
        let active_tab = active_tab.clone();
        Callback::from(move |id: String| active_tab.set(id))
    };
    let tabs: [(&str, &str); 8] = [
        (GPT2API_TAB_OVERVIEW, "Dashboard"),
        (GPT2API_TAB_ACCOUNTS, "Accounts"),
        (GPT2API_TAB_PROXIES, "Proxies"),
        (GPT2API_TAB_GROUPS, "Groups"),
        (GPT2API_TAB_KEYS, "Keys & Routing"),
        (GPT2API_TAB_CONTRIBUTIONS, "Contributions"),
        (GPT2API_TAB_USAGE, "Usage Logs"),
        (GPT2API_TAB_ADVANCED, "Advanced"),
    ];
    let active = (*active_tab).clone();
    let contribution_total_pages = ((*contribution_total).saturating_add(24) / 25).max(1);

    html! {
        <main class={classes!("container", "py-8", "space-y-5")}>
            <section class={classes!("bg-[var(--surface)]", "border", "border-[var(--border)]", "rounded-[var(--radius)]", "shadow-[var(--shadow)]", "p-5")}>
                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                    <div>
                        <h1 class={classes!("m-0", "text-xl", "font-semibold")}>{ "gpt2api-rs Admin" }</h1>
                        <p class={classes!("m-0", "text-sm", "text-[var(--muted)]")}>
                            { "Operate service health, accounts, proxies, key routing, and usage logs for the deployed gpt2api-rs service." }
                        </p>
                    </div>
                    <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                        <Link<Route> to={Route::Admin} classes={classes!("btn-fluent-secondary")}>
                            { "Back to /admin" }
                        </Link<Route>>
                        <button class={classes!("btn-fluent-primary")} onclick={{
                            let reload_all = reload_all.clone();
                            Callback::from(move |_| reload_all.emit(()))
                        }} disabled={*loading}>
                            { if *loading { "Loading..." } else { "Reload" } }
                        </button>
                    </div>
                </div>
                if let Some(err) = &*load_error {
                    <div class={classes!("mt-3", "rounded-[var(--radius)]", "border", "border-red-400/40", "bg-red-500/10", "px-3", "py-2", "text-sm", "text-red-700", "dark:text-red-200")}>
                        { err.clone() }
                    </div>
                }
                if let Some(message) = &*notice {
                    <div class={classes!("mt-3", "rounded-[var(--radius)]", "border", "border-emerald-400/40", "bg-emerald-500/10", "px-3", "py-2", "text-sm", "text-emerald-700", "dark:text-emerald-200")}>
                        { message.clone() }
                    </div>
                }
            </section>

            { render_tab_bar(&active, &tabs, &on_tab_select, None) }

            if active == GPT2API_TAB_OVERVIEW {
            <section class={classes!("bg-[var(--surface)]", "border", "border-[var(--border)]", "rounded-[var(--radius)]", "shadow-[var(--shadow)]", "p-5", "space-y-3")}>
                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                    <div>
                        <h2 class={classes!("m-0", "text-lg", "font-semibold")}>{ "Config" }</h2>
                        <p class={classes!("m-0", "text-sm", "text-[var(--muted)]")}>
                            { format!("Config file: {}{}", (*config_path), if *configured { " (configured)" } else { " (not configured)" }) }
                        </p>
                    </div>
                    <button class={classes!("btn-fluent-primary")} onclick={on_save_config} disabled={*saving_config}>
                        { if *saving_config { "Saving..." } else { "Save Config" } }
                    </button>
                </div>
                <label class="block text-sm">
                    <span>{ "Base URL" }</span>
                    <input
                        class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2"
                        value={config.base_url.clone()}
                        oninput={{
                            let config = config.clone();
                            Callback::from(move |e: InputEvent| {
                                let value = e.target_unchecked_into::<HtmlInputElement>().value();
                                let mut next = (*config).clone();
                                next.base_url = value;
                                config.set(next);
                            })
                        }}
                    />
                </label>
                <label class="block text-sm">
                    <span>{ "Admin Token" }</span>
                    <input
                        class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2"
                        value={config.admin_token.clone()}
                        oninput={{
                            let config = config.clone();
                            Callback::from(move |e: InputEvent| {
                                let value = e.target_unchecked_into::<HtmlInputElement>().value();
                                let mut next = (*config).clone();
                                next.admin_token = value;
                                config.set(next);
                            })
                        }}
                    />
                </label>
                <label class="block text-sm">
                    <span>{ "Public API Key" }</span>
                    <input
                        class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2"
                        value={config.api_key.clone()}
                        oninput={{
                            let config = config.clone();
                            Callback::from(move |e: InputEvent| {
                                let value = e.target_unchecked_into::<HtmlInputElement>().value();
                                let mut next = (*config).clone();
                                next.api_key = value;
                                config.set(next);
                            })
                        }}
                    />
                </label>
                <label class="block text-sm">
                    <span>{ "Timeout Seconds" }</span>
                    <input
                        class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2"
                        value={config.timeout_seconds.to_string()}
                        oninput={{
                            let config = config.clone();
                            Callback::from(move |e: InputEvent| {
                                let value = e.target_unchecked_into::<HtmlInputElement>().value();
                                let mut next = (*config).clone();
                                next.timeout_seconds = value.parse::<u64>().unwrap_or(60);
                                config.set(next);
                            })
                        }}
                    />
                </label>
            </section>

            <section class={classes!("grid", "gap-4", "md:grid-cols-4")}>
                <div class={classes!("rounded", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-4")}>
                    <div class={classes!("text-xs", "uppercase", "text-[var(--muted)]")}>{ "Accounts" }</div>
                    <div class={classes!("mt-1", "text-2xl", "font-semibold")}>{ accounts.len() }</div>
                    <div class={classes!("text-xs", "text-[var(--muted)]")}>{ format!("{} active", accounts.iter().filter(|a| a.status == "active").count()) }</div>
                </div>
                <div class={classes!("rounded", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-4")}>
                    <div class={classes!("text-xs", "uppercase", "text-[var(--muted)]")}>{ "Keys" }</div>
                    <div class={classes!("mt-1", "text-2xl", "font-semibold")}>{ keys.len() }</div>
                    <div class={classes!("text-xs", "text-[var(--muted)]")}>{ format!("{} admin", keys.iter().filter(|key| key.role == "admin").count()) }</div>
                </div>
                <div class={classes!("rounded", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-4")}>
                    <div class={classes!("text-xs", "uppercase", "text-[var(--muted)]")}>{ "Proxies" }</div>
                    <div class={classes!("mt-1", "text-2xl", "font-semibold")}>{ proxy_configs.len() }</div>
                    <div class={classes!("text-xs", "text-[var(--muted)]")}>{ format!("{} active", proxy_configs.iter().filter(|proxy| proxy.status == "active").count()) }</div>
                </div>
                <div class={classes!("rounded", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-4")}>
                    <div class={classes!("text-xs", "uppercase", "text-[var(--muted)]")}>{ "Usage" }</div>
                    <div class={classes!("mt-1", "text-2xl", "font-semibold")}>{ *usage_current_rpm }</div>
                    <div class={classes!("text-xs", "text-[var(--muted)]")}>{ format!("in-flight {}", *usage_current_in_flight) }</div>
                </div>
            </section>

            <section class={classes!("grid", "gap-5")}>
                <article class={classes!("bg-[var(--surface)]", "border", "border-[var(--border)]", "rounded-[var(--radius)]", "shadow-[var(--shadow)]", "p-5")}>
                    <h2 class={classes!("m-0", "text-lg", "font-semibold")}>{ "Service Snapshot" }</h2>
                    <pre class={classes!("mt-3", "overflow-x-auto", "rounded", "bg-[var(--surface-alt)]", "p-3", "text-xs")}>{ (*status_json).clone() }</pre>
                </article>
            </section>
            }

            if active == GPT2API_TAB_ACCOUNTS {
            <section class={classes!("bg-[var(--surface)]", "border", "border-[var(--border)]", "rounded-[var(--radius)]", "shadow-[var(--shadow)]", "p-5", "space-y-4")}>
                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                    <div>
                        <h2 class={classes!("m-0", "text-lg", "font-semibold")}>{ "Accounts" }</h2>
                        <p class={classes!("m-0", "text-sm", "text-[var(--muted)]")}>{ "Import, refresh, delete, and update upstream ChatGPT accounts." }</p>
                    </div>
                    <button class={classes!("btn-fluent-secondary")} onclick={on_refresh_all_accounts}>{ "Refresh All Accounts" }</button>
                </div>

                <div class={classes!("grid", "gap-4", "lg:grid-cols-2")}>
                    <div>
                        <label class="block text-sm">
                            <span>{ "Access Tokens (one per line)" }</span>
                            <textarea
                                class="mt-1 h-32 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2"
                                value={(*import_access_tokens).clone()}
                                oninput={{
                                    let import_access_tokens = import_access_tokens.clone();
                                    Callback::from(move |e: InputEvent| {
                                        import_access_tokens.set(e.target_unchecked_into::<HtmlTextAreaElement>().value());
                                    })
                                }}
                            />
                        </label>
                    </div>
                    <div>
                        <label class="block text-sm">
                            <span>{ "Session JSONs (one JSON blob per line)" }</span>
                            <textarea
                                class="mt-1 h-32 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2"
                                value={(*import_session_jsons).clone()}
                                oninput={{
                                    let import_session_jsons = import_session_jsons.clone();
                                    Callback::from(move |e: InputEvent| {
                                        import_session_jsons.set(e.target_unchecked_into::<HtmlTextAreaElement>().value());
                                    })
                                }}
                            />
                        </label>
                    </div>
                </div>
                <button class={classes!("btn-fluent-primary")} onclick={on_import_accounts}>{ "Import Accounts" }</button>

                <div class={classes!("flex", "items-center", "gap-3", "flex-wrap")}>
                    <div class={classes!("flex-1", "min-w-[240px]")}>
                        <SearchBox
                            value={(*accounts_search).clone()}
                            on_change={{
                                let accounts_search = accounts_search.clone();
                                Callback::from(move |v: String| accounts_search.set(v))
                            }}
                            placeholder={"按名称 / access token / user agent 搜索"}
                        />
                    </div>
                    <span class={classes!("text-xs", "text-[var(--muted)]")}>
                        { format!("{} / {}", filtered_accounts.len(), accounts.len()) }
                    </span>
                </div>

                <div class={classes!("overflow-x-auto")}>
                    <table class={classes!("w-full", "text-sm")}>
                        <thead>
                            <tr class={classes!("text-left", "border-b", "border-[var(--border)]")}>
                                <th class="py-2 pr-3">{ "Name" }</th>
                                <th class="py-2 pr-3">{ "Token" }</th>
                                <th class="py-2 pr-3">{ "Status" }</th>
                                <th class="py-2 pr-3">{ "Plan" }</th>
                                <th class="py-2 pr-3">{ "Quota" }</th>
                                <th class="py-2 pr-3">{ "Restore At" }</th>
                                <th class="py-2 pr-3">{ "Last Refresh" }</th>
                                <th class="py-2 pr-3">{ "Scheduler" }</th>
                                <th class="py-2 pr-3">{ "Proxy" }</th>
                                <th class="py-2 pr-3">{ "Actions" }</th>
                            </tr>
                        </thead>
                        <tbody>
                            { for filtered_accounts.iter().map(|account| {
                                let account_for_edit = account.clone();
                                let account_for_delete = account.clone();
                                let update_access_token = update_access_token.clone();
                                let update_plan_type = update_plan_type.clone();
                                let update_status = update_status.clone();
                                let update_quota_remaining = update_quota_remaining.clone();
                                let update_restore_at = update_restore_at.clone();
                                let update_session_token = update_session_token.clone();
                                let update_user_agent = update_user_agent.clone();
                                let update_impersonate_browser = update_impersonate_browser.clone();
                                let update_request_max_concurrency = update_request_max_concurrency.clone();
                                let update_request_min_start_interval_ms = update_request_min_start_interval_ms.clone();
                                let update_proxy_mode = update_proxy_mode.clone();
                                let update_proxy_config_id = update_proxy_config_id.clone();
                                let selected_scheduler_account_name = selected_scheduler_account_name.clone();
                                    let load_error = load_error.clone();
                                    let notice = notice.clone();
                                    let reload_all = reload_all.clone();
                                    let on_set_account_proxy = on_set_account_proxy.clone();
                                    let saving_account_proxy_name = saving_account_proxy_name.clone();
                                    let current_proxy_value = match account.proxy_mode.as_str() {
                                        "fixed" => account
                                            .proxy_config_id
                                            .as_ref()
                                            .map(|id| format!("fixed:{id}"))
                                            .unwrap_or_else(|| "inherit".to_string()),
                                        "direct" => "direct".to_string(),
                                        _ => "inherit".to_string(),
                                    };
                                    html! {
                                    <tr class={classes!("border-b", "border-[var(--border)]", "align-top")}>
                                        <td class="py-2 pr-3">{ account.name.clone() }</td>
                                        <td class="py-2 pr-3">
                                            <MaskedSecretCode
                                                value={account.access_token.clone()}
                                                copy_label={"access token"}
                                                on_copy={on_copy.clone()}
                                            />
                                        </td>
                                        <td class="py-2 pr-3">{ account.status.clone() }</td>
                                        <td class="py-2 pr-3">{ account.plan_type.clone().unwrap_or_else(|| "-".to_string()) }</td>
                                        <td class="py-2 pr-3">
                                            { if account.quota_known { account.quota_remaining.to_string() } else { "unknown".to_string() } }
                                        </td>
                                        <td class="py-2 pr-3">{ format_account_restore_at(account) }</td>
                                        <td class="py-2 pr-3">
                                            { account.last_refresh_at.map(|ts| format_ms(ts * 1000)).unwrap_or_else(|| "-".to_string()) }
                                        </td>
                                        <td class="py-2 pr-3">
                                            <div class={classes!("text-xs", "font-mono", "text-[var(--muted)]")}>
                                                { format_account_scheduler(account) }
                                            </div>
                                        </td>
                                            <td class="py-2 pr-3">
                                                <select
                                                    class="w-full min-w-[14rem] rounded border border-[var(--border)] bg-transparent px-2 py-1 text-xs"
                                                    value={current_proxy_value}
                                                    disabled={*saving_account_proxy_name == Some(account.name.clone())}
                                                    onchange={{
                                                        let on_set_account_proxy = on_set_account_proxy.clone();
                                                        let access_token = account.access_token.clone();
                                                        let account_name = account.name.clone();
                                                        Callback::from(move |e: Event| {
                                                            on_set_account_proxy.emit((
                                                                access_token.clone(),
                                                                account_name.clone(),
                                                                e.target_unchecked_into::<HtmlSelectElement>().value(),
                                                            ))
                                                        })
                                                    }}
                                                >
                                                    <option value="inherit">{ "Inherit default" }</option>
                                                    <option value="direct">{ "Direct" }</option>
                                                    { for proxy_configs.iter().filter(|proxy| proxy.status == "active").map(|proxy| html! {
                                                        <option value={format!("fixed:{}", proxy.id)}>{ format!("Fixed · {}", proxy.name) }</option>
                                                    }) }
                                                </select>
                                                <div class={classes!("mt-1", "text-xs", "text-[var(--muted)]", "break-all")}>
                                                    { format_account_effective_proxy(account) }
                                                </div>
                                                <div class={classes!("mt-1", "text-xs", "font-mono", "text-[var(--muted)]")}>
                                                    { format_account_proxy_binding(account) }
                                                </div>
                                            </td>
                                        <td class="py-2 pr-3">
                                            <div class={classes!("flex", "gap-2", "flex-wrap")}>
                                                <button
                                                    class={classes!("btn-fluent-secondary")}
                                                    onclick={Callback::from(move |_| {
                                                        let profile = parse_browser_profile(&account_for_edit);
                                                        update_access_token.set(account_for_edit.access_token.clone());
                                                        update_plan_type.set(account_for_edit.plan_type.clone().unwrap_or_default());
                                                        update_status.set(account_for_edit.status.clone());
                                                        update_quota_remaining.set(account_for_edit.quota_remaining.to_string());
                                                        update_restore_at.set(account_for_edit.restore_at.clone().unwrap_or_default());
                                                        update_session_token.set(profile.session_token.unwrap_or_default());
                                                        update_user_agent.set(profile.user_agent.unwrap_or_default());
                                                        update_impersonate_browser.set(profile.impersonate_browser.unwrap_or_default());
                                                        update_request_max_concurrency.set(account_for_edit.request_max_concurrency.map(|v| v.to_string()).unwrap_or_default());
                                                        update_request_min_start_interval_ms.set(account_for_edit.request_min_start_interval_ms.map(|v| v.to_string()).unwrap_or_default());
                                                        update_proxy_mode.set(account_for_edit.proxy_mode.clone());
                                                        update_proxy_config_id.set(account_for_edit.proxy_config_id.clone().unwrap_or_default());
                                                        selected_scheduler_account_name.set(account_for_edit.name.clone());
                                                    })}
                                                >
                                                        { "Advanced" }
                                                </button>
                                                <button
                                                    class={classes!("btn-fluent-secondary")}
                                                    onclick={Callback::from(move |_| {
                                                        if !confirm_destructive("确认删除这个 gpt2api-rs 账户？此操作不可撤销。") {
                                                            return;
                                                        }
                                                        load_error.set(None);
                                                        notice.set(None);
                                                        let load_error = load_error.clone();
                                                        let notice = notice.clone();
                                                        let reload_all = reload_all.clone();
                                                        let access_token = account_for_delete.access_token.clone();
                                                        spawn_local(async move {
                                                            match delete_admin_gpt2api_rs_accounts(&AdminGpt2ApiRsDeleteAccountsRequest {
                                                                access_tokens: vec![access_token],
                                                            })
                                                            .await
                                                            {
                                                                Ok(_) => {
                                                                    notice.set(Some("Deleted account".to_string()));
                                                                    reload_all.emit(());
                                                                }
                                                                Err(err) => load_error.set(Some(err)),
                                                            }
                                                        });
                                                    })}
                                                >
                                                    { "Delete" }
                                                </button>
                                            </div>
                                            if let Some(err) = account.last_error.clone() {
                                                <div class={classes!("mt-2", "text-xs", "text-red-600")}>{ err }</div>
                                            }
                                        </td>
                                    </tr>
                                }
                            }) }
                        </tbody>
                    </table>
                </div>

                <details class={classes!("rounded-[var(--radius)]", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                    <summary class={classes!("cursor-pointer", "text-sm", "font-semibold")}>
                        { "Advanced account fields" }
                        <span class={classes!("ml-2", "font-normal", "text-[var(--muted)]")}>
                            { if (*selected_scheduler_account_name).trim().is_empty() { "Select Load Account to edit scheduler, tokens, and metadata." } else { "Loaded account is ready for advanced edits." } }
                        </span>
                    </summary>
                    <div class={classes!("mt-4", "space-y-4")}>
                <div class={classes!("rounded-[var(--radius)]", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-4", "space-y-4")}>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <div>
                            <h3 class={classes!("m-0", "text-base", "font-semibold")}>{ "Account Scheduler" }</h3>
                            <p class={classes!("m-0", "mt-1", "text-sm", "text-[var(--muted)]")}>
                                { "Per-account concurrency and minimum start interval mirror the Kiro account scheduler flow: load one account, edit both integer values, then save them together." }
                            </p>
                        </div>
                        <span class={classes!("text-xs", "font-mono", "text-[var(--muted)]")}>
                            {
                                if (*selected_scheduler_account_name).trim().is_empty() {
                                    "No account loaded".to_string()
                                } else {
                                    format!("Editing {}", (*selected_scheduler_account_name))
                                }
                            }
                        </span>
                    </div>
                    <div class={classes!("grid", "gap-4", "md:grid-cols-3")}>
                        <label class="block text-sm md:col-span-1">
                            <span>{ "Account" }</span>
                            <input
                                class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2"
                                value={(*selected_scheduler_account_name).clone()}
                                readonly=true
                            />
                        </label>
                        <label class="block text-sm">
                            <span>{ "Request Max Concurrency" }</span>
                            <input class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2" value={(*update_request_max_concurrency).clone()} oninput={{
                                let update_request_max_concurrency = update_request_max_concurrency.clone();
                                Callback::from(move |e: InputEvent| update_request_max_concurrency.set(e.target_unchecked_into::<HtmlInputElement>().value()))
                            }} />
                        </label>
                        <label class="block text-sm">
                            <span>{ "Request Min Start Interval Ms" }</span>
                            <input class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2" value={(*update_request_min_start_interval_ms).clone()} oninput={{
                                let update_request_min_start_interval_ms = update_request_min_start_interval_ms.clone();
                                Callback::from(move |e: InputEvent| update_request_min_start_interval_ms.set(e.target_unchecked_into::<HtmlInputElement>().value()))
                            }} />
                        </label>
                    </div>
                    <div class={classes!("flex", "items-center", "gap-3", "flex-wrap")}>
                        <button class={classes!("btn-fluent-primary")} onclick={on_save_account_scheduler} disabled={*saving_account_scheduler}>
                            { if *saving_account_scheduler { "Saving..." } else { "Save Account Scheduler" } }
                        </button>
                        <span class={classes!("text-xs", "text-[var(--muted)]")}>
                            { "These two values directly gate request fan-out for the selected upstream ChatGPT account." }
                        </span>
                    </div>
                </div>

                <div class={classes!("grid", "gap-4", "lg:grid-cols-2")}>
                    <label class="block text-sm">
                        <span>{ "Selected Access Token" }</span>
                        <input class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2" value={(*update_access_token).clone()} oninput={{
                            let update_access_token = update_access_token.clone();
                            Callback::from(move |e: InputEvent| update_access_token.set(e.target_unchecked_into::<HtmlInputElement>().value()))
                        }} />
                    </label>
                    <label class="block text-sm">
                        <span>{ "Plan Type" }</span>
                        <input class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2" value={(*update_plan_type).clone()} oninput={{
                            let update_plan_type = update_plan_type.clone();
                            Callback::from(move |e: InputEvent| update_plan_type.set(e.target_unchecked_into::<HtmlInputElement>().value()))
                        }} />
                    </label>
                    <label class="block text-sm">
                        <span>{ "Status" }</span>
                        <input class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2" value={(*update_status).clone()} oninput={{
                            let update_status = update_status.clone();
                            Callback::from(move |e: InputEvent| update_status.set(e.target_unchecked_into::<HtmlInputElement>().value()))
                        }} />
                    </label>
                    <label class="block text-sm">
                        <span>{ "Quota Remaining" }</span>
                        <input class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2" value={(*update_quota_remaining).clone()} oninput={{
                            let update_quota_remaining = update_quota_remaining.clone();
                            Callback::from(move |e: InputEvent| update_quota_remaining.set(e.target_unchecked_into::<HtmlInputElement>().value()))
                        }} />
                    </label>
                    <label class="block text-sm">
                        <span>{ "Restore At" }</span>
                        <input class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2" value={(*update_restore_at).clone()} oninput={{
                            let update_restore_at = update_restore_at.clone();
                            Callback::from(move |e: InputEvent| update_restore_at.set(e.target_unchecked_into::<HtmlInputElement>().value()))
                        }} />
                    </label>
                    <label class="block text-sm">
                        <span>{ "Session Token" }</span>
                        <input class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2" value={(*update_session_token).clone()} oninput={{
                            let update_session_token = update_session_token.clone();
                            Callback::from(move |e: InputEvent| update_session_token.set(e.target_unchecked_into::<HtmlInputElement>().value()))
                        }} />
                    </label>
                    <label class="block text-sm">
                        <span>{ "User Agent" }</span>
                        <input class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2" value={(*update_user_agent).clone()} oninput={{
                            let update_user_agent = update_user_agent.clone();
                            Callback::from(move |e: InputEvent| update_user_agent.set(e.target_unchecked_into::<HtmlInputElement>().value()))
                        }} />
                    </label>
                    <label class="block text-sm">
                        <span>{ "Impersonate Browser" }</span>
                        <input class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2" value={(*update_impersonate_browser).clone()} oninput={{
                            let update_impersonate_browser = update_impersonate_browser.clone();
                            Callback::from(move |e: InputEvent| update_impersonate_browser.set(e.target_unchecked_into::<HtmlInputElement>().value()))
                        }} />
                    </label>
                    <label class="block text-sm">
                        <span>{ "Proxy Mode" }</span>
                        <select
                            class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2"
                            value={(*update_proxy_mode).clone()}
                            onchange={{
                                let update_proxy_mode = update_proxy_mode.clone();
                                Callback::from(move |e: Event| {
                                    update_proxy_mode.set(e.target_unchecked_into::<HtmlSelectElement>().value())
                                })
                            }}
                        >
                            <option value="inherit">{ "inherit" }</option>
                            <option value="direct">{ "direct" }</option>
                            <option value="fixed">{ "fixed" }</option>
                        </select>
                    </label>
                    <label class="block text-sm">
                        <span>{ "Proxy Config" }</span>
                        <select
                            class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2"
                            value={(*update_proxy_config_id).clone()}
                            onchange={{
                                let update_proxy_config_id = update_proxy_config_id.clone();
                                Callback::from(move |e: Event| {
                                    update_proxy_config_id
                                        .set(e.target_unchecked_into::<HtmlSelectElement>().value())
                                })
                            }}
                            disabled={(*update_proxy_mode).as_str() != "fixed"}
                        >
                            <option value="">{ "Select proxy config" }</option>
                            { for proxy_configs.iter().map(|proxy_config| html! {
                                <option value={proxy_config.id.clone()}>
                                    { format!("{} · {}", proxy_config.name, proxy_config.proxy_url) }
                                </option>
                            }) }
                        </select>
                    </label>
                </div>
                <div class={classes!("text-xs", "text-[var(--muted)]")}>
                    {
                        if (*update_proxy_mode).as_str() == "fixed" && (*update_proxy_config_id).trim().is_empty() {
                            "Fixed mode requires a saved proxy config.".to_string()
                        } else if (*update_proxy_mode).as_str() == "direct" {
                            "Direct mode bypasses the global default proxy.".to_string()
                        } else {
                            "Inherit mode follows the gpt2api-rs default upstream proxy.".to_string()
                        }
                    }
                </div>
                <button class={classes!("btn-fluent-primary")} onclick={on_update_account}>{ "Update Selected Account" }</button>
                    </div>
                </details>
            </section>
            }

            if active == GPT2API_TAB_PROXIES {
            <section class={classes!("bg-[var(--surface)]", "border", "border-[var(--border)]", "rounded-[var(--radius)]", "shadow-[var(--shadow)]", "p-5", "space-y-4")}>
                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                    <div>
                        <h2 class={classes!("m-0", "text-lg", "font-semibold")}>{ "Proxies" }</h2>
                        <p class={classes!("m-0", "text-sm", "text-[var(--muted)]")}>{ "Create, test, and assign upstream proxies. Account rows can bind to these proxies directly." }</p>
                    </div>
                    <button
                        class={classes!("btn-fluent-secondary")}
                        onclick={{
                            let reset_proxy_form = reset_proxy_form.clone();
                            Callback::from(move |_| reset_proxy_form.emit(()))
                        }}
                    >
                        { if (*editing_proxy_id).is_some() { "New Proxy" } else { "Reset" } }
                    </button>
                </div>

                <div class={classes!("grid", "gap-4", "lg:grid-cols-[minmax(18rem,26rem)_minmax(0,1fr)]")}>
                    <div class={classes!("space-y-3", "rounded", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                        <div class={classes!("text-sm", "font-semibold")}>
                            { if (*editing_proxy_id).is_some() { "Edit Proxy" } else { "Create Proxy" } }
                        </div>
                        <label class="block text-sm">
                            <span>{ "Name" }</span>
                            <input class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2" value={(*proxy_form_name).clone()} oninput={{
                                let proxy_form_name = proxy_form_name.clone();
                                Callback::from(move |e: InputEvent| proxy_form_name.set(e.target_unchecked_into::<HtmlInputElement>().value()))
                            }} />
                        </label>
                        <label class="block text-sm">
                            <span>{ "Proxy URL" }</span>
                            <input class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2 font-mono" value={(*proxy_form_url).clone()} oninput={{
                                let proxy_form_url = proxy_form_url.clone();
                                Callback::from(move |e: InputEvent| proxy_form_url.set(e.target_unchecked_into::<HtmlInputElement>().value()))
                            }} />
                        </label>
                        <div class={classes!("grid", "gap-3", "md:grid-cols-2")}>
                            <label class="block text-sm">
                                <span>{ "Username" }</span>
                                <input class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2" value={(*proxy_form_username).clone()} oninput={{
                                    let proxy_form_username = proxy_form_username.clone();
                                    Callback::from(move |e: InputEvent| proxy_form_username.set(e.target_unchecked_into::<HtmlInputElement>().value()))
                                }} />
                            </label>
                            <label class="block text-sm">
                                <span>{ "Password" }</span>
                                <input class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2" value={(*proxy_form_password).clone()} oninput={{
                                    let proxy_form_password = proxy_form_password.clone();
                                    Callback::from(move |e: InputEvent| proxy_form_password.set(e.target_unchecked_into::<HtmlInputElement>().value()))
                                }} />
                            </label>
                        </div>
                        <label class="block text-sm">
                            <span>{ "Status" }</span>
                            <select class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2" value={(*proxy_form_status).clone()} onchange={{
                                let proxy_form_status = proxy_form_status.clone();
                                Callback::from(move |e: Event| proxy_form_status.set(e.target_unchecked_into::<HtmlSelectElement>().value()))
                            }}>
                                <option value="active">{ "active" }</option>
                                <option value="disabled">{ "disabled" }</option>
                            </select>
                        </label>
                        <button class={classes!("btn-fluent-primary")} onclick={on_submit_proxy_config} disabled={*saving_proxy}>
                            { if *saving_proxy { "Saving..." } else if (*editing_proxy_id).is_some() { "Update Proxy" } else { "Create Proxy" } }
                        </button>
                    </div>

                    <div class={classes!("overflow-x-auto")}>
                        <table class={classes!("w-full", "text-sm")}>
                            <thead>
                                <tr class={classes!("text-left", "border-b", "border-[var(--border)]")}>
                                    <th class="py-2 pr-3">{ "Name" }</th>
                                    <th class="py-2 pr-3">{ "URL" }</th>
                                    <th class="py-2 pr-3">{ "Bound Accounts" }</th>
                                    <th class="py-2 pr-3">{ "Actions" }</th>
                                </tr>
                            </thead>
                            <tbody>
                                { for proxy_configs.iter().map(|proxy_config| {
                                    let proxy_for_edit = proxy_config.clone();
                                    let proxy_for_check = proxy_config.clone();
                                    let proxy_for_delete = proxy_config.clone();
                                    let bound_accounts: Vec<String> = accounts
                                        .iter()
                                        .filter(|account| account.proxy_mode == "fixed" && account.proxy_config_id.as_deref() == Some(proxy_config.id.as_str()))
                                        .map(|account| account.name.clone())
                                        .collect();
                                    html! {
                                        <tr class={classes!("border-b", "border-[var(--border)]", "align-top")}>
                                            <td class="py-2 pr-3">
                                                <div class={classes!("font-medium")}>{ proxy_config.name.clone() }</div>
                                                <div class={classes!("mt-1", "text-xs", "text-[var(--muted)]")}>{ proxy_config.status.clone() }</div>
                                            </td>
                                            <td class="py-2 pr-3">
                                                <div class={classes!("font-mono", "text-xs", "break-all")}>{ proxy_config.proxy_url.clone() }</div>
                                            </td>
                                            <td class="py-2 pr-3">
                                                { if bound_accounts.is_empty() { "none".to_string() } else { bound_accounts.join(", ") } }
                                            </td>
                                            <td class="py-2 pr-3">
                                                <div class={classes!("flex", "gap-2", "flex-wrap")}>
                                                    <button class={classes!("btn-fluent-secondary")} onclick={{
                                                        let on_edit_proxy_config = on_edit_proxy_config.clone();
                                                        Callback::from(move |_| on_edit_proxy_config.emit(proxy_for_edit.clone()))
                                                    }}>{ "Edit" }</button>
                                                    <button class={classes!("btn-fluent-secondary")} onclick={{
                                                        let on_check_proxy_config = on_check_proxy_config.clone();
                                                        Callback::from(move |_| on_check_proxy_config.emit(proxy_for_check.clone()))
                                                    }} disabled={*checking_proxy}>{ if *checking_proxy { "Checking..." } else { "Check" } }</button>
                                                    <button class={classes!("btn-fluent-secondary")} onclick={{
                                                        let on_delete_proxy_config = on_delete_proxy_config.clone();
                                                        Callback::from(move |_| on_delete_proxy_config.emit(proxy_for_delete.clone()))
                                                    }}>{ "Delete" }</button>
                                                </div>
                                            </td>
                                        </tr>
                                    }
                                }) }
                            </tbody>
                        </table>
                        if proxy_configs.is_empty() {
                            <p class={classes!("m-0", "mt-3", "text-sm", "text-[var(--muted)]")}>{ "No proxy configs yet." }</p>
                        }
                    </div>
                </div>
            </section>
            }

            if active == GPT2API_TAB_GROUPS {
            <section class={classes!("bg-[var(--surface)]", "border", "border-[var(--border)]", "rounded-[var(--radius)]", "shadow-[var(--shadow)]", "p-5", "space-y-4")}>
                <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                    <div>
                        <h2 class={classes!("m-0", "text-lg", "font-semibold")}>{ "Account Groups" }</h2>
                        <p class={classes!("m-0", "mt-1", "text-sm", "text-[var(--muted)]")}>
                            { "Use groups as the route unit. Keys can use the full account pool, an automatic subset group, or a single-account group for fixed routing." }
                        </p>
                    </div>
                    <button
                        class={classes!("btn-fluent-secondary")}
                        onclick={{
                            let reload_all = reload_all.clone();
                            Callback::from(move |_| reload_all.emit(()))
                        }}
                        disabled={*loading}
                    >
                        { if *loading { "Refreshing..." } else { "Refresh Groups" } }
                    </button>
                </div>

                <div class={classes!("max-w-md")}>
                    <SearchBox
                        value={(*account_groups_search).clone()}
                        on_change={on_account_groups_search_change.clone()}
                        placeholder={"Search group name / id / member account"}
                    />
                </div>

                <div class={classes!("rounded-[var(--radius)]", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <div>
                            <h3 class={classes!("m-0", "text-base", "font-semibold")}>{ "Create Account Group" }</h3>
                            <p class={classes!("m-0", "mt-1", "text-xs", "text-[var(--muted)]")}>
                                { "Collapsed by default. Select one member to make a fixed-route group; select multiple members for an automatic subset." }
                            </p>
                        </div>
                        <button
                            type="button"
                            class={classes!("btn-fluent-secondary")}
                            onclick={{
                                let account_group_form_expanded = account_group_form_expanded.clone();
                                Callback::from(move |_| account_group_form_expanded.set(!*account_group_form_expanded))
                            }}
                        >
                            { if *account_group_form_expanded { "Collapse" } else { "Expand" } }
                        </button>
                    </div>

                    if *account_group_form_expanded {
                    <div class={classes!("mt-4", "grid", "gap-3")}>
                        <label class="block text-sm">
                            <span>{ "Name" }</span>
                            <input
                                class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2"
                                value={(*create_account_group_name).clone()}
                                oninput={{
                                    let create_account_group_name = create_account_group_name.clone();
                                    Callback::from(move |e: InputEvent| {
                                        create_account_group_name.set(e.target_unchecked_into::<HtmlInputElement>().value())
                                    })
                                }}
                            />
                        </label>

                        <div class={classes!("grid", "gap-2", "xl:grid-cols-2")}>
                            { for accounts.iter().map(|account| {
                                let checked = create_account_group_account_names.iter().any(|name| name == &account.name);
                                let account_name = account.name.clone();
                                let on_toggle_create_account_group_member =
                                    on_toggle_create_account_group_member.clone();
                                html! {
                                    <label class={classes!(
                                        "flex", "cursor-pointer", "items-center", "gap-3", "rounded", "border", "px-3", "py-2",
                                        if checked { "border-sky-500/40 bg-sky-500/10" } else { "border-[var(--border)] bg-[var(--surface)]" }
                                    )}>
                                        <input
                                            type="checkbox"
                                            checked={checked}
                                            onchange={Callback::from(move |_| {
                                                on_toggle_create_account_group_member.emit(account_name.clone())
                                            })}
                                        />
                                        <div class={classes!("min-w-0")}>
                                            <div class={classes!("font-medium")}>{ account.name.clone() }</div>
                                            <div class={classes!("text-xs", "text-[var(--muted)]")}>
                                                { format!("{} · quota {}", account.status, account.quota_remaining) }
                                            </div>
                                        </div>
                                    </label>
                                }
                            }) }
                        </div>

                        <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                            <span class={classes!("text-xs", "text-[var(--muted)]")}>
                                { format!(
                                    "Selected: {}",
                                    if create_account_group_account_names.is_empty() {
                                        "none".to_string()
                                    } else {
                                        create_account_group_account_names.join(", ")
                                    }
                                ) }
                            </span>
                            <button
                                class={classes!("btn-fluent-primary")}
                                onclick={on_create_account_group}
                                disabled={*creating_account_group}
                            >
                                { if *creating_account_group { "Creating..." } else { "Create Group" } }
                            </button>
                        </div>
                    </div>
                    }
                </div>

                <div class={classes!("grid", "gap-4", "2xl:grid-cols-2")}>
                    if account_groups.is_empty() && !*loading {
                        <div class={classes!("rounded", "border", "border-dashed", "border-[var(--border)]", "px-4", "py-8", "text-center", "text-[var(--muted)]")}>
                            { "No account groups yet." }
                        </div>
                    } else if filtered_account_groups.is_empty() {
                        <div class={classes!("rounded", "border", "border-dashed", "border-[var(--border)]", "px-4", "py-8", "text-center", "text-[var(--muted)]")}>
                            { "No matching account groups." }
                        </div>
                    } else {
                        { for filtered_account_groups.iter().map(|group| html! {
                            <Gpt2ApiAccountGroupEditor
                                key={group.id.clone()}
                                group={group.clone()}
                                accounts={(*accounts).clone()}
                                on_changed={reload_all.clone()}
                                on_error={on_account_group_error.clone()}
                                on_notice={on_account_group_notice.clone()}
                            />
                        }) }
                    }
                </div>
            </section>
            }

            if active == GPT2API_TAB_KEYS {
            <section class={classes!("grid", "gap-5")}>
                <article class={classes!("bg-[var(--surface)]", "border", "border-[var(--border)]", "rounded-[var(--radius)]", "shadow-[var(--shadow)]", "p-5")}>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <div>
                            <h2 class={classes!("m-0", "text-lg", "font-semibold")}>{ "API Keys" }</h2>
                            <p class={classes!("m-0", "mt-1", "text-sm", "text-[var(--muted)]")}>
                                { "Create, reissue, disable, delete, or promote public keys. Admin-role keys unlock the product admin tools after logging in at /gpt2api/login." }
                            </p>
                        </div>
                        <button class={classes!("btn-fluent-secondary")} onclick={{
                            let reset_key_form = reset_key_form.clone();
                            Callback::from(move |_| reset_key_form.emit(()))
                        }}>
                            { if (*editing_key_id).is_some() { "New Key" } else { "Reset Form" } }
                        </button>
                    </div>

                    <div class={classes!("mt-4", "grid", "gap-3", "md:grid-cols-2", "xl:grid-cols-4")}>
                        <label class="block text-sm">
                            <span>{ "Name" }</span>
                            <input
                                class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2"
                                value={(*key_form_name).clone()}
                                oninput={{
                                    let key_form_name = key_form_name.clone();
                                    Callback::from(move |e: InputEvent| {
                                        key_form_name.set(e.target_unchecked_into::<HtmlInputElement>().value())
                                    })
                                }}
                            />
                        </label>
                        <label class="block text-sm">
                            <span>{ "Status" }</span>
                            <input
                                class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2"
                                value={(*key_form_status).clone()}
                                oninput={{
                                    let key_form_status = key_form_status.clone();
                                    Callback::from(move |e: InputEvent| {
                                        key_form_status.set(e.target_unchecked_into::<HtmlInputElement>().value())
                                    })
                                }}
                            />
                        </label>
                        <label class="block text-sm">
                            <span>{ "Quota Total Calls" }</span>
                            <input
                                class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2"
                                value={(*key_form_quota_total_calls).clone()}
                                oninput={{
                                    let key_form_quota_total_calls = key_form_quota_total_calls.clone();
                                    Callback::from(move |e: InputEvent| {
                                        key_form_quota_total_calls.set(e.target_unchecked_into::<HtmlInputElement>().value())
                                    })
                                }}
                            />
                        </label>
                        <label class="block text-sm">
                            <span>{ "Route" }</span>
                            <select
                                key={format!("route-mode-{}", (*editing_key_id).clone().unwrap_or_default())}
                                class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2"
                                value={(*key_form_route_strategy).clone()}
                                onchange={{
                                    let key_form_route_strategy = key_form_route_strategy.clone();
                                    let key_form_fixed_account_name = key_form_fixed_account_name.clone();
                                    let key_form_account_group_id = key_form_account_group_id.clone();
                                    let accounts = accounts.clone();
                                    Callback::from(move |e: Event| {
                                        let next = e.target_unchecked_into::<HtmlSelectElement>().value();
                                        if next == "account" && (*key_form_fixed_account_name).trim().is_empty() {
                                            if let Some(first_account) = accounts.first() {
                                                key_form_fixed_account_name.set(first_account.name.clone());
                                            }
                                        } else if next != "account" {
                                            key_form_fixed_account_name.set(String::new());
                                        }
                                        if next != "group" {
                                            key_form_account_group_id.set(String::new());
                                        }
                                        key_form_route_strategy.set(next)
                                    })
                                }}
                            >
                                <option value="auto">{ "All accounts" }</option>
                                <option value="group">{ "Account group" }</option>
                                <option value="account">{ "Bind one account" }</option>
                            </select>
                        </label>
                        <label class="block text-sm">
                            <span>{ "Role" }</span>
                            <select
                                key={format!("role-{}", (*editing_key_id).clone().unwrap_or_default())}
                                class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2"
                                value={(*key_form_role).clone()}
                                onchange={{
                                    let key_form_role = key_form_role.clone();
                                    Callback::from(move |e: Event| {
                                        key_form_role.set(e.target_unchecked_into::<HtmlSelectElement>().value())
                                    })
                                }}
                            >
                                <option value="user">{ "User" }</option>
                                <option value="admin">{ "Admin" }</option>
                            </select>
                        </label>
                        if (*key_form_route_strategy).as_str() == "group" {
                            <label class="block text-sm md:col-span-2 xl:col-span-2">
                                <span>{ "Account Group" }</span>
                                <select
                                    key={format!("group-{}", (*editing_key_id).clone().unwrap_or_default())}
                                    class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2"
                                    value={(*key_form_account_group_id).clone()}
                                    onchange={{
                                        let key_form_account_group_id = key_form_account_group_id.clone();
                                        Callback::from(move |e: Event| {
                                            key_form_account_group_id.set(e.target_unchecked_into::<HtmlSelectElement>().value())
                                        })
                                    }}
                                >
                                    <option value="">{ "-- Select group --" }</option>
                                    { for account_groups.iter().map(|group| html! {
                                        <option value={group.id.clone()}>{ format!("{} ({} accounts)", group.name, group.account_names.len()) }</option>
                                    }) }
                                </select>
                            </label>
                        } else if (*key_form_route_strategy).as_str() == "account" {
                            <div class={classes!("md:col-span-2", "xl:col-span-4", "rounded-[var(--radius)]", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                    <div>
                                        <div class={classes!("text-sm", "font-semibold")}>{ "Bound Account" }</div>
                                        <div class={classes!("text-xs", "text-[var(--muted)]")}>
                                            {
                                                if accounts.is_empty() {
                                                    "No imported ChatGPT accounts are available. Import or refresh an account first.".to_string()
                                                } else {
                                                    format!("Choose exactly one upstream account. {} account(s) available.", accounts.len())
                                                }
                                            }
                                        </div>
                                    </div>
                                    <select
                                        key={format!("account-{}", (*editing_key_id).clone().unwrap_or_default())}
                                        class="min-w-[18rem] rounded border border-[var(--border)] bg-[var(--surface)] px-3 py-2 text-sm"
                                        value={(*key_form_fixed_account_name).clone()}
                                        disabled={accounts.is_empty()}
                                        onchange={{
                                            let key_form_fixed_account_name = key_form_fixed_account_name.clone();
                                            Callback::from(move |e: Event| {
                                                key_form_fixed_account_name.set(e.target_unchecked_into::<HtmlSelectElement>().value())
                                            })
                                        }}
                                    >
                                        <option value="">{ "-- Select account --" }</option>
                                        { for accounts.iter().map(|account| html! {
                                            <option value={account.name.clone()}>{ gpt2api_account_option_label(account) }</option>
                                        }) }
                                    </select>
                                </div>
                                if !accounts.is_empty() {
                                    <div class={classes!("mt-3", "grid", "gap-2", "md:grid-cols-2", "2xl:grid-cols-3")}>
                                        { for accounts.iter().map(|account| {
                                            let selected = (*key_form_fixed_account_name) == account.name;
                                            let account_name = account.name.clone();
                                            html! {
                                                <button
                                                    type="button"
                                                    class={classes!(
                                                        "w-full", "rounded-[var(--radius)]", "border", "px-3", "py-3", "text-left", "transition",
                                                        if selected {
                                                            "border-emerald-500 bg-emerald-500/10 ring-2 ring-emerald-500/20"
                                                        } else {
                                                            "border-[var(--border)] bg-[var(--surface)] hover:border-emerald-500/70 hover:bg-emerald-500/5"
                                                        }
                                                    )}
                                                    onclick={{
                                                        let key_form_fixed_account_name = key_form_fixed_account_name.clone();
                                                        Callback::from(move |_| key_form_fixed_account_name.set(account_name.clone()))
                                                    }}
                                                >
                                                    <div class={classes!("flex", "items-center", "justify-between", "gap-3")}>
                                                        <span class={classes!("font-mono", "text-sm", "font-semibold")}>{ account.name.clone() }</span>
                                                        <span class={classes!(
                                                            "rounded-full", "px-2", "py-0.5", "text-xs",
                                                            if account.status == "active" {
                                                                "bg-emerald-500/10 text-emerald-700 dark:text-emerald-200"
                                                            } else {
                                                                "bg-amber-500/10 text-amber-700 dark:text-amber-200"
                                                            }
                                                        )}>{ account.status.clone() }</span>
                                                    </div>
                                                    <div class={classes!("mt-1", "text-xs", "text-[var(--muted)]")}>
                                                        { format!(
                                                            "{} · quota {}{}",
                                                            account.email.clone().unwrap_or_else(|| "no email".to_string()),
                                                            account.quota_remaining,
                                                            if account.quota_known { "" } else { " (unknown)" }
                                                        ) }
                                                    </div>
                                                    <div class={classes!("mt-1", "text-xs", "text-[var(--muted)]")}>
                                                        { format!(
                                                            "proxy: {}",
                                                            account.effective_proxy_config_name
                                                                .clone()
                                                                .or_else(|| account.effective_proxy_url.clone())
                                                                .unwrap_or_else(|| account.effective_proxy_source.clone())
                                                        ) }
                                                    </div>
                                                </button>
                                            }
                                        }) }
                                    </div>
                                }
                            </div>
                        }
                        <label class="block text-sm">
                            <span>{ "Request Max Concurrency" }</span>
                            <input
                                class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2"
                                value={(*key_form_request_max_concurrency).clone()}
                                oninput={{
                                    let key_form_request_max_concurrency =
                                        key_form_request_max_concurrency.clone();
                                    Callback::from(move |e: InputEvent| {
                                        key_form_request_max_concurrency
                                            .set(e.target_unchecked_into::<HtmlInputElement>().value())
                                    })
                                }}
                            />
                        </label>
                        <label class="block text-sm md:col-span-2 xl:col-span-2">
                            <span>{ "Request Min Start Interval Ms" }</span>
                            <input
                                class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2"
                                value={(*key_form_request_min_start_interval_ms).clone()}
                                oninput={{
                                    let key_form_request_min_start_interval_ms =
                                        key_form_request_min_start_interval_ms.clone();
                                    Callback::from(move |e: InputEvent| {
                                        key_form_request_min_start_interval_ms
                                            .set(e.target_unchecked_into::<HtmlInputElement>().value())
                                    })
                                }}
                            />
                        </label>
                    </div>

                    <div class={classes!("mt-4", "flex", "items-center", "gap-2", "flex-wrap")}>
                        <button
                            class={classes!("btn-fluent-primary")}
                            onclick={on_submit_key}
                            disabled={*saving_key}
                        >
                            {
                                if *saving_key {
                                    "Saving..."
                                } else if (*editing_key_id).is_some() {
                                    "Update Key"
                                } else {
                                    "Create Key"
                                }
                            }
                        </button>
                        if let Some(key_id) = (*editing_key_id).clone() {
                            <span class={classes!("text-sm", "text-[var(--muted)]")}>
                                { format!("Editing key {key_id}") }
                            </span>
                        }
                    </div>

                    if let Some(secret) = (*latest_key_secret).clone() {
                        <div class={classes!("mt-4", "rounded-[var(--radius)]", "border", "border-emerald-400/40", "bg-emerald-500/10", "p-4")}>
                            <div class={classes!("text-sm", "font-medium")}>{ "Stored plaintext key (use this for /gpt2api/login)" }</div>
                            <p class={classes!("m-0", "mt-1", "text-xs", "text-[var(--muted)]")}>
                                { "This sk-... value is the real login credential. It is now stored with the key and will stay visible in the inventory below after reload." }
                            </p>
                            <div class={classes!("mt-3")}>
                                <MaskedSecretCode
                                    value={secret}
                                    copy_label={"plaintext key"}
                                    on_copy={on_copy.clone()}
                                />
                            </div>
                        </div>
                    }

                    <div class={classes!("mt-5", "overflow-x-auto")}>
                        <table class={classes!("w-full", "text-sm")}>
                            <thead>
                                <tr class={classes!("text-left", "border-b", "border-[var(--border)]")}>
                                    <th class="py-2 pr-3">{ "Name" }</th>
                                    <th class="py-2 pr-3">{ "Role" }</th>
                                    <th class="py-2 pr-3">{ "Status" }</th>
                                    <th class="py-2 pr-3">{ "Quota" }</th>
                                    <th class="py-2 pr-3">{ "Plaintext Key" }</th>
                                    <th class="py-2 pr-3">{ "Actions" }</th>
                                </tr>
                            </thead>
                            <tbody>
                                { for keys.iter().map(|key| html! {
                                    <tr class={classes!("border-b", "border-[var(--border)]")}>
                                        <td class="py-2 pr-3">
                                            <div class={classes!("font-medium")}>{ key.name.clone() }</div>
                                            <div class={classes!("text-xs", "text-[var(--muted)]")}>
                                                {
                                                    if let Some(account_name) = key.fixed_account_name.as_ref().filter(|value| !value.trim().is_empty()) {
                                                        format!("bound account: {account_name}")
                                                    } else if key.route_strategy == "fixed" {
                                                        key.account_group_id
                                                            .as_ref()
                                                            .map(|id| format!("bound: {}", gpt2api_group_name_for_id(&account_groups, id)))
                                                            .unwrap_or_else(|| "bound: not configured".to_string())
                                                    } else {
                                                        key.account_group_id
                                                            .as_ref()
                                                            .map(|id| format!("auto group: {}", gpt2api_group_name_for_id(&account_groups, id)))
                                                            .unwrap_or_else(|| "auto: all accounts".to_string())
                                                    }
                                                }
                                            </div>
                                        </td>
                                        <td class="py-2 pr-3">
                                            <span class={classes!(
                                                "inline-flex",
                                                "rounded-full",
                                                "px-2.5",
                                                "py-1",
                                                "text-xs",
                                                "font-medium",
                                                match key.role.as_str() {
                                                    "admin" => "bg-indigo-500/10 text-indigo-700 dark:text-indigo-200",
                                                    _ => "bg-slate-500/10 text-slate-700 dark:text-slate-200",
                                                }
                                            )}>
                                                { key.role.clone() }
                                            </span>
                                        </td>
                                        <td class="py-2 pr-3">
                                            <span class={classes!(
                                                "inline-flex",
                                                "rounded-full",
                                                "px-2.5",
                                                "py-1",
                                                "text-xs",
                                                "font-medium",
                                                match key.status.as_str() {
                                                    "active" => "bg-emerald-500/10 text-emerald-700 dark:text-emerald-200",
                                                    "disabled" => "bg-red-500/10 text-red-700 dark:text-red-200",
                                                    _ => "bg-amber-500/10 text-amber-700 dark:text-amber-200",
                                                }
                                            )}>
                                                { key.status.clone() }
                                            </span>
                                        </td>
                                        <td class="py-2 pr-3">{ format!("{}/{}", key.quota_used_calls, key.quota_total_calls) }</td>
                                        <td class="py-2 pr-3">
                                            {
                                                if let Some(secret_plaintext) = key.secret_plaintext.clone() {
                                                    html! {
                                                        <MaskedSecretCode
                                                            value={secret_plaintext}
                                                            copy_label={"plaintext key"}
                                                            on_copy={on_copy.clone()}
                                                        />
                                                    }
                                                } else {
                                                    html! {
                                                        <span class={classes!("text-xs", "text-[var(--muted)]")}>
                                                            { "No stored plaintext yet" }
                                                        </span>
                                                    }
                                                }
                                            }
                                        </td>
                                        <td class="py-2 pr-3">
                                            <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                                                <button
                                                    class={classes!("btn-terminal", "!px-2.5", "!py-1.5", "!text-xs")}
                                                    onclick={{
                                                        let on_edit_key = on_edit_key.clone();
                                                        let key = key.clone();
                                                        Callback::from(move |_| on_edit_key.emit(key.clone()))
                                                    }}
                                                >
                                                    { "Edit" }
                                                </button>
                                                <button
                                                    class={classes!("btn-terminal", "!px-2.5", "!py-1.5", "!text-xs")}
                                                    onclick={{
                                                        let on_rotate_key = on_rotate_key.clone();
                                                        let key = key.clone();
                                                        Callback::from(move |_| on_rotate_key.emit(key.clone()))
                                                    }}
                                                >
                                                    { "Reissue" }
                                                </button>
                                                <button
                                                    class={classes!("btn-terminal", "!px-2.5", "!py-1.5", "!text-xs", "text-red-600")}
                                                    onclick={{
                                                        let on_delete_key = on_delete_key.clone();
                                                        let key = key.clone();
                                                        Callback::from(move |_| on_delete_key.emit(key.clone()))
                                                    }}
                                                >
                                                    { "Delete" }
                                                </button>
                                            </div>
                                        </td>
                                    </tr>
                                }) }
                            </tbody>
                        </table>
                    </div>
                </article>
            </section>
            }

            if active == GPT2API_TAB_CONTRIBUTIONS {
            <section class={classes!("bg-[var(--surface)]", "border", "border-[var(--border)]", "rounded-[var(--radius)]", "shadow-[var(--shadow)]", "p-5", "space-y-4")}>
                <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                    <div>
                        <h2 class={classes!("m-0", "text-lg", "font-semibold")}>{ "GPT Contributions" }</h2>
                        <p class={classes!("m-0", "mt-1", "text-sm", "text-[var(--muted)]")}>
                            { "Review public GPT account submissions from /llm-access. Approval imports the account, creates a fixed-route key, binds the contributor email, and emails /gpt2api/login instructions." }
                        </p>
                    </div>
                    <button
                        class={classes!("btn-fluent-secondary")}
                        onclick={{
                            let reload_contribution_requests = reload_contribution_requests.clone();
                            Callback::from(move |_| reload_contribution_requests.emit((None, None)))
                        }}
                        disabled={*contribution_loading}
                    >
                        { if *contribution_loading { "Refreshing..." } else { "Refresh" } }
                    </button>
                </div>

                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                    <label class={classes!("text-sm")}>
                        <span class={classes!("mr-2", "text-[var(--muted)]")}>{ "Status" }</span>
                        <select
                            class="rounded border border-[var(--border)] bg-transparent px-3 py-2"
                            value={(*contribution_status_filter).clone()}
                            onchange={on_contribution_status_filter_change}
                        >
                            <option value="">{ "All" }</option>
                            <option value="pending">{ "Pending" }</option>
                            <option value="failed">{ "Failed" }</option>
                            <option value="issued">{ "Issued" }</option>
                            <option value="rejected">{ "Rejected" }</option>
                        </select>
                    </label>
                    <div class={classes!("flex", "items-center", "gap-2", "text-sm", "text-[var(--muted)]")}>
                        <span>{ format!("{} requests", *contribution_total) }</span>
                        <button
                            class={classes!("btn-fluent-secondary")}
                            disabled={*contribution_page <= 1}
                            onclick={{
                                let on_contribution_page_change = on_contribution_page_change.clone();
                                let page = (*contribution_page).saturating_sub(1).max(1);
                                Callback::from(move |_| on_contribution_page_change.emit(page))
                            }}
                        >
                            { "Prev" }
                        </button>
                        <span>{ format!("{}/{}", *contribution_page, contribution_total_pages) }</span>
                        <button
                            class={classes!("btn-fluent-secondary")}
                            disabled={*contribution_page >= contribution_total_pages}
                            onclick={{
                                let on_contribution_page_change = on_contribution_page_change.clone();
                                let page = (*contribution_page).saturating_add(1);
                                Callback::from(move |_| on_contribution_page_change.emit(page))
                            }}
                        >
                            { "Next" }
                        </button>
                    </div>
                </div>

                if contribution_requests.is_empty() && !*contribution_loading {
                    <div class={classes!("rounded", "border", "border-dashed", "border-[var(--border)]", "px-4", "py-8", "text-center", "text-[var(--muted)]")}>
                        { "No GPT contribution requests for this filter." }
                    </div>
                } else {
                    <div class={classes!("grid", "gap-4")}>
                        { for contribution_requests.iter().map(|item| {
                            let approving = contribution_action_inflight.contains(&item.request_id);
                            let finalized = item.status == "issued" || item.status == "rejected";
                            let request_id_for_approve = item.request_id.clone();
                            let request_id_for_reject = item.request_id.clone();
                            let credential_type = match (
                                item.access_token.as_ref().filter(|value| !value.trim().is_empty()),
                                item.session_json.as_ref().filter(|value| !value.trim().is_empty()),
                            ) {
                                (Some(_), Some(_)) => "access token + session JSON",
                                (Some(_), None) => "access token",
                                (None, Some(_)) => "session JSON",
                                (None, None) => "none",
                            };
                            html! {
                                <article class={classes!("rounded-[var(--radius)]", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                                    <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                                        <div class={classes!("min-w-0")}>
                                            <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                                                <h3 class={classes!("m-0", "font-mono", "text-base", "font-semibold")}>{ item.account_name.clone() }</h3>
                                                <span class={classes!(
                                                    "rounded-full", "px-2.5", "py-1", "text-xs", "font-semibold",
                                                    match item.status.as_str() {
                                                        "pending" => "bg-amber-500/10 text-amber-700 dark:text-amber-200",
                                                        "issued" => "bg-emerald-500/10 text-emerald-700 dark:text-emerald-200",
                                                        "failed" => "bg-red-500/10 text-red-700 dark:text-red-200",
                                                        "rejected" => "bg-slate-500/10 text-slate-700 dark:text-slate-200",
                                                        _ => "bg-sky-500/10 text-sky-700 dark:text-sky-200",
                                                    }
                                                )}>{ item.status.clone() }</span>
                                            </div>
                                            <div class={classes!("mt-1", "text-xs", "text-[var(--muted)]")}>
                                                { format!("{} · {} · {}", item.request_id, item.requester_email, credential_type) }
                                            </div>
                                        </div>
                                        <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                                            <button
                                                class={classes!("btn-fluent-primary")}
                                                disabled={approving || finalized}
                                                onclick={{
                                                    let on_approve_contribution = on_approve_contribution.clone();
                                                    Callback::from(move |_| on_approve_contribution.emit(request_id_for_approve.clone()))
                                                }}
                                            >
                                                { if approving { "Working..." } else { "Approve & Issue" } }
                                            </button>
                                            <button
                                                class={classes!("btn-fluent-secondary")}
                                                disabled={approving || finalized}
                                                onclick={{
                                                    let on_reject_contribution = on_reject_contribution.clone();
                                                    Callback::from(move |_| on_reject_contribution.emit(request_id_for_reject.clone()))
                                                }}
                                            >
                                                { "Reject" }
                                            </button>
                                        </div>
                                    </div>

                                    <div class={classes!("mt-3", "grid", "gap-3", "lg:grid-cols-3")}>
                                        <div>
                                            <div class={classes!("text-xs", "uppercase", "text-[var(--muted)]")}>{ "Contributor" }</div>
                                            <div class={classes!("mt-1", "text-sm")}>{ item.requester_email.clone() }</div>
                                            <div class={classes!("mt-1", "text-xs", "text-[var(--muted)]")}>
                                                { item.github_id.clone().unwrap_or_else(|| "no GitHub ID".to_string()) }
                                            </div>
                                        </div>
                                        <div>
                                            <div class={classes!("text-xs", "uppercase", "text-[var(--muted)]")}>{ "Review State" }</div>
                                            <div class={classes!("mt-1", "text-sm")}>
                                                { item.imported_account_name.clone().unwrap_or_else(|| "not imported".to_string()) }
                                            </div>
                                            <div class={classes!("mt-1", "text-xs", "text-[var(--muted)]")}>
                                                { item.issued_key_name.clone().unwrap_or_else(|| "no issued key".to_string()) }
                                            </div>
                                        </div>
                                        <div>
                                            <div class={classes!("text-xs", "uppercase", "text-[var(--muted)]")}>{ "Source" }</div>
                                            <div class={classes!("mt-1", "text-sm")}>{ format!("{} · {}", item.ip_region, item.client_ip) }</div>
                                            <div class={classes!("mt-1", "text-xs", "text-[var(--muted)]")}>{ format_ms(item.created_at) }</div>
                                        </div>
                                    </div>

                                    <div class={classes!("mt-3", "rounded", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-3")}>
                                        <div class={classes!("text-xs", "uppercase", "text-[var(--muted)]")}>{ "Message" }</div>
                                        <p class={classes!("m-0", "mt-1", "whitespace-pre-wrap", "text-sm")}>{ item.contributor_message.clone() }</p>
                                    </div>

                                    if let Some(reason) = item.failure_reason.as_ref().filter(|value| !value.trim().is_empty()) {
                                        <div class={classes!("mt-3", "rounded", "border", "border-red-400/35", "bg-red-500/8", "p-3", "text-sm", "text-red-700", "dark:text-red-200")}>
                                            { reason.clone() }
                                        </div>
                                    }

                                    <details class={classes!("mt-3")}>
                                        <summary class={classes!("cursor-pointer", "text-sm", "font-semibold")}>{ "Credentials" }</summary>
                                        <div class={classes!("mt-3", "grid", "gap-3")}>
                                            if let Some(access_token) = item.access_token.clone() {
                                                <div>
                                                    <div class={classes!("mb-1", "text-xs", "uppercase", "text-[var(--muted)]")}>{ "access_token" }</div>
                                                    <MaskedSecretCode value={access_token} copy_label={"GPT access token"} on_copy={on_copy.clone()} />
                                                </div>
                                            }
                                            if let Some(session_json) = item.session_json.clone() {
                                                <div>
                                                    <div class={classes!("mb-1", "text-xs", "uppercase", "text-[var(--muted)]")}>{ "session_json" }</div>
                                                    <MaskedSecretCode value={session_json} copy_label={"GPT session JSON"} on_copy={on_copy.clone()} />
                                                </div>
                                            }
                                        </div>
                                    </details>
                                </article>
                            }
                        }) }
                    </div>
                }
            </section>
            }

            if active == GPT2API_TAB_USAGE {
            <section class={classes!("bg-[var(--surface)]", "border", "border-[var(--border)]", "rounded-[var(--radius)]", "shadow-[var(--shadow)]", "p-5")}>
                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                    <div>
                        <h2 class={classes!("m-0", "text-lg", "font-semibold")}>{ "Usage Logs" }</h2>
                        <p class={classes!("m-0", "mt-1", "text-sm", "text-[var(--muted)]")}>
                            { "Credit totals are aggregated from gpt2api-rs DuckDB usage events, not from mutable key counters." }
                        </p>
                    </div>
                    <div class={classes!("flex", "items-center", "gap-2", "flex-wrap", "text-sm", "text-[var(--muted)]")}>
                        <span class={classes!("rounded-full", "border", "border-[var(--border)]", "px-3", "py-1", "text-xs", "font-semibold")}>
                            { format!("RPM {}", *usage_current_rpm) }
                        </span>
                        <span class={classes!("rounded-full", "border", "border-[var(--border)]", "px-3", "py-1", "text-xs", "font-semibold")}>
                            { format!("In Flight {}", *usage_current_in_flight) }
                        </span>
                        <span>{ format!("{} events", *usage_total) }</span>
                        <span>{ format!("{} credits", *usage_billable_total) }</span>
                    </div>
                </div>

                <div class={classes!("mt-4", "grid", "gap-3", "xl:grid-cols-[minmax(16rem,1fr)_minmax(14rem,20rem)_8rem_auto]", "items-end")}>
                    <label class={classes!("text-sm")}>
                        <span class={classes!("text-[var(--muted)]")}>{ "Search" }</span>
                        <div class={classes!("mt-1")}>
                            <SearchBox
                                value={(*usage_search).clone()}
                                on_change={{
                                    let usage_search = usage_search.clone();
                                    let usage_offset = usage_offset.clone();
                                    Callback::from(move |value: String| {
                                        usage_search.set(value);
                                        usage_offset.set(0);
                                    })
                                }}
                                on_submit={{
                                    let reload_usage_page = reload_usage_page.clone();
                                    Callback::from(move |_| reload_usage_page.emit(0))
                                }}
                                placeholder={AttrValue::Static("Search key, request, endpoint, IP, prompt")}
                            />
                        </div>
                    </label>
                    <label class={classes!("text-sm")}>
                        <span class={classes!("text-[var(--muted)]")}>{ "Key" }</span>
                        <select
                            class={classes!("mt-1", "w-full", "rounded", "border", "border-[var(--border)]", "bg-transparent", "px-3", "py-2")}
                            value={(*usage_key_filter).clone()}
                            onchange={{
                                let usage_key_filter = usage_key_filter.clone();
                                let usage_offset = usage_offset.clone();
                                Callback::from(move |e: Event| {
                                    usage_key_filter.set(e.target_unchecked_into::<HtmlSelectElement>().value());
                                    usage_offset.set(0);
                                })
                            }}
                        >
                            <option value="">{ "All keys" }</option>
                            { for keys.iter().map(|key| html! {
                                <option value={key.id.clone()}>{ format!("{} · {}", key.name, key.id) }</option>
                            }) }
                        </select>
                    </label>
                    <label class={classes!("text-sm")}>
                        <span class={classes!("text-[var(--muted)]")}>{ "Limit" }</span>
                        <input
                            class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2"
                            value={(*usage_limit).clone()}
                            oninput={{
                                let usage_limit = usage_limit.clone();
                                let usage_offset = usage_offset.clone();
                                Callback::from(move |e: InputEvent| {
                                    usage_limit.set(e.target_unchecked_into::<HtmlInputElement>().value());
                                    usage_offset.set(0);
                                })
                            }}
                        />
                    </label>
                    <button
                        class={classes!("btn-fluent-secondary")}
                        onclick={{
                            let reload_usage_page = reload_usage_page.clone();
                            Callback::from(move |_| reload_usage_page.emit(0))
                        }}
                    >
                        { "Reload Logs" }
                    </button>
                </div>

                {
                    if let Some(item) = (*selected_usage_event).clone() {
                        let credits = if item.billable_credits > 0 { item.billable_credits } else { item.billable_images };
                        let headers = item
                            .request_headers_json
                            .as_deref()
                            .map(pretty_json_text)
                            .unwrap_or_else(|| "{}".to_string());
                        let last_message = item
                            .last_message_content
                            .clone()
                            .or_else(|| item.prompt_preview.clone())
                            .unwrap_or_default();
                        let request_body = item
                            .request_body_json
                            .as_deref()
                            .map(pretty_json_text)
                            .unwrap_or_default();
                        html! {
                            <div
                                id="gpt2api-usage-detail"
                                class={classes!(
                                    "fixed", "right-6", "top-20", "z-50", "w-[min(46rem,calc(100vw-2rem))]",
                                    "max-h-[82vh]", "resize", "overflow-auto", "rounded-[var(--radius)]",
                                    "border", "border-[var(--border)]", "bg-[var(--surface)]",
                                    "shadow-2xl"
                                )}
                            >
                                <div
                                    id="gpt2api-usage-detail-handle"
                                    class={classes!("cursor-move", "border-b", "border-[var(--border)]", "px-4", "py-3", "flex", "items-center", "justify-between", "gap-3")}
                                >
                                    <div>
                                        <h3 class={classes!("m-0", "text-base", "font-semibold")}>{ "Usage Event Detail" }</h3>
                                        <p class={classes!("m-0", "mt-1", "font-mono", "text-xs", "text-[var(--muted)]")}>{ item.event_id.clone() }</p>
                                    </div>
                                    <button
                                        class={classes!("btn-terminal", "!px-2.5", "!py-1.5", "!text-xs")}
                                        onclick={{
                                            let selected_usage_event = selected_usage_event.clone();
                                            Callback::from(move |_| selected_usage_event.set(None))
                                        }}
                                    >
                                        { "Close" }
                                    </button>
                                </div>
                                <div class={classes!("grid", "gap-4", "p-4")}>
                                    <div class={classes!("grid", "gap-3", "md:grid-cols-2")}>
                                        <UsageDetailField label="Key" value={format!("{} · {}", item.key_name, item.key_id)} />
                                        <UsageDetailField label="Request" value={format!("{} {}", item.request_method, item.request_url)} />
                                        <UsageDetailField label="Status" value={format!("{}", item.status_code)} />
                                        <UsageDetailField label="Latency" value={format!("{} ms", item.latency_ms)} />
                                        <UsageDetailField label="Mode" value={item.mode.clone()} />
                                        <UsageDetailField label="Size" value={item.image_size.clone().unwrap_or_else(|| "-".to_string())} />
                                        <UsageDetailField label="Credits" value={format!("{credits}")} />
                                        <UsageDetailField label="IP" value={if item.client_ip.is_empty() { "-".to_string() } else { item.client_ip.clone() }} />
                                        <UsageDetailField label="Session" value={item.session_id.clone().unwrap_or_default()} />
                                        <UsageDetailField label="Task" value={item.task_id.clone().unwrap_or_default()} />
                                    </div>
                                    if !last_message.is_empty() {
                                        <section>
                                            <h4 class={classes!("m-0", "mb-2", "text-sm", "font-semibold")}>{ "Last Message / Prompt" }</h4>
                                            <pre class={classes!("max-h-44", "overflow-auto", "rounded", "bg-[var(--surface-alt)]", "p-3", "text-xs", "whitespace-pre-wrap")}>{ last_message }</pre>
                                        </section>
                                    }
                                    <section>
                                        <h4 class={classes!("m-0", "mb-2", "text-sm", "font-semibold")}>{ "Headers" }</h4>
                                        <pre class={classes!("max-h-56", "overflow-auto", "rounded", "bg-[var(--surface-alt)]", "p-3", "text-xs", "whitespace-pre-wrap")}>{ headers }</pre>
                                    </section>
                                    if item.status_code >= 400 {
                                        <section>
                                            <h4 class={classes!("m-0", "mb-2", "text-sm", "font-semibold")}>{ "Error" }</h4>
                                            <pre class={classes!("max-h-44", "overflow-auto", "rounded", "bg-[var(--surface-alt)]", "p-3", "text-xs", "whitespace-pre-wrap")}>
                                                { format!(
                                                    "code: {}\nmessage: {}",
                                                    item.error_code.clone().unwrap_or_default(),
                                                    item.error_message.clone().unwrap_or_default()
                                                ) }
                                            </pre>
                                        </section>
                                    }
                                    if !request_body.is_empty() {
                                        <section>
                                            <h4 class={classes!("m-0", "mb-2", "text-sm", "font-semibold")}>{ "Failed Request Body" }</h4>
                                            <pre class={classes!("max-h-64", "overflow-auto", "rounded", "bg-[var(--surface-alt)]", "p-3", "text-xs", "whitespace-pre-wrap")}>{ request_body }</pre>
                                        </section>
                                    }
                                </div>
                            </div>
                        }
                    } else {
                        Html::default()
                    }
                }

                <div class={classes!("mt-4", "flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                    <span class={classes!("text-sm", "text-[var(--muted)]")}>
                        { format!("offset {}", *usage_offset) }
                    </span>
                    <div class={classes!("flex", "items-center", "gap-2")}>
                        <button
                            class={classes!("btn-fluent-secondary")}
                            disabled={*usage_offset == 0}
                            onclick={{
                                let reload_usage_page = reload_usage_page.clone();
                                let usage_offset = usage_offset.clone();
                                let usage_limit = usage_limit.clone();
                                Callback::from(move |_| {
                                    let limit = (*usage_limit).trim().parse::<u64>().unwrap_or(50).max(1);
                                    reload_usage_page.emit((*usage_offset).saturating_sub(limit));
                                })
                            }}
                        >
                            { "Previous" }
                        </button>
                        <button
                            class={classes!("btn-fluent-secondary")}
                            disabled={!*usage_has_more}
                            onclick={{
                                let reload_usage_page = reload_usage_page.clone();
                                let usage_offset = usage_offset.clone();
                                let usage_limit = usage_limit.clone();
                                Callback::from(move |_| {
                                    let limit = (*usage_limit).trim().parse::<u64>().unwrap_or(50).max(1);
                                    reload_usage_page.emit((*usage_offset).saturating_add(limit));
                                })
                            }}
                        >
                            { "Next" }
                        </button>
                    </div>
                </div>

                <div class={classes!("mt-4", "flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                    <div class={classes!("text-xs", "text-[var(--muted)]")}>
                        { "Columns are wide; use the mirror scrollbar or arrow buttons to inspect logs horizontally." }
                    </div>
                    <div class={classes!("flex", "items-center", "gap-2")}>
                        <button
                            type="button"
                            class={classes!("btn-terminal")}
                            title="Scroll left"
                            aria-label="Scroll left"
                            onclick={{
                                let scroll_usage_table_by = scroll_usage_table_by.clone();
                                Callback::from(move |_| scroll_usage_table_by.emit(-360))
                            }}
                        >
                            <i class={classes!("fas", "fa-arrow-left")} />
                        </button>
                        <button
                            type="button"
                            class={classes!("btn-terminal")}
                            title="Scroll right"
                            aria-label="Scroll right"
                            onclick={{
                                let scroll_usage_table_by = scroll_usage_table_by.clone();
                                Callback::from(move |_| scroll_usage_table_by.emit(360))
                            }}
                        >
                            <i class={classes!("fas", "fa-arrow-right")} />
                        </button>
                    </div>
                </div>

                <div
                    ref={usage_scroll_top_ref}
                    class={classes!("mt-3", "overflow-x-auto", "overflow-y-hidden", "rounded-[var(--radius)]", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-2", "py-2")}
                    onscroll={on_usage_scroll_top}
                >
                    <div
                        class={classes!("h-3", "rounded-full", "bg-[linear-gradient(90deg,rgba(37,99,235,0.18),rgba(16,185,129,0.22))]")}
                        style={format!("width: {}px;", (*usage_scroll_width).max(1))}
                    />
                </div>

                <div
                    ref={usage_scroll_bottom_ref}
                    class={classes!("mt-4", "overflow-x-auto", "rounded-[var(--radius)]", "border", "border-[var(--border)]")}
                    onscroll={on_usage_scroll_bottom}
                >
                    <table class={classes!("min-w-[124rem]", "w-full", "text-sm")}>
                        <thead>
                            <tr class={classes!("text-left", "border-b", "border-[var(--border)]", "text-[var(--muted)]")}>
                                <th class="py-2 pr-3 pl-3">{ "Time" }</th>
                                <th class="py-2 pr-3">{ "Key" }</th>
                                <th class="py-2 pr-3">{ "Endpoint" }</th>
                                <th class="py-2 pr-3">{ "Mode" }</th>
                                <th class="py-2 pr-3">{ "Size" }</th>
                                <th class="py-2 pr-3">{ "Account" }</th>
                                <th class="py-2 pr-3">{ "Images" }</th>
                                <th class="py-2 pr-3">{ "Credits" }</th>
                                <th class="py-2 pr-3">{ "Context" }</th>
                                <th class="py-2 pr-3">{ "Status" }</th>
                                <th class="py-2 pr-3">{ "IP" }</th>
                                <th class="py-2 pr-3">{ "Latency" }</th>
                                <th class="py-2 pr-3">{ "Last Message" }</th>
                                <th class="py-2 pr-3">{ "Request" }</th>
                                <th class="py-2 pr-3">{ "Details" }</th>
                            </tr>
                        </thead>
                        <tbody>
                            if usage.is_empty() {
                                <tr>
                                    <td colspan="15" class="py-8 text-center text-[var(--muted)]">{ "No usage events for this filter" }</td>
                                </tr>
                            } else {
                                { for usage.iter().map(|item| {
                                    let credits = if item.billable_credits > 0 { item.billable_credits } else { item.billable_images };
                                    let status_classes = if item.status_code >= 400 {
                                        classes!("font-semibold", "text-red-600")
                                    } else {
                                        classes!("font-semibold")
                                    };
                                    let size_text = item.image_size.clone().unwrap_or_else(|| "-".to_string());
                                    let last_message = item
                                        .last_message_content
                                        .clone()
                                        .or_else(|| item.prompt_preview.clone())
                                        .unwrap_or_default();
                                    html! {
                                        <tr class={classes!("border-b", "border-[var(--border)]", "align-top")}>
                                            <td class="py-2 pr-3 pl-3 whitespace-nowrap">{ format_ms(item.created_at * 1000) }</td>
                                            <td class="py-2 pr-3 min-w-[14rem]">
                                                <div class={classes!("font-medium")}>{ item.key_name.clone() }</div>
                                                <div class={classes!("font-mono", "text-xs", "text-[var(--muted)]")}>{ item.key_id.clone() }</div>
                                            </td>
                                            <td class="py-2 pr-3 min-w-[14rem]">
                                                <div>{ format!("{} {}", item.request_method, item.request_url) }</div>
                                                <div class={classes!("font-mono", "text-xs", "text-[var(--muted)]")}>{ item.endpoint.clone() }</div>
                                            </td>
                                            <td class="py-2 pr-3">{ item.mode.clone() }</td>
                                            <td class="py-2 pr-3">
                                                <div>{ size_text }</div>
                                                <div class={classes!("text-xs", "text-[var(--muted)]")}>
                                                    { if item.size_credit_units > 0 { format!("{} credit/image", item.size_credit_units) } else { "-".to_string() } }
                                                </div>
                                            </td>
                                            <td class="py-2 pr-3">{ item.account_name.clone() }</td>
                                            <td class="py-2 pr-3">{ format!("{}/{}", item.generated_n, item.requested_n) }</td>
                                            <td class="py-2 pr-3 font-semibold">{ credits }</td>
                                            <td class="py-2 pr-3">
                                                { format!("text {} · image {} · +{}", item.context_text_count, item.context_image_count, item.context_credit_surcharge) }
                                            </td>
                                            <td class="py-2 pr-3">
                                                <div class={status_classes}>{ item.status_code }</div>
                                                <div class={classes!("text-xs", "text-[var(--muted)]")}>{ item.error_code.clone().unwrap_or_default() }</div>
                                            </td>
                                            <td class="py-2 pr-3">{ if item.client_ip.is_empty() { "-".to_string() } else { item.client_ip.clone() } }</td>
                                            <td class="py-2 pr-3">{ format!("{} ms", item.latency_ms) }</td>
                                            <td class="py-2 pr-3 min-w-[20rem]">{ last_message }</td>
                                            <td class="py-2 pr-3 min-w-[16rem]">
                                                <div class={classes!("font-mono", "text-xs")}>{ item.request_id.clone() }</div>
                                                <div class={classes!("font-mono", "text-xs", "text-[var(--muted)]")}>
                                                    { item.session_id.clone().unwrap_or_default() }
                                                </div>
                                                <div class={classes!("font-mono", "text-xs", "text-[var(--muted)]")}>
                                                    { item.task_id.clone().unwrap_or_default() }
                                                </div>
                                            </td>
                                            <td class="py-2 pr-3">
                                                <button
                                                    class={classes!("btn-terminal", "!px-2.5", "!py-1.5", "!text-xs")}
                                                    onclick={{
                                                        let selected_usage_event = selected_usage_event.clone();
                                                        let item = item.clone();
                                                        Callback::from(move |_| selected_usage_event.set(Some(item.clone())))
                                                    }}
                                                >
                                                    { "View" }
                                                </button>
                                            </td>
                                        </tr>
                                    }
                                }) }
                            }
                        </tbody>
                    </table>
                </div>
            </section>
            }

            if active == GPT2API_TAB_ADVANCED {
            <section class={classes!("bg-[var(--surface)]", "border", "border-[var(--border)]", "rounded-[var(--radius)]", "shadow-[var(--shadow)]", "p-5", "space-y-4")}>
                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                    <div>
                        <h2 class={classes!("m-0", "text-lg", "font-semibold")}>{ "Advanced Diagnostics" }</h2>
                        <p class={classes!("m-0", "text-sm", "text-[var(--muted)]")}>{ "Raw service probes and compatibility playgrounds live here so they do not distract from daily admin work." }</p>
                    </div>
                    <button class={classes!("btn-fluent-secondary")} onclick={on_test_login} disabled={*loading}>
                        { "Test Login" }
                    </button>
                </div>
                <div class={classes!("grid", "gap-4", "lg:grid-cols-3")}>
                    <article class={classes!("rounded", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                        <h3 class={classes!("m-0", "text-sm", "font-semibold")}>{ "Version" }</h3>
                        <pre class={classes!("mt-2", "overflow-x-auto", "rounded", "bg-[var(--surface)]", "p-3", "text-xs")}>{ (*version_json).clone() }</pre>
                    </article>
                    <article class={classes!("rounded", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                        <h3 class={classes!("m-0", "text-sm", "font-semibold")}>{ "Models" }</h3>
                        <pre class={classes!("mt-2", "overflow-x-auto", "rounded", "bg-[var(--surface)]", "p-3", "text-xs")}>{ (*models_json).clone() }</pre>
                    </article>
                    <article class={classes!("rounded", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                        <h3 class={classes!("m-0", "text-sm", "font-semibold")}>{ "Login Probe" }</h3>
                        <pre class={classes!("mt-2", "overflow-x-auto", "rounded", "bg-[var(--surface)]", "p-3", "text-xs")}>{ (*login_json).clone() }</pre>
                    </article>
                </div>
            </section>
            }

            if active == GPT2API_TAB_IMAGES {
            <section class={classes!("grid", "gap-5", "lg:grid-cols-2")}>
                <article class={classes!("bg-[var(--surface)]", "border", "border-[var(--border)]", "rounded-[var(--radius)]", "shadow-[var(--shadow)]", "p-5", "space-y-3")}>
                    <h2 class={classes!("m-0", "text-lg", "font-semibold")}>{ "Image Generations" }</h2>
                    <input class="w-full rounded border border-[var(--border)] bg-transparent px-3 py-2" placeholder="Prompt" value={(*generation_prompt).clone()} oninput={{
                        let generation_prompt = generation_prompt.clone();
                        Callback::from(move |e: InputEvent| generation_prompt.set(e.target_unchecked_into::<HtmlInputElement>().value()))
                    }} />
                    <div class={classes!("grid", "gap-3", "sm:grid-cols-3")}>
                        <input class="rounded border border-[var(--border)] bg-transparent px-3 py-2" placeholder="Model" value={(*generation_model).clone()} oninput={{
                            let generation_model = generation_model.clone();
                            Callback::from(move |e: InputEvent| generation_model.set(e.target_unchecked_into::<HtmlInputElement>().value()))
                        }} />
                        <input class="rounded border border-[var(--border)] bg-transparent px-3 py-2" placeholder="n" value={(*generation_n).clone()} oninput={{
                            let generation_n = generation_n.clone();
                            Callback::from(move |e: InputEvent| generation_n.set(e.target_unchecked_into::<HtmlInputElement>().value()))
                        }} />
                        <div class={classes!("grid", "gap-1")}>
                            <input class="rounded border border-[var(--border)] bg-transparent px-3 py-2" placeholder="Size, e.g. 1536x1024" value={(*generation_size).clone()} oninput={{
                            let generation_size = generation_size.clone();
                                Callback::from(move |e: InputEvent| generation_size.set(e.target_unchecked_into::<HtmlInputElement>().value()))
                            }} />
                            <span class={classes!("text-xs", "text-[var(--muted)]")}>{ image_size_credit_hint((*generation_size).as_str()) }</span>
                        </div>
                    </div>
                    <button class={classes!("btn-fluent-primary")} onclick={on_generate_images}>{ "Call /v1/images/generations" }</button>
                    <div class={classes!("grid", "gap-3", "sm:grid-cols-2")}>
                        { for generation_images.iter().map(|url| html! { <img src={url.clone()} class="w-full rounded border border-[var(--border)]" /> }) }
                    </div>
                    <pre class={classes!("overflow-x-auto", "rounded", "bg-[var(--surface-alt)]", "p-3", "text-xs")}>{ (*generation_output).clone() }</pre>
                </article>

                <article class={classes!("bg-[var(--surface)]", "border", "border-[var(--border)]", "rounded-[var(--radius)]", "shadow-[var(--shadow)]", "p-5", "space-y-3")}>
                    <h2 class={classes!("m-0", "text-lg", "font-semibold")}>{ "Image Edits / Reference Style" }</h2>
                    <input class="w-full rounded border border-[var(--border)] bg-transparent px-3 py-2" placeholder="Prompt" value={(*edit_prompt).clone()} oninput={{
                        let edit_prompt = edit_prompt.clone();
                        Callback::from(move |e: InputEvent| edit_prompt.set(e.target_unchecked_into::<HtmlInputElement>().value()))
                    }} />
                    <div class={classes!("grid", "gap-3", "sm:grid-cols-3")}>
                        <input class="rounded border border-[var(--border)] bg-transparent px-3 py-2" placeholder="Model" value={(*edit_model).clone()} oninput={{
                            let edit_model = edit_model.clone();
                            Callback::from(move |e: InputEvent| edit_model.set(e.target_unchecked_into::<HtmlInputElement>().value()))
                        }} />
                        <input class="rounded border border-[var(--border)] bg-transparent px-3 py-2" placeholder="n" value={(*edit_n).clone()} oninput={{
                            let edit_n = edit_n.clone();
                            Callback::from(move |e: InputEvent| edit_n.set(e.target_unchecked_into::<HtmlInputElement>().value()))
                        }} />
                        <div class={classes!("grid", "gap-1")}>
                            <input class="rounded border border-[var(--border)] bg-transparent px-3 py-2" placeholder="Size, e.g. 1024x1536" value={(*edit_size).clone()} oninput={{
                            let edit_size = edit_size.clone();
                                Callback::from(move |e: InputEvent| edit_size.set(e.target_unchecked_into::<HtmlInputElement>().value()))
                            }} />
                            <span class={classes!("text-xs", "text-[var(--muted)]")}>{ image_size_credit_hint((*edit_size).as_str()) }</span>
                        </div>
                    </div>
                    <input type="file" accept="image/*" class="block w-full text-sm" onchange={on_edit_image_file_change} />
                    <p class={classes!("m-0", "text-xs", "text-[var(--muted)]")}>
                        { format!("Selected file: {} ({})", (*edit_file_name), (*edit_mime_type)) }
                    </p>
                    if !(*edit_image_base64).is_empty() {
                        <img src={format!("data:{};base64,{}", (*edit_mime_type), (*edit_image_base64))} class="max-h-64 rounded border border-[var(--border)]" />
                    }
                    <button class={classes!("btn-fluent-primary")} onclick={on_edit_images}>{ "Call /v1/images/edits" }</button>
                    <div class={classes!("grid", "gap-3", "sm:grid-cols-2")}>
                        { for edit_images.iter().map(|url| html! { <img src={url.clone()} class="w-full rounded border border-[var(--border)]" /> }) }
                    </div>
                    <pre class={classes!("overflow-x-auto", "rounded", "bg-[var(--surface-alt)]", "p-3", "text-xs")}>{ (*edit_output).clone() }</pre>
                </article>
            </section>
            }

            if active == GPT2API_TAB_PLAYGROUND {
            <section class={classes!("grid", "gap-5", "lg:grid-cols-2")}>
                <article class={classes!("bg-[var(--surface)]", "border", "border-[var(--border)]", "rounded-[var(--radius)]", "shadow-[var(--shadow)]", "p-5", "space-y-3")}>
                    <h2 class={classes!("m-0", "text-lg", "font-semibold")}>{ "Chat Completions Playground" }</h2>
                    <textarea class="h-80 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2 font-mono text-xs" value={(*chat_request_json).clone()} oninput={{
                        let chat_request_json = chat_request_json.clone();
                        Callback::from(move |e: InputEvent| chat_request_json.set(e.target_unchecked_into::<HtmlTextAreaElement>().value()))
                    }} />
                    <button class={classes!("btn-fluent-primary")} onclick={on_run_chat_completions}>{ "Call /v1/chat/completions" }</button>
                    <pre class={classes!("overflow-x-auto", "rounded", "bg-[var(--surface-alt)]", "p-3", "text-xs")}>{ (*chat_output).clone() }</pre>
                </article>

                <article class={classes!("bg-[var(--surface)]", "border", "border-[var(--border)]", "rounded-[var(--radius)]", "shadow-[var(--shadow)]", "p-5", "space-y-3")}>
                    <h2 class={classes!("m-0", "text-lg", "font-semibold")}>{ "Responses Playground" }</h2>
                    <textarea class="h-80 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2 font-mono text-xs" value={(*responses_request_json).clone()} oninput={{
                        let responses_request_json = responses_request_json.clone();
                        Callback::from(move |e: InputEvent| responses_request_json.set(e.target_unchecked_into::<HtmlTextAreaElement>().value()))
                    }} />
                    <button class={classes!("btn-fluent-primary")} onclick={on_run_responses}>{ "Call /v1/responses" }</button>
                    <pre class={classes!("overflow-x-auto", "rounded", "bg-[var(--surface-alt)]", "p-3", "text-xs")}>{ (*responses_output).clone() }</pre>
                </article>
            </section>
            }
        </main>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::AdminGpt2ApiRsAccountView;

    #[test]
    fn format_account_restore_at_uses_timestamp_when_present() {
        let account = AdminGpt2ApiRsAccountView {
            restore_at: Some("2026-04-24T12:00:00Z".to_string()),
            ..AdminGpt2ApiRsAccountView::default()
        };

        assert_eq!(format_account_restore_at(&account), "2026-04-24T12:00:00Z");
    }

    #[test]
    fn format_account_restore_at_falls_back_for_blank_values() {
        let account = AdminGpt2ApiRsAccountView {
            restore_at: Some("   ".to_string()),
            ..AdminGpt2ApiRsAccountView::default()
        };

        assert_eq!(format_account_restore_at(&account), "-");
    }
}
