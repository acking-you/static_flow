//! Canonical prompt projection for Kiro prefix-cache simulation.
//!
//! This module projects the corrected Kiro `ConversationState` into two
//! source-of-truth views:
//! - exact canonical history anchors for conversation recovery
//! - stable-prefix spans for shared prefix-cache simulation
//!
//! The two views deliberately use different windows. Lookup anchors only cover
//! the history that already existed before the current turn, while resume
//! anchors append the finalized current turn plus assistant response.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use xxhash_rust::xxh3::{xxh3_128, xxh3_64};

use crate::wire::{
    AssistantMessage, ConversationState, KiroDocument, KiroImage, Message, Tool, UserInputMessage,
    UserInputMessageContext, UserMessage,
};

/// Number of token atoms packed into a single prefix-cache page.
///
/// Pages are the unit of prefix matching, which keeps the shared trie compact
/// even as global request volume grows. Shared with the simulator for stats.
pub const PREFIX_CACHE_PAGE_SIZE: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
// A canonical unit is the smallest semantic fragment we retain before packing
// it into fixed-size cache pages. We keep the stable string key for anchor/hash
// construction, while token atoms feed the page-based prefix tree.
struct CanonicalInputUnit {
    pub key: String,
    pub token_atoms: Vec<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// Prefix-cache matching operates on fixed-size token pages instead of single
// tokens so the shared trie stays compact even when the global request volume
// grows.
pub struct CanonicalTokenPage {
    pub key: u128,
    pub token_count: u16,
}

/// Canonical, source-of-truth prompt projection derived from a corrected Kiro
/// `ConversationState`.
///
/// `lookup_anchor_hash` only covers the already-known history prefix.
/// `stable_prefix_pages` additionally includes current-turn tool definitions,
/// because they influence cacheability of the current upstream call. Resume
/// anchors intentionally exclude those tool definitions and instead append the
/// finalized current turn as history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptProjection {
    pub lookup_anchor_hash: String,
    pub stable_prefix_pages: Vec<CanonicalTokenPage>,
    pub projected_input_token_count: u64,
    stable_prefix_segment_keys: Vec<String>,
    history_anchor_segments: Vec<String>,
    current_turn_history_segments: Vec<String>,
}

impl PromptProjection {
    pub fn from_conversation_state(state: &ConversationState) -> Self {
        let history_units = canonicalize_history(&state.history);
        let history_anchor_segments = history_units
            .iter()
            .map(|unit| unit.key.clone())
            .collect::<Vec<_>>();
        let mut stable_prefix_units = history_units;
        stable_prefix_units.extend(canonicalize_tools(
            &state
                .current_message
                .user_input_message
                .user_input_message_context
                .tools,
        ));
        let current_turn_input_units =
            canonicalize_current_turn_for_input(&state.current_message.user_input_message);
        let current_turn_history_segments =
            canonicalize_current_turn_as_history(&state.current_message.user_input_message);
        let stable_prefix_segment_keys = stable_prefix_units
            .iter()
            .map(|unit| unit.key.clone())
            .collect::<Vec<_>>();
        let stable_prefix_pages = build_token_pages(&stable_prefix_units);
        let projected_input_token_count = stable_prefix_units
            .iter()
            .chain(current_turn_input_units.iter())
            .map(|unit| unit.token_atoms.len() as u64)
            .sum();

        Self {
            lookup_anchor_hash: hash_segments(&history_anchor_segments),
            stable_prefix_pages,
            projected_input_token_count,
            stable_prefix_segment_keys,
            history_anchor_segments,
            current_turn_history_segments,
        }
    }

    pub fn build_resume_anchor_hash(&self, assistant_message: &AssistantMessage) -> String {
        let assistant_segments = canonicalize_assistant_message(assistant_message);
        let mut hasher = Sha256::new();
        update_hash_segments(
            &mut hasher,
            self.history_anchor_segments
                .iter()
                .chain(self.current_turn_history_segments.iter())
                .chain(assistant_segments.iter()),
        );
        format!("{:x}", hasher.finalize())
    }

    pub fn stable_prefix_token_count(&self) -> u64 {
        self.stable_prefix_pages
            .iter()
            .map(|page| u64::from(page.token_count))
            .sum()
    }

    pub fn stable_prefix_segment_keys(&self) -> &[String] {
        &self.stable_prefix_segment_keys
    }

    pub fn history_anchor_segments(&self) -> &[String] {
        &self.history_anchor_segments
    }

    pub fn current_turn_history_segments(&self) -> &[String] {
        &self.current_turn_history_segments
    }

    pub fn into_runtime_projection(self) -> RuntimePromptProjection {
        let mut resume_anchor_hasher = Sha256::new();
        update_hash_segments(
            &mut resume_anchor_hasher,
            self.history_anchor_segments
                .iter()
                .chain(self.current_turn_history_segments.iter()),
        );
        RuntimePromptProjection {
            lookup_anchor_hash: self.lookup_anchor_hash,
            stable_prefix_pages: self.stable_prefix_pages,
            projected_input_token_count: self.projected_input_token_count,
            resume_anchor_hasher,
        }
    }
}

#[derive(Clone)]
pub struct RuntimePromptProjection {
    lookup_anchor_hash: String,
    stable_prefix_pages: Vec<CanonicalTokenPage>,
    projected_input_token_count: u64,
    resume_anchor_hasher: Sha256,
}

impl RuntimePromptProjection {
    pub fn from_conversation_state(state: &ConversationState) -> Self {
        build_runtime_prompt_projection(state)
    }

    pub fn lookup_anchor_hash(&self) -> &str {
        &self.lookup_anchor_hash
    }

    pub fn stable_prefix_pages(&self) -> &[CanonicalTokenPage] {
        &self.stable_prefix_pages
    }

    pub fn projected_input_token_count(&self) -> u64 {
        self.projected_input_token_count
    }

    pub fn build_resume_anchor_hash(&self, assistant_message: &AssistantMessage) -> String {
        let mut hasher = self.resume_anchor_hasher.clone();
        let assistant_segments = canonicalize_assistant_message(assistant_message);
        update_hash_segments(&mut hasher, assistant_segments.iter());
        format!("{:x}", hasher.finalize())
    }
}

fn canonicalize_history(history: &[Message]) -> Vec<CanonicalInputUnit> {
    let mut units = Vec::new();
    for message in history {
        match message {
            Message::User(message) => {
                units
                    .extend(canonicalize_user_message("history_user", &message.user_input_message));
            },
            Message::Assistant(message) => units.extend(canonicalize_assistant_segments(
                "history_assistant",
                &message.assistant_response_message,
            )),
        }
    }
    units
}

fn build_runtime_prompt_projection(state: &ConversationState) -> RuntimePromptProjection {
    let mut builder = RuntimePromptProjectionBuilder::new();

    for message in &state.history {
        match message {
            Message::User(message) => {
                builder.add_history_units(canonicalize_user_message(
                    "history_user",
                    &message.user_input_message,
                ));
            },
            Message::Assistant(message) => {
                builder.add_history_units(canonicalize_assistant_segments(
                    "history_assistant",
                    &message.assistant_response_message,
                ));
            },
        }
    }

    builder.add_stable_units(canonicalize_tools(
        &state
            .current_message
            .user_input_message
            .user_input_message_context
            .tools,
    ));
    builder.add_current_input_units(canonicalize_current_turn_for_input(
        &state.current_message.user_input_message,
    ));
    builder.add_current_history_units(canonicalize_current_turn_as_history(
        &state.current_message.user_input_message,
    ));

    builder.finish()
}

struct RuntimePromptProjectionBuilder {
    lookup_anchor_hasher: Sha256,
    resume_anchor_hasher: Sha256,
    stable_prefix_pages: TokenPageBuilder,
    projected_input_token_count: u64,
}

impl RuntimePromptProjectionBuilder {
    fn new() -> Self {
        Self {
            lookup_anchor_hasher: Sha256::new(),
            resume_anchor_hasher: Sha256::new(),
            stable_prefix_pages: TokenPageBuilder::new(),
            projected_input_token_count: 0,
        }
    }

    fn add_history_units(&mut self, units: Vec<CanonicalInputUnit>) {
        for unit in units {
            update_hash_segment(&mut self.lookup_anchor_hasher, &unit.key);
            update_hash_segment(&mut self.resume_anchor_hasher, &unit.key);
            self.add_stable_unit(unit);
        }
    }

    fn add_stable_units(&mut self, units: Vec<CanonicalInputUnit>) {
        for unit in units {
            self.add_stable_unit(unit);
        }
    }

    fn add_current_input_units(&mut self, units: Vec<CanonicalInputUnit>) {
        for unit in units {
            self.projected_input_token_count = self
                .projected_input_token_count
                .saturating_add(unit.token_atoms.len() as u64);
        }
    }

    fn add_current_history_units(&mut self, units: Vec<String>) {
        for unit in units {
            update_hash_segment(&mut self.resume_anchor_hasher, &unit);
        }
    }

    fn add_stable_unit(&mut self, unit: CanonicalInputUnit) {
        self.projected_input_token_count = self
            .projected_input_token_count
            .saturating_add(unit.token_atoms.len() as u64);
        self.stable_prefix_pages.push_atoms(&unit.token_atoms);
    }

    fn finish(self) -> RuntimePromptProjection {
        RuntimePromptProjection {
            lookup_anchor_hash: format!("{:x}", self.lookup_anchor_hasher.finalize()),
            stable_prefix_pages: self.stable_prefix_pages.finish(),
            projected_input_token_count: self.projected_input_token_count,
            resume_anchor_hasher: self.resume_anchor_hasher,
        }
    }
}

struct TokenPageBuilder {
    pages: Vec<CanonicalTokenPage>,
    current: Vec<u64>,
}

impl TokenPageBuilder {
    fn new() -> Self {
        Self {
            pages: Vec::new(),
            current: Vec::with_capacity(PREFIX_CACHE_PAGE_SIZE),
        }
    }

    fn push_atoms(&mut self, atoms: &[u64]) {
        for atom in atoms {
            self.current.push(*atom);
            if self.current.len() == PREFIX_CACHE_PAGE_SIZE {
                self.pages.push(build_token_page(&self.current));
                self.current.clear();
            }
        }
    }

    fn finish(mut self) -> Vec<CanonicalTokenPage> {
        if !self.current.is_empty() {
            self.pages.push(build_token_page(&self.current));
        }
        self.pages
    }
}

fn canonicalize_current_turn_as_history(message: &UserInputMessage) -> Vec<String> {
    canonicalize_user_input_message("history_user", message)
        .into_iter()
        .map(|unit| unit.key)
        .collect()
}

fn canonicalize_current_turn_for_input(message: &UserInputMessage) -> Vec<CanonicalInputUnit> {
    canonicalize_user_input_message("current_user", message)
}

fn canonicalize_user_message(kind_prefix: &str, message: &UserMessage) -> Vec<CanonicalInputUnit> {
    canonicalize_user_message_parts(kind_prefix, UserMessageParts {
        content: &message.content,
        images: &message.images,
        documents: &message.documents,
        context: &message.user_input_message_context,
    })
}

fn canonicalize_user_input_message(
    kind_prefix: &str,
    message: &UserInputMessage,
) -> Vec<CanonicalInputUnit> {
    canonicalize_user_message_parts(kind_prefix, UserMessageParts {
        content: &message.content,
        images: &message.images,
        documents: &message.documents,
        context: &message.user_input_message_context,
    })
}

struct UserMessageParts<'a> {
    content: &'a str,
    images: &'a [KiroImage],
    documents: &'a [KiroDocument],
    context: &'a UserInputMessageContext,
}

fn canonicalize_user_message_parts(
    kind_prefix: &str,
    message: UserMessageParts<'_>,
) -> Vec<CanonicalInputUnit> {
    let mut units = Vec::new();
    let normalized_content = normalize_text(message.content);
    if !normalized_content.is_empty() {
        let key = serialize_canonical_segment(&CanonicalTextSegment {
            kind: format!("{kind_prefix}_text"),
            text: normalized_content.clone(),
        });
        units.push(CanonicalInputUnit {
            key,
            token_atoms: tokenize_text_atoms(&normalized_content),
        });
    }

    for image in message.images {
        let key = serialize_canonical_segment(&CanonicalImageSegment {
            kind: format!("{kind_prefix}_image"),
            format: normalize_text(&image.format),
            digest: sha256_hex(image.source.bytes.as_bytes()),
        });
        units.push(CanonicalInputUnit {
            key,
            token_atoms: Vec::new(),
        });
    }

    for document in message.documents {
        let key = serialize_canonical_segment(&CanonicalDocumentSegment {
            kind: format!("{kind_prefix}_document"),
            name: normalize_text(&document.name),
            format: normalize_text(&document.format),
            digest: sha256_hex(document.source.bytes.as_bytes()),
        });
        units.push(CanonicalInputUnit {
            key,
            token_atoms: Vec::new(),
        });
    }

    for result in &message.context.tool_results {
        let canonical_content = canonical_tool_result_content(&result.content);
        let key = serialize_canonical_segment(&CanonicalToolResultSegment {
            kind: format!("{kind_prefix}_tool_result"),
            tool_use_id: normalize_text(&result.tool_use_id),
            status: result
                .status
                .as_deref()
                .map(normalize_text)
                .unwrap_or_default(),
            is_error: result.is_error,
            content: canonical_content.clone(),
        });
        let token_source = format!(
            "{}\n{}\n{}",
            result.tool_use_id,
            result.status.as_deref().unwrap_or_default(),
            serde_json::to_string(&canonical_content).unwrap_or_default()
        );
        units.push(CanonicalInputUnit {
            key,
            token_atoms: tokenize_text_atoms(&token_source),
        });
    }

    units
}

fn canonicalize_assistant_message(message: &AssistantMessage) -> Vec<String> {
    canonicalize_assistant_segments("history_assistant", message)
        .into_iter()
        .map(|unit| unit.key)
        .collect()
}

fn canonicalize_assistant_segments(
    kind_prefix: &str,
    message: &AssistantMessage,
) -> Vec<CanonicalInputUnit> {
    let mut units = Vec::new();
    let normalized_content = normalize_text(&message.content);
    if !normalized_content.is_empty() {
        let key = serialize_canonical_segment(&CanonicalTextSegment {
            kind: format!("{kind_prefix}_text"),
            text: normalized_content.clone(),
        });
        units.push(CanonicalInputUnit {
            key,
            token_atoms: tokenize_text_atoms(&normalized_content),
        });
    }

    for tool_use in message.tool_uses.as_deref().unwrap_or(&[]) {
        let canonical_input = canonicalize_json(&tool_use.input);
        let key = serialize_canonical_segment(&CanonicalToolUseSegment {
            kind: format!("{kind_prefix}_tool_use"),
            tool_use_id: normalize_text(&tool_use.tool_use_id),
            name: normalize_text(&tool_use.name),
            input: canonical_input.clone(),
        });
        let token_source = format!(
            "{}\n{}\n{}",
            tool_use.tool_use_id,
            tool_use.name,
            serde_json::to_string(&canonical_input).unwrap_or_default()
        );
        units.push(CanonicalInputUnit {
            key,
            token_atoms: tokenize_text_atoms(&token_source),
        });
    }

    units
}

fn canonicalize_tools(tools: &[Tool]) -> Vec<CanonicalInputUnit> {
    let mut units = Vec::with_capacity(tools.len());
    for tool in tools {
        let name = normalize_text(&tool.tool_specification.name);
        let description = normalize_text(&tool.tool_specification.description);
        let canonical_schema = canonicalize_json(&tool.tool_specification.input_schema.json);
        let key = serialize_canonical_segment(&CanonicalToolDefinitionSegment {
            kind: "stable_tool_definition".to_string(),
            name: name.clone(),
            description: description.clone(),
            input_schema: canonical_schema.clone(),
        });
        let token_source = format!(
            "{name}\n{description}\n{}",
            serde_json::to_string(&canonical_schema).unwrap_or_default()
        );
        units.push(CanonicalInputUnit {
            key,
            token_atoms: tokenize_text_atoms(&token_source),
        });
    }
    units
}

fn canonical_tool_result_content(content: &[Map<String, Value>]) -> Value {
    Value::Array(
        content
            .iter()
            .map(|item| canonicalize_json(&Value::Object(item.clone())))
            .collect(),
    )
}

fn build_token_pages(units: &[CanonicalInputUnit]) -> Vec<CanonicalTokenPage> {
    let mut pages = Vec::new();
    let mut current = Vec::<u64>::with_capacity(PREFIX_CACHE_PAGE_SIZE);
    for atom in units
        .iter()
        .flat_map(|unit| unit.token_atoms.iter().copied())
    {
        current.push(atom);
        if current.len() == PREFIX_CACHE_PAGE_SIZE {
            pages.push(build_token_page(&current));
            current.clear();
        }
    }
    if !current.is_empty() {
        pages.push(build_token_page(&current));
    }
    pages
}

// A page key is the hash of the packed token atom stream. The tree stores only
// this compact page identity plus token count; it does not retain the original
// strings or token vectors per node.
fn build_token_page(atoms: &[u64]) -> CanonicalTokenPage {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(atoms));
    for atom in atoms {
        bytes.extend_from_slice(&atom.to_le_bytes());
    }
    CanonicalTokenPage {
        key: xxh3_128(&bytes),
        token_count: u16::try_from(atoms.len()).expect("page token count should fit in u16"),
    }
}

fn tokenize_text_atoms(text: &str) -> Vec<u64> {
    let mut atoms = Vec::new();
    let mut ascii_word_start = None::<usize>;
    let mut ascii_word_end = 0usize;

    for (index, ch) in text.char_indices() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            if ascii_word_start.is_none() {
                ascii_word_start = Some(index);
            }
            ascii_word_end = index + ch.len_utf8();
            continue;
        }

        if let Some(start) = ascii_word_start.take() {
            atoms.push(hash_token_atom(&text[start..ascii_word_end]));
        }

        if ch.is_whitespace() {
            continue;
        }

        let end = index + ch.len_utf8();
        atoms.push(hash_token_atom(&text[index..end]));
    }

    if let Some(start) = ascii_word_start {
        atoms.push(hash_token_atom(&text[start..ascii_word_end]));
    }

    if atoms.is_empty() && !text.is_empty() {
        atoms.push(hash_token_atom(text));
    }
    atoms
}

fn hash_token_atom(text: &str) -> u64 {
    xxh3_64(text.as_bytes())
}

fn normalize_text(raw: &str) -> String {
    raw.replace("\r\n", "\n").trim().to_string()
}

fn canonicalize_json(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(canonicalize_json).collect()),
        Value::Object(map) => {
            let sorted = map
                .iter()
                .map(|(key, value)| (key.clone(), canonicalize_json(value)))
                .collect::<BTreeMap<_, _>>();
            let mut normalized = Map::new();
            for (key, value) in sorted {
                normalized.insert(key, value);
            }
            Value::Object(normalized)
        },
        _ => value.clone(),
    }
}

fn hash_segments(segments: &[String]) -> String {
    let mut hasher = Sha256::new();
    update_hash_segments(&mut hasher, segments.iter());
    format!("{:x}", hasher.finalize())
}

fn update_hash_segments<'a>(hasher: &mut Sha256, segments: impl IntoIterator<Item = &'a String>) {
    for segment in segments {
        update_hash_segment(hasher, segment);
    }
}

fn update_hash_segment(hasher: &mut Sha256, segment: &str) {
    let len = segment.len() as u64;
    hasher.update(len.to_le_bytes());
    hasher.update(segment.as_bytes());
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn serialize_canonical_segment<T: Serialize>(segment: &T) -> String {
    serde_json::to_string(segment).expect("canonical segments should serialize")
}

#[derive(Serialize)]
struct CanonicalTextSegment {
    kind: String,
    text: String,
}

#[derive(Serialize)]
struct CanonicalImageSegment {
    kind: String,
    format: String,
    digest: String,
}

#[derive(Serialize)]
struct CanonicalDocumentSegment {
    kind: String,
    name: String,
    format: String,
    digest: String,
}

#[derive(Serialize)]
struct CanonicalToolResultSegment {
    kind: String,
    tool_use_id: String,
    status: String,
    is_error: bool,
    content: Value,
}

#[derive(Serialize)]
struct CanonicalToolUseSegment {
    kind: String,
    tool_use_id: String,
    name: String,
    input: Value,
}

#[derive(Serialize)]
struct CanonicalToolDefinitionSegment {
    kind: String,
    name: String,
    description: String,
    input_schema: Value,
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::{hash_segments, PromptProjection, RuntimePromptProjection};
    use crate::wire::{
        AssistantMessage, ConversationState, CurrentMessage, HistoryAssistantMessage,
        HistoryUserMessage, InputSchema, Message, Tool, ToolResult, ToolSpecification,
        ToolUseEntry, UserInputMessage, UserInputMessageContext,
    };

    fn tool(name: &str, description: &str, schema: Value) -> Tool {
        Tool {
            tool_specification: ToolSpecification {
                name: name.to_string(),
                description: description.to_string(),
                input_schema: InputSchema::from_json(schema),
            },
        }
    }

    fn history_user(content: &str) -> Message {
        Message::User(HistoryUserMessage::new(content, "ignored-model"))
    }

    fn history_assistant(content: &str) -> Message {
        Message::Assistant(HistoryAssistantMessage::new(content))
    }

    #[test]
    fn prompt_projection_excludes_current_turn_from_lookup_anchor() {
        let state = ConversationState::new("conv-1")
            .with_history(vec![history_user("previous user"), history_assistant("previous answer")])
            .with_current_message(CurrentMessage::new(UserInputMessage::new(
                "new current turn",
                "ignored-model",
            )));

        let projection = PromptProjection::from_conversation_state(&state);
        let resume_anchor =
            projection.build_resume_anchor_hash(&AssistantMessage::new("assistant next"));

        assert_eq!(
            projection.lookup_anchor_hash,
            hash_segments(&projection.history_anchor_segments)
        );
        assert!(projection
            .history_anchor_segments
            .iter()
            .all(|segment| !segment.contains("new current turn")));
        assert_ne!(projection.lookup_anchor_hash, resume_anchor);
    }

    #[test]
    fn prompt_projection_excludes_current_tool_results_from_stable_prefix() {
        let current = UserInputMessage::new("continue", "ignored-model").with_context(
            UserInputMessageContext::new()
                .with_tool_results(vec![ToolResult::success("current-tool", "current result")])
                .with_tools(vec![tool(
                    "search_files",
                    "Search files",
                    json!({"type":"object","properties":{"query":{"type":"string"}}}),
                )]),
        );
        let state = ConversationState::new("conv-1")
            .with_history(vec![history_user("existing history")])
            .with_current_message(CurrentMessage::new(current));

        let projection = PromptProjection::from_conversation_state(&state);
        let stable_prefix = projection
            .stable_prefix_segment_keys
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!stable_prefix.contains("current-tool"));
        assert!(!stable_prefix.contains("current result"));
        assert!(stable_prefix.contains("search_files"));
    }

    #[test]
    fn prompt_projection_is_stable_for_equivalent_history() {
        let left = ConversationState::new("left")
            .with_history(vec![history_user("  hello world\r\n"), history_assistant("done  ")])
            .with_current_message(CurrentMessage::new(
                UserInputMessage::new("current", "ignored-model").with_context(
                    UserInputMessageContext::new().with_tools(vec![tool(
                        "inspect_project",
                        " Inspect project ",
                        json!({
                            "properties": {
                                "path": {"type":"string"},
                                "recursive": {"type":"boolean"}
                            },
                            "type":"object"
                        }),
                    )]),
                ),
            ));
        let right = ConversationState::new("right")
            .with_history(vec![history_user("hello world"), history_assistant("done")])
            .with_current_message(CurrentMessage::new(
                UserInputMessage::new("different current", "ignored-model").with_context(
                    UserInputMessageContext::new().with_tools(vec![tool(
                        "inspect_project",
                        "Inspect project",
                        json!({
                            "type":"object",
                            "properties": {
                                "recursive": {"type":"boolean"},
                                "path": {"type":"string"}
                            }
                        }),
                    )]),
                ),
            ));

        let left_projection = PromptProjection::from_conversation_state(&left);
        let right_projection = PromptProjection::from_conversation_state(&right);

        assert_eq!(left_projection.lookup_anchor_hash, right_projection.lookup_anchor_hash);
        assert_eq!(left_projection.stable_prefix_pages, right_projection.stable_prefix_pages);
        assert_ne!(
            left_projection.projected_input_token_count,
            right_projection.projected_input_token_count
        );
    }

    #[test]
    fn prompt_projection_resume_anchor_ignores_current_tool_definitions() {
        let base_history = vec![history_user("existing history")];
        let current_a = UserInputMessage::new("continue", "ignored-model").with_context(
            UserInputMessageContext::new().with_tools(vec![tool(
                "search_files",
                "Search files",
                json!({"type":"object","properties":{"query":{"type":"string"}}}),
            )]),
        );
        let current_b = UserInputMessage::new("continue", "ignored-model").with_context(
            UserInputMessageContext::new().with_tools(vec![tool(
                "read_file",
                "Read file",
                json!({"type":"object","properties":{"path":{"type":"string"}}}),
            )]),
        );
        let state_a = ConversationState::new("conv-a")
            .with_history(base_history.clone())
            .with_current_message(CurrentMessage::new(current_a));
        let state_b = ConversationState::new("conv-b")
            .with_history(base_history)
            .with_current_message(CurrentMessage::new(current_b));

        let projection_a = PromptProjection::from_conversation_state(&state_a);
        let projection_b = PromptProjection::from_conversation_state(&state_b);
        let assistant = AssistantMessage::new("assistant reply")
            .with_tool_uses(vec![ToolUseEntry::new("tool-1", "search_files")]);

        assert_eq!(
            projection_a.build_resume_anchor_hash(&assistant),
            projection_b.build_resume_anchor_hash(&assistant)
        );
        assert_ne!(
            projection_a.stable_prefix_segment_keys,
            projection_b.stable_prefix_segment_keys
        );
    }

    #[test]
    fn runtime_prompt_projection_preserves_matching_and_resume_hashes() {
        let state = ConversationState::new("conv-runtime")
            .with_history(vec![history_user("existing history"), history_assistant("done")])
            .with_current_message(CurrentMessage::new(
                UserInputMessage::new("continue", "ignored-model").with_context(
                    UserInputMessageContext::new().with_tools(vec![tool(
                        "search_files",
                        "Search files",
                        json!({"type":"object","properties":{"query":{"type":"string"}}}),
                    )]),
                ),
            ));
        let projection = PromptProjection::from_conversation_state(&state);
        let assistant = AssistantMessage::new("assistant reply")
            .with_tool_uses(vec![ToolUseEntry::new("tool-1", "search_files")]);
        let expected_resume_anchor = projection.build_resume_anchor_hash(&assistant);
        let expected_lookup_anchor = projection.lookup_anchor_hash.clone();
        let expected_pages = projection.stable_prefix_pages.clone();
        let expected_projected_tokens = projection.projected_input_token_count;

        let runtime_projection = RuntimePromptProjection::from_conversation_state(&state);

        assert_eq!(runtime_projection.lookup_anchor_hash(), expected_lookup_anchor);
        assert_eq!(runtime_projection.stable_prefix_pages(), expected_pages);
        assert_eq!(runtime_projection.projected_input_token_count(), expected_projected_tokens);
        assert_eq!(runtime_projection.build_resume_anchor_hash(&assistant), expected_resume_anchor);
    }
}
