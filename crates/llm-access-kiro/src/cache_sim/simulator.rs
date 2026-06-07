//! `KiroCacheSimulator`: the public entry point for Kiro prefix-cache
//! simulation.
//!
//! It owns the shared prefix tree and the conversation-anchor index, and drives
//! them from prompt projections built off a corrected `ConversationState`.

use std::time::{Duration, Instant};

use chrono::Utc;
use serde::Serialize;

use super::{
    anchor_index::{AnchorTokenCounts, ConversationAnchorIndex, ConversationAnchorRuntimeStats},
    prefix_tree::{skip_prefix_section, PrefixCacheMatch, PrefixTree, PrefixTreeRuntimeStats},
    projection::{PromptProjection, RuntimePromptProjection, PREFIX_CACHE_PAGE_SIZE},
    snapshot::{
        decode_frame, finalize_frame, union_anchor_rows, write_varint, DecodedFrame,
        KiroSnapshotImportOutcome, SnapshotCaps, SnapshotHeader, SnapshotReader,
    },
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
            None,
            now,
            config.conversation_anchor_ttl,
            config.conversation_anchor_max_entries,
        );
    }

    /// Recover the previous turn's cached input-token counts (real + local) for
    /// the conversation that produced this prompt prefix, if still cached.
    /// Drives the proactive-compaction gate's threshold so it does not rely on
    /// the local request estimate alone. Read-only on recency.
    pub fn recover_token_counts_from_runtime_projection(
        &self,
        projection: &RuntimePromptProjection,
        config: KiroCacheSimulationConfig,
        now: Instant,
    ) -> Option<AnchorTokenCounts> {
        let mut index = self.anchor_index.lock();
        index.recover_token_counts(
            projection.lookup_anchor_hash(),
            now,
            config.conversation_anchor_ttl,
        )
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "one over the limit after adding token_counts; the args are cohesive (projection \
                  + recorded facts + config + clock) and a borrowed param struct would add more \
                  surface than it removes"
    )]
    pub fn record_success_from_runtime_projection(
        &self,
        projection: &RuntimePromptProjection,
        assistant_message: &AssistantMessage,
        conversation_id: &str,
        token_counts: Option<AnchorTokenCounts>,
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
            token_counts,
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

    /// Serialize the live simulator state into a gzip-framed snapshot blob.
    ///
    /// TTL pruning runs first so stale state is never persisted. Returns `None`
    /// only when there is nothing worth saving (Formula mode with no anchors);
    /// anchors are persisted even in Formula mode because they drive the
    /// proactive-compaction gate.
    pub fn export_snapshot(
        &self,
        config: KiroCacheSimulationConfig,
        caps: SnapshotCaps,
        now: Instant,
    ) -> Option<Vec<u8>> {
        let prefix_tree_mode = matches!(config.mode, KiroCacheSimulationMode::PrefixTree);
        let mut raw = Vec::new();
        let mut prefix_section = Vec::new();
        let resident_tokens = {
            let mut tree = self.prefix_tree.lock();
            tree.prune_expired(now, config.prefix_cache_entry_ttl);
            if prefix_tree_mode {
                tree.encode_section(&mut prefix_section, now, caps.max_tokens);
                tree.resident_tokens()
            } else {
                // Empty prefix section (root with zero children).
                write_varint(&mut prefix_section, 0);
                0
            }
        };
        let mut anchor_section = Vec::new();
        let anchors_empty = {
            let mut index = self.anchor_index.lock();
            index.remove_expired(now, config.conversation_anchor_ttl);
            index.encode_section(&mut anchor_section, now, caps.max_anchor_entries);
            index.is_empty()
        };
        // Nothing worth persisting. Skipping here avoids writing an empty own
        // key that would otherwise shadow warm peer snapshots on the next
        // restart (a cold node's first scheduled flush would do exactly that).
        if resident_tokens == 0 && anchors_empty {
            return None;
        }
        SnapshotHeader {
            snapshot_unix_ms: Utc::now().timestamp_millis(),
            resident_tokens,
        }
        .write(&mut raw);
        raw.extend_from_slice(&prefix_section);
        raw.extend_from_slice(&anchor_section);
        match finalize_frame(raw) {
            Ok(blob) => Some(blob),
            Err(err) => {
                tracing::warn!(error = %err, "failed to finalize kiro cache snapshot");
                None
            },
        }
    }

    /// Restore simulator state from this node's snapshot plus peer snapshots.
    ///
    /// Prefix tree: own snapshot wins; otherwise the newest decodable peer
    /// seeds it (single source, no page-level merge). Anchors: union across own
    /// and all peers (newest-touch wins), then capped. Every decode failure is
    /// counted and skipped; this never panics and never fails startup.
    pub fn import_snapshot(
        &self,
        own: Option<&[u8]>,
        peers: &[Vec<u8>],
        config: KiroCacheSimulationConfig,
        caps: SnapshotCaps,
        now: Instant,
    ) -> KiroSnapshotImportOutcome {
        let now_unix_ms = Utc::now().timestamp_millis();
        let ttl = config.prefix_cache_entry_ttl;
        let anchor_ttl = config.conversation_anchor_ttl;
        let max_tokens = caps.max_tokens.unwrap_or(config.prefix_cache_max_tokens);
        let max_anchor_entries = caps
            .max_anchor_entries
            .unwrap_or(config.conversation_anchor_max_entries);

        let mut outcome = KiroSnapshotImportOutcome::default();
        let own_frame = decode_blob(own, &mut outcome.decode_errors);
        let mut peer_frames: Vec<DecodedFrame> = Vec::new();
        for peer in peers {
            if let Some(frame) = decode_blob(Some(peer.as_slice()), &mut outcome.decode_errors) {
                peer_frames.push(frame);
            }
        }

        // Prefix tree: prefer own, else newest peer — but an empty own snapshot
        // (e.g. a cold node's first flush, or one whose tree expired on decode)
        // must not shadow a warm peer. Walk candidates own-first then by
        // recency, installing the first tree that actually holds tokens.
        let mut candidates: Vec<(&DecodedFrame, bool)> = Vec::new();
        if let Some(frame) = own_frame.as_ref() {
            candidates.push((frame, true));
        }
        let mut sorted_peers: Vec<&DecodedFrame> = peer_frames.iter().collect();
        sorted_peers.sort_by_key(|frame| std::cmp::Reverse(frame.header.snapshot_unix_ms));
        candidates.extend(sorted_peers.into_iter().map(|frame| (frame, false)));

        for (frame, is_own) in candidates {
            let mut reader = SnapshotReader::new(&frame.sections);
            match PrefixTree::decode_section(
                &mut reader,
                frame.header.snapshot_unix_ms,
                now,
                now_unix_ms,
                ttl,
            ) {
                Ok(mut tree) => {
                    tree.enforce_token_budget(max_tokens);
                    if tree.resident_tokens() == 0 {
                        // Empty tree: do not let it shadow a warmer source.
                        continue;
                    }
                    outcome.prefix_resident_tokens = tree.resident_tokens();
                    outcome.prefix_from_own = is_own;
                    outcome.prefix_from_peer = !is_own;
                    *self.prefix_tree.lock() = tree;
                    break;
                },
                Err(err) => {
                    tracing::warn!(error = %err, "failed to decode kiro prefix snapshot section");
                },
            }
        }

        // Anchors: union across own + all peers.
        let mut rows = Vec::new();
        let frames = own_frame.iter().chain(peer_frames.iter());
        for frame in frames {
            let mut reader = SnapshotReader::new(&frame.sections);
            if skip_prefix_section(&mut reader).is_err() {
                continue;
            }
            match ConversationAnchorIndex::decode_section(
                &mut reader,
                frame.header.snapshot_unix_ms,
            ) {
                Ok(decoded) => rows.extend(decoded),
                Err(_) => continue,
            }
        }
        let merged = union_anchor_rows(rows, now_unix_ms, anchor_ttl, max_anchor_entries);
        {
            let mut index = self.anchor_index.lock();
            index.rebuild_from_rows(merged, now, anchor_ttl, max_anchor_entries);
            outcome.anchor_entries = index.len();
        }
        outcome
    }
}

fn decode_blob(blob: Option<&[u8]>, errors: &mut usize) -> Option<DecodedFrame> {
    let blob = blob?;
    match decode_frame(blob) {
        Ok(frame) => Some(frame),
        Err(err) => {
            tracing::warn!(error = %err, "failed to decode kiro cache snapshot blob");
            *errors += 1;
            None
        },
    }
}
