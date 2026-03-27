use js_sys::Date;

use crate::api::{LlmGatewayAccessResponse, LlmGatewayPublicKeyView};

pub const REMOTE_COMPACT_ARTICLE_ID: &str = "codex-compact-local-and-remote-deep-dive";

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

pub fn resolved_base_url(access: &LlmGatewayAccessResponse) -> String {
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

pub fn codex_provider_config(base_url: &str) -> String {
    format!(
        r#"model_provider = "staticflow"

[model_providers.staticflow]
name = "OpenAI"
base_url = "{base_url}"
wire_api = "responses"
requires_openai_auth = true
supports_websockets = false

# optional
model = "gpt-5.4"
model_reasoning_effort = "xhigh""#
    )
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

pub fn chat_curl_example(base_url: &str, secret: &str) -> String {
    format!(
        r#"curl {base_url}/chat/completions \
  -H 'Authorization: Bearer {secret}' \
  -H 'Content-Type: application/json' \
  -d '{{
    "model": "gpt-5.4",
    "messages": [
      {{"role": "system", "content": "You are a concise assistant."}},
      {{"role": "user", "content": "Reply with exactly OK."}}
    ],
    "stream": false
  }}'"#
    )
}

pub fn chat_python_example(base_url: &str, secret: &str) -> String {
    format!(
        r#"from openai import OpenAI

client = OpenAI(
    base_url="{base_url}",
    api_key="{secret}",
)

resp = client.chat.completions.create(
    model="gpt-5.4",
    messages=[
        {{"role": "system", "content": "You are a concise assistant."}},
        {{"role": "user", "content": "Reply with exactly OK."}},
    ],
)

print(resp.choices[0].message.content)"#
    )
}
