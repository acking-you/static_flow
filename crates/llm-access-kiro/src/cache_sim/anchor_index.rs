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

#[derive(Debug)]
struct ConversationAnchorEntry {
    conversation_id: String,
    real_input_tokens: Option<i32>,
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

    /// Read the stored real input-token count for an anchor without bumping
    /// recency. Used by the pre-dispatch proactive-compaction gate to recover
    /// the *previous* turn's true (upstream contextUsage-derived) consumption,
    /// so the gate threshold does not drift on the local request estimate.
    /// Returns `None` if absent, expired, or never stored.
    pub fn get_real_input_tokens(
        &mut self,
        anchor: &str,
        now: Instant,
        ttl: Duration,
    ) -> Option<i32> {
        let cache = self.cache.as_mut()?;
        let entry = cache.peek(anchor)?;
        if now.duration_since(entry.last_touched_at) > ttl {
            cache.pop(anchor);
            return None;
        }
        entry.real_input_tokens
    }

    /// Record (or refresh) the conversation id behind an anchor, evicting
    /// expired entries first.
    pub fn insert(
        &mut self,
        anchor: String,
        conversation_id: String,
        real_input_tokens: Option<i32>,
        now: Instant,
        ttl: Duration,
        max_entries: usize,
    ) {
        self.ensure_capacity(max_entries);
        self.remove_expired(now, ttl);
        if let Some(cache) = self.cache.as_mut() {
            cache.put(anchor, ConversationAnchorEntry {
                conversation_id,
                real_input_tokens,
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

    use super::ConversationAnchorIndex;

    const TTL: Duration = Duration::from_secs(300);
    const MAX: usize = 16;

    #[test]
    fn real_input_tokens_round_trip() {
        let mut index = ConversationAnchorIndex::default();
        let now = Instant::now();
        index.insert("anchor-a".to_string(), "conv-1".to_string(), Some(812_345), now, TTL, MAX);
        assert_eq!(index.get_real_input_tokens("anchor-a", now, TTL), Some(812_345));
    }

    #[test]
    fn real_input_tokens_absent_when_not_stored() {
        let mut index = ConversationAnchorIndex::default();
        let now = Instant::now();
        index.insert("anchor-a".to_string(), "conv-1".to_string(), None, now, TTL, MAX);
        assert_eq!(index.get_real_input_tokens("anchor-a", now, TTL), None);
        assert_eq!(index.get_real_input_tokens("missing", now, TTL), None);
    }

    #[test]
    fn real_input_tokens_expire_with_ttl() {
        let mut index = ConversationAnchorIndex::default();
        let now = Instant::now();
        index.insert("anchor-a".to_string(), "conv-1".to_string(), Some(500_000), now, TTL, MAX);
        let later = now + TTL + Duration::from_secs(1);
        assert_eq!(index.get_real_input_tokens("anchor-a", later, TTL), None);
    }

    #[test]
    fn peek_does_not_bump_recency() {
        // get_real_input_tokens must not refresh last_touched_at, otherwise a
        // hot anchor would never expire. After peeking just before the TTL
        // boundary, the entry must still expire at the original deadline.
        let mut index = ConversationAnchorIndex::default();
        let now = Instant::now();
        index.insert("anchor-a".to_string(), "conv-1".to_string(), Some(700_000), now, TTL, MAX);
        let near = now + TTL - Duration::from_secs(1);
        assert_eq!(index.get_real_input_tokens("anchor-a", near, TTL), Some(700_000));
        let past = now + TTL + Duration::from_secs(1);
        assert_eq!(index.get_real_input_tokens("anchor-a", past, TTL), None);
    }
}
