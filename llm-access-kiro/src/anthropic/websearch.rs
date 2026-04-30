//! Pure web search request and response shaping for the Anthropic-compatible
//! Kiro path.

use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::{
    stream::{anthropic_usage_json, SseEvent},
    types::MessagesRequest,
};

#[derive(Debug, Serialize)]
pub struct McpRequest {
    pub id: String,
    pub jsonrpc: String,
    pub method: String,
    pub params: McpParams,
}

#[derive(Debug, Serialize)]
pub struct McpParams {
    pub name: String,
    pub arguments: McpArguments,
}

#[derive(Debug, Serialize)]
pub struct McpArguments {
    pub query: String,
}

#[derive(Debug, Deserialize)]
pub struct McpResponse {
    pub error: Option<McpError>,
    pub result: Option<McpResult>,
}

#[derive(Debug, Deserialize)]
pub struct McpError {
    pub code: Option<i32>,
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct McpResult {
    pub content: Vec<McpContent>,
    #[serde(rename = "isError")]
    pub is_error: bool,
}

#[derive(Debug, Deserialize)]
pub struct McpContent {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WebSearchResults {
    pub results: Vec<WebSearchResult>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WebSearchResult {
    pub title: String,
    pub url: String,
    pub snippet: Option<String>,
    #[serde(rename = "publishedDate")]
    pub published_date: Option<i64>,
}

pub fn has_web_search_tool(req: &MessagesRequest) -> bool {
    req.tools.as_ref().is_some_and(|tools| {
        tools.len() == 1 && tools.first().is_some_and(|tool| tool.is_web_search())
    })
}

pub fn extract_search_query(req: &MessagesRequest) -> Option<String> {
    let first_msg = req.messages.first()?;
    let text = match &first_msg.content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(arr) => {
            let first_block = arr.first()?;
            if first_block.get("type")?.as_str()? == "text" {
                first_block.get("text")?.as_str()?.to_string()
            } else {
                return None;
            }
        },
        _ => return None,
    };

    const PREFIX: &str = "Perform a web search for the query: ";
    let query =
        if let Some(stripped) = text.strip_prefix(PREFIX) { stripped.to_string() } else { text };
    let query = query.trim().to_string();
    (!query.is_empty()).then_some(query)
}

pub fn create_mcp_request(query: &str) -> (String, McpRequest) {
    let request_id = format!(
        "web_search_tooluse_{}_{}",
        Uuid::new_v4().simple(),
        chrono::Utc::now().timestamp_millis()
    );
    let tool_use_id = format!("srvtoolu_{}", &Uuid::new_v4().simple().to_string()[..32]);
    (tool_use_id, McpRequest {
        id: request_id,
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: McpParams {
            name: "web_search".to_string(),
            arguments: McpArguments {
                query: query.to_string(),
            },
        },
    })
}

pub fn parse_search_results(mcp_response: &McpResponse) -> Option<WebSearchResults> {
    let result = mcp_response.result.as_ref()?;
    if result.is_error {
        return None;
    }
    let content = result.content.first()?;
    if content.content_type != "text" {
        return None;
    }
    serde_json::from_str(&content.text).ok()
}

pub fn should_propagate_mcp_error_text(err_text: &str) -> bool {
    err_text.contains("quota exhausted")
        || err_text.contains("no kiro account available")
        || err_text.contains("Missing API key")
        || err_text.contains("fixed route account ")
        || err_text.contains("no configured auto accounts are available")
        || err_text.contains("fixed route_strategy requires fixed_account_name")
        || err_text.contains("unsupported route strategy")
}

pub fn estimate_output_tokens(summary: &str) -> i32 {
    ((summary.chars().count() as i32) + 3) / 4
}

pub fn generate_websearch_events(
    model: &str,
    query: &str,
    tool_use_id: &str,
    search_results: Option<&WebSearchResults>,
    input_tokens: i32,
    summary: &str,
    output_tokens: i32,
) -> Vec<SseEvent> {
    let message_id = format!("msg_{}", &Uuid::new_v4().simple().to_string()[..24]);
    let mut events = vec![SseEvent::new(
        "message_start",
        json!({
            "type": "message_start",
            "message": {
                "id": message_id,
                "type": "message",
                "role": "assistant",
                "model": model,
                "content": [],
                "stop_reason": null,
                "usage": anthropic_usage_json(input_tokens, 0, 0)
            }
        }),
    )];

    let decision_text = format!("I'll search for \"{query}\".");
    events.push(SseEvent::new(
        "content_block_start",
        json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "text", "text": ""}
        }),
    ));
    events.push(SseEvent::new(
        "content_block_delta",
        json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": decision_text}
        }),
    ));
    events.push(SseEvent::new(
        "content_block_stop",
        json!({"type": "content_block_stop", "index": 0}),
    ));

    events.push(SseEvent::new(
        "content_block_start",
        json!({
            "type": "content_block_start",
            "index": 1,
            "content_block": {
                "id": tool_use_id,
                "type": "server_tool_use",
                "name": "web_search",
                "input": {"query": query}
            }
        }),
    ));
    events.push(SseEvent::new(
        "content_block_stop",
        json!({"type": "content_block_stop", "index": 1}),
    ));

    events.push(SseEvent::new(
        "content_block_start",
        json!({
            "type": "content_block_start",
            "index": 2,
            "content_block": {
                "type": "web_search_tool_result",
                "content": create_search_result_blocks(search_results)
            }
        }),
    ));
    events.push(SseEvent::new(
        "content_block_stop",
        json!({"type": "content_block_stop", "index": 2}),
    ));

    events.push(SseEvent::new(
        "content_block_start",
        json!({
            "type": "content_block_start",
            "index": 3,
            "content_block": {"type": "text", "text": ""}
        }),
    ));
    for chunk in summary.chars().collect::<Vec<_>>().chunks(100) {
        let text: String = chunk.iter().collect();
        events.push(SseEvent::new(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 3,
                "delta": {"type": "text_delta", "text": text}
            }),
        ));
    }
    events.push(SseEvent::new(
        "content_block_stop",
        json!({"type": "content_block_stop", "index": 3}),
    ));
    events.push(SseEvent::new(
        "message_delta",
        json!({
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn"},
            "usage": {
                "output_tokens": output_tokens,
                "server_tool_use": {
                    "web_search_requests": 1
                }
            }
        }),
    ));
    events.push(SseEvent::new("message_stop", json!({"type": "message_stop"})));
    events
}

pub fn create_non_stream_content_blocks(
    query: &str,
    tool_use_id: &str,
    search_results: &Option<WebSearchResults>,
    summary: &str,
) -> Vec<serde_json::Value> {
    vec![
        json!({
            "type": "text",
            "text": format!("I'll search for \"{query}\".")
        }),
        json!({
            "type": "server_tool_use",
            "id": tool_use_id,
            "name": "web_search",
            "input": {"query": query}
        }),
        json!({
            "type": "web_search_tool_result",
            "content": create_search_result_blocks(search_results.as_ref())
        }),
        json!({
            "type": "text",
            "text": summary
        }),
    ]
}

pub fn generate_search_summary(query: &str, results: &Option<WebSearchResults>) -> String {
    let mut summary = format!("Here are the search results for \"{query}\":\n\n");
    if let Some(results) = results {
        for (index, result) in results.results.iter().enumerate() {
            summary.push_str(&format!("{}. **{}**\n", index + 1, result.title));
            if let Some(snippet) = &result.snippet {
                let snippet = truncate_chars(snippet, 200);
                summary.push_str(&format!("   {snippet}\n"));
            }
            summary.push_str(&format!("   Source: {}\n\n", result.url));
        }
    } else {
        summary.push_str("No results found.\n");
    }
    summary.push_str(
        "\nPlease note that these are web search results and may not be fully accurate or \
         up-to-date.",
    );
    summary
}

fn create_search_result_blocks(
    search_results: Option<&WebSearchResults>,
) -> Vec<serde_json::Value> {
    search_results
        .map(|results| {
            results
                .results
                .iter()
                .map(|result| {
                    let page_age = result.published_date.and_then(|ms| {
                        chrono::DateTime::from_timestamp_millis(ms)
                            .map(|dt| dt.format("%B %-d, %Y").to_string())
                    });
                    json!({
                        "type": "web_search_result",
                        "title": result.title,
                        "url": result.url,
                        "encrypted_content": result.snippet.clone().unwrap_or_default(),
                        "page_age": page_age
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    match value.char_indices().nth(max_chars) {
        Some((idx, _)) => format!("{}...", &value[..idx]),
        None => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        super::types::{Message, Tool},
        *,
    };

    fn base_request(tools: Option<Vec<Tool>>, content: serde_json::Value) -> MessagesRequest {
        MessagesRequest {
            model: "claude-sonnet-4-6".to_string(),
            _max_tokens: 1024,
            messages: vec![Message {
                role: "user".to_string(),
                content,
            }],
            stream: true,
            system: None,
            tools,
            _tool_choice: None,
            thinking: None,
            output_config: None,
            metadata: None,
        }
    }

    #[test]
    fn detects_pure_web_search_tool_only() {
        let req = base_request(
            Some(vec![Tool {
                tool_type: Some("web_search_20250305".to_string()),
                name: "web_search".to_string(),
                description: String::new(),
                input_schema: Default::default(),
                max_uses: Some(8),
            }]),
            serde_json::json!("test"),
        );
        assert!(has_web_search_tool(&req));
    }

    #[test]
    fn rejects_mixed_tools_for_web_search_short_circuit() {
        let req = base_request(
            Some(vec![
                Tool {
                    tool_type: Some("web_search_20250305".to_string()),
                    name: "web_search".to_string(),
                    description: String::new(),
                    input_schema: Default::default(),
                    max_uses: Some(8),
                },
                Tool {
                    tool_type: Some("custom".to_string()),
                    name: "other".to_string(),
                    description: String::new(),
                    input_schema: Default::default(),
                    max_uses: None,
                },
            ]),
            serde_json::json!("test"),
        );
        assert!(!has_web_search_tool(&req));
    }

    #[test]
    fn extracts_prefixed_query() {
        let req = base_request(
            None,
            serde_json::json!([{
                "type": "text",
                "text": "Perform a web search for the query: static flow kiro"
            }]),
        );
        assert_eq!(extract_search_query(&req).as_deref(), Some("static flow kiro"));
    }

    #[test]
    fn websearch_stream_message_start_marks_half_input_as_cache_creation() {
        let events = generate_websearch_events(
            "claude-sonnet-4-6",
            "static flow kiro",
            "toolu_test",
            None,
            125,
            "summary",
            16,
        );
        let message_start = events
            .iter()
            .find(|event| event.event == "message_start")
            .expect("should include message_start");
        assert_eq!(
            message_start.data["message"]["usage"]["cache_creation_input_tokens"],
            serde_json::json!(62)
        );
        assert_eq!(
            message_start.data["message"]["usage"]["cache_read_input_tokens"],
            serde_json::json!(0)
        );
    }

    #[test]
    fn websearch_route_related_errors_should_be_propagated() {
        assert!(should_propagate_mcp_error_text("fixed route account `alpha` is not available"));
        assert!(should_propagate_mcp_error_text("no configured auto accounts are available"));
        assert!(should_propagate_mcp_error_text(
            "fixed route_strategy requires fixed_account_name"
        ));
        assert!(should_propagate_mcp_error_text("unsupported route strategy `none`"));
    }

    #[test]
    fn websearch_non_route_error_should_fallback() {
        assert!(!should_propagate_mcp_error_text("MCP error: -1 - temporary endpoint issue"));
    }
}
