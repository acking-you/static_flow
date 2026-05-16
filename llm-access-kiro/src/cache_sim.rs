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
    collections::BTreeMap,
    num::NonZeroUsize,
    time::{Duration, Instant},
};

use charabia::Tokenize;
use lru::LruCache;
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use xxhash_rust::xxh3::{xxh3_128, xxh3_64};

use crate::wire::{
    AssistantMessage, ConversationState, Message, Tool, UserInputMessage, UserMessage,
};

const PREFIX_CACHE_PAGE_SIZE: usize = 64;
const PREFIX_CHILD_SORT_THRESHOLD: usize = 16;
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
struct PrefixTree {
    root: PrefixNode,
    resident_tokens: u64,
}

#[derive(Debug, Default)]
struct PrefixNode {
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
struct PrefixEdge {
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

    fn snapshot_stats(&self, max_entries: usize) -> ConversationAnchorRuntimeStats {
        let entries = self.cache.as_ref().map_or(0, LruCache::len);
        ConversationAnchorRuntimeStats {
            entries,
            max_entries: max_entries.max(1),
            estimated_memory_bytes: estimate_anchor_index_memory_bytes(entries),
        }
    }
}

fn estimate_prefix_tree_memory_bytes(child_capacity: usize, page_count: usize) -> u64 {
    let root_bytes = std::mem::size_of::<PrefixNode>();
    let edge_bytes = child_capacity.saturating_mul(std::mem::size_of::<PrefixEdge>());
    let page_bytes = page_count.saturating_mul(std::mem::size_of::<CanonicalTokenPage>());
    root_bytes
        .saturating_add(edge_bytes)
        .saturating_add(page_bytes) as u64
}

fn estimate_anchor_index_memory_bytes(entries: usize) -> u64 {
    let entry_bytes = std::mem::size_of::<ConversationAnchorEntry>();
    let key_bytes = std::mem::size_of::<String>();
    entries.saturating_mul(entry_bytes.saturating_add(key_bytes)) as u64
}

fn insert_prefix_path(node: &mut PrefixNode, pages: &[CanonicalTokenPage], now: Instant) -> u64 {
    let mut added_tokens: u64 = 0;
    let mut current = node;
    let mut offset = 0usize;
    while offset < pages.len() {
        let Some(edge_index) = find_child_edge_index(current, pages[offset].key) else {
            let edge = PrefixEdge::new(&pages[offset..], now);
            added_tokens = added_tokens.saturating_add(edge.token_count);
            push_child_edge(current, edge);
            return added_tokens;
        };

        let edge = &mut current.children[edge_index];
        let common = common_prefix_len(&edge.pages, &pages[offset..]);
        if common == 0 {
            let edge = PrefixEdge::new(&pages[offset..], now);
            added_tokens = added_tokens.saturating_add(edge.token_count);
            push_child_edge(current, edge);
            return added_tokens;
        }
        if common < edge.pages.len() {
            split_edge_at(edge, common, now);
        } else {
            edge.last_touched_at = now;
        }
        offset += common;
        if offset == pages.len() {
            return added_tokens;
        }
        current = &mut current.children[edge_index].child;
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
                for edge in &mut current.children {
                    stack.push((&mut edge.child as *mut PrefixNode, false));
                }
                continue;
            }

            let mut index = 0usize;
            while index < current.children.len() {
                if now.duration_since(current.children[index].last_touched_at) > ttl {
                    let edge = current.children.remove(index);
                    removed_tokens = removed_tokens.saturating_add(subtree_token_count_edge(&edge));
                } else {
                    index += 1;
                }
            }
        }
    }

    removed_tokens
}

fn subtree_token_count_edge(edge: &PrefixEdge) -> u64 {
    let mut total = edge.token_count;
    let mut stack = vec![&edge.child];
    while let Some(current) = stack.pop() {
        for edge in &current.children {
            total = total.saturating_add(edge.token_count);
            stack.push(&edge.child);
        }
    }
    total
}

fn find_coldest_leaf_path(node: &PrefixNode) -> Option<Vec<usize>> {
    struct Frame<'a> {
        node: &'a PrefixNode,
        incoming_last_touched_at: Option<Instant>,
        next_child: usize,
    }

    let mut best: Option<(Instant, Vec<usize>)> = None;
    let mut path = Vec::<usize>::new();
    let mut stack = vec![Frame {
        node,
        incoming_last_touched_at: None,
        next_child: 0,
    }];

    while let Some(frame) = stack.last_mut() {
        if frame.node.children.is_empty() {
            if let Some(last_touched_at) = frame.incoming_last_touched_at {
                match &best {
                    Some((current_oldest, _)) if last_touched_at >= *current_oldest => {},
                    _ => best = Some((last_touched_at, path.clone())),
                }
            }
            stack.pop();
            if !path.is_empty() {
                path.pop();
            }
            continue;
        }

        if frame.next_child >= frame.node.children.len() {
            stack.pop();
            if !path.is_empty() {
                path.pop();
            }
            continue;
        }

        let edge_index = frame.next_child;
        frame.next_child += 1;
        let edge = &frame.node.children[edge_index];
        path.push(edge_index);
        stack.push(Frame {
            node: &edge.child,
            incoming_last_touched_at: Some(edge.last_touched_at),
            next_child: 0,
        });
    }

    best.map(|(_, path)| path)
}

fn remove_leaf_path(node: &mut PrefixNode, path: &[usize]) -> u64 {
    if path.is_empty() {
        return 0;
    }

    let mut lineage = Vec::with_capacity(path.len());
    let mut current_ptr = node as *mut PrefixNode;

    // The lineage stores each parent pointer plus the child index used to descend
    // one level. This lets us prune empty ancestors iteratively on the way back
    // up without recursive calls.
    // SAFETY: `lineage` stores parent pointers discovered by walking the tree
    // from the exclusive mutable root borrow. We only remove descendants while
    // walking back up that exact lineage, so each pointer remains valid until
    // the moment its corresponding child entry is removed.
    unsafe {
        for key in path {
            let current = &mut *current_ptr;
            let Some(edge) = current.children.get_mut(*key) else {
                return 0;
            };
            lineage.push((current_ptr, *key));
            current_ptr = &mut edge.child as *mut PrefixNode;
        }

        let (leaf_parent_ptr, leaf_index) = *lineage
            .last()
            .expect("non-empty path should always record one lineage entry");
        let leaf_parent = &mut *leaf_parent_ptr;
        if leaf_index >= leaf_parent.children.len() {
            return 0;
        }
        let removed_edge = leaf_parent.children.remove(leaf_index);
        let removed_subtree_tokens = subtree_token_count_edge(&removed_edge);
        if removed_subtree_tokens == 0 {
            return 0;
        }

        let mut removed_tokens = removed_subtree_tokens;
        for &(parent_ptr, child_index) in lineage[..lineage.len().saturating_sub(1)].iter().rev() {
            let parent = &mut *parent_ptr;
            let Some(edge) = parent.children.get(child_index) else {
                break;
            };
            if !edge.child.children.is_empty() {
                break;
            }
            let edge = parent.children.remove(child_index);
            removed_tokens = removed_tokens.saturating_add(edge.token_count);
        }

        removed_tokens
    }
}

fn push_child_edge(node: &mut PrefixNode, edge: PrefixEdge) {
    node.children.push(edge);
    node.children_sorted = false;
}

fn find_child_edge_index(node: &mut PrefixNode, first_page_key: u128) -> Option<usize> {
    if node.children.len() < PREFIX_CHILD_SORT_THRESHOLD {
        return find_child_edge_index_linear(node, first_page_key);
    }
    if !node.children_sorted {
        node.children
            .sort_unstable_by_key(|edge| edge.first_page_key());
        node.children_sorted = true;
    }
    node.children
        .binary_search_by_key(&first_page_key, |edge| edge.first_page_key())
        .ok()
}

fn find_child_edge_index_linear(node: &PrefixNode, first_page_key: u128) -> Option<usize> {
    node.children
        .iter()
        .position(|edge| edge.first_page_key() == first_page_key)
}

fn common_prefix_len(left: &[CanonicalTokenPage], right: &[CanonicalTokenPage]) -> usize {
    left.iter()
        .zip(right)
        .take_while(|(left, right)| left.key == right.key)
        .count()
}

fn split_edge_at(edge: &mut PrefixEdge, split_at: usize, prefix_last_touched_at: Instant) {
    debug_assert!(split_at > 0);
    debug_assert!(split_at < edge.pages.len());

    let old_pages = std::mem::take(&mut edge.pages).into_vec();
    let old_last_touched_at = edge.last_touched_at;
    let old_child = std::mem::take(&mut edge.child);
    let mut prefix_pages = old_pages;
    let suffix_pages = prefix_pages.split_off(split_at);
    let prefix_token_count = prefix_pages_token_count(&prefix_pages);
    let suffix_token_count = prefix_pages_token_count(&suffix_pages);

    edge.pages = prefix_pages.into_boxed_slice();
    edge.token_count = prefix_token_count;
    edge.last_touched_at = prefix_last_touched_at;
    edge.child = PrefixNode {
        children: vec![PrefixEdge {
            pages: suffix_pages.into_boxed_slice(),
            token_count: suffix_token_count,
            last_touched_at: old_last_touched_at,
            child: old_child,
        }],
        children_sorted: false,
    };
}

fn prefix_pages_token_count(pages: &[CanonicalTokenPage]) -> u64 {
    pages.iter().map(|page| u64::from(page.token_count)).sum()
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
        documents: message.documents.clone(),
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
        documents: message.documents.clone(),
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

    for document in &message.documents {
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
        let len = segment.len() as u64;
        hasher.update(len.to_le_bytes());
        hasher.update(segment.as_bytes());
    }
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
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;
    use crate::wire::{
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

        let runtime_projection = projection.into_runtime_projection();

        assert_eq!(runtime_projection.lookup_anchor_hash(), expected_lookup_anchor);
        assert_eq!(runtime_projection.stable_prefix_pages(), expected_pages);
        assert_eq!(runtime_projection.projected_input_token_count(), expected_projected_tokens);
        assert_eq!(runtime_projection.build_resume_anchor_hash(&assistant), expected_resume_anchor);
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
    fn cache_simulator_snapshot_reports_prefix_tree_and_anchor_usage() {
        let state = ConversationState::new("conv-1")
            .with_history(vec![history_user(&"stable prefix ".repeat(256))])
            .with_current_message(CurrentMessage::new(UserInputMessage::new(
                "continue analysis",
                "ignored-model",
            )));
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
        let snapshot = simulator.snapshot_stats(config, now + Duration::from_secs(1));

        assert_eq!(snapshot.mode, KiroCacheSimulationMode::PrefixTree);
        assert_eq!(snapshot.page_size_tokens, PREFIX_CACHE_PAGE_SIZE);
        assert_eq!(snapshot.prefix_tree.resident_tokens, projection.stable_prefix_token_count());
        assert_eq!(snapshot.prefix_tree.max_tokens, config.prefix_cache_max_tokens);
        assert!(snapshot.prefix_tree.node_count <= 2);
        assert_eq!(snapshot.prefix_tree.leaf_count, 1);
        assert!(snapshot.prefix_tree.estimated_memory_bytes > 0);
        assert_eq!(snapshot.conversation_anchors.entries, 1);
        assert_eq!(
            snapshot.conversation_anchors.max_entries,
            config.conversation_anchor_max_entries
        );
    }

    #[test]
    fn prefix_tree_compresses_long_single_branch() {
        let pages = numbered_pages(512, 10_000);
        let mut tree = PrefixTree::default();
        let now = Instant::now();
        let ttl = Duration::from_secs(300);

        tree.insert(&pages, now, ttl, u64::MAX);

        let snapshot = tree.snapshot_stats(u64::MAX);
        assert_eq!(snapshot.resident_tokens, pages_token_count(&pages));
        assert_eq!(snapshot.node_count, 2);
        assert_eq!(snapshot.edge_count, 1);
        assert_eq!(snapshot.leaf_count, 1);
        let matched = tree.match_prefix(&pages, now + Duration::from_secs(1), ttl);
        assert_eq!(matched.matched_pages, pages.len());
        assert_eq!(matched.matched_tokens, pages_token_count(&pages));
    }

    #[test]
    fn prefix_tree_splits_compressed_edges_on_divergence() {
        let first = pages_from_keys(&[1, 2, 3, 4]);
        let second = pages_from_keys(&[1, 2, 9, 10]);
        let divergent = pages_from_keys(&[1, 2, 3, 99]);
        let mut tree = PrefixTree::default();
        let now = Instant::now();
        let ttl = Duration::from_secs(300);

        tree.insert(&first, now, ttl, u64::MAX);
        tree.insert(&second, now + Duration::from_secs(1), ttl, u64::MAX);

        let snapshot = tree.snapshot_stats(u64::MAX);
        assert_eq!(
            snapshot.resident_tokens,
            pages_token_count(&first) + pages_token_count(&second[2..])
        );
        assert_eq!(snapshot.node_count, 4);
        assert_eq!(snapshot.edge_count, 3);
        assert_eq!(snapshot.leaf_count, 2);

        let matched_first = tree.match_prefix(&first, now + Duration::from_secs(2), ttl);
        assert_eq!(matched_first.matched_pages, first.len());
        assert_eq!(matched_first.matched_tokens, pages_token_count(&first));

        let matched_second = tree.match_prefix(&second, now + Duration::from_secs(3), ttl);
        assert_eq!(matched_second.matched_pages, second.len());
        assert_eq!(matched_second.matched_tokens, pages_token_count(&second));

        let matched_divergent = tree.match_prefix(&divergent, now + Duration::from_secs(4), ttl);
        assert_eq!(matched_divergent.matched_pages, 3);
        assert_eq!(matched_divergent.matched_tokens, pages_token_count(&divergent[..3]));
    }

    #[test]
    fn prefix_tree_partial_match_only_refreshes_touched_prefix() {
        let first = pages_from_keys(&[1, 2, 3, 4]);
        let second = pages_from_keys(&[1, 2, 9, 10]);
        let divergent = pages_from_keys(&[1, 2, 3, 99]);
        let mut tree = PrefixTree::default();
        let now = Instant::now();
        let ttl = Duration::from_secs(30);

        tree.insert(&first, now, ttl, u64::MAX);
        tree.insert(&second, now, ttl, u64::MAX);
        let matched = tree.match_prefix(&divergent, now + Duration::from_secs(10), ttl);
        assert_eq!(matched.matched_pages, 3);

        tree.prune_expired(now + Duration::from_secs(35), ttl);

        assert_eq!(tree.resident_tokens, pages_token_count(&divergent[..3]));
        let retained = tree.match_prefix(&divergent[..3], now + Duration::from_secs(36), ttl);
        assert_eq!(retained.matched_pages, 3);
        let expired_branch = tree.match_prefix(&second, now + Duration::from_secs(37), ttl);
        assert_eq!(expired_branch.matched_pages, 2);
    }

    #[test]
    fn radix_prefix_tree_matches_plain_trie_hit_semantics() {
        let ttl = Duration::from_secs(30);
        let now = Instant::now();
        let first = pages_from_keys(&[1, 2, 3, 4]);
        let second = pages_from_keys(&[1, 2, 9, 10]);
        let divergent = pages_from_keys(&[1, 2, 3, 99]);
        let short_prefix = pages_from_keys(&[1, 2]);
        let missing = pages_from_keys(&[7, 8]);
        let mut radix = PrefixTree::default();
        let mut plain = PlainPrefixTree::default();

        compare_insert(&mut radix, &mut plain, &first, now, ttl);
        compare_match(&mut radix, &mut plain, &first, now + Duration::from_secs(1), ttl);
        compare_insert(&mut radix, &mut plain, &second, now + Duration::from_secs(2), ttl);
        compare_match(&mut radix, &mut plain, &divergent, now + Duration::from_secs(10), ttl);
        compare_match(&mut radix, &mut plain, &second, now + Duration::from_secs(11), ttl);
        compare_match(&mut radix, &mut plain, &short_prefix, now + Duration::from_secs(12), ttl);
        compare_match(&mut radix, &mut plain, &missing, now + Duration::from_secs(13), ttl);

        let prune_at = now + Duration::from_secs(45);
        radix.prune_expired(prune_at, ttl);
        plain.prune_expired(prune_at, ttl);
        assert_eq!(radix.resident_tokens, plain.resident_tokens);
        compare_match(&mut radix, &mut plain, &divergent, now + Duration::from_secs(46), ttl);
        compare_match(&mut radix, &mut plain, &second, now + Duration::from_secs(47), ttl);
    }

    #[test]
    fn radix_prefix_tree_matches_plain_trie_budget_eviction_semantics() {
        let ttl = Duration::from_secs(300);
        let now = Instant::now();
        let shared_first = pages_from_keys(&[1, 2, 3]);
        let shared_second = pages_from_keys(&[1, 2, 9]);
        let independent = pages_from_keys(&[5, 6]);
        let newest = pages_from_keys(&[7, 8, 9]);
        let max_tokens = 50;
        let mut radix = PrefixTree::default();
        let mut plain = PlainPrefixTree::default();

        compare_insert_with_limit(&mut radix, &mut plain, &shared_first, now, ttl, max_tokens);
        compare_insert_with_limit(
            &mut radix,
            &mut plain,
            &shared_second,
            now + Duration::from_secs(1),
            ttl,
            max_tokens,
        );
        compare_insert_with_limit(
            &mut radix,
            &mut plain,
            &independent,
            now + Duration::from_secs(2),
            ttl,
            max_tokens,
        );
        compare_match(&mut radix, &mut plain, &shared_second, now + Duration::from_secs(3), ttl);
        compare_insert_with_limit(
            &mut radix,
            &mut plain,
            &newest,
            now + Duration::from_secs(4),
            ttl,
            max_tokens,
        );

        compare_match(&mut radix, &mut plain, &shared_first, now + Duration::from_secs(5), ttl);
        compare_match(&mut radix, &mut plain, &shared_second, now + Duration::from_secs(6), ttl);
        compare_match(&mut radix, &mut plain, &newest, now + Duration::from_secs(7), ttl);
    }

    #[test]
    fn prefix_tree_sorts_high_fanout_node_lazily_without_changing_hits() {
        let ttl = Duration::from_secs(300);
        let now = Instant::now();
        let mut radix = PrefixTree::default();
        let mut plain = PlainPrefixTree::default();

        for key in (0..32).rev() {
            compare_insert(&mut radix, &mut plain, &pages_from_keys(&[key]), now, ttl);
        }

        assert_ne!(root_first_page_keys(&radix), (0..32).collect::<Vec<_>>());
        compare_match(
            &mut radix,
            &mut plain,
            &pages_from_keys(&[17]),
            now + Duration::from_secs(1),
            ttl,
        );
        assert_eq!(root_first_page_keys(&radix), (0..32).collect::<Vec<_>>());
    }

    fn numbered_pages(count: usize, start: u128) -> Vec<CanonicalTokenPage> {
        (0..count)
            .map(|index| CanonicalTokenPage {
                key: start + index as u128,
                token_count: 64,
            })
            .collect()
    }

    fn pages_from_keys(keys: &[u128]) -> Vec<CanonicalTokenPage> {
        keys.iter()
            .map(|key| CanonicalTokenPage {
                key: *key,
                token_count: 10,
            })
            .collect()
    }

    fn pages_token_count(pages: &[CanonicalTokenPage]) -> u64 {
        pages.iter().map(|page| u64::from(page.token_count)).sum()
    }

    fn compare_insert(
        radix: &mut PrefixTree,
        plain: &mut PlainPrefixTree,
        pages: &[CanonicalTokenPage],
        now: Instant,
        ttl: Duration,
    ) {
        compare_insert_with_limit(radix, plain, pages, now, ttl, u64::MAX);
    }

    fn compare_insert_with_limit(
        radix: &mut PrefixTree,
        plain: &mut PlainPrefixTree,
        pages: &[CanonicalTokenPage],
        now: Instant,
        ttl: Duration,
        max_tokens: u64,
    ) {
        radix.insert(pages, now, ttl, max_tokens);
        plain.insert(pages, now, ttl, max_tokens);
        assert_eq!(radix.resident_tokens, plain.resident_tokens);
    }

    fn compare_match(
        radix: &mut PrefixTree,
        plain: &mut PlainPrefixTree,
        pages: &[CanonicalTokenPage],
        now: Instant,
        ttl: Duration,
    ) {
        let radix_match = radix.match_prefix(pages, now, ttl);
        let plain_match = plain.match_prefix(pages, now, ttl);
        assert_eq!(radix_match, plain_match);
        assert_eq!(radix.resident_tokens, plain.resident_tokens);
    }

    fn root_first_page_keys(tree: &PrefixTree) -> Vec<u128> {
        tree.root
            .children
            .iter()
            .map(|edge| edge.pages[0].key)
            .collect()
    }

    #[derive(Default)]
    struct PlainPrefixTree {
        root: PlainPrefixNode,
        resident_tokens: u64,
    }

    #[derive(Default)]
    struct PlainPrefixNode {
        token_count: u64,
        last_touched_at: Option<Instant>,
        children: BTreeMap<u128, PlainPrefixNode>,
    }

    impl PlainPrefixTree {
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
                child.last_touched_at = Some(now);
                matched.matched_pages = matched.matched_pages.saturating_add(1);
                matched.matched_tokens = matched.matched_tokens.saturating_add(child.token_count);
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
            let mut current = &mut self.root;
            for page in pages {
                let child = current.children.entry(page.key).or_insert_with(|| {
                    self.resident_tokens = self
                        .resident_tokens
                        .saturating_add(u64::from(page.token_count));
                    PlainPrefixNode {
                        token_count: u64::from(page.token_count),
                        last_touched_at: Some(now),
                        children: BTreeMap::new(),
                    }
                });
                child.last_touched_at = Some(now);
                current = child;
            }
            while self.resident_tokens > max_tokens {
                let Some(path) = plain_coldest_leaf_path(&self.root) else {
                    break;
                };
                let removed = plain_remove_leaf_path(&mut self.root, &path);
                if removed == 0 {
                    break;
                }
                self.resident_tokens = self.resident_tokens.saturating_sub(removed);
            }
        }

        fn prune_expired(&mut self, now: Instant, ttl: Duration) {
            let removed = prune_expired_plain_children(&mut self.root, now, ttl);
            self.resident_tokens = self.resident_tokens.saturating_sub(removed);
        }
    }

    fn prune_expired_plain_children(
        node: &mut PlainPrefixNode,
        now: Instant,
        ttl: Duration,
    ) -> u64 {
        let mut removed_tokens = 0u64;
        for child in node.children.values_mut() {
            removed_tokens =
                removed_tokens.saturating_add(prune_expired_plain_children(child, now, ttl));
        }
        let expired_keys = node
            .children
            .iter()
            .filter(|(_, child)| {
                child
                    .last_touched_at
                    .is_some_and(|last_touched_at| now.duration_since(last_touched_at) > ttl)
            })
            .map(|(key, _)| *key)
            .collect::<Vec<_>>();
        for key in expired_keys {
            if let Some(child) = node.children.remove(&key) {
                removed_tokens = removed_tokens.saturating_add(plain_subtree_token_count(&child));
            }
        }
        removed_tokens
    }

    fn plain_coldest_leaf_path(node: &PlainPrefixNode) -> Option<Vec<u128>> {
        fn walk(
            node: &PlainPrefixNode,
            path: &mut Vec<u128>,
            best: &mut Option<(Instant, Vec<u128>)>,
        ) {
            if node.children.is_empty() {
                if let Some(last_touched_at) = node.last_touched_at {
                    match best {
                        Some((current_oldest, _)) if last_touched_at >= *current_oldest => {},
                        _ => *best = Some((last_touched_at, path.clone())),
                    }
                }
                return;
            }
            for (key, child) in &node.children {
                path.push(*key);
                walk(child, path, best);
                path.pop();
            }
        }

        let mut best = None;
        walk(node, &mut Vec::new(), &mut best);
        best.map(|(_, path)| path)
    }

    fn plain_remove_leaf_path(node: &mut PlainPrefixNode, path: &[u128]) -> u64 {
        fn remove_at(node: &mut PlainPrefixNode, path: &[u128]) -> u64 {
            let Some((key, remaining)) = path.split_first() else {
                return 0;
            };
            if remaining.is_empty() {
                return node
                    .children
                    .remove(key)
                    .map(|child| plain_subtree_token_count(&child))
                    .unwrap_or(0);
            }
            let Some(child) = node.children.get_mut(key) else {
                return 0;
            };
            let mut removed = remove_at(child, remaining);
            if child.children.is_empty() {
                let token_count = child.token_count;
                if node.children.remove(key).is_some() {
                    removed = removed.saturating_add(token_count);
                }
            }
            removed
        }

        remove_at(node, path)
    }

    fn plain_subtree_token_count(node: &PlainPrefixNode) -> u64 {
        let mut total = node.token_count;
        for child in node.children.values() {
            total = total.saturating_add(plain_subtree_token_count(child));
        }
        total
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
