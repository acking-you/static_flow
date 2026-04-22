use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{File, HtmlInputElement, HtmlTextAreaElement};
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
        delete_admin_gpt2api_rs_accounts, fetch_admin_gpt2api_rs_accounts,
        fetch_admin_gpt2api_rs_config, fetch_admin_gpt2api_rs_keys, fetch_admin_gpt2api_rs_models,
        fetch_admin_gpt2api_rs_status, fetch_admin_gpt2api_rs_usage,
        fetch_admin_gpt2api_rs_version, import_admin_gpt2api_rs_accounts,
        post_admin_gpt2api_rs_login, refresh_admin_gpt2api_rs_accounts,
        update_admin_gpt2api_rs_account, update_admin_gpt2api_rs_config, AdminGpt2ApiRsAccountView,
        AdminGpt2ApiRsDeleteAccountsRequest, AdminGpt2ApiRsImageEditRequest,
        AdminGpt2ApiRsImageGenerationRequest, AdminGpt2ApiRsImportAccountsRequest,
        AdminGpt2ApiRsKeyView, AdminGpt2ApiRsRefreshAccountsRequest,
        AdminGpt2ApiRsUpdateAccountRequest, AdminGpt2ApiRsUsageEventView, Gpt2ApiRsConfig,
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
    let accounts_search = use_state(String::new);
    let keys = use_state(Vec::<AdminGpt2ApiRsKeyView>::new);
    let usage = use_state(Vec::<AdminGpt2ApiRsUsageEventView>::new);
    let usage_limit = use_state(|| "50".to_string());

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
        let update_request_max_concurrency = update_request_max_concurrency.clone();
        let update_request_min_start_interval_ms = update_request_min_start_interval_ms.clone();
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
            let request_max_concurrency = match (*update_request_max_concurrency).trim() {
                "" => None,
                value => match value.parse::<u64>() {
                    Ok(parsed) => Some(parsed),
                    Err(_) => {
                        load_error
                            .set(Some("request_max_concurrency must be an integer".to_string()));
                        return;
                    },
                },
            };
            let request_min_start_interval_ms = match (*update_request_min_start_interval_ms).trim()
            {
                "" => None,
                value => match value.parse::<u64>() {
                    Ok(parsed) => Some(parsed),
                    Err(_) => {
                        load_error.set(Some(
                            "request_min_start_interval_ms must be an integer".to_string(),
                        ));
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
                    request_max_concurrency,
                    request_min_start_interval_ms,
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
    let filtered_accounts: Vec<AdminGpt2ApiRsAccountView> = use_memo(
        ((*accounts).clone(), accounts_query_lower.clone()),
        |(items, q)| {
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
        },
    )
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
                                <th class="py-2 pr-3">{ "Last Refresh" }</th>
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
                                        <td class="py-2 pr-3">
                                            { account.last_refresh_at.map(|ts| format_ms(ts * 1000)).unwrap_or_else(|| "-".to_string()) }
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
                                                    })}
                                                >
                                                    { "Load To Form" }
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
                <button class={classes!("btn-fluent-primary")} onclick={on_update_account}>{ "Update Selected Account" }</button>
            </section>
            }

            if active == GPT2API_TAB_KEYS {
            <section class={classes!("grid", "gap-5", "lg:grid-cols-2")}>
                <article class={classes!("bg-[var(--surface)]", "border", "border-[var(--border)]", "rounded-[var(--radius)]", "shadow-[var(--shadow)]", "p-5")}>
                    <h2 class={classes!("m-0", "text-lg", "font-semibold")}>{ "API Keys" }</h2>
                    <div class={classes!("mt-3", "overflow-x-auto")}>
                        <table class={classes!("w-full", "text-sm")}>
                            <thead>
                                <tr class={classes!("text-left", "border-b", "border-[var(--border)]")}>
                                    <th class="py-2 pr-3">{ "Name" }</th>
                                    <th class="py-2 pr-3">{ "Secret Hash" }</th>
                                    <th class="py-2 pr-3">{ "Quota" }</th>
                                    <th class="py-2 pr-3">{ "Route" }</th>
                                </tr>
                            </thead>
                            <tbody>
                                { for keys.iter().map(|key| html! {
                                    <tr class={classes!("border-b", "border-[var(--border)]")}>
                                        <td class="py-2 pr-3">{ key.name.clone() }</td>
                                        <td class="py-2 pr-3">
                                            <MaskedSecretCode
                                                value={key.secret_hash.clone()}
                                                copy_label={"secret hash"}
                                                on_copy={on_copy.clone()}
                                            />
                                        </td>
                                        <td class="py-2 pr-3">{ format!("{}/{}", key.quota_used_images, key.quota_total_images) }</td>
                                        <td class="py-2 pr-3">{ key.route_strategy.clone() }</td>
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
