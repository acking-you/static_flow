//! `KiroCacheSimulator`: the public entry point for Kiro prefix-cache
//! simulation.
//!
//! It owns the shared prefix tree and the conversation-anchor index, and drives
//! them from prompt projections built off a corrected `ConversationState`.

use std::time::{Duration, Instant};

use serde::Serialize;

use super::{
    anchor_index::{ConversationAnchorIndex, ConversationAnchorRuntimeStats},
    prefix_tree::{PrefixCacheMatch, PrefixTree, PrefixTreeRuntimeStats},
    projection::{PromptProjection, RuntimePromptProjection, PREFIX_CACHE_PAGE_SIZE},
};
use crate::wire::AssistantMessage;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct KiroCacheRuntimeStats {
    pub mode: KiroCacheSimulationMode,
    pub page_size_tokens: usize,
    pub prefix_tree: PrefixTreeRuntimeStats,
    pub conversation_anchors: ConversationAnchorRuntimeStats,
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
