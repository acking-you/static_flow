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
