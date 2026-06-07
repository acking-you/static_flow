//! Conversation-anchor recovery index.
//!
//! Maps resume-anchor hashes to the upstream conversation id via a TTL-bounded
//! LRU, letting the simulator recover the conversation that produced a given
//! prompt prefix after a successful turn.

use std::{
    num::NonZeroUsize,
    time::{Duration, Instant},
};

use lru::LruCache;
use serde::Serialize;

use super::snapshot::{
    write_varint, write_zigzag, DecodedAnchor, RebuildRow, SnapshotError, SnapshotReader,
};

/// Minimum wire bytes for one encoded anchor row: 32B key + 1B conv_len varint
/// (empty id) + 1B token-counts flag + 1B age varint. Used to bound decode
/// preallocation against the bytes actually present, never the untrusted count.
const MIN_ANCHOR_ROW_WIRE_BYTES: usize = 35;
/// Hard upper bound on one anchor's `conversation_id` length. Upstream ids are
/// short opaque strings (UUID-shaped), so this is far above any real value; it
/// caps the per-row `String` so a corrupt wire varint cannot make a single
/// retained anchor allocate up to the whole decompressed frame.
const MAX_ANCHOR_CONVERSATION_ID_BYTES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct ConversationAnchorRuntimeStats {
    pub entries: usize,
    pub max_entries: usize,
    pub estimated_memory_bytes: u64,
}

/// Per-turn input-token counts cached on a conversation anchor, used by the
/// proactive-compaction gate to estimate the *current* turn's real consumption.
///
/// `real` is the upstream contextUsage-derived count (accurate where the local
/// estimate drifts); `local` is the `count_all_tokens` estimate for the same
/// turn. Storing both lets the next turn add its own local delta on top of the
/// previous real value: `real_prev + max(0, local_now - local_prev)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnchorTokenCounts {
    pub real_input_tokens: i32,
    pub local_input_tokens: i32,
}

#[derive(Debug)]
struct ConversationAnchorEntry {
    conversation_id: String,
    token_counts: Option<AnchorTokenCounts>,
    last_touched_at: Instant,
}

/// TTL-bounded LRU mapping a resume-anchor hash to its conversation id.
///
/// Owned by `KiroCacheSimulator` behind a mutex. The backing cache is created
/// lazily on first use so the default value stays cheap.
#[derive(Debug, Default)]
pub struct ConversationAnchorIndex {
    cache: Option<LruCache<String, ConversationAnchorEntry>>,
}

impl ConversationAnchorIndex {
    /// Look up a conversation id by anchor, refreshing its recency on hit and
    /// dropping it if it has already outlived `ttl`.
    pub fn get(
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

    /// Read the stored per-turn token counts for an anchor without bumping
    /// recency. Used by the pre-dispatch proactive-compaction gate to recover
    /// the *previous* turn's true (upstream contextUsage-derived) consumption,
    /// so the gate threshold does not drift on the local request estimate.
    /// Returns `None` if absent, expired, or never stored.
    pub fn recover_token_counts(
        &mut self,
        anchor: &str,
        now: Instant,
        ttl: Duration,
    ) -> Option<AnchorTokenCounts> {
        let cache = self.cache.as_mut()?;
        // Extract the values first so the immutable borrow from `peek` is
        // released before the conditional `pop` mutation. `saturating_duration_since`
        // avoids a panic if `now` precedes `last_touched_at` under monotonic
        // clock drift / virtualized test clocks.
        let (expired, token_counts) = match cache.peek(anchor) {
            Some(entry) => {
                (now.saturating_duration_since(entry.last_touched_at) > ttl, entry.token_counts)
            },
            None => return None,
        };
        if expired {
            cache.pop(anchor);
            return None;
        }
        token_counts
    }

    /// Record (or refresh) the conversation id behind an anchor, evicting
    /// expired entries first.
    pub fn insert(
        &mut self,
        anchor: String,
        conversation_id: String,
        token_counts: Option<AnchorTokenCounts>,
        now: Instant,
        ttl: Duration,
        max_entries: usize,
    ) {
        self.ensure_capacity(max_entries);
        self.remove_expired(now, ttl);
        if let Some(cache) = self.cache.as_mut() {
            cache.put(anchor, ConversationAnchorEntry {
                conversation_id,
                token_counts,
                last_touched_at: now,
            });
        }
    }

    /// Resize the backing LRU to `max_entries`, preserving recency order.
    pub fn ensure_capacity(&mut self, max_entries: usize) {
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

    /// Evict the least-recently-used entries that have outlived `ttl`.
    pub fn remove_expired(&mut self, now: Instant, ttl: Duration) {
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

    /// Report current entry count and estimated memory footprint.
    pub fn snapshot_stats(&self, max_entries: usize) -> ConversationAnchorRuntimeStats {
        let entries = self.cache.as_ref().map_or(0, LruCache::len);
        ConversationAnchorRuntimeStats {
            entries,
            max_entries: max_entries.max(1),
            estimated_memory_bytes: estimate_anchor_index_memory_bytes(entries),
        }
    }

    /// Whether the backing cache holds no entries.
    pub(super) fn is_empty(&self) -> bool {
        match self.cache.as_ref() {
            Some(cache) => cache.is_empty(),
            None => true,
        }
    }

    /// Current entry count.
    pub(super) fn len(&self) -> usize {
        self.cache.as_ref().map_or(0, LruCache::len)
    }

    /// Serialize the index as a flat anchor section into `out`, most-recently
    /// used first. With `cap_entries`, only the hottest `cap_entries` rows are
    /// written. The 64-hex key is stored as its raw 32 bytes to halve its size.
    pub(super) fn encode_section(
        &self,
        out: &mut Vec<u8>,
        now: Instant,
        cap_entries: Option<usize>,
    ) {
        let mut rows: Vec<u8> = Vec::new();
        let mut count = 0u64;
        if let Some(cache) = self.cache.as_ref() {
            for (hex_key, entry) in cache.iter() {
                if cap_entries.is_some_and(|max| count as usize >= max) {
                    break;
                }
                let Ok(raw) = hex::decode(hex_key) else {
                    continue;
                };
                if raw.len() != 32 {
                    continue;
                }
                rows.extend_from_slice(&raw);
                write_varint(&mut rows, entry.conversation_id.len() as u64);
                rows.extend_from_slice(entry.conversation_id.as_bytes());
                match entry.token_counts {
                    Some(counts) => {
                        rows.push(1);
                        write_zigzag(&mut rows, i64::from(counts.real_input_tokens));
                        write_zigzag(&mut rows, i64::from(counts.local_input_tokens));
                    },
                    None => rows.push(0),
                }
                let age_secs = now
                    .saturating_duration_since(entry.last_touched_at)
                    .as_secs();
                write_varint(&mut rows, age_secs);
                count += 1;
            }
        }
        write_varint(out, count);
        out.extend_from_slice(&rows);
    }

    /// Decode a flat anchor section into source rows. No TTL filtering or
    /// insertion happens here; the cross-node union owns recency and capacity.
    /// `max_rows` caps how many rows are materialized so an untrusted `count`
    /// cannot drive an unbounded allocation.
    pub(super) fn decode_section(
        reader: &mut SnapshotReader<'_>,
        snapshot_unix_ms: i64,
        max_rows: usize,
    ) -> Result<Vec<DecodedAnchor>, SnapshotError> {
        let count = reader.read_varint()?;
        // Never preallocate from the untrusted wire `count`. Bound it by both the
        // caller's row ceiling and the rows the remaining bytes could physically
        // hold (each row is at least MIN_ANCHOR_ROW_WIRE_BYTES).
        let byte_bound = reader.remaining() / MIN_ANCHOR_ROW_WIRE_BYTES;
        let prealloc = usize::try_from(count)
            .unwrap_or(usize::MAX)
            .min(max_rows)
            .min(byte_bound);
        let mut rows = Vec::with_capacity(prealloc);
        for _ in 0..count {
            // Stop at the ceiling. The encoder writes most-recently-used rows
            // first, so the retained rows are the hottest; the remainder is
            // dropped before the cross-node union (which caps again anyway).
            if rows.len() >= max_rows {
                break;
            }
            let hex_key = hex::encode(reader.read_bytes(32)?);
            let conv_len =
                usize::try_from(reader.read_varint()?).map_err(|_| SnapshotError::Malformed)?;
            // Reject an over-long conversation id rather than allocate it; the
            // writer only emits short upstream ids.
            if conv_len > MAX_ANCHOR_CONVERSATION_ID_BYTES {
                return Err(SnapshotError::Malformed);
            }
            let conv_bytes = reader.read_bytes(conv_len)?;
            let conversation_id =
                String::from_utf8(conv_bytes.to_vec()).map_err(|_| SnapshotError::Malformed)?;
            let token_counts = if reader.read_u8()? == 1 {
                // Reject out-of-range token counts rather than silently wrapping
                // them; a corrupt value would otherwise flow into the
                // proactive-compaction gate's effective-token estimate.
                let real_input_tokens =
                    i32::try_from(reader.read_zigzag()?).map_err(|_| SnapshotError::Malformed)?;
                let local_input_tokens =
                    i32::try_from(reader.read_zigzag()?).map_err(|_| SnapshotError::Malformed)?;
                Some(AnchorTokenCounts {
                    real_input_tokens,
                    local_input_tokens,
                })
            } else {
                None
            };
            let age_secs = reader.read_varint()?;
            rows.push(DecodedAnchor {
                hex: hex_key,
                conversation_id,
                token_counts,
                age_secs,
                snapshot_unix_ms,
            });
        }
        Ok(rows)
    }

    /// Replace the index contents from union rows (ordered oldest-first), then
    /// drop anything already past `ttl`.
    pub(super) fn rebuild_from_rows(
        &mut self,
        rows: Vec<RebuildRow>,
        now: Instant,
        ttl: Duration,
        max_entries: usize,
    ) {
        self.ensure_capacity(max_entries);
        if let Some(cache) = self.cache.as_mut() {
            cache.clear();
        }
        for row in rows {
            let last_touched_at = now
                .checked_sub(Duration::from_secs(row.eff_age_secs))
                .unwrap_or(now);
            if let Some(cache) = self.cache.as_mut() {
                cache.put(row.hex, ConversationAnchorEntry {
                    conversation_id: row.conversation_id,
                    token_counts: row.token_counts,
                    last_touched_at,
                });
            }
        }
        self.remove_expired(now, ttl);
    }
}

fn estimate_anchor_index_memory_bytes(entries: usize) -> u64 {
    let entry_bytes = std::mem::size_of::<ConversationAnchorEntry>();
    let key_bytes = std::mem::size_of::<String>();
    entries.saturating_mul(entry_bytes.saturating_add(key_bytes)) as u64
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{AnchorTokenCounts, ConversationAnchorIndex};

    const TTL: Duration = Duration::from_secs(300);
    const MAX: usize = 16;

    fn counts(real: i32, local: i32) -> AnchorTokenCounts {
        AnchorTokenCounts {
            real_input_tokens: real,
            local_input_tokens: local,
        }
    }

    #[test]
    fn anchor_section_round_trip_recovers_entries() {
        use crate::cache_sim::snapshot::{union_anchor_rows, SnapshotReader};

        let key_a = "a".repeat(64);
        let key_b = "b".repeat(64);
        let mut index = ConversationAnchorIndex::default();
        let now = Instant::now();
        index.insert(
            key_a.clone(),
            "conv-a".to_string(),
            Some(counts(812_345, 760_000)),
            now,
            TTL,
            MAX,
        );
        index.insert(key_b.clone(), "conv-b".to_string(), None, now, TTL, MAX);

        let snapshot_unix_ms = 1_700_000_000_000i64;
        let mut buf = Vec::new();
        index.encode_section(&mut buf, now, None);

        let mut reader = SnapshotReader::new(&buf);
        let rows = ConversationAnchorIndex::decode_section(&mut reader, snapshot_unix_ms, MAX)
            .expect("decode anchor section");
        assert_eq!(rows.len(), 2);
        let merged = union_anchor_rows(rows, snapshot_unix_ms, TTL, MAX);

        let mut restored = ConversationAnchorIndex::default();
        let restore_now = Instant::now();
        restored.rebuild_from_rows(merged, restore_now, TTL, MAX);

        assert_eq!(restored.get(&key_a, restore_now, TTL, MAX), Some("conv-a".to_string()));
        assert_eq!(
            restored.recover_token_counts(&key_a, restore_now, TTL),
            Some(counts(812_345, 760_000))
        );
        assert_eq!(restored.get(&key_b, restore_now, TTL, MAX), Some("conv-b".to_string()));
        assert_eq!(restored.recover_token_counts(&key_b, restore_now, TTL), None);
    }

    #[test]
    fn anchor_section_decode_rejects_out_of_range_token_count() {
        use crate::cache_sim::snapshot::{
            write_varint, write_zigzag, SnapshotError, SnapshotReader,
        };

        // Hand-build a one-entry anchor section whose real_input_tokens zigzag
        // exceeds i32::MAX. It must be rejected as malformed rather than wrap
        // into a bogus token count that would mislead the compaction gate.
        let mut buf = Vec::new();
        write_varint(&mut buf, 1); // anchor_count
        buf.extend_from_slice(&[0xaa; 32]); // 32B raw key
        write_varint(&mut buf, 4); // conv_id_len
        buf.extend_from_slice(b"conv");
        buf.push(1); // token_counts present
        write_zigzag(&mut buf, i64::from(i32::MAX) + 1); // out of i32 range
        write_zigzag(&mut buf, 0);
        write_varint(&mut buf, 0); // age_secs

        let mut reader = SnapshotReader::new(&buf);
        assert!(matches!(
            ConversationAnchorIndex::decode_section(&mut reader, 0, MAX),
            Err(SnapshotError::Malformed)
        ));
    }

    #[test]
    fn anchor_section_decode_caps_rows_at_max() {
        use crate::cache_sim::snapshot::SnapshotReader;

        // Encode five entries but decode with max_rows = 2: only the first two
        // (most-recently-used) are materialized, bounding the allocation.
        let mut index = ConversationAnchorIndex::default();
        let now = Instant::now();
        for i in 0..5u8 {
            // 64 identical hex digits -> a valid 32-byte raw key.
            let key = String::from_utf8(vec![b'a' + i; 64]).expect("ascii hex key");
            index.insert(key, format!("conv-{i}"), None, now, TTL, 64);
        }
        let mut buf = Vec::new();
        index.encode_section(&mut buf, now, None);

        let mut reader = SnapshotReader::new(&buf);
        let rows = ConversationAnchorIndex::decode_section(&mut reader, 0, 2)
            .expect("decode anchor section");
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn anchor_section_decode_rejects_oversized_conversation_id() {
        use crate::cache_sim::snapshot::{write_varint, SnapshotError, SnapshotReader};

        // A conversation_id longer than the hard cap is rejected right after the
        // length varint, before the String is allocated, so one corrupt anchor
        // cannot pull in up to a whole decompressed frame.
        let mut buf = Vec::new();
        write_varint(&mut buf, 1); // anchor_count
        buf.extend_from_slice(&[0xaa; 32]); // 32B raw key
        write_varint(&mut buf, (super::MAX_ANCHOR_CONVERSATION_ID_BYTES + 1) as u64);

        let mut reader = SnapshotReader::new(&buf);
        assert!(matches!(
            ConversationAnchorIndex::decode_section(&mut reader, 0, MAX),
            Err(SnapshotError::Malformed)
        ));
    }

    #[test]
    fn token_counts_round_trip() {
        let mut index = ConversationAnchorIndex::default();
        let now = Instant::now();
        index.insert(
            "anchor-a".to_string(),
            "conv-1".to_string(),
            Some(counts(812_345, 760_000)),
            now,
            TTL,
            MAX,
        );
        assert_eq!(
            index.recover_token_counts("anchor-a", now, TTL),
            Some(counts(812_345, 760_000))
        );
    }

    #[test]
    fn token_counts_absent_when_not_stored() {
        let mut index = ConversationAnchorIndex::default();
        let now = Instant::now();
        index.insert("anchor-a".to_string(), "conv-1".to_string(), None, now, TTL, MAX);
        assert_eq!(index.recover_token_counts("anchor-a", now, TTL), None);
        assert_eq!(index.recover_token_counts("missing", now, TTL), None);
    }

    #[test]
    fn token_counts_expire_with_ttl() {
        let mut index = ConversationAnchorIndex::default();
        let now = Instant::now();
        index.insert(
            "anchor-a".to_string(),
            "conv-1".to_string(),
            Some(counts(500_000, 480_000)),
            now,
            TTL,
            MAX,
        );
        let later = now + TTL + Duration::from_secs(1);
        assert_eq!(index.recover_token_counts("anchor-a", later, TTL), None);
    }

    #[test]
    fn peek_does_not_bump_recency() {
        // recover_token_counts must not refresh last_touched_at, otherwise a hot
        // anchor would never expire. After peeking just before the TTL boundary,
        // the entry must still expire at the original deadline.
        let mut index = ConversationAnchorIndex::default();
        let now = Instant::now();
        index.insert(
            "anchor-a".to_string(),
            "conv-1".to_string(),
            Some(counts(700_000, 690_000)),
            now,
            TTL,
            MAX,
        );
        let near = now + TTL - Duration::from_secs(1);
        assert_eq!(
            index.recover_token_counts("anchor-a", near, TTL),
            Some(counts(700_000, 690_000))
        );
        let past = now + TTL + Duration::from_secs(1);
        assert_eq!(index.recover_token_counts("anchor-a", past, TTL), None);
    }
}
