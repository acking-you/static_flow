use js_sys::Date;
use serde::Deserialize;
use yew::prelude::*;

use crate::api::{LlmGatewayAccessResponse, LlmGatewayPublicKeyView};

pub const REMOTE_COMPACT_ARTICLE_ID: &str = "codex-compact-local-and-remote-deep-dive";

fn masked_secret_value(value: &str) -> String {
    let len = value.chars().count().clamp(12, 32);
    "•".repeat(len)
}

#[derive(Properties, PartialEq)]
pub struct MaskedSecretCodeProps {
    pub value: String,
    pub copy_label: AttrValue,
    pub on_copy: Callback<(String, String)>,
    #[prop_or_default]
    pub code_class: Classes,
}

#[function_component(MaskedSecretCode)]
pub fn masked_secret_code(props: &MaskedSecretCodeProps) -> Html {
    let revealed = use_state(|| false);
    let value = props.value.clone();
    let visible_value = if *revealed { value.clone() } else { masked_secret_value(&value) };

    html! {
        <div class={classes!("flex", "items-start", "justify-between", "gap-3")}>
            <code class={classes!("min-w-0", "flex-1", "break-all", "font-mono", "text-xs", props.code_class.clone())}>
                { visible_value }
            </code>
            <div class={classes!("flex", "items-center", "gap-2", "shrink-0")}>
                <button
                    type="button"
                    class={classes!("btn-terminal", "!px-2.5", "!py-1.5", "!text-xs")}
                    title={if *revealed { "隐藏" } else { "显示" }}
                    aria-label={if *revealed { "隐藏" } else { "显示" }}
                    onclick={{
                        let revealed = revealed.clone();
                        Callback::from(move |_| revealed.set(!*revealed))
                    }}
                >
                    <i class={classes!("fas", if *revealed { "fa-eye-slash" } else { "fa-eye" })}></i>
                </button>
                <button
                    type="button"
                    class={classes!("btn-terminal", "btn-terminal-primary", "!px-2.5", "!py-1.5", "!text-xs")}
                    title="复制"
                    aria-label="复制"
                    onclick={{
                        let on_copy = props.on_copy.clone();
                        let copy_label = props.copy_label.to_string();
                        let value = value.clone();
                        Callback::from(move |_| on_copy.emit((copy_label.clone(), value.clone())))
                    }}
                >
                    <i class={classes!("fas", "fa-copy")}></i>
                </button>
            </div>
        </div>
    }
}

pub fn format_ms(ts_ms: i64) -> String {
    let d = Date::new(&wasm_bindgen::JsValue::from_f64(ts_ms as f64));
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        d.get_full_year(),
        d.get_month() + 1,
        d.get_date(),
        d.get_hours(),
        d.get_minutes(),
        d.get_seconds(),
    )
}

/// Ask the user for confirmation via the browser's native dialog.
/// Returns false if the user cancels or the dialog can't open (SSR/no window).
/// Used for destructive actions (delete / purge / revoke) to standardize UX
/// across admin pages.
pub fn confirm_destructive(message: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.confirm_with_message(message).ok())
        .unwrap_or(false)
}

/// ISO-style UTC timestamp `"YYYY-MM-DD HH:MM:SS.sss"` (T replaced with space,
/// trailing Z trimmed). Used by the admin SSE stream pages where sub-second
/// precision is useful for debugging chunk ordering.
pub fn format_ms_iso(ts_ms: i64) -> String {
    Date::new(&wasm_bindgen::JsValue::from_f64(ts_ms as f64))
        .to_iso_string()
        .as_string()
        .unwrap_or_else(|| ts_ms.to_string())
        .replace('T', " ")
        .trim_end_matches('Z')
        .to_string()
}

pub fn usage_ratio(key: &LlmGatewayPublicKeyView) -> f64 {
    if key.quota_billable_limit == 0 {
        0.0
    } else {
        let used = (key.quota_billable_limit as i64 - key.remaining_billable).max(0) as f64;
        (used / key.quota_billable_limit as f64).clamp(0.0, 1.0)
    }
}

pub fn format_percent(value: f64) -> String {
    format!("{:.0}%", value.clamp(0.0, 100.0))
}

pub fn format_window_label(window_duration_mins: Option<i64>, fallback: &str) -> String {
    const MINUTES_PER_HOUR: i64 = 60;
    const MINUTES_PER_DAY: i64 = 24 * MINUTES_PER_HOUR;
    const MINUTES_PER_WEEK: i64 = 7 * MINUTES_PER_DAY;
    const MINUTES_PER_MONTH: i64 = 30 * MINUTES_PER_DAY;
    const ROUNDING_BIAS_MINUTES: i64 = 3;

    let Some(minutes) = window_duration_mins else {
        return fallback.to_string();
    };
    let minutes = minutes.max(0);
    if minutes <= MINUTES_PER_DAY.saturating_add(ROUNDING_BIAS_MINUTES) {
        let adjusted = minutes.saturating_add(ROUNDING_BIAS_MINUTES);
        let hours = std::cmp::max(1, adjusted / MINUTES_PER_HOUR);
        format!("{hours}h")
    } else if minutes <= MINUTES_PER_WEEK.saturating_add(ROUNDING_BIAS_MINUTES) {
        "weekly".to_string()
    } else if minutes <= MINUTES_PER_MONTH.saturating_add(ROUNDING_BIAS_MINUTES) {
        "monthly".to_string()
    } else {
        "annual".to_string()
    }
}

pub fn format_reset_hint(ts_secs: Option<i64>) -> String {
    let Some(ts_secs) = ts_secs else {
        return "重置时间未知".to_string();
    };
    let ts_ms = ts_secs.saturating_mul(1000);
    let now_ms = Date::now() as i64;
    let delta_ms = ts_ms - now_ms;
    if delta_ms > 0 {
        let minutes = ((delta_ms + 59_999) / 60_000).max(1);
        if minutes < 60 {
            format!("{minutes} 分钟后重置")
        } else if minutes < 24 * 60 {
            format!("约 {} 小时后重置", ((minutes + 59) / 60).max(1))
        } else {
            format!("约 {} 天后重置", ((minutes + 1_439) / 1_440).max(1))
        }
    } else {
        format!("已到重置时间 {}", format_ms(ts_ms))
    }
}

pub fn format_kiro_disabled_reason(reason: Option<&str>) -> Option<String> {
    match reason.map(str::trim).filter(|value| !value.is_empty()) {
        Some("invalid_refresh_token") => Some("Disabled: invalid refresh token".to_string()),
        Some("manual") => Some("Disabled: manual".to_string()),
        Some(other) => Some(format!("Disabled: {other}")),
        None => None,
    }
}

pub fn pretty_limit_name(raw: &str) -> String {
    let cleaned = raw.replace(['_', '-'], " ");
    let trimmed = if cleaned.len() >= 5 && cleaned[..5].eq_ignore_ascii_case("codex") {
        cleaned[5..].trim_start()
    } else {
        cleaned.as_str()
    };
    if trimmed.is_empty() {
        cleaned
    } else {
        trimmed.to_string()
    }
}

/// Format a number with comma separators: 1234567 → "1,234,567"
pub fn format_number_u64(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            result.push(',');
        }
        result.push(ch);
    }
    result
}

pub fn format_number_i64(n: i64) -> String {
    if n < 0 {
        format!("-{}", format_number_u64(n.unsigned_abs()))
    } else {
        format_number_u64(n as u64)
    }
}

fn resolved_public_url(path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        return path.to_string();
    }
    let origin = web_sys::window()
        .and_then(|window| window.location().origin().ok())
        .unwrap_or_default();
    if origin.is_empty() {
        path.to_string()
    } else {
        format!("{origin}{path}")
    }
}

pub fn resolved_base_url(access: &LlmGatewayAccessResponse) -> String {
    resolved_public_url(&access.base_url)
}

pub fn resolved_model_catalog_url(access: &LlmGatewayAccessResponse) -> String {
    resolved_public_url(&access.model_catalog_path)
}

pub fn example_key_secret(access: &LlmGatewayAccessResponse) -> String {
    access
        .keys
        .first()
        .map(|key| key.secret.clone())
        .unwrap_or_else(|| "<copy-a-public-key>".to_string())
}

pub fn example_key_name(access: &LlmGatewayAccessResponse) -> String {
    access
        .keys
        .first()
        .map(|key| key.name.clone())
        .unwrap_or_else(|| "公开测试 Key".to_string())
}

pub fn codex_provider_config(base_url: &str, default_model: &str) -> String {
    format!(
        r#"model_provider = "staticflow"
model_catalog_json = "model_catalog.json"

[model_providers.staticflow]
name = "OpenAI"
base_url = "{base_url}"
wire_api = "responses"
requires_openai_auth = true
supports_websockets = false

# optional
model = "{default_model}"
model_reasoning_effort = "xhigh""#
    )
}

pub fn codex_model_catalog_download_command(model_catalog_url: &str) -> String {
    format!(r#"curl -fsSL "{model_catalog_url}" -o ~/.codex/model_catalog.json"#)
}

pub fn codex_login_command() -> String {
    "codex login --with-api-key".to_string()
}

pub fn codex_auth_json(secret: &str) -> String {
    format!(
        r#"{{
  "auth_mode": "apikey",
  "OPENAI_API_KEY": "{secret}",
  "tokens": null,
  "last_refresh": null
}}"#
    )
}

pub fn chat_curl_example(base_url: &str, secret: &str, model: &str) -> String {
    format!(
        r#"curl {base_url}/chat/completions \
  -H 'Authorization: Bearer {secret}' \
  -H 'Content-Type: application/json' \
  -d '{{
    "model": "{model}",
    "messages": [
      {{"role": "system", "content": "You are a concise assistant."}},
      {{"role": "user", "content": "Reply with exactly OK."}}
    ],
    "stream": false
  }}'"#
    )
}

pub fn chat_python_example(base_url: &str, secret: &str, model: &str) -> String {
    format!(
        r#"from openai import OpenAI

client = OpenAI(
    base_url="{base_url}",
    api_key="{secret}",
)

resp = client.chat.completions.create(
    model="{model}",
    messages=[
        {{"role": "system", "content": "You are a concise assistant."}},
        {{"role": "user", "content": "Reply with exactly OK."}},
    ],
)

print(resp.choices[0].message.content)"#
    )
}

/// Format a float with 2 decimal places.
pub fn format_float2(value: f64) -> String {
    format!("{value:.2}")
}

/// Compute usage ratio (0.0–1.0) from optional Kiro credit fields.
pub fn kiro_credit_ratio(current: Option<f64>, limit: Option<f64>) -> f64 {
    match (current, limit) {
        (Some(used), Some(cap)) if cap > 0.0 => (used / cap).clamp(0.0, 1.0),
        _ => 0.0,
    }
}

/// Compute usage ratio (0.0–1.0) from a Kiro key's remaining/limit fields.
pub fn kiro_key_usage_ratio(remaining: i64, limit: u64) -> f64 {
    if limit == 0 {
        return 0.0;
    }
    let used = (limit as i64 - remaining).max(0) as f64;
    (used / limit as f64).clamp(0.0, 1.0)
}

#[derive(Debug, Deserialize)]
struct PublicCatalogModelInfo {
    slug: String,
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    supported_in_api: bool,
}

#[derive(Debug, Deserialize)]
struct PublicCatalogModelsResponse {
    models: Vec<PublicCatalogModelInfo>,
}

pub fn preferred_model_slug_from_catalog_json(raw: &str) -> Option<String> {
    let parsed = serde_json::from_str::<PublicCatalogModelsResponse>(raw).ok()?;
    parsed
        .models
        .into_iter()
        .filter(|model| {
            model.supported_in_api
                && model
                    .visibility
                    .as_deref()
                    .map(|value| value.eq_ignore_ascii_case("list"))
                    .unwrap_or(true)
        })
        .max_by_key(preferred_catalog_model_rank)
        .map(|model| model.slug)
}

fn preferred_catalog_model_rank(model: &PublicCatalogModelInfo) -> (i32, i32, i32, i32, i32) {
    if let Some((major, minor, exact_base, variant_rank)) = parse_gpt_model_rank(&model.slug) {
        return (2, major, minor, exact_base, variant_rank);
    }
    (1, 0, 0, 0, 0)
}

fn parse_gpt_model_rank(slug: &str) -> Option<(i32, i32, i32, i32)> {
    let rest = slug.strip_prefix("gpt-")?;
    let version_part = rest.split('-').next()?;
    let mut numbers = version_part.split('.');
    let major = numbers.next()?.parse::<i32>().ok()?;
    let minor = numbers.next()?.parse::<i32>().ok()?;
    if numbers.next().is_some() {
        return None;
    }
    let exact_base = i32::from(rest == version_part);
    let variant_rank = if exact_base == 1 {
        3
    } else if rest.contains("-mini") {
        2
    } else if rest.contains("-codex") {
        1
    } else {
        0
    };
    Some((major, minor, exact_base, variant_rank))
}

#[cfg(test)]
mod tests {
    use super::{
        codex_model_catalog_download_command, codex_provider_config, format_kiro_disabled_reason,
        preferred_model_slug_from_catalog_json,
    };

    #[test]
    fn format_kiro_disabled_reason_maps_known_codes() {
        assert_eq!(
            format_kiro_disabled_reason(Some("invalid_refresh_token")).as_deref(),
            Some("Disabled: invalid refresh token")
        );
        assert_eq!(
            format_kiro_disabled_reason(Some("manual")).as_deref(),
            Some("Disabled: manual")
        );
    }

    #[test]
    fn format_kiro_disabled_reason_ignores_empty_values() {
        assert!(format_kiro_disabled_reason(None).is_none());
        assert!(format_kiro_disabled_reason(Some("   ")).is_none());
    }

    #[test]
    fn provider_config_includes_relative_model_catalog_path_and_selected_model() {
        let config = codex_provider_config("http://127.0.0.1:39180/api/llm-gateway/v1", "gpt-5.5");

        assert!(config.contains(r#"model_catalog_json = "model_catalog.json""#));
        assert!(config.contains(r#"model = "gpt-5.5""#));
    }

    #[test]
    fn preferred_model_slug_ignores_hidden_and_unsupported_models() {
        let raw = r#"{
          "models": [
            { "slug": "codex-auto-review", "visibility": "hide", "supported_in_api": true },
            { "slug": "gpt-5.3-codex-spark", "visibility": "list", "supported_in_api": false },
            { "slug": "gpt-5.5", "visibility": "list", "supported_in_api": true },
            { "slug": "gpt-5.4", "visibility": "list", "supported_in_api": true }
          ]
        }"#;

        assert_eq!(preferred_model_slug_from_catalog_json(raw), Some("gpt-5.5".to_string()));
    }

    #[test]
    fn preferred_model_slug_prefers_latest_base_model() {
        let raw = r#"{
          "models": [
            { "slug": "gpt-5.4", "visibility": "list", "supported_in_api": true },
            { "slug": "gpt-5.5-mini", "visibility": "list", "supported_in_api": true },
            { "slug": "gpt-5.5", "visibility": "list", "supported_in_api": true }
          ]
        }"#;

        assert_eq!(preferred_model_slug_from_catalog_json(raw), Some("gpt-5.5".to_string()));
    }

    #[test]
    fn model_catalog_download_command_targets_codex_home_file() {
        let command = codex_model_catalog_download_command(
            "https://ackingliu.top/api/llm-gateway/model-catalog.json",
        );

        assert!(command.contains("model-catalog.json"));
        assert!(command.contains("~/.codex/model_catalog.json"));
    }
}
