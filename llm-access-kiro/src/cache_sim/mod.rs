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
//!
//! ## Module map
//!
//! `cache_sim.rs` is the facade: it owns the public projection/simulator types
//! (`PromptProjection`, `RuntimePromptProjection`, `KiroCacheSimulator`,
//! `KiroCacheSimulationConfig`, the `*RuntimeStats` views) and the private data
//! structures (`PrefixTree`/`PrefixNode`/`PrefixEdge`,
//! `ConversationAnchorIndex`, the canonical-segment structs) together with all
//! their `impl` blocks. The pure helper functions are grouped into descendant
//! submodules:
//!
//! ```text
//!  ConversationState
//!        |
//!        v
//!  [canonicalize]  history/current-turn/tool canonicalization -> input units
//!        |
//!        v
//!  [tokenize]      input units -> fixed-size canonical token pages
//!        |              (uses [hashing] for stable per-atom/page hashes)
//!        v
//!  KiroCacheSimulator  --uses-->  [prefix_tree]  insert/prune/evict the
//!                                                 shared-prefix radix tree
//! ```
//!
//! Impl blocks stay in the parent so they keep private access to the data
//! structures' fields; submodule helpers are descendants and likewise retain
//! that access.

use std::{
    collections::BTreeMap,
    num::NonZeroUsize,
    time::{Duration, Instant},
};

use lru::LruCache;
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use xxhash_rust::xxh3::{xxh3_128, xxh3_64};

use crate::wire::{
    AssistantMessage, ConversationState, KiroDocument, KiroImage, Message, Tool, UserInputMessage,
    UserInputMessageContext, UserMessage,
};

mod canonicalize;
mod hashing;
mod prefix_tree;
mod tokenize;

pub(crate) use canonicalize::*;
pub(crate) use hashing::*;
pub(crate) use prefix_tree::*;
pub(crate) use tokenize::*;

const PREFIX_CACHE_PAGE_SIZE: usize = 64;
const PREFIX_CHILD_SORT_THRESHOLD: usize = 16;
#[derive(Debug, Clone, PartialEq, Eq)]
// A canonical unit is the smallest semantic fragment we retain before packing
// it into fixed-size cache pages. We keep the stable string key for anchor/hash
// construction, while token atoms feed the page-based prefix tree.
pub(crate) struct CanonicalInputUnit {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KiroCacheSimulationMode {
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
pub struct KiroCacheSimulationConfig {
    pub mode: KiroCacheSimulationMode,
    pub prefix_cache_max_tokens: u64,
    pub prefix_cache_entry_ttl: Duration,
    pub conversation_anchor_max_entries: usize,
    pub conversation_anchor_ttl: Duration,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PrefixCacheMatch {
    pub matched_pages: usize,
    pub matched_tokens: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct KiroCacheRuntimeStats {
    pub mode: KiroCacheSimulationMode,
    pub page_size_tokens: usize,
    pub prefix_tree: PrefixTreeRuntimeStats,
    pub conversation_anchors: ConversationAnchorRuntimeStats,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct PrefixTreeRuntimeStats {
    pub resident_tokens: u64,
    pub max_tokens: u64,
    pub node_count: usize,
    pub leaf_count: usize,
    pub edge_count: usize,
    pub child_capacity: usize,
    pub estimated_memory_bytes: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct ConversationAnchorRuntimeStats {
    pub entries: usize,
    pub max_entries: usize,
    pub estimated_memory_bytes: u64,
}
#[derive(Default)]
pub struct KiroCacheSimulator {
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

    pub fn match_prefix_from_runtime_projection(
        &self,
        projection: &RuntimePromptProjection,
        config: KiroCacheSimulationConfig,
        now: Instant,
    ) -> PrefixCacheMatch {
        if matches!(config.mode, KiroCacheSimulationMode::Formula) {
            return PrefixCacheMatch::default();
        }
        let mut tree = self.prefix_tree.lock();
        tree.match_prefix(projection.stable_prefix_pages(), now, config.prefix_cache_entry_ttl)
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

    pub fn recover_conversation_id_from_runtime_projection(
        &self,
        projection: &RuntimePromptProjection,
        config: KiroCacheSimulationConfig,
        now: Instant,
    ) -> Option<String> {
        let mut index = self.anchor_index.lock();
        index.get(
            projection.lookup_anchor_hash(),
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

    pub fn record_success_from_runtime_projection(
        &self,
        projection: &RuntimePromptProjection,
        assistant_message: &AssistantMessage,
        conversation_id: &str,
        record_prefix_tree: bool,
        config: KiroCacheSimulationConfig,
        now: Instant,
    ) {
        if record_prefix_tree && matches!(config.mode, KiroCacheSimulationMode::PrefixTree) {
            let mut tree = self.prefix_tree.lock();
            tree.insert(
                projection.stable_prefix_pages(),
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

    pub fn snapshot_stats(
        &self,
        config: KiroCacheSimulationConfig,
        now: Instant,
    ) -> KiroCacheRuntimeStats {
        let prefix_tree = {
            let mut tree = self.prefix_tree.lock();
            tree.prune_expired(now, config.prefix_cache_entry_ttl);
            tree.snapshot_stats(config.prefix_cache_max_tokens)
        };
        let conversation_anchors = {
            let mut index = self.anchor_index.lock();
            index.ensure_capacity(config.conversation_anchor_max_entries);
            index.remove_expired(now, config.conversation_anchor_ttl);
            index.snapshot_stats(config.conversation_anchor_max_entries)
        };
        KiroCacheRuntimeStats {
            mode: config.mode,
            page_size_tokens: PREFIX_CACHE_PAGE_SIZE,
            prefix_tree,
            conversation_anchors,
        }
    }
}
#[derive(Debug, Default)]
pub(crate) struct PrefixTree {
    root: PrefixNode,
    resident_tokens: u64,
}
#[derive(Debug, Default)]
pub(crate) struct PrefixNode {
    children: Vec<PrefixEdge>,
    children_sorted: bool,
}
impl Drop for PrefixNode {
    fn drop(&mut self) {
        let mut stack = std::mem::take(&mut self.children);
        while let Some(mut edge) = stack.pop() {
            stack.extend(std::mem::take(&mut edge.child.children));
        }
    }
}
#[derive(Debug)]
pub(crate) struct PrefixEdge {
    pages: Box<[CanonicalTokenPage]>,
    token_count: u64,
    last_touched_at: Instant,
    child: PrefixNode,
}
impl PrefixEdge {
    fn new(pages: &[CanonicalTokenPage], now: Instant) -> Self {
        debug_assert!(!pages.is_empty());
        Self {
            pages: pages.to_vec().into_boxed_slice(),
            token_count: prefix_pages_token_count(pages),
            last_touched_at: now,
            child: PrefixNode::default(),
        }
    }

    fn first_page_key(&self) -> u128 {
        self.pages[0].key
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
        let mut offset = 0usize;
        while offset < pages.len() {
            let Some(edge_index) = find_child_edge_index(current, pages[offset].key) else {
                break;
            };
            let edge = &mut current.children[edge_index];
            let common = common_prefix_len(&edge.pages, &pages[offset..]);
            if common == 0 {
                break;
            }
            matched.matched_pages = matched.matched_pages.saturating_add(common);
            matched.matched_tokens = matched
                .matched_tokens
                .saturating_add(prefix_pages_token_count(&edge.pages[..common]));
            if common < edge.pages.len() {
                split_edge_at(edge, common, now);
                break;
            }
            edge.last_touched_at = now;
            offset += common;
            current = &mut edge.child;
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

    fn snapshot_stats(&self, max_tokens: u64) -> PrefixTreeRuntimeStats {
        let mut node_count = 0usize;
        let mut leaf_count = 0usize;
        let mut edge_count = 0usize;
        let mut child_capacity = 0usize;
        let mut page_count = 0usize;
        let mut stack = vec![(&self.root, true)];

        while let Some((node, is_root)) = stack.pop() {
            node_count = node_count.saturating_add(1);
            edge_count = edge_count.saturating_add(node.children.len());
            child_capacity = child_capacity.saturating_add(node.children.capacity());
            page_count = page_count.saturating_add(
                node.children
                    .iter()
                    .map(|edge| edge.pages.len())
                    .sum::<usize>(),
            );
            if node.children.is_empty() && !is_root {
                leaf_count = leaf_count.saturating_add(1);
            }
            stack.extend(node.children.iter().map(|edge| (&edge.child, false)));
        }

        let estimated_memory_bytes = estimate_prefix_tree_memory_bytes(child_capacity, page_count);
        PrefixTreeRuntimeStats {
            resident_tokens: self.resident_tokens,
            max_tokens,
            node_count,
            leaf_count,
            edge_count,
            child_capacity,
            estimated_memory_bytes,
        }
    }
}
#[derive(Debug)]
pub(crate) struct ConversationAnchorEntry {
    conversation_id: String,
    last_touched_at: Instant,
}
#[derive(Debug, Default)]
pub(crate) struct ConversationAnchorIndex {
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

    fn snapshot_stats(&self, max_entries: usize) -> ConversationAnchorRuntimeStats {
        let entries = self.cache.as_ref().map_or(0, LruCache::len);
        ConversationAnchorRuntimeStats {
            entries,
            max_entries: max_entries.max(1),
            estimated_memory_bytes: estimate_anchor_index_memory_bytes(entries),
        }
    }
}
pub(crate) struct RuntimePromptProjectionBuilder {
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
pub(crate) struct TokenPageBuilder {
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
pub(crate) struct UserMessageParts<'a> {
    content: &'a str,
    images: &'a [KiroImage],
    documents: &'a [KiroDocument],
    context: &'a UserInputMessageContext,
}
#[derive(Serialize)]
pub(crate) struct CanonicalTextSegment {
    kind: String,
    text: String,
}
#[derive(Serialize)]
pub(crate) struct CanonicalImageSegment {
    kind: String,
    format: String,
    digest: String,
}
#[derive(Serialize)]
pub(crate) struct CanonicalDocumentSegment {
    kind: String,
    name: String,
    format: String,
    digest: String,
}
#[derive(Serialize)]
pub(crate) struct CanonicalToolResultSegment {
    kind: String,
    tool_use_id: String,
    status: String,
    is_error: bool,
    content: Value,
}
#[derive(Serialize)]
pub(crate) struct CanonicalToolUseSegment {
    kind: String,
    tool_use_id: String,
    name: String,
    input: Value,
}
#[derive(Serialize)]
pub(crate) struct CanonicalToolDefinitionSegment {
    kind: String,
    name: String,
    description: String,
    input_schema: Value,
}

#[cfg(test)]
mod tests;
