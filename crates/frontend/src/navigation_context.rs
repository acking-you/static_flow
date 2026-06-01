use std::collections::BTreeMap;

use web_sys::window;

const RETURN_URL_KEY: &str = "sf:detail:return:url";
const RETURN_SCROLL_KEY: &str = "sf:detail:return:scroll";
const RETURN_TS_KEY: &str = "sf:detail:return:ts";
const RETURN_STATE_KEY: &str = "sf:detail:return:state";
const RETURN_ARMED_KEY: &str = "sf:detail:return:armed";
const RETURN_CONTEXT_TTL_MS: i64 = 30 * 60 * 1000;

#[derive(Debug, Clone, PartialEq)]
pub struct DetailReturnContext {
    pub source_url: String,
    pub scroll_y: f64,
    pub timestamp_ms: i64,
    pub page_state: BTreeMap<String, String>,
}

fn now_ms() -> i64 {
    js_sys::Date::now() as i64
}

fn normalize_url_for_match(url: &str) -> String {
    url.split('#')
        .next()
        .map(str::trim)
        .unwrap_or("")
        .trim_end_matches('/')
        .to_string()
}

fn parse_page_state(encoded: &str) -> BTreeMap<String, String> {
    let mut state = BTreeMap::new();
    if encoded.trim().is_empty() {
        return state;
    }

    for pair in encoded.split('&') {
        if pair.trim().is_empty() {
            continue;
        }
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or_default();
        let value = parts.next().unwrap_or_default();
        let decoded_key = urlencoding::decode(key)
            .map(|v| v.into_owned())
            .unwrap_or_else(|_| key.to_string());
        let decoded_value = urlencoding::decode(value)
            .map(|v| v.into_owned())
            .unwrap_or_else(|_| value.to_string());
        state.insert(decoded_key, decoded_value);
    }
    state
}

fn encode_page_state(state: &BTreeMap<String, String>) -> String {
    state
        .iter()
        .map(|(key, value)| format!("{}={}", urlencoding::encode(key), urlencoding::encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

pub fn current_page_url() -> Option<String> {
    let win = window()?;
    let location = win.location();
    let path = location.pathname().ok()?;
    let search = location.search().unwrap_or_default();
    let hash = location.hash().unwrap_or_default();
    Some(format!("{path}{search}{hash}"))
}

pub fn current_scroll_y() -> f64 {
    window().and_then(|win| win.scroll_y().ok()).unwrap_or(0.0)
}

pub fn is_return_armed() -> bool {
    window()
        .and_then(|win| win.session_storage().ok().flatten())
        .and_then(|storage| storage.get_item(RETURN_ARMED_KEY).ok().flatten())
        .as_deref()
        == Some("1")
}

pub fn save_context_for_current_page(page_state: BTreeMap<String, String>) {
    if let Some(url) = current_page_url() {
        save_context(&url, current_scroll_y(), page_state);
    }
}

pub fn save_context(source_url: &str, scroll_y: f64, page_state: BTreeMap<String, String>) {
    if !source_url.starts_with('/') {
        return;
    }
    if let Some(storage) = window().and_then(|win| win.session_storage().ok().flatten()) {
        let _ = storage.set_item(RETURN_URL_KEY, source_url);
        let _ = storage.set_item(RETURN_SCROLL_KEY, &scroll_y.to_string());
        let _ = storage.set_item(RETURN_TS_KEY, &now_ms().to_string());
        let _ = storage.set_item(RETURN_STATE_KEY, &encode_page_state(&page_state));
    }
}

pub fn arm_context_for_return() {
    if let Some(storage) = window().and_then(|win| win.session_storage().ok().flatten()) {
        let _ = storage.set_item(RETURN_ARMED_KEY, "1");
    }
}

pub fn clear_context() {
    if let Some(storage) = window().and_then(|win| win.session_storage().ok().flatten()) {
        let _ = storage.remove_item(RETURN_URL_KEY);
        let _ = storage.remove_item(RETURN_SCROLL_KEY);
        let _ = storage.remove_item(RETURN_TS_KEY);
        let _ = storage.remove_item(RETURN_STATE_KEY);
        let _ = storage.remove_item(RETURN_ARMED_KEY);
    }
}

pub fn peek_context() -> Option<DetailReturnContext> {
    let storage = window().and_then(|win| win.session_storage().ok().flatten())?;
    let source_url = storage.get_item(RETURN_URL_KEY).ok().flatten()?;
    if !source_url.starts_with('/') {
        return None;
    }

    let timestamp_ms = storage
        .get_item(RETURN_TS_KEY)
        .ok()
        .flatten()
        .and_then(|raw| raw.parse::<i64>().ok())?;
    if now_ms().saturating_sub(timestamp_ms) > RETURN_CONTEXT_TTL_MS {
        return None;
    }

    let scroll_y = storage
        .get_item(RETURN_SCROLL_KEY)
        .ok()
        .flatten()
        .and_then(|raw| raw.parse::<f64>().ok())
        .unwrap_or(0.0)
        .max(0.0);

    let page_state = storage
        .get_item(RETURN_STATE_KEY)
        .ok()
        .flatten()
        .map(|raw| parse_page_state(&raw))
        .unwrap_or_default();

    Some(DetailReturnContext {
        source_url,
        scroll_y,
        timestamp_ms,
        page_state,
    })
}

pub fn pop_context_if_armed_for_current_page() -> Option<DetailReturnContext> {
    if !is_return_armed() {
        return None;
    }

    let current = current_page_url()?;
    let context = peek_context()?;

    if normalize_url_for_match(&context.source_url) != normalize_url_for_match(&current) {
        return None;
    }

    clear_context();
    Some(context)
}

pub fn navigate_spa_to(url: &str) -> bool {
    let Some(win) = window() else {
        return false;
    };
    if let Ok(history) = win.history() {
        if history
            .push_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(url))
            .is_ok()
        {
            if let Ok(event) = web_sys::Event::new("popstate") {
                let _ = win.dispatch_event(&event);
            }
            return true;
        }
    }
    false
}
