use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use wasm_bindgen::{prelude::*, JsCast};
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{File, HtmlInputElement, HtmlSelectElement, HtmlTextAreaElement};
use yew::prelude::*;
use yew_router::prelude::Link;

#[wasm_bindgen(inline_js = r#"
export function gpt2api_copy_text(text) {
    if (navigator.clipboard) {
        navigator.clipboard.writeText(text).catch(function(){});
    }
}
"#)]
extern "C" {
    fn gpt2api_copy_text(text: &str);
}

use crate::{
    api::{
        admin_gpt2api_rs_chat_completions, admin_gpt2api_rs_edit_images,
        admin_gpt2api_rs_generate_images, admin_gpt2api_rs_responses,
        check_admin_gpt2api_rs_proxy_config, create_admin_gpt2api_rs_key,
        create_admin_gpt2api_rs_proxy_config, delete_admin_gpt2api_rs_accounts,
        delete_admin_gpt2api_rs_key, delete_admin_gpt2api_rs_proxy_config,
        fetch_admin_gpt2api_rs_accounts, fetch_admin_gpt2api_rs_config,
        fetch_admin_gpt2api_rs_keys, fetch_admin_gpt2api_rs_models,
        fetch_admin_gpt2api_rs_proxy_configs, fetch_admin_gpt2api_rs_status,
        fetch_admin_gpt2api_rs_usage, fetch_admin_gpt2api_rs_version,
        import_admin_gpt2api_rs_accounts, post_admin_gpt2api_rs_login,
        refresh_admin_gpt2api_rs_accounts, rotate_admin_gpt2api_rs_key,
        update_admin_gpt2api_rs_account, update_admin_gpt2api_rs_config,
        update_admin_gpt2api_rs_key, update_admin_gpt2api_rs_proxy_config,
        AdminGpt2ApiRsAccountView, AdminGpt2ApiRsCreateKeyRequest,
        AdminGpt2ApiRsCreateProxyConfigRequest, AdminGpt2ApiRsDeleteAccountsRequest,
        AdminGpt2ApiRsImageEditRequest, AdminGpt2ApiRsImageGenerationRequest,
        AdminGpt2ApiRsImportAccountsRequest, AdminGpt2ApiRsKeyView, AdminGpt2ApiRsProxyCheckResult,
        AdminGpt2ApiRsProxyConfigView, AdminGpt2ApiRsRefreshAccountsRequest,
        AdminGpt2ApiRsUpdateAccountRequest, AdminGpt2ApiRsUpdateKeyRequest,
        AdminGpt2ApiRsUpdateProxyConfigRequest, AdminGpt2ApiRsUsageEventView, Gpt2ApiRsConfig,
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

// Tabs on the gpt2api-rs admin page. Using &'static str to slot straight into
// the shared `render_tab_bar` helper without boxing.
const GPT2API_TAB_OVERVIEW: &str = "overview";
const GPT2API_TAB_ACCOUNTS: &str = "accounts";
const GPT2API_TAB_KEYS: &str = "keys";
const GPT2API_TAB_IMAGES: &str = "images";
const GPT2API_TAB_PLAYGROUND: &str = "playground";

fn pretty_json(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn parse_json_text(raw: &str) -> Result<serde_json::Value, String> {
    serde_json::from_str(raw).map_err(|err| format!("JSON parse error: {err}"))
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
    let accounts_search = use_state(String::new);
    let keys = use_state(Vec::<AdminGpt2ApiRsKeyView>::new);
    let usage = use_state(Vec::<AdminGpt2ApiRsUsageEventView>::new);
    let usage_limit = use_state(|| "50".to_string());
    let editing_key_id = use_state(|| None::<String>);
    let key_form_name = use_state(String::new);
    let key_form_status = use_state(|| "active".to_string());
    let key_form_quota_total_calls = use_state(|| "100".to_string());
    let key_form_route_strategy = use_state(|| "auto".to_string());
    let key_form_account_group_id = use_state(String::new);
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

    let editing_proxy_id = use_state(|| None::<String>);
    let proxy_form_name = use_state(String::new);
    let proxy_form_url = use_state(|| "http://127.0.0.1:11118".to_string());
    let proxy_form_username = use_state(String::new);
    let proxy_form_password = use_state(String::new);
    let proxy_form_status = use_state(|| "active".to_string());
    let saving_proxy = use_state(|| false);
    let checking_proxy = use_state(|| false);

    let generation_prompt = use_state(String::new);
    let generation_model = use_state(|| "gpt-image-1".to_string());
    let generation_n = use_state(|| "1".to_string());
    let generation_output = use_state(|| "{}".to_string());
    let generation_images = use_state(Vec::<String>::new);

    let edit_prompt = use_state(String::new);
    let edit_model = use_state(|| "gpt-image-1".to_string());
    let edit_n = use_state(|| "1".to_string());
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
        let keys = keys.clone();
        let usage = usage.clone();
        let usage_limit = usage_limit.clone();
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
            let keys = keys.clone();
            let usage = usage.clone();
            let usage_limit = usage_limit.clone();
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
                match fetch_admin_gpt2api_rs_keys().await {
                    Ok(value) => keys.set(value),
                    Err(err) => load_error.set(Some(err)),
                }
                let limit = (*usage_limit).trim().parse::<u64>().unwrap_or(50).max(1);
                match fetch_admin_gpt2api_rs_usage(limit).await {
                    Ok(value) => usage.set(value),
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

    let reset_key_form = {
        let editing_key_id = editing_key_id.clone();
        let key_form_name = key_form_name.clone();
        let key_form_status = key_form_status.clone();
        let key_form_quota_total_calls = key_form_quota_total_calls.clone();
        let key_form_route_strategy = key_form_route_strategy.clone();
        let key_form_account_group_id = key_form_account_group_id.clone();
        let key_form_request_max_concurrency = key_form_request_max_concurrency.clone();
        let key_form_request_min_start_interval_ms = key_form_request_min_start_interval_ms.clone();
        let latest_key_secret = latest_key_secret.clone();
        Callback::from(move |_| {
            editing_key_id.set(None);
            key_form_name.set(String::new());
            key_form_status.set("active".to_string());
            key_form_quota_total_calls.set("100".to_string());
            key_form_route_strategy.set("auto".to_string());
            key_form_account_group_id.set(String::new());
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
        let key_form_account_group_id = key_form_account_group_id.clone();
        let key_form_request_max_concurrency = key_form_request_max_concurrency.clone();
        let key_form_request_min_start_interval_ms = key_form_request_min_start_interval_ms.clone();
        let latest_key_secret = latest_key_secret.clone();
        Callback::from(move |key: AdminGpt2ApiRsKeyView| {
            editing_key_id.set(Some(key.id));
            key_form_name.set(key.name);
            key_form_status.set(key.status);
            key_form_quota_total_calls.set(key.quota_total_calls.to_string());
            key_form_route_strategy.set(key.route_strategy);
            key_form_account_group_id.set(key.account_group_id.unwrap_or_default());
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
        let key_form_account_group_id = key_form_account_group_id.clone();
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
            let route_strategy = (*key_form_route_strategy).trim().to_string();
            if route_strategy.is_empty() {
                load_error.set(Some("route_strategy is required".to_string()));
                return;
            }
            let account_group_id = (!(*key_form_account_group_id).trim().is_empty())
                .then(|| (*key_form_account_group_id).trim().to_string());
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
            let key_form_account_group_id = key_form_account_group_id.clone();
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
                        account_group_id: account_group_id.clone(),
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
                        account_group_id: account_group_id.clone(),
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
                        key_form_route_strategy.set(key.route_strategy.clone());
                        key_form_account_group_id
                            .set(key.account_group_id.clone().unwrap_or_default());
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

    // Tab wiring. Pure UI switch — all data is still reloaded together by
    // `reload_all`, so switching tabs does not trigger additional network.
    let on_tab_select = {
        let active_tab = active_tab.clone();
        Callback::from(move |id: String| active_tab.set(id))
    };
    let tabs: [(&str, &str); 5] = [
        (GPT2API_TAB_OVERVIEW, "Overview"),
        (GPT2API_TAB_ACCOUNTS, "Accounts"),
        (GPT2API_TAB_KEYS, "Keys & Usage"),
        (GPT2API_TAB_IMAGES, "Image Gen"),
        (GPT2API_TAB_PLAYGROUND, "Playground"),
    ];
    let active = (*active_tab).clone();

    html! {
        <main class={classes!("container", "py-8", "space-y-5")}>
            <section class={classes!("bg-[var(--surface)]", "border", "border-[var(--border)]", "rounded-[var(--radius)]", "shadow-[var(--shadow)]", "p-5")}>
                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                    <div>
                        <h1 class={classes!("m-0", "text-xl", "font-semibold")}>{ "gpt2api-rs Admin" }</h1>
                        <p class={classes!("m-0", "text-sm", "text-[var(--muted)]")}>
                            { "Manage the deployed gpt2api-rs service, sync config, operate accounts, inspect usage, and run image-generation playground calls through StaticFlow admin." }
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
                        <button class={classes!("btn-fluent-secondary")} onclick={on_test_login} disabled={*loading}>
                            { "Test Login" }
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

            <section class={classes!("grid", "gap-5", "lg:grid-cols-2")}>
                <article class={classes!("bg-[var(--surface)]", "border", "border-[var(--border)]", "rounded-[var(--radius)]", "shadow-[var(--shadow)]", "p-5")}>
                    <h2 class={classes!("m-0", "text-lg", "font-semibold")}>{ "Service Snapshot" }</h2>
                    <pre class={classes!("mt-3", "overflow-x-auto", "rounded", "bg-[var(--surface-alt)]", "p-3", "text-xs")}>{ (*status_json).clone() }</pre>
                </article>
                <article class={classes!("bg-[var(--surface)]", "border", "border-[var(--border)]", "rounded-[var(--radius)]", "shadow-[var(--shadow)]", "p-5")}>
                    <h2 class={classes!("m-0", "text-lg", "font-semibold")}>{ "Version / Models / Login" }</h2>
                    <h3 class={classes!("mt-3", "mb-1", "text-sm", "font-semibold")}>{ "Version" }</h3>
                    <pre class={classes!("overflow-x-auto", "rounded", "bg-[var(--surface-alt)]", "p-3", "text-xs")}>{ (*version_json).clone() }</pre>
                    <h3 class={classes!("mt-3", "mb-1", "text-sm", "font-semibold")}>{ "Models" }</h3>
                    <pre class={classes!("overflow-x-auto", "rounded", "bg-[var(--surface-alt)]", "p-3", "text-xs")}>{ (*models_json).clone() }</pre>
                    <h3 class={classes!("mt-3", "mb-1", "text-sm", "font-semibold")}>{ "Login" }</h3>
                    <pre class={classes!("overflow-x-auto", "rounded", "bg-[var(--surface-alt)]", "p-3", "text-xs")}>{ (*login_json).clone() }</pre>
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

                <div class={classes!("rounded-[var(--radius)]", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4", "space-y-4")}>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <div>
                            <h3 class={classes!("m-0", "text-base", "font-semibold")}>{ "Proxy Configs" }</h3>
                            <p class={classes!("m-0", "mt-1", "text-sm", "text-[var(--muted)]")}>
                                { "Reusable per-account upstream proxy configs. New configs default to http://127.0.0.1:11118 for this rollout." }
                            </p>
                        </div>
                        <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                            if let Some(proxy_id) = (*editing_proxy_id).clone() {
                                <span class={classes!("text-xs", "font-mono", "text-[var(--muted)]")}>
                                    { format!("Editing {}", proxy_id) }
                                </span>
                            }
                            <button
                                class={classes!("btn-fluent-secondary")}
                                onclick={{
                                    let reset_proxy_form = reset_proxy_form.clone();
                                    Callback::from(move |_| reset_proxy_form.emit(()))
                                }}
                            >
                                { if (*editing_proxy_id).is_some() { "New Proxy Config" } else { "Reset Form" } }
                            </button>
                        </div>
                    </div>

                    <div class={classes!("grid", "gap-4", "lg:grid-cols-[minmax(0,24rem)_minmax(0,1fr)]")}>
                        <div class={classes!("space-y-3")}>
                            <label class="block text-sm">
                                <span>{ "Name" }</span>
                                <input
                                    class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2"
                                    value={(*proxy_form_name).clone()}
                                    oninput={{
                                        let proxy_form_name = proxy_form_name.clone();
                                        Callback::from(move |e: InputEvent| {
                                            proxy_form_name.set(e.target_unchecked_into::<HtmlInputElement>().value())
                                        })
                                    }}
                                />
                            </label>
                            <label class="block text-sm">
                                <span>{ "Proxy URL" }</span>
                                <input
                                    class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2 font-mono"
                                    value={(*proxy_form_url).clone()}
                                    oninput={{
                                        let proxy_form_url = proxy_form_url.clone();
                                        Callback::from(move |e: InputEvent| {
                                            proxy_form_url.set(e.target_unchecked_into::<HtmlInputElement>().value())
                                        })
                                    }}
                                />
                            </label>
                            <div class={classes!("grid", "gap-3", "md:grid-cols-2")}>
                                <label class="block text-sm">
                                    <span>{ "Proxy Username" }</span>
                                    <input
                                        class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2"
                                        value={(*proxy_form_username).clone()}
                                        oninput={{
                                            let proxy_form_username = proxy_form_username.clone();
                                            Callback::from(move |e: InputEvent| {
                                                proxy_form_username.set(e.target_unchecked_into::<HtmlInputElement>().value())
                                            })
                                        }}
                                    />
                                </label>
                                <label class="block text-sm">
                                    <span>{ "Proxy Password" }</span>
                                    <input
                                        class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2"
                                        value={(*proxy_form_password).clone()}
                                        oninput={{
                                            let proxy_form_password = proxy_form_password.clone();
                                            Callback::from(move |e: InputEvent| {
                                                proxy_form_password.set(e.target_unchecked_into::<HtmlInputElement>().value())
                                            })
                                        }}
                                    />
                                </label>
                            </div>
                            <label class="block text-sm">
                                <span>{ "Status" }</span>
                                <select
                                    class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2"
                                    value={(*proxy_form_status).clone()}
                                    onchange={{
                                        let proxy_form_status = proxy_form_status.clone();
                                        Callback::from(move |e: Event| {
                                            proxy_form_status.set(e.target_unchecked_into::<HtmlSelectElement>().value())
                                        })
                                    }}
                                >
                                    <option value="active">{ "active" }</option>
                                    <option value="disabled">{ "disabled" }</option>
                                </select>
                            </label>
                            <div class={classes!("flex", "items-center", "gap-3", "flex-wrap")}>
                                <button
                                    class={classes!("btn-fluent-primary")}
                                    onclick={on_submit_proxy_config}
                                    disabled={*saving_proxy}
                                >
                                    {
                                        if *saving_proxy {
                                            "Saving..."
                                        } else if (*editing_proxy_id).is_some() {
                                            "Update Proxy Config"
                                        } else {
                                            "Create Proxy Config"
                                        }
                                    }
                                </button>
                                <span class={classes!("text-xs", "text-[var(--muted)]")}>
                                    { "账号可绑定 inherit / direct / fixed 三种代理模式。" }
                                </span>
                            </div>
                        </div>

                        <div class={classes!("overflow-x-auto")}>
                            <table class={classes!("w-full", "text-sm")}>
                                <thead>
                                    <tr class={classes!("text-left", "border-b", "border-[var(--border)]")}>
                                        <th class="py-2 pr-3">{ "Name" }</th>
                                        <th class="py-2 pr-3">{ "Proxy URL" }</th>
                                        <th class="py-2 pr-3">{ "Status" }</th>
                                        <th class="py-2 pr-3">{ "Actions" }</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    { for proxy_configs.iter().map(|proxy_config| {
                                        let proxy_for_edit = proxy_config.clone();
                                        let proxy_for_check = proxy_config.clone();
                                        let proxy_for_delete = proxy_config.clone();
                                        html! {
                                            <tr class={classes!("border-b", "border-[var(--border)]", "align-top")}>
                                                <td class="py-2 pr-3">
                                                    <div class={classes!("font-medium")}>{ proxy_config.name.clone() }</div>
                                                    <div class={classes!("mt-1", "text-xs", "text-[var(--muted)]")}>
                                                        { format!(
                                                            "created {} · updated {}",
                                                            format_ms(proxy_config.created_at * 1000),
                                                            format_ms(proxy_config.updated_at * 1000),
                                                        ) }
                                                    </div>
                                                </td>
                                                <td class="py-2 pr-3">
                                                    <div class={classes!("font-mono", "text-xs", "break-all")}>
                                                        { proxy_config.proxy_url.clone() }
                                                    </div>
                                                    if let Some(username) = proxy_config.proxy_username.clone() {
                                                        <div class={classes!("mt-1", "text-xs", "text-[var(--muted)]")}>
                                                            { format!("user={username}") }
                                                        </div>
                                                    }
                                                </td>
                                                <td class="py-2 pr-3">{ proxy_config.status.clone() }</td>
                                                <td class="py-2 pr-3">
                                                    <div class={classes!("flex", "gap-2", "flex-wrap")}>
                                                        <button
                                                            class={classes!("btn-fluent-secondary")}
                                                            onclick={{
                                                                let on_edit_proxy_config = on_edit_proxy_config.clone();
                                                                Callback::from(move |_| on_edit_proxy_config.emit(proxy_for_edit.clone()))
                                                            }}
                                                        >
                                                            { "Edit" }
                                                        </button>
                                                        <button
                                                            class={classes!("btn-fluent-secondary")}
                                                            onclick={{
                                                                let on_check_proxy_config = on_check_proxy_config.clone();
                                                                Callback::from(move |_| on_check_proxy_config.emit(proxy_for_check.clone()))
                                                            }}
                                                            disabled={*checking_proxy}
                                                        >
                                                            { if *checking_proxy { "Checking..." } else { "Check" } }
                                                        </button>
                                                        <button
                                                            class={classes!("btn-fluent-secondary")}
                                                            onclick={{
                                                                let on_delete_proxy_config = on_delete_proxy_config.clone();
                                                                Callback::from(move |_| on_delete_proxy_config.emit(proxy_for_delete.clone()))
                                                            }}
                                                        >
                                                            { "Delete" }
                                                        </button>
                                                    </div>
                                                </td>
                                            </tr>
                                        }
                                    }) }
                                </tbody>
                            </table>
                            if proxy_configs.is_empty() {
                                <p class={classes!("m-0", "mt-3", "text-sm", "text-[var(--muted)]")}>
                                    { "No proxy configs yet." }
                                </p>
                            }
                        </div>
                    </div>
                </div>

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
                                            <div class={classes!("text-xs", "font-mono")}>
                                                { format_account_proxy_binding(account) }
                                            </div>
                                            <div class={classes!("mt-1", "text-xs", "text-[var(--muted)]", "break-all")}>
                                                { format_account_effective_proxy(account) }
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
                                                    { "Load Account" }
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

                <div class={classes!("rounded-[var(--radius)]", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4", "space-y-4")}>
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
            </section>
            }

            if active == GPT2API_TAB_KEYS {
            <section class={classes!("grid", "gap-5", "lg:grid-cols-2")}>
                <article class={classes!("bg-[var(--surface)]", "border", "border-[var(--border)]", "rounded-[var(--radius)]", "shadow-[var(--shadow)]", "p-5")}>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <div>
                            <h2 class={classes!("m-0", "text-lg", "font-semibold")}>{ "API Keys" }</h2>
                            <p class={classes!("m-0", "mt-1", "text-sm", "text-[var(--muted)]")}>
                                { "Create, reissue, disable, or delete public keys. The plaintext sk-... secret is stored directly and can be copied from the inventory below whenever you need to log in." }
                            </p>
                        </div>
                        <button class={classes!("btn-fluent-secondary")} onclick={{
                            let reset_key_form = reset_key_form.clone();
                            Callback::from(move |_| reset_key_form.emit(()))
                        }}>
                            { if (*editing_key_id).is_some() { "New Key" } else { "Reset Form" } }
                        </button>
                    </div>

                    <div class={classes!("mt-4", "grid", "gap-3", "sm:grid-cols-2")}>
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
                            <span>{ "Route Strategy" }</span>
                            <input
                                class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2"
                                value={(*key_form_route_strategy).clone()}
                                oninput={{
                                    let key_form_route_strategy = key_form_route_strategy.clone();
                                    Callback::from(move |e: InputEvent| {
                                        key_form_route_strategy.set(e.target_unchecked_into::<HtmlInputElement>().value())
                                    })
                                }}
                            />
                        </label>
                        <label class="block text-sm">
                            <span>{ "Account Group ID" }</span>
                            <input
                                class="mt-1 w-full rounded border border-[var(--border)] bg-transparent px-3 py-2"
                                value={(*key_form_account_group_id).clone()}
                                oninput={{
                                    let key_form_account_group_id = key_form_account_group_id.clone();
                                    Callback::from(move |e: InputEvent| {
                                        key_form_account_group_id.set(e.target_unchecked_into::<HtmlInputElement>().value())
                                    })
                                }}
                            />
                        </label>
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
                        <label class="block text-sm sm:col-span-2">
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
                                                { format!("route={}{}", key.route_strategy, key.account_group_id.as_ref().map(|id| format!(" · group={id}")).unwrap_or_default()) }
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

                <article class={classes!("bg-[var(--surface)]", "border", "border-[var(--border)]", "rounded-[var(--radius)]", "shadow-[var(--shadow)]", "p-5")}>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <h2 class={classes!("m-0", "text-lg", "font-semibold")}>{ "Usage" }</h2>
                        <div class={classes!("flex", "items-center", "gap-2")}>
                            <input class="rounded border border-[var(--border)] bg-transparent px-3 py-2 text-sm" value={(*usage_limit).clone()} oninput={{
                                let usage_limit = usage_limit.clone();
                                Callback::from(move |e: InputEvent| usage_limit.set(e.target_unchecked_into::<HtmlInputElement>().value()))
                            }} />
                            <button class={classes!("btn-fluent-secondary")} onclick={{
                                let reload_all = reload_all.clone();
                                Callback::from(move |_| reload_all.emit(()))
                            }}>{ "Reload Usage" }</button>
                        </div>
                    </div>
                    <div class={classes!("mt-3", "max-h-[28rem]", "overflow-auto")}>
                        <table class={classes!("w-full", "text-sm")}>
                            <thead>
                                <tr class={classes!("text-left", "border-b", "border-[var(--border)]")}>
                                    <th class="py-2 pr-3">{ "Time" }</th>
                                    <th class="py-2 pr-3">{ "Endpoint" }</th>
                                    <th class="py-2 pr-3">{ "Account" }</th>
                                    <th class="py-2 pr-3">{ "Status" }</th>
                                    <th class="py-2 pr-3">{ "Latency" }</th>
                                </tr>
                            </thead>
                            <tbody>
                                { for usage.iter().map(|item| html! {
                                    <tr class={classes!("border-b", "border-[var(--border)]", "align-top")}>
                                        <td class="py-2 pr-3">{ format_ms(item.created_at * 1000) }</td>
                                        <td class="py-2 pr-3">{ item.endpoint.clone() }</td>
                                        <td class="py-2 pr-3">{ item.account_name.clone() }</td>
                                        <td class="py-2 pr-3">{ item.status_code }</td>
                                        <td class="py-2 pr-3">{ format!("{} ms", item.latency_ms) }</td>
                                    </tr>
                                }) }
                            </tbody>
                        </table>
                    </div>
                </article>
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
                    <div class={classes!("grid", "gap-3", "sm:grid-cols-2")}>
                        <input class="rounded border border-[var(--border)] bg-transparent px-3 py-2" placeholder="Model" value={(*generation_model).clone()} oninput={{
                            let generation_model = generation_model.clone();
                            Callback::from(move |e: InputEvent| generation_model.set(e.target_unchecked_into::<HtmlInputElement>().value()))
                        }} />
                        <input class="rounded border border-[var(--border)] bg-transparent px-3 py-2" placeholder="n" value={(*generation_n).clone()} oninput={{
                            let generation_n = generation_n.clone();
                            Callback::from(move |e: InputEvent| generation_n.set(e.target_unchecked_into::<HtmlInputElement>().value()))
                        }} />
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
                    <div class={classes!("grid", "gap-3", "sm:grid-cols-2")}>
                        <input class="rounded border border-[var(--border)] bg-transparent px-3 py-2" placeholder="Model" value={(*edit_model).clone()} oninput={{
                            let edit_model = edit_model.clone();
                            Callback::from(move |e: InputEvent| edit_model.set(e.target_unchecked_into::<HtmlInputElement>().value()))
                        }} />
                        <input class="rounded border border-[var(--border)] bg-transparent px-3 py-2" placeholder="n" value={(*edit_n).clone()} oninput={{
                            let edit_n = edit_n.clone();
                            Callback::from(move |e: InputEvent| edit_n.set(e.target_unchecked_into::<HtmlInputElement>().value()))
                        }} />
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
