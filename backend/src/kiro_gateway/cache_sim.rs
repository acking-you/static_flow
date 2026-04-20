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

use std::{
    collections::{BTreeMap, HashMap},
    num::NonZeroUsize,
    time::{Duration, Instant},
};

use charabia::Tokenize;
use lru::LruCache;
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use xxhash_rust::xxh3::{xxh3_128, xxh3_64};

use super::wire::{
    AssistantMessage, ConversationState, Message, Tool, UserInputMessage, UserMessage,
};
use crate::state::LlmGatewayRuntimeConfig;

const PREFIX_CACHE_PAGE_SIZE: usize = 64;
const CLAUDE_CODE_BILLING_HEADER_PREFIX: &str = "x-anthropic-billing-header:";
const CLAUDE_CODE_CLI_SYSTEM_IDENTITY_LINE: &str =
    "You are Claude Code, Anthropic's official CLI for Claude.";
const CLAUDE_AGENT_SDK_SYSTEM_IDENTITY_LINE: &str =
    "You are a Claude agent, built on Anthropic's Claude Agent SDK.";
const STABLE_THINKING_PREFIX_TAGS: [(&str, &str); 3] = [
    ("<thinking_mode>", "</thinking_mode>"),
    ("<thinking_effort>", "</thinking_effort>"),
    ("<max_thinking_length>", "</max_thinking_length>"),
];

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
pub(crate) struct CanonicalTokenPage {
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
pub(crate) struct PromptProjection {
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
        let mut segments = Vec::with_capacity(
            self.history_anchor_segments.len() + self.current_turn_history_segments.len() + 4,
        );
        segments.extend(self.history_anchor_segments.iter().cloned());
        segments.extend(self.current_turn_history_segments.iter().cloned());
        segments.extend(canonicalize_assistant_message(assistant_message));
        hash_segments(&segments)
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KiroCacheSimulationMode {
    Formula,
    PrefixTree,
}

impl KiroCacheSimulationMode {
    pub fn from_runtime_value(value: &str) -> Self {
        match value {
            "prefix_tree" => Self::PrefixTree,
            _ => Self::Formula,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct KiroCacheSimulationConfig {
    pub mode: KiroCacheSimulationMode,
    pub prefix_cache_max_tokens: u64,
    pub prefix_cache_entry_ttl: Duration,
    pub conversation_anchor_max_entries: usize,
    pub conversation_anchor_ttl: Duration,
}

impl From<&LlmGatewayRuntimeConfig> for KiroCacheSimulationConfig {
    fn from(value: &LlmGatewayRuntimeConfig) -> Self {
        Self {
            mode: KiroCacheSimulationMode::from_runtime_value(&value.kiro_prefix_cache_mode),
            prefix_cache_max_tokens: value.kiro_prefix_cache_max_tokens,
            prefix_cache_entry_ttl: Duration::from_secs(value.kiro_prefix_cache_entry_ttl_seconds),
            conversation_anchor_max_entries: usize::try_from(
                value.kiro_conversation_anchor_max_entries,
            )
            .unwrap_or(usize::MAX),
            conversation_anchor_ttl: Duration::from_secs(
                value.kiro_conversation_anchor_ttl_seconds,
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct PrefixCacheMatch {
    pub matched_pages: usize,
    pub matched_tokens: u64,
}

#[derive(Default)]
pub(crate) struct KiroCacheSimulator {
    prefix_tree: parking_lot::Mutex<PrefixTree>,
    anchor_index: parking_lot::Mutex<ConversationAnchorIndex>,
}

impl KiroCacheSimulator {
    // Match against the global shared prefix tree. The caller is expected to
    // provide a prompt projection built from the corrected `ConversationState`,
    // not the raw client JSON, so cache simulation follows the actual upstream
    // request shape.
    pub fn match_prefix(
        &self,
        projection: &PromptProjection,
        config: KiroCacheSimulationConfig,
        now: Instant,
    ) -> PrefixCacheMatch {
        if matches!(config.mode, KiroCacheSimulationMode::Formula) {
            return PrefixCacheMatch::default();
        }
        let mut tree = self.prefix_tree.lock();
        tree.match_prefix(&projection.stable_prefix_pages, now, config.prefix_cache_entry_ttl)
    }

    pub fn recover_conversation_id(
        &self,
        projection: &PromptProjection,
        config: KiroCacheSimulationConfig,
        now: Instant,
    ) -> Option<String> {
        let mut index = self.anchor_index.lock();
        index.get(
            &projection.lookup_anchor_hash,
            now,
            config.conversation_anchor_ttl,
            config.conversation_anchor_max_entries,
        )
    }

    pub fn record_success(
        &self,
        projection: &PromptProjection,
        assistant_message: &AssistantMessage,
        conversation_id: &str,
        record_prefix_tree: bool,
        config: KiroCacheSimulationConfig,
        now: Instant,
    ) {
        if record_prefix_tree && matches!(config.mode, KiroCacheSimulationMode::PrefixTree) {
            let mut tree = self.prefix_tree.lock();
            tree.insert(
                &projection.stable_prefix_pages,
                now,
                config.prefix_cache_entry_ttl,
                config.prefix_cache_max_tokens,
            );
        }
        let resume_anchor_hash = projection.build_resume_anchor_hash(assistant_message);
        let mut index = self.anchor_index.lock();
        index.insert(
            resume_anchor_hash,
            conversation_id.to_string(),
            now,
            config.conversation_anchor_ttl,
            config.conversation_anchor_max_entries,
        );
    }
}

#[derive(Debug, Default)]
struct PrefixTree {
    root: PrefixNode,
    resident_tokens: u64,
}

#[derive(Debug)]
struct PrefixNode {
    token_count: u64,
    last_touched_at: Instant,
    children: HashMap<u128, PrefixNode>,
}

impl Default for PrefixNode {
    fn default() -> Self {
        Self {
            token_count: 0,
            last_touched_at: Instant::now(),
            children: HashMap::new(),
        }
    }
}

impl Drop for PrefixNode {
    fn drop(&mut self) {
        // Deep prefix paths can legitimately reach tens of thousands of pages.
        // Clearing children iteratively prevents subtree destruction from
        // recursing on the thread stack when a large branch is evicted.
        let mut stack = std::mem::take(&mut self.children)
            .into_values()
            .collect::<Vec<_>>();
        while let Some(mut child) = stack.pop() {
            stack.extend(std::mem::take(&mut child.children).into_values());
        }
    }
}

impl PrefixNode {
    fn new(token_count: u64, now: Instant) -> Self {
        Self {
            token_count,
            last_touched_at: now,
            children: HashMap::new(),
        }
    }
}

impl PrefixTree {
    // Matching only counts full pages. Partial-page matches are ignored on
    // purpose so the reported cache hit stays conservative.
    fn match_prefix(
        &mut self,
        pages: &[CanonicalTokenPage],
        now: Instant,
        ttl: Duration,
    ) -> PrefixCacheMatch {
        self.prune_expired(now, ttl);
        let mut current = &mut self.root;
        let mut matched = PrefixCacheMatch::default();
        for page in pages {
            let Some(child) = current.children.get_mut(&page.key) else {
                break;
            };
            child.last_touched_at = now;
            matched.matched_pages += 1;
            matched.matched_tokens += child.token_count;
            current = child;
        }
        matched
    }

    fn insert(
        &mut self,
        pages: &[CanonicalTokenPage],
        now: Instant,
        ttl: Duration,
        max_tokens: u64,
    ) {
        self.prune_expired(now, ttl);
        let added_tokens = insert_prefix_path(&mut self.root, pages, now);
        self.resident_tokens = self.resident_tokens.saturating_add(added_tokens);
        while self.resident_tokens > max_tokens {
            let Some(path) = find_coldest_leaf_path(&self.root) else {
                break;
            };
            let removed = remove_leaf_path(&mut self.root, &path);
            if removed == 0 {
                break;
            }
            self.resident_tokens = self.resident_tokens.saturating_sub(removed);
        }
    }

    fn prune_expired(&mut self, now: Instant, ttl: Duration) {
        let removed = prune_expired_children(&mut self.root, now, ttl);
        self.resident_tokens = self.resident_tokens.saturating_sub(removed);
    }
}

#[derive(Debug)]
struct ConversationAnchorEntry {
    conversation_id: String,
    last_touched_at: Instant,
}

#[derive(Debug, Default)]
struct ConversationAnchorIndex {
    cache: Option<LruCache<String, ConversationAnchorEntry>>,
}

impl ConversationAnchorIndex {
    fn get(
        &mut self,
        anchor: &str,
        now: Instant,
        ttl: Duration,
        max_entries: usize,
    ) -> Option<String> {
        self.ensure_capacity(max_entries);
        let expired = self
            .cache
            .as_mut()
            .and_then(|cache| cache.peek(anchor))
            .is_some_and(|entry| now.duration_since(entry.last_touched_at) > ttl);
        if expired {
            if let Some(cache) = self.cache.as_mut() {
                cache.pop(anchor);
            }
            return None;
        }
        let cache = self.cache.as_mut()?;
        let entry = cache.get_mut(anchor)?;
        entry.last_touched_at = now;
        Some(entry.conversation_id.clone())
    }

    fn insert(
        &mut self,
        anchor: String,
        conversation_id: String,
        now: Instant,
        ttl: Duration,
        max_entries: usize,
    ) {
        self.ensure_capacity(max_entries);
        self.remove_expired(now, ttl);
        if let Some(cache) = self.cache.as_mut() {
            cache.put(anchor, ConversationAnchorEntry {
                conversation_id,
                last_touched_at: now,
            });
        }
    }

    fn ensure_capacity(&mut self, max_entries: usize) {
        let capacity = NonZeroUsize::new(max_entries.max(1)).expect("max_entries is positive");
        match self.cache.as_mut() {
            Some(cache) if cache.cap() == capacity => {},
            Some(cache) => {
                let mut replacement = LruCache::new(capacity);
                while let Some((key, value)) = cache.pop_lru() {
                    replacement.put(key, value);
                }
                self.cache = Some(replacement);
            },
            None => self.cache = Some(LruCache::new(capacity)),
        }
    }

    fn remove_expired(&mut self, now: Instant, ttl: Duration) {
        let Some(cache) = self.cache.as_mut() else {
            return;
        };
        while cache
            .peek_lru()
            .is_some_and(|(_, entry)| now.duration_since(entry.last_touched_at) > ttl)
        {
            let _ = cache.pop_lru();
        }
    }
}

fn insert_prefix_path(node: &mut PrefixNode, pages: &[CanonicalTokenPage], now: Instant) -> u64 {
    let mut added_tokens: u64 = 0;
    let mut current = node;
    for page in pages {
        let child = current.children.entry(page.key).or_insert_with(|| {
            added_tokens = added_tokens.saturating_add(u64::from(page.token_count));
            PrefixNode::new(u64::from(page.token_count), now)
        });
        child.last_touched_at = now;
        current = child;
    }
    added_tokens
}

fn prune_expired_children(node: &mut PrefixNode, now: Instant, ttl: Duration) -> u64 {
    let mut removed_tokens: u64 = 0;
    let mut stack = vec![(node as *mut PrefixNode, false)];

    // We use an explicit DFS stack so prefix paths with tens of thousands of
    // pages never recurse on the thread stack. The raw pointers all originate
    // from the unique mutable borrow of `node`, and a node is only removed
    // after its children have already been processed.
    // SAFETY: every pointer in `stack` comes from the unique mutable borrow of
    // `node`. A node is only detached from its parent after all of its
    // descendants have already been processed, so no queued pointer can dangle.
    unsafe {
        while let Some((node_ptr, visited_children)) = stack.pop() {
            let current = &mut *node_ptr;
            if !visited_children {
                stack.push((node_ptr, true));
                for child in current.children.values_mut() {
                    stack.push((child as *mut PrefixNode, false));
                }
                continue;
            }

            let expired_keys = current
                .children
                .iter()
                .filter(|(_, child)| now.duration_since(child.last_touched_at) > ttl)
                .map(|(key, _)| *key)
                .collect::<Vec<_>>();
            for key in expired_keys {
                if let Some(child) = current.children.remove(&key) {
                    removed_tokens = removed_tokens.saturating_add(subtree_token_count(&child));
                }
            }
        }
    }

    removed_tokens
}

fn subtree_token_count(node: &PrefixNode) -> u64 {
    let mut total: u64 = 0;
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        total = total.saturating_add(current.token_count);
        stack.extend(current.children.values());
    }
    total
}

fn find_coldest_leaf_path(node: &PrefixNode) -> Option<Vec<u128>> {
    struct Frame<'a> {
        node: &'a PrefixNode,
        child_keys: Vec<u128>,
        next_child: usize,
    }

    let mut best: Option<(Instant, Vec<u128>)> = None;
    let mut path = Vec::<u128>::new();
    let mut stack = vec![Frame {
        node,
        child_keys: node.children.keys().copied().collect(),
        next_child: 0,
    }];

    while let Some(frame) = stack.last_mut() {
        if frame.child_keys.is_empty() {
            if !path.is_empty() {
                match &best {
                    Some((current_oldest, _)) if frame.node.last_touched_at >= *current_oldest => {
                    },
                    _ => best = Some((frame.node.last_touched_at, path.clone())),
                }
            }
            stack.pop();
            if !path.is_empty() {
                path.pop();
            }
            continue;
        }

        if frame.next_child >= frame.child_keys.len() {
            stack.pop();
            if !path.is_empty() {
                path.pop();
            }
            continue;
        }

        let page_key = frame.child_keys[frame.next_child];
        frame.next_child += 1;
        let child = frame
            .node
            .children
            .get(&page_key)
            .expect("frame child key should resolve");
        path.push(page_key);
        stack.push(Frame {
            node: child,
            child_keys: child.children.keys().copied().collect(),
            next_child: 0,
        });
    }

    best.map(|(_, path)| path)
}

fn remove_leaf_path(node: &mut PrefixNode, path: &[u128]) -> u64 {
    if path.is_empty() {
        return 0;
    }

    let mut lineage = Vec::with_capacity(path.len());
    let mut current_ptr = node as *mut PrefixNode;

    // The lineage stores each parent pointer plus the child key used to descend
    // one level. This lets us prune empty ancestors iteratively on the way back
    // up without recursive calls.
    // SAFETY: `lineage` stores parent pointers discovered by walking the tree
    // from the exclusive mutable root borrow. We only remove descendants while
    // walking back up that exact lineage, so each pointer remains valid until
    // the moment its corresponding child entry is removed.
    unsafe {
        for key in path {
            let current = &mut *current_ptr;
            let Some(child) = current.children.get_mut(key) else {
                return 0;
            };
            lineage.push((current_ptr, *key));
            current_ptr = child as *mut PrefixNode;
        }

        let removed_subtree_tokens = subtree_token_count(&*current_ptr);
        if removed_subtree_tokens == 0 {
            return 0;
        }

        let (leaf_parent_ptr, leaf_key) = *lineage
            .last()
            .expect("non-empty path should always record one lineage entry");
        let leaf_parent = &mut *leaf_parent_ptr;
        if leaf_parent.children.remove(&leaf_key).is_none() {
            return 0;
        }

        let mut removed_tokens = removed_subtree_tokens;
        for &(parent_ptr, child_key) in lineage[..lineage.len().saturating_sub(1)].iter().rev() {
            let parent = &mut *parent_ptr;
            let Some(child) = parent.children.get(&child_key) else {
                break;
            };
            if !child.children.is_empty() {
                break;
            }
            let ancestor_token_count = child.token_count;
            if parent.children.remove(&child_key).is_none() {
                break;
            }
            removed_tokens = removed_tokens.saturating_add(ancestor_token_count);
        }

        removed_tokens
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

fn canonicalize_current_turn_as_history(message: &UserInputMessage) -> Vec<String> {
    canonicalize_user_message("history_user", &UserMessage {
        content: message.content.clone(),
        images: message.images.clone(),
        user_input_message_context: message.user_input_message_context.clone(),
        model_id: message.model_id.clone(),
        origin: message.origin.clone(),
    })
    .into_iter()
    .map(|unit| unit.key)
    .collect()
}

fn canonicalize_current_turn_for_input(message: &UserInputMessage) -> Vec<CanonicalInputUnit> {
    canonicalize_user_message("current_user", &UserMessage {
        content: message.content.clone(),
        images: message.images.clone(),
        user_input_message_context: message.user_input_message_context.clone(),
        model_id: message.model_id.clone(),
        origin: message.origin.clone(),
    })
}

fn canonicalize_user_message(kind_prefix: &str, message: &UserMessage) -> Vec<CanonicalInputUnit> {
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

    for image in &message.images {
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

    for result in &message.user_input_message_context.tool_results {
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
    for token in text.tokenize() {
        // Use the original token surface instead of the normalized lemma so
        // prefix hits never over-merge distinct prompts that only share a
        // language-level normalization.
        let surface = &text[token.byte_start..token.byte_end];
        if surface.is_empty() {
            continue;
        }
        atoms.push(hash_token_atom(surface));
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
    let normalized = raw.replace("\r\n", "\n").trim().to_string();
    strip_volatile_claude_code_billing_header(&normalized)
}

fn strip_volatile_claude_code_billing_header(text: &str) -> String {
    let Some(billing_header_pos) = text.find(CLAUDE_CODE_BILLING_HEADER_PREFIX) else {
        return text.to_string();
    };
    let stable_prefix = &text[..billing_header_pos];
    if !stable_prefix.is_empty() && !contains_only_stable_thinking_prefix(stable_prefix) {
        return text.to_string();
    }
    let Some((_, remainder)) = text[billing_header_pos..].split_once('\n') else {
        return text.to_string();
    };
    let trimmed_remainder = remainder.trim_start();
    if !trimmed_remainder.starts_with(CLAUDE_CODE_CLI_SYSTEM_IDENTITY_LINE)
        && !trimmed_remainder.starts_with(CLAUDE_AGENT_SDK_SYSTEM_IDENTITY_LINE)
    {
        return text.to_string();
    }
    let normalized_remainder = trimmed_remainder.trim();
    if stable_prefix.is_empty() {
        normalized_remainder.to_string()
    } else if stable_prefix.ends_with('\n') {
        format!("{stable_prefix}{normalized_remainder}")
    } else {
        format!("{stable_prefix}\n{normalized_remainder}")
    }
}

fn contains_only_stable_thinking_prefix(prefix: &str) -> bool {
    let mut remaining = prefix;
    loop {
        let trimmed = remaining.trim_start();
        if trimmed.is_empty() {
            return true;
        }
        remaining = trimmed;

        let mut matched_tag = false;
        for (start_tag, end_tag) in STABLE_THINKING_PREFIX_TAGS {
            let Some(after_start) = remaining.strip_prefix(start_tag) else {
                continue;
            };
            let Some(end_pos) = after_start.find(end_tag) else {
                return false;
            };
            remaining = &after_start[end_pos + end_tag.len()..];
            matched_tag = true;
            break;
        }
        if !matched_tag {
            return false;
        }
    }
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
    for segment in segments {
        let len = segment.len() as u64;
        hasher.update(len.to_le_bytes());
        hasher.update(segment.as_bytes());
    }
    format!("{:x}", hasher.finalize())
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
    use serde_json::json;

    use super::*;
    use crate::kiro_gateway::wire::{
        CurrentMessage, HistoryAssistantMessage, HistoryUserMessage, InputSchema, Tool, ToolResult,
        ToolSpecification, ToolUseEntry, UserInputMessage, UserInputMessageContext,
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
    fn cache_simulator_matches_when_claude_code_billing_header_hash_changes() {
        let first_header = concat!(
            "x-anthropic-billing-header: cc_version=2.1.114.069; ",
            "cc_entrypoint=cli; cch=638d8;\n",
            "You are Claude Code, Anthropic's official CLI for Claude.\n",
            "You are an interactive agent that helps users with software engineering tasks."
        );
        let second_header = concat!(
            "x-anthropic-billing-header: cc_version=2.1.114.069; ",
            "cc_entrypoint=cli; cch=f5339;\n",
            "You are Claude Code, Anthropic's official CLI for Claude.\n",
            "You are an interactive agent that helps users with software engineering tasks."
        );
        let assistant = AssistantMessage::new("assistant reply");
        let simulator = KiroCacheSimulator::default();
        let config = KiroCacheSimulationConfig {
            mode: KiroCacheSimulationMode::PrefixTree,
            prefix_cache_max_tokens: 100_000,
            prefix_cache_entry_ttl: Duration::from_secs(300),
            conversation_anchor_max_entries: 32,
            conversation_anchor_ttl: Duration::from_secs(300),
        };
        let now = Instant::now();
        let first_state = ConversationState::new("conv-1")
            .with_history(vec![history_user(first_header), history_assistant("done")])
            .with_current_message(CurrentMessage::new(
                UserInputMessage::new("continue", "ignored-model").with_context(
                    UserInputMessageContext::new().with_tools(vec![tool(
                        "search_files",
                        "Search files",
                        json!({"type":"object","properties":{"query":{"type":"string"}}}),
                    )]),
                ),
            ));
        let second_state = ConversationState::new("conv-1")
            .with_history(vec![history_user(second_header), history_assistant("done")])
            .with_current_message(CurrentMessage::new(
                UserInputMessage::new("continue", "ignored-model").with_context(
                    UserInputMessageContext::new().with_tools(vec![tool(
                        "search_files",
                        "Search files",
                        json!({"type":"object","properties":{"query":{"type":"string"}}}),
                    )]),
                ),
            ));

        let first_projection = PromptProjection::from_conversation_state(&first_state);
        simulator.record_success(&first_projection, &assistant, "real-conv", true, config, now);

        let second_projection = PromptProjection::from_conversation_state(&second_state);
        let matched =
            simulator.match_prefix(&second_projection, config, now + Duration::from_secs(1));

        assert_eq!(first_projection.lookup_anchor_hash, second_projection.lookup_anchor_hash);
        assert_eq!(first_projection.stable_prefix_pages, second_projection.stable_prefix_pages);
        assert_eq!(matched.matched_pages, second_projection.stable_prefix_pages.len());
        assert!(matched.matched_tokens > 0);
    }

    #[test]
    fn cache_simulator_matches_when_agent_sdk_billing_header_hash_changes() {
        let first_header = concat!(
            "x-anthropic-billing-header: cc_version=2.1.114.eee; ",
            "cc_entrypoint=sdk-cli; cch=fb0be;\n",
            "You are a Claude agent, built on Anthropic's Claude Agent SDK.\n",
            "You are an interactive agent that helps users with software engineering tasks."
        );
        let second_header = concat!(
            "x-anthropic-billing-header: cc_version=2.1.114.eee; ",
            "cc_entrypoint=sdk-cli; cch=9ac44;\n",
            "You are a Claude agent, built on Anthropic's Claude Agent SDK.\n",
            "You are an interactive agent that helps users with software engineering tasks."
        );
        let assistant = AssistantMessage::new("assistant reply");
        let simulator = KiroCacheSimulator::default();
        let config = KiroCacheSimulationConfig {
            mode: KiroCacheSimulationMode::PrefixTree,
            prefix_cache_max_tokens: 100_000,
            prefix_cache_entry_ttl: Duration::from_secs(300),
            conversation_anchor_max_entries: 32,
            conversation_anchor_ttl: Duration::from_secs(300),
        };
        let now = Instant::now();
        let first_state = ConversationState::new("conv-1")
            .with_history(vec![history_user(first_header), history_assistant("done")])
            .with_current_message(CurrentMessage::new(
                UserInputMessage::new("continue", "ignored-model").with_context(
                    UserInputMessageContext::new().with_tools(vec![tool(
                        "search_files",
                        "Search files",
                        json!({"type":"object","properties":{"query":{"type":"string"}}}),
                    )]),
                ),
            ));
        let second_state = ConversationState::new("conv-1")
            .with_history(vec![history_user(second_header), history_assistant("done")])
            .with_current_message(CurrentMessage::new(
                UserInputMessage::new("continue", "ignored-model").with_context(
                    UserInputMessageContext::new().with_tools(vec![tool(
                        "search_files",
                        "Search files",
                        json!({"type":"object","properties":{"query":{"type":"string"}}}),
                    )]),
                ),
            ));

        let first_projection = PromptProjection::from_conversation_state(&first_state);
        simulator.record_success(&first_projection, &assistant, "real-conv", true, config, now);

        let second_projection = PromptProjection::from_conversation_state(&second_state);
        let matched =
            simulator.match_prefix(&second_projection, config, now + Duration::from_secs(1));

        assert_eq!(first_projection.lookup_anchor_hash, second_projection.lookup_anchor_hash);
        assert_eq!(first_projection.stable_prefix_pages, second_projection.stable_prefix_pages);
        assert_eq!(matched.matched_pages, second_projection.stable_prefix_pages.len());
        assert!(matched.matched_tokens > 0);
    }

    #[test]
    fn cache_simulator_matches_when_agent_sdk_billing_header_is_prefixed_by_thinking_tags() {
        let first_header = concat!(
            "<thinking_mode>adaptive</thinking_mode>",
            "<thinking_effort>max</thinking_effort>\n",
            "x-anthropic-billing-header: cc_version=2.1.114.eee; ",
            "cc_entrypoint=sdk-cli; cch=fb0be;\n",
            "You are a Claude agent, built on Anthropic's Claude Agent SDK.\n",
            "You are an interactive agent that helps users with software engineering tasks."
        );
        let second_header = concat!(
            "<thinking_mode>adaptive</thinking_mode>",
            "<thinking_effort>max</thinking_effort>\n",
            "x-anthropic-billing-header: cc_version=2.1.114.eee; ",
            "cc_entrypoint=sdk-cli; cch=9ac44;\n",
            "You are a Claude agent, built on Anthropic's Claude Agent SDK.\n",
            "You are an interactive agent that helps users with software engineering tasks."
        );
        let assistant = AssistantMessage::new("assistant reply");
        let simulator = KiroCacheSimulator::default();
        let config = KiroCacheSimulationConfig {
            mode: KiroCacheSimulationMode::PrefixTree,
            prefix_cache_max_tokens: 100_000,
            prefix_cache_entry_ttl: Duration::from_secs(300),
            conversation_anchor_max_entries: 32,
            conversation_anchor_ttl: Duration::from_secs(300),
        };
        let now = Instant::now();
        let first_state = ConversationState::new("conv-1")
            .with_history(vec![history_user(first_header), history_assistant("done")])
            .with_current_message(CurrentMessage::new(
                UserInputMessage::new("continue", "ignored-model").with_context(
                    UserInputMessageContext::new().with_tools(vec![tool(
                        "search_files",
                        "Search files",
                        json!({"type":"object","properties":{"query":{"type":"string"}}}),
                    )]),
                ),
            ));
        let second_state = ConversationState::new("conv-1")
            .with_history(vec![history_user(second_header), history_assistant("done")])
            .with_current_message(CurrentMessage::new(
                UserInputMessage::new("continue", "ignored-model").with_context(
                    UserInputMessageContext::new().with_tools(vec![tool(
                        "search_files",
                        "Search files",
                        json!({"type":"object","properties":{"query":{"type":"string"}}}),
                    )]),
                ),
            ));

        let first_projection = PromptProjection::from_conversation_state(&first_state);
        simulator.record_success(&first_projection, &assistant, "real-conv", true, config, now);

        let second_projection = PromptProjection::from_conversation_state(&second_state);
        let matched =
            simulator.match_prefix(&second_projection, config, now + Duration::from_secs(1));

        assert_eq!(first_projection.lookup_anchor_hash, second_projection.lookup_anchor_hash);
        assert_eq!(first_projection.stable_prefix_pages, second_projection.stable_prefix_pages);
        assert_eq!(matched.matched_pages, second_projection.stable_prefix_pages.len());
        assert!(matched.matched_tokens > 0);
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
    fn cache_simulator_matches_stable_prefix_after_success_is_recorded() {
        let state = ConversationState::new("conv-1")
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
        let assistant = AssistantMessage::new("assistant reply");
        let simulator = KiroCacheSimulator::default();
        let config = KiroCacheSimulationConfig {
            mode: KiroCacheSimulationMode::PrefixTree,
            prefix_cache_max_tokens: 100_000,
            prefix_cache_entry_ttl: Duration::from_secs(300),
            conversation_anchor_max_entries: 32,
            conversation_anchor_ttl: Duration::from_secs(300),
        };
        let now = Instant::now();

        simulator.record_success(&projection, &assistant, "real-conv", true, config, now);
        let matched = simulator.match_prefix(&projection, config, now + Duration::from_secs(1));

        assert_eq!(matched.matched_pages, projection.stable_prefix_pages.len());
        assert!(matched.matched_tokens > 0);
    }

    #[test]
    fn cache_simulator_recovers_resume_anchor_from_post_turn_history() {
        let initial_state = ConversationState::new("fallback-conv")
            .with_history(vec![history_user("existing history"), history_assistant("done")])
            .with_current_message(CurrentMessage::new(UserInputMessage::new(
                "continue analysis",
                "ignored-model",
            )));
        let projection = PromptProjection::from_conversation_state(&initial_state);
        let assistant = AssistantMessage::new("assistant reply");
        let simulator = KiroCacheSimulator::default();
        let config = KiroCacheSimulationConfig {
            mode: KiroCacheSimulationMode::PrefixTree,
            prefix_cache_max_tokens: 100_000,
            prefix_cache_entry_ttl: Duration::from_secs(300),
            conversation_anchor_max_entries: 32,
            conversation_anchor_ttl: Duration::from_secs(300),
        };
        let now = Instant::now();
        simulator.record_success(&projection, &assistant, "real-conv", true, config, now);

        let follow_up_state = ConversationState::new("new-fallback")
            .with_history(vec![
                history_user("existing history"),
                history_assistant("done"),
                Message::User(HistoryUserMessage::new("continue analysis", "ignored-model")),
                Message::Assistant(HistoryAssistantMessage {
                    assistant_response_message: assistant.clone(),
                }),
            ])
            .with_current_message(CurrentMessage::new(UserInputMessage::new(
                "next step",
                "ignored-model",
            )));
        let follow_up_projection = PromptProjection::from_conversation_state(&follow_up_state);

        assert_eq!(
            simulator.recover_conversation_id(
                &follow_up_projection,
                config,
                now + Duration::from_secs(1)
            ),
            Some("real-conv".to_string())
        );
    }

    #[test]
    fn cache_simulator_can_record_anchor_without_warming_prefix_tree() {
        let initial_state = ConversationState::new("fallback-conv")
            .with_history(vec![history_user("existing history"), history_assistant("done")])
            .with_current_message(CurrentMessage::new(UserInputMessage::new(
                "continue analysis",
                "ignored-model",
            )));
        let projection = PromptProjection::from_conversation_state(&initial_state);
        let assistant = AssistantMessage::new("assistant reply");
        let simulator = KiroCacheSimulator::default();
        let config = KiroCacheSimulationConfig {
            mode: KiroCacheSimulationMode::PrefixTree,
            prefix_cache_max_tokens: 100_000,
            prefix_cache_entry_ttl: Duration::from_secs(300),
            conversation_anchor_max_entries: 32,
            conversation_anchor_ttl: Duration::from_secs(300),
        };
        let now = Instant::now();

        simulator.record_success(&projection, &assistant, "real-conv", false, config, now);

        let matched = simulator.match_prefix(&projection, config, now + Duration::from_secs(1));
        assert_eq!(matched, PrefixCacheMatch::default());

        let follow_up_state = ConversationState::new("new-fallback")
            .with_history(vec![
                history_user("existing history"),
                history_assistant("done"),
                Message::User(HistoryUserMessage::new("continue analysis", "ignored-model")),
                Message::Assistant(HistoryAssistantMessage {
                    assistant_response_message: assistant.clone(),
                }),
            ])
            .with_current_message(CurrentMessage::new(UserInputMessage::new(
                "next step",
                "ignored-model",
            )));
        let follow_up_projection = PromptProjection::from_conversation_state(&follow_up_state);
        assert_eq!(
            simulator.recover_conversation_id(
                &follow_up_projection,
                config,
                now + Duration::from_secs(1)
            ),
            Some("real-conv".to_string())
        );
    }

    #[test]
    fn prefix_tree_handles_deep_paths_without_recursive_helpers() {
        let depth = 20_000usize;
        let pages = (0..depth)
            .map(|index| CanonicalTokenPage {
                key: index as u128 + 1,
                token_count: 64,
            })
            .collect::<Vec<_>>();
        let mut tree = PrefixTree::default();
        let now = Instant::now();
        let ttl = Duration::from_secs(300);

        tree.insert(&pages, now, ttl, u64::MAX);
        let matched = tree.match_prefix(&pages, now + Duration::from_secs(1), ttl);
        assert_eq!(matched.matched_pages, depth);
        assert_eq!(matched.matched_tokens, depth as u64 * 64);

        tree.prune_expired(now + ttl + Duration::from_secs(2), ttl);
        assert_eq!(tree.resident_tokens, 0);
        assert!(tree.root.children.is_empty());
    }
}
