use js_sys::Date;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use wasm_bindgen::{prelude::*, JsCast};
use web_sys::File;
use yew::prelude::*;
use yew_router::prelude::Link;

use crate::router::Route;

const AUTH_KEY_STORAGE_KEY: &str = "gpt2api_auth_key";
const IMAGE_HISTORY_STORAGE_KEY: &str = "gpt2api_image_history";
const CHAT_HISTORY_STORAGE_KEY: &str = "gpt2api_chat_history";

#[wasm_bindgen(inline_js = r#"
const DB_NAME = "staticflow-gpt2api";
const STORE_NAME = "kv";

function openDb() {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, 1);
    request.onupgradeneeded = () => {
      const db = request.result;
      if (!db.objectStoreNames.contains(STORE_NAME)) {
        db.createObjectStore(STORE_NAME);
      }
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error || new Error("failed to open indexeddb"));
  });
}

export async function sf_gpt2api_idb_get(key) {
  const db = await openDb();
  return await new Promise((resolve, reject) => {
    const tx = db.transaction(STORE_NAME, "readonly");
    const req = tx.objectStore(STORE_NAME).get(key);
    req.onsuccess = () => resolve(req.result ?? null);
    req.onerror = () => reject(req.error || new Error("indexeddb get failed"));
  });
}

export async function sf_gpt2api_idb_set(key, value) {
  const db = await openDb();
  return await new Promise((resolve, reject) => {
    const tx = db.transaction(STORE_NAME, "readwrite");
    const req = tx.objectStore(STORE_NAME).put(value, key);
    req.onsuccess = () => resolve(null);
    req.onerror = () => reject(req.error || new Error("indexeddb set failed"));
  });
}

export async function sf_gpt2api_idb_delete(key) {
  const db = await openDb();
  return await new Promise((resolve, reject) => {
    const tx = db.transaction(STORE_NAME, "readwrite");
    const req = tx.objectStore(STORE_NAME).delete(key);
    req.onsuccess = () => resolve(null);
    req.onerror = () => reject(req.error || new Error("indexeddb delete failed"));
  });
}

export async function sf_gpt2api_file_to_data_url(file) {
  return await new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result || ""));
    reader.onerror = () => reject(reader.error || new Error("failed to read file"));
    reader.readAsDataURL(file);
  });
}

function boundaryIndex(buffer) {
  const lf = buffer.indexOf("\n\n");
  const crlf = buffer.indexOf("\r\n\r\n");
  if (lf === -1) {
    return crlf === -1 ? null : [crlf, 4];
  }
  if (crlf === -1) {
    return [lf, 2];
  }
  return lf <= crlf ? [lf, 2] : [crlf, 4];
}

function extractPayload(eventBlock) {
  const payload = eventBlock
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.startsWith("data:"))
    .map((line) => line.slice(5).trim())
    .filter(Boolean)
    .join("\n");
  return payload || null;
}

async function decodeError(response) {
  const text = await response.text();
  if (!text) {
    return `HTTP ${response.status}`;
  }
  try {
    const value = JSON.parse(text);
    if (typeof value?.error === "string") {
      return value.error;
    }
    if (typeof value?.message === "string") {
      return value.message;
    }
  } catch (_) {}
  return text;
}

export async function sf_gpt2api_stream_chat(url, authKey, bodyJson, onData) {
  const response = await fetch(url, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "authorization": `Bearer ${authKey}`,
      "x-sf-client": "web",
    },
    body: bodyJson,
  });
  if (!response.ok) {
    throw new Error(await decodeError(response));
  }
  const reader = response.body?.getReader();
  if (!reader) {
    throw new Error("missing response stream");
  }
  const decoder = new TextDecoder();
  let buffer = "";
  while (true) {
    const { done, value } = await reader.read();
    if (done) {
      buffer += decoder.decode();
      break;
    }
    buffer += decoder.decode(value, { stream: true });
    while (true) {
      const boundary = boundaryIndex(buffer);
      if (!boundary) {
        break;
      }
      const [index, length] = boundary;
      const block = buffer.slice(0, index);
      buffer = buffer.slice(index + length);
      const payload = extractPayload(block);
      if (payload) {
        onData(payload);
      }
    }
  }
  const trailing = extractPayload(buffer);
  if (trailing) {
    onData(trailing);
  }
}
"#)]
extern "C" {
    #[wasm_bindgen(catch)]
    async fn sf_gpt2api_idb_get(key: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    async fn sf_gpt2api_idb_set(key: &str, value: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    async fn sf_gpt2api_idb_delete(key: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    async fn sf_gpt2api_file_to_data_url(file: &File) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    async fn sf_gpt2api_stream_chat(
        url: &str,
        auth_key: &str,
        body_json: &str,
        on_data: &js_sys::Function,
    ) -> Result<JsValue, JsValue>;
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredGpt2ApiReferenceImage {
    pub name: String,
    pub mime_type: String,
    pub data_url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredGpt2ApiImage {
    pub id: String,
    #[serde(default)]
    pub b64_json: Option<String>,
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredGpt2ApiImageConversation {
    pub id: String,
    pub title: String,
    pub created_at_ms: i64,
    pub prompt: String,
    pub model: String,
    pub mode: String,
    pub count: usize,
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub reference_images: Vec<StoredGpt2ApiReferenceImage>,
    #[serde(default)]
    pub images: Vec<StoredGpt2ApiImage>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredGpt2ApiChatConversation {
    pub id: String,
    pub title: String,
    pub created_at_ms: i64,
    pub prompt: String,
    pub model: String,
    pub status: String,
    #[serde(default)]
    pub answer: String,
    #[serde(default)]
    pub error: Option<String>,
}

pub async fn load_auth_key() -> Result<String, String> {
    load_json_value::<String>(AUTH_KEY_STORAGE_KEY)
        .await
        .map(|value| value.unwrap_or_default())
}

pub async fn save_auth_key(value: &str) -> Result<(), String> {
    let normalized = value.trim();
    if normalized.is_empty() {
        clear_auth_key().await
    } else {
        save_json_value(AUTH_KEY_STORAGE_KEY, &normalized.to_string()).await
    }
}

pub async fn clear_auth_key() -> Result<(), String> {
    delete_json_value(AUTH_KEY_STORAGE_KEY).await
}

pub async fn list_image_conversations() -> Result<Vec<StoredGpt2ApiImageConversation>, String> {
    let mut items =
        load_json_value::<Vec<StoredGpt2ApiImageConversation>>(IMAGE_HISTORY_STORAGE_KEY)
            .await?
            .unwrap_or_default()
            .into_iter()
            .map(normalize_image_conversation)
            .collect::<Vec<_>>();
    items.sort_by(|left, right| right.created_at_ms.cmp(&left.created_at_ms));
    Ok(items)
}

pub async fn save_image_conversation(
    conversation: &StoredGpt2ApiImageConversation,
) -> Result<(), String> {
    let mut items = list_image_conversations().await?;
    items.retain(|item| item.id != conversation.id);
    items.push(normalize_image_conversation(conversation.clone()));
    items.sort_by(|left, right| right.created_at_ms.cmp(&left.created_at_ms));
    save_json_value(IMAGE_HISTORY_STORAGE_KEY, &items).await
}

pub async fn delete_image_conversation(id: &str) -> Result<(), String> {
    let items = list_image_conversations().await?;
    let next = items
        .into_iter()
        .filter(|item| item.id != id)
        .collect::<Vec<_>>();
    save_json_value(IMAGE_HISTORY_STORAGE_KEY, &next).await
}

pub async fn clear_image_conversations() -> Result<(), String> {
    delete_json_value(IMAGE_HISTORY_STORAGE_KEY).await
}

pub async fn list_chat_conversations() -> Result<Vec<StoredGpt2ApiChatConversation>, String> {
    let mut items = load_json_value::<Vec<StoredGpt2ApiChatConversation>>(CHAT_HISTORY_STORAGE_KEY)
        .await?
        .unwrap_or_default()
        .into_iter()
        .map(normalize_chat_conversation)
        .collect::<Vec<_>>();
    items.sort_by(|left, right| right.created_at_ms.cmp(&left.created_at_ms));
    Ok(items)
}

pub async fn save_chat_conversation(
    conversation: &StoredGpt2ApiChatConversation,
) -> Result<(), String> {
    let mut items = list_chat_conversations().await?;
    items.retain(|item| item.id != conversation.id);
    items.push(normalize_chat_conversation(conversation.clone()));
    items.sort_by(|left, right| right.created_at_ms.cmp(&left.created_at_ms));
    save_json_value(CHAT_HISTORY_STORAGE_KEY, &items).await
}

pub async fn delete_chat_conversation(id: &str) -> Result<(), String> {
    let items = list_chat_conversations().await?;
    let next = items
        .into_iter()
        .filter(|item| item.id != id)
        .collect::<Vec<_>>();
    save_json_value(CHAT_HISTORY_STORAGE_KEY, &next).await
}

pub async fn clear_chat_conversations() -> Result<(), String> {
    delete_json_value(CHAT_HISTORY_STORAGE_KEY).await
}

pub async fn file_to_data_url(file: &File) -> Result<String, String> {
    sf_gpt2api_file_to_data_url(file)
        .await
        .map_err(js_error_message)
        .and_then(|value| {
            value
                .as_string()
                .ok_or_else(|| "failed to decode file preview".to_string())
        })
}

pub async fn stream_chat_completion<F>(
    url: &str,
    auth_key: &str,
    body_json: &str,
    on_data: F,
) -> Result<(), String>
where
    F: FnMut(String) + 'static,
{
    let closure = Closure::<dyn FnMut(String)>::wrap(Box::new(on_data));
    let result = sf_gpt2api_stream_chat(url, auth_key, body_json, closure.as_ref().unchecked_ref())
        .await
        .map_err(js_error_message);
    drop(closure);
    result.map(|_| ())
}

pub fn create_client_id() -> String {
    format!("{}-{}", Date::now() as i64, js_sys::Math::random().to_string().replace("0.", ""))
}

pub fn now_ms() -> i64 {
    Date::now() as i64
}

pub fn build_conversation_title(prompt: &str, max_chars: usize) -> String {
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        return "未命名".to_string();
    }
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    format!("{}...", trimmed.chars().take(max_chars).collect::<String>())
}

pub fn format_conversation_time(ts_ms: i64) -> String {
    let date = Date::new(&JsValue::from_f64(ts_ms as f64));
    format!(
        "{:02}/{:02} {:02}:{:02}",
        date.get_month() + 1,
        date.get_date(),
        date.get_hours(),
        date.get_minutes()
    )
}

pub fn nav_shell(active: &'static str, on_logout: Callback<MouseEvent>) -> Html {
    let nav_link = |route: Route, label: &'static str, key: &'static str| {
        let class_name = if active == key {
            "relative py-2 text-[15px] font-semibold text-stone-950"
        } else {
            "relative py-2 text-[15px] font-medium text-stone-500 transition hover:text-stone-900"
        };
        html! {
            <Link<Route> to={route} classes={classes!(class_name)}>
                { label }
                if active == key {
                    <span class={classes!("absolute", "inset-x-0", "-bottom-[3px]", "h-0.5", "bg-stone-950")}></span>
                }
            </Link<Route>>
        }
    };

    html! {
        <section class={classes!("mx-auto", "w-full", "max-w-[1380px]", "px-3", "pt-1")}>
            <header class={classes!("flex", "h-12", "items-start", "justify-between", "pt-1")}>
                <div class={classes!("flex", "flex-1", "items-center", "gap-3")}>
                    <Link<Route>
                        to={Route::Gpt2ApiImage}
                        classes={classes!("py-2", "text-[15px]", "font-semibold", "tracking-tight", "text-stone-950", "transition", "hover:text-stone-700")}
                    >
                        { "StaticFlow gpt2api" }
                    </Link<Route>>
                    <span class={classes!("rounded-md", "bg-stone-100", "px-2", "py-1", "text-[11px]", "font-medium", "text-stone-500")}>
                        { "public" }
                    </span>
                </div>
                <div class={classes!("flex", "justify-center", "gap-8")}>
                    { nav_link(Route::Gpt2ApiImage, "画图", "image") }
                    { nav_link(Route::Gpt2ApiChat, "聊天", "chat") }
                </div>
                <div class={classes!("flex", "flex-1", "items-center", "justify-end", "gap-3")}>
                    <button
                        type="button"
                        class={classes!("py-2", "text-sm", "text-stone-400", "transition", "hover:text-stone-700")}
                        onclick={on_logout}
                    >
                        { "退出" }
                    </button>
                </div>
            </header>
        </section>
    }
}

fn normalize_image_conversation(
    mut conversation: StoredGpt2ApiImageConversation,
) -> StoredGpt2ApiImageConversation {
    conversation.title = build_conversation_title(&conversation.prompt, 5);
    if conversation.mode != "edit" {
        conversation.mode = "generate".to_string();
    }
    conversation.images = conversation
        .images
        .into_iter()
        .map(normalize_stored_image)
        .collect();
    if conversation.count == 0 {
        conversation.count = conversation.images.len().max(1);
    }
    conversation
}

fn normalize_chat_conversation(
    mut conversation: StoredGpt2ApiChatConversation,
) -> StoredGpt2ApiChatConversation {
    conversation.title = build_conversation_title(&conversation.prompt, 12);
    conversation
}

fn normalize_stored_image(mut image: StoredGpt2ApiImage) -> StoredGpt2ApiImage {
    if image.status != "loading" && image.status != "error" && image.status != "success" {
        image.status = if image
            .b64_json
            .as_deref()
            .is_some_and(|value| !value.is_empty())
        {
            "success".to_string()
        } else {
            "loading".to_string()
        };
    }
    image
}

async fn load_json_value<T>(key: &str) -> Result<Option<T>, String>
where
    T: DeserializeOwned,
{
    let value = sf_gpt2api_idb_get(key).await.map_err(js_error_message)?;
    if value.is_null() || value.is_undefined() {
        return Ok(None);
    }
    let text = value
        .as_string()
        .ok_or_else(|| "indexeddb value is not a string".to_string())?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|err| format!("failed to decode stored json: {err}"))
}

async fn save_json_value<T>(key: &str, value: &T) -> Result<(), String>
where
    T: Serialize,
{
    let text = serde_json::to_string(value)
        .map_err(|err| format!("failed to encode stored json: {err}"))?;
    sf_gpt2api_idb_set(key, &text)
        .await
        .map_err(js_error_message)?;
    Ok(())
}

async fn delete_json_value(key: &str) -> Result<(), String> {
    sf_gpt2api_idb_delete(key).await.map_err(js_error_message)?;
    Ok(())
}

fn js_error_message(err: JsValue) -> String {
    if let Some(message) = err.as_string() {
        return message;
    }
    if let Some(error) = err.dyn_ref::<js_sys::Error>() {
        return error.message().into();
    }
    format!("{err:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_conversation_title_truncates_long_prompt() {
        assert_eq!(build_conversation_title("abcdef", 5), "abcde...");
        assert_eq!(build_conversation_title("abc", 5), "abc");
    }

    #[test]
    fn normalize_image_conversation_fills_missing_fields() {
        let conversation = StoredGpt2ApiImageConversation {
            id: "demo".to_string(),
            title: String::new(),
            created_at_ms: 1,
            prompt: "一个很长的提示词".to_string(),
            model: "gpt-image-1".to_string(),
            mode: String::new(),
            count: 0,
            status: "success".to_string(),
            error: None,
            reference_images: Vec::new(),
            images: vec![StoredGpt2ApiImage {
                id: "image-1".to_string(),
                b64_json: Some("abc".to_string()),
                status: String::new(),
                error: None,
            }],
        };

        let normalized = normalize_image_conversation(conversation);
        assert_eq!(normalized.title, "一个很长的...");
        assert_eq!(normalized.mode, "generate");
        assert_eq!(normalized.count, 1);
        assert_eq!(normalized.images[0].status, "success");
    }
}
