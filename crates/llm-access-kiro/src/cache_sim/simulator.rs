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
        decode_frame, finalize_frame, write_varint, AnchorUnion, DecodedFrame,
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
        let max_tokens = caps.max_tokens.unwrap_or(config.prefix_cache_max_tokens);
        let max_anchor_entries = caps
            .max_anchor_entries
            .unwrap_or(config.conversation_anchor_max_entries);
        let ctx = RestoreCtx {
            now,
            now_unix_ms: Utc::now().timestamp_millis(),
            ttl: config.prefix_cache_entry_ttl,
            anchor_ttl: config.conversation_anchor_ttl,
            max_tokens,
            // Each retained page costs >= 1 token, so the page budget tracks the
            // token budget; a section that cannot fit it is rejected on decode.
            max_pages: usize::try_from(max_tokens).unwrap_or(usize::MAX),
            max_anchor_entries,
        };

        let mut outcome = KiroSnapshotImportOutcome::default();
        let mut best: Option<PrefixCandidate> = None;
        let mut anchors = AnchorUnion::new(ctx.now_unix_ms, ctx.anchor_ttl, ctx.max_anchor_entries);

        // Stream frames one at a time so only a single decompressed frame is
        // resident at once: decode, fold its prefix candidate, fold its bounded
        // anchor rows into the running union (which dedups + trims to
        // max_anchor_entries each fold), then drop its section bytes before the
        // next peer. Own is folded first so a non-empty own tree wins; among
        // peers the newest snapshot wins, and an empty/expired tree never
        // shadows a warm source.
        if let Some(frame) = decode_blob(own, &mut outcome.decode_errors) {
            fold_restore_frame(&frame, true, &ctx, &mut best, &mut anchors);
        }
        for peer in peers {
            if let Some(frame) = decode_blob(Some(peer.as_slice()), &mut outcome.decode_errors) {
                fold_restore_frame(&frame, false, &ctx, &mut best, &mut anchors);
            }
        }

        if let Some(candidate) = best {
            outcome.prefix_resident_tokens = candidate.tree.resident_tokens();
            outcome.prefix_from_own = candidate.from_own;
            outcome.prefix_from_peer = !candidate.from_own;
            *self.prefix_tree.lock() = candidate.tree;
        }

        let merged = anchors.finish();
        {
            let mut index = self.anchor_index.lock();
            index.rebuild_from_rows(merged, ctx.now, ctx.anchor_ttl, ctx.max_anchor_entries);
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

/// Scalar restore parameters threaded into per-frame folding, kept in one
/// struct so `fold_restore_frame` stays within the argument budget.
struct RestoreCtx {
    now: Instant,
    now_unix_ms: i64,
    ttl: Duration,
    anchor_ttl: Duration,
    max_tokens: u64,
    max_pages: usize,
    max_anchor_entries: usize,
}

/// The best prefix tree found so far while streaming decoded frames.
struct PrefixCandidate {
    tree: PrefixTree,
    from_own: bool,
    snapshot_unix_ms: i64,
}

/// Whether a `(from_own, snapshot_unix_ms)` tree should replace the current
/// best prefix candidate. Own always wins; among peers the newest snapshot
/// wins. Keeps selection order-independent across the streamed frames.
fn prefix_candidate_wins(
    best: Option<&PrefixCandidate>,
    from_own: bool,
    snapshot_unix_ms: i64,
) -> bool {
    match best {
        None => true,
        Some(existing) if existing.from_own => false,
        Some(_) if from_own => true,
        Some(existing) => snapshot_unix_ms > existing.snapshot_unix_ms,
    }
}

/// Decode one frame's prefix tree (folding it into `best` when non-empty and it
/// wins) and append its bounded anchor rows. The caller drops the frame's
/// section bytes after this returns, so peak memory stays at one decompressed
/// frame plus the retained best tree rather than every peer frame at once.
fn fold_restore_frame(
    frame: &DecodedFrame,
    from_own: bool,
    ctx: &RestoreCtx,
    best: &mut Option<PrefixCandidate>,
    anchors: &mut AnchorUnion,
) {
    let mut reader = SnapshotReader::new(&frame.sections);
    match PrefixTree::decode_section(
        &mut reader,
        frame.header.snapshot_unix_ms,
        ctx.now,
        ctx.now_unix_ms,
        ctx.ttl,
        ctx.max_pages,
    ) {
        Ok(mut tree) => {
            tree.enforce_token_budget(ctx.max_tokens);
            if tree.resident_tokens() > 0
                && prefix_candidate_wins(best.as_ref(), from_own, frame.header.snapshot_unix_ms)
            {
                *best = Some(PrefixCandidate {
                    tree,
                    from_own,
                    snapshot_unix_ms: frame.header.snapshot_unix_ms,
                });
            }
        },
        Err(err) => {
            tracing::warn!(error = %err, "failed to decode kiro prefix snapshot section");
        },
    }

    // Anchors from a fresh reader so a rejected/oversized prefix does not stop
    // anchor recovery for this frame. Folding into the running union dedups and
    // trims to max_anchor_entries now, so memory does not scale with peer count.
    let mut anchor_reader = SnapshotReader::new(&frame.sections);
    if skip_prefix_section(&mut anchor_reader).is_err() {
        return;
    }
    if let Ok(decoded) = ConversationAnchorIndex::decode_section(
        &mut anchor_reader,
        frame.header.snapshot_unix_ms,
        ctx.max_anchor_entries,
    ) {
        anchors.fold(decoded);
    }
}
