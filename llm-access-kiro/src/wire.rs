//! Kiro upstream wire protocol types.
//!
//! Defines the request/response envelope, conversation state, message history,
//! tool definitions, and streaming event types used to communicate with the
//! Kiro `generateAssistantResponse` API.

use serde::{Deserialize, Serialize};

use crate::parser::{
    error::{ParseError, ParseResult},
    frame::Frame,
};

/// Full conversation state sent to the Kiro upstream API.
///
/// Contains the current user message, conversation history, and optional
/// continuation/trigger metadata used for multi-turn agent interactions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_continuation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_task_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_trigger_type: Option<String>,
    pub current_message: CurrentMessage,
    pub conversation_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<Message>,
}

impl ConversationState {
    pub fn new(conversation_id: impl Into<String>) -> Self {
        Self {
            agent_continuation_id: None,
            agent_task_type: None,
            chat_trigger_type: None,
            current_message: CurrentMessage::default(),
            conversation_id: conversation_id.into(),
            history: Vec::new(),
        }
    }

    pub fn with_agent_continuation_id(mut self, id: impl Into<String>) -> Self {
        self.agent_continuation_id = Some(id.into());
        self
    }

    pub fn with_agent_task_type(mut self, task_type: impl Into<String>) -> Self {
        self.agent_task_type = Some(task_type.into());
        self
    }

    pub fn with_chat_trigger_type(mut self, trigger_type: impl Into<String>) -> Self {
        self.chat_trigger_type = Some(trigger_type.into());
        self
    }

    pub fn with_current_message(mut self, message: CurrentMessage) -> Self {
        self.current_message = message;
        self
    }

    pub fn with_history(mut self, history: Vec<Message>) -> Self {
        self.history = history;
        self
    }
}

/// Wrapper for the current turn's user input message.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentMessage {
    pub user_input_message: UserInputMessage,
}

impl CurrentMessage {
    pub fn new(user_input_message: UserInputMessage) -> Self {
        Self {
            user_input_message,
        }
    }
}

/// User input message for the current conversation turn, including content,
/// model selection, optional images, and tool context.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInputMessage {
    pub user_input_message_context: UserInputMessageContext,
    pub content: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<KiroImage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

impl UserInputMessage {
    pub fn new(content: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self {
            user_input_message_context: UserInputMessageContext::default(),
            content: content.into(),
            model_id: model_id.into(),
            images: Vec::new(),
            origin: Some("AI_EDITOR".to_string()),
        }
    }

    pub fn with_context(mut self, context: UserInputMessageContext) -> Self {
        self.user_input_message_context = context;
        self
    }

    pub fn with_images(mut self, images: Vec<KiroImage>) -> Self {
        self.images = images;
        if !self.images.is_empty() {
            // Kiro rejects current-turn image payloads when `origin` is present.
            self.origin = None;
        }
        self
    }

    pub fn with_origin(mut self, origin: impl Into<String>) -> Self {
        if self.images.is_empty() {
            self.origin = Some(origin.into());
        } else {
            self.origin = None;
        }
        self
    }
}

/// Contextual payload attached to a user input message: available tools
/// and any pending tool results from a previous assistant turn.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInputMessageContext {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_results: Vec<ToolResult>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
}

impl UserInputMessageContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_tools(mut self, tools: Vec<Tool>) -> Self {
        self.tools = tools;
        self
    }

    pub fn with_tool_results(mut self, results: Vec<ToolResult>) -> Self {
        self.tool_results = results;
        self
    }
}

/// Base64-encoded image attachment for a Kiro message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KiroImage {
    pub format: String,
    pub source: KiroImageSource,
}

impl KiroImage {
    pub fn from_base64(format: impl Into<String>, data: impl Into<String>) -> Self {
        Self {
            format: format.into(),
            source: KiroImageSource {
                bytes: data.into(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KiroImageSource {
    pub bytes: String,
}

/// A conversation history entry: either a user message or an assistant
/// response. Deserialized via `#[serde(untagged)]` — user messages are tried
/// first.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    User(HistoryUserMessage),
    Assistant(HistoryAssistantMessage),
}

/// History-wrapped user message (keyed by `userInputMessage` in JSON).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryUserMessage {
    pub user_input_message: UserMessage,
}

impl HistoryUserMessage {
    pub fn new(content: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self {
            user_input_message: UserMessage::new(content, model_id),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserMessage {
    pub content: String,
    pub model_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<KiroImage>,
    #[serde(default, skip_serializing_if = "is_default_context")]
    pub user_input_message_context: UserInputMessageContext,
}

fn is_default_context(ctx: &UserInputMessageContext) -> bool {
    ctx.tools.is_empty() && ctx.tool_results.is_empty()
}

impl UserMessage {
    pub fn new(content: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            model_id: model_id.into(),
            origin: Some("AI_EDITOR".to_string()),
            images: Vec::new(),
            user_input_message_context: UserInputMessageContext::default(),
        }
    }

    pub fn with_context(mut self, context: UserInputMessageContext) -> Self {
        self.user_input_message_context = context;
        self
    }
}

/// History-wrapped assistant response (keyed by `assistantResponseMessage` in
/// JSON).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryAssistantMessage {
    pub assistant_response_message: AssistantMessage,
}

impl HistoryAssistantMessage {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            assistant_response_message: AssistantMessage::new(content),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessage {
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_uses: Option<Vec<ToolUseEntry>>,
}

impl AssistantMessage {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            tool_uses: None,
        }
    }

    pub fn with_tool_uses(mut self, tool_uses: Vec<ToolUseEntry>) -> Self {
        self.tool_uses = Some(tool_uses);
        self
    }
}

/// Tool definition wrapper sent inside `UserInputMessageContext`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub tool_specification: ToolSpecification,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpecification {
    pub name: String,
    pub description: String,
    pub input_schema: InputSchema,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputSchema {
    pub json: serde_json::Value,
}

impl InputSchema {
    pub fn from_json(json: serde_json::Value) -> Self {
        Self {
            json,
        }
    }
}

/// Result of a tool invocation, returned to the assistant in the next turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub tool_use_id: String,
    pub content: Vec<serde_json::Map<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_error: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl ToolResult {
    pub fn success(tool_use_id: impl Into<String>, content: impl Into<String>) -> Self {
        let mut map = serde_json::Map::new();
        map.insert("text".to_string(), serde_json::Value::String(content.into()));
        Self {
            tool_use_id: tool_use_id.into(),
            content: vec![map],
            status: Some("success".to_string()),
            is_error: false,
        }
    }

    pub fn error(tool_use_id: impl Into<String>, content: impl Into<String>) -> Self {
        let mut map = serde_json::Map::new();
        map.insert("text".to_string(), serde_json::Value::String(content.into()));
        Self {
            tool_use_id: tool_use_id.into(),
            content: vec![map],
            status: Some("error".to_string()),
            is_error: true,
        }
    }
}

/// A tool invocation recorded in an assistant response's history.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolUseEntry {
    pub tool_use_id: String,
    pub name: String,
    pub input: serde_json::Value,
}

impl ToolUseEntry {
    pub fn new(tool_use_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            name: name.into(),
            input: serde_json::json!({}),
        }
    }

    pub fn with_input(mut self, input: serde_json::Value) -> Self {
        self.input = input;
        self
    }
}

/// Top-level request body sent to the Kiro `generateAssistantResponse`
/// endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KiroRequest {
    pub conversation_state: ConversationState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_arn: Option<String>,
}

/// OAuth token refresh request (social auth flow).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshRequest {
    pub refresh_token: String,
}

/// OAuth token refresh response (social auth flow).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub profile_arn: Option<String>,
    #[serde(default)]
    pub expires_in: Option<i64>,
}

/// IAM Identity Center (IdC) token refresh request.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdcRefreshRequest {
    pub client_id: String,
    pub client_secret: String,
    pub refresh_token: String,
    pub grant_type: String,
}

/// IAM Identity Center (IdC) token refresh response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdcRefreshResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<i64>,
    #[serde(default)]
    pub profile_arn: Option<String>,
}

/// Response from the Kiro `getUsageLimits` endpoint, containing subscription
/// info, usage breakdowns, bonuses, and free-trial details.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageLimitsResponse {
    #[serde(default)]
    pub next_date_reset: Option<f64>,
    #[serde(default)]
    pub subscription_info: Option<SubscriptionInfo>,
    #[serde(default)]
    pub usage_breakdown_list: Vec<UsageBreakdown>,
    #[serde(default)]
    pub user_info: Option<UserInfo>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionInfo {
    #[serde(default)]
    pub subscription_title: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInfo {
    #[serde(default)]
    pub user_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageBreakdown {
    #[serde(default)]
    pub current_usage_with_precision: f64,
    #[serde(default)]
    pub bonuses: Vec<Bonus>,
    #[serde(default)]
    pub free_trial_info: Option<FreeTrialInfo>,
    #[serde(default)]
    pub next_date_reset: Option<f64>,
    #[serde(default)]
    pub usage_limit_with_precision: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bonus {
    #[serde(default)]
    pub current_usage: f64,
    #[serde(default)]
    pub usage_limit: f64,
    #[serde(default)]
    pub status: Option<String>,
}

impl Bonus {
    pub fn is_active(&self) -> bool {
        self.status.as_deref() == Some("ACTIVE")
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FreeTrialInfo {
    #[serde(default)]
    pub current_usage_with_precision: f64,
    #[serde(default)]
    pub free_trial_status: Option<String>,
    #[serde(default)]
    pub usage_limit_with_precision: f64,
}

impl FreeTrialInfo {
    pub fn is_active(&self) -> bool {
        self.free_trial_status.as_deref() == Some("ACTIVE")
    }
}

impl UsageLimitsResponse {
    pub fn subscription_title(&self) -> Option<&str> {
        self.subscription_info
            .as_ref()
            .and_then(|info| info.subscription_title.as_deref())
    }

    pub fn user_id(&self) -> Option<&str> {
        self.user_info
            .as_ref()
            .and_then(|info| info.user_id.as_deref())
    }

    fn primary_breakdown(&self) -> Option<&UsageBreakdown> {
        self.usage_breakdown_list.first()
    }

    pub fn usage_limit(&self) -> f64 {
        let Some(breakdown) = self.primary_breakdown() else {
            return 0.0;
        };
        let mut total = breakdown.usage_limit_with_precision;
        if let Some(free_trial) = &breakdown.free_trial_info {
            if free_trial.is_active() {
                total += free_trial.usage_limit_with_precision;
            }
        }
        for bonus in &breakdown.bonuses {
            if bonus.is_active() {
                total += bonus.usage_limit;
            }
        }
        total
    }

    pub fn current_usage(&self) -> f64 {
        let Some(breakdown) = self.primary_breakdown() else {
            return 0.0;
        };
        let mut total = breakdown.current_usage_with_precision;
        if let Some(free_trial) = &breakdown.free_trial_info {
            if free_trial.is_active() {
                total += free_trial.current_usage_with_precision;
            }
        }
        for bonus in &breakdown.bonuses {
            if bonus.is_active() {
                total += bonus.current_usage;
            }
        }
        total
    }
}

/// Discriminant for Kiro streaming event types received in the event stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventType {
    AssistantResponse,
    ToolUse,
    Metering,
    ContextUsage,
    Unknown,
}

impl EventType {
    pub fn from_wire_name(value: &str) -> Self {
        match value {
            "assistantResponseEvent" => Self::AssistantResponse,
            "toolUseEvent" => Self::ToolUse,
            "meteringEvent" => Self::Metering,
            "contextUsageEvent" => Self::ContextUsage,
            _ => Self::Unknown,
        }
    }
}

/// Trait for event payloads that can be deserialized from an AWS event stream
/// frame.
pub trait EventPayload: Sized {
    fn from_frame(frame: &Frame) -> ParseResult<Self>;
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AssistantResponseEvent {
    #[serde(default)]
    pub content: String,
    #[serde(flatten)]
    #[serde(skip_serializing)]
    pub _extra: serde_json::Value,
}

impl EventPayload for AssistantResponseEvent {
    fn from_frame(frame: &Frame) -> ParseResult<Self> {
        frame.payload_as_json()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolUseEvent {
    pub name: String,
    pub tool_use_id: String,
    #[serde(default)]
    pub input: String,
    #[serde(default)]
    pub stop: bool,
}

impl EventPayload for ToolUseEvent {
    fn from_frame(frame: &Frame) -> ParseResult<Self> {
        frame.payload_as_json()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsageEvent {
    #[serde(default)]
    pub context_usage_percentage: f64,
}

impl EventPayload for ContextUsageEvent {
    fn from_frame(frame: &Frame) -> ParseResult<Self> {
        frame.payload_as_json()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeteringEvent {
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(rename = "unitPlural", default)]
    pub _unit_plural: Option<String>,
    #[serde(default)]
    pub usage: Option<f64>,
}

impl MeteringEvent {
    pub fn credit_usage(&self) -> Option<f64> {
        self.usage
            .filter(|_| self.unit.as_deref() == Some("credit"))
    }
}

impl EventPayload for MeteringEvent {
    fn from_frame(frame: &Frame) -> ParseResult<Self> {
        frame.payload_as_json()
    }
}

/// Parsed streaming event from the Kiro upstream response.
///
/// Variants cover normal content events, tool-use requests, metering/context
/// metadata, and upstream error/exception frames.
#[derive(Debug, Clone)]
pub enum Event {
    AssistantResponse(AssistantResponseEvent),
    ToolUse(ToolUseEvent),
    Metering(MeteringEvent),
    ContextUsage(ContextUsageEvent),
    Unknown {},
    Error { error_code: String, error_message: String },
    Exception { exception_type: String, message: String },
}

impl Event {
    pub fn from_frame(frame: Frame) -> ParseResult<Self> {
        let message_type = frame.message_type().unwrap_or("event");
        match message_type {
            "event" => Self::parse_event(frame),
            "error" => Self::parse_error(frame),
            "exception" => Self::parse_exception(frame),
            other => Err(ParseError::InvalidMessageType(other.to_string())),
        }
    }

    fn parse_event(frame: Frame) -> ParseResult<Self> {
        match EventType::from_wire_name(frame.event_type().unwrap_or("unknown")) {
            EventType::AssistantResponse => {
                Ok(Self::AssistantResponse(AssistantResponseEvent::from_frame(&frame)?))
            },
            EventType::ToolUse => Ok(Self::ToolUse(ToolUseEvent::from_frame(&frame)?)),
            EventType::Metering => Ok(Self::Metering(MeteringEvent::from_frame(&frame)?)),
            EventType::ContextUsage => {
                Ok(Self::ContextUsage(ContextUsageEvent::from_frame(&frame)?))
            },
            EventType::Unknown => Ok(Self::Unknown {}),
        }
    }

    fn parse_error(frame: Frame) -> ParseResult<Self> {
        Ok(Self::Error {
            error_code: frame
                .headers
                .error_code()
                .unwrap_or("UnknownError")
                .to_string(),
            error_message: frame.payload_as_str(),
        })
    }

    fn parse_exception(frame: Frame) -> ParseResult<Self> {
        Ok(Self::Exception {
            exception_type: frame
                .headers
                .exception_type()
                .unwrap_or("UnknownException")
                .to_string(),
            message: frame.payload_as_str(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::IdcRefreshResponse;

    #[test]
    fn idc_refresh_response_deserializes_profile_arn() {
        let payload: IdcRefreshResponse = serde_json::from_str(
            r#"{
                "accessToken": "access-token",
                "refreshToken": "refresh-token",
                "expiresIn": 3600,
                "profileArn": "arn:aws:iam::123456789012:role/KiroProfile"
            }"#,
        )
        .expect("idc refresh response should deserialize");

        assert_eq!(
            payload.profile_arn.as_deref(),
            Some("arn:aws:iam::123456789012:role/KiroProfile")
        );
    }
}
