use std::{
    cell::RefCell,
    fmt::Write as _,
    num::NonZeroUsize,
    sync::Mutex,
    time::{Duration, Instant},
};

use lru::LruCache;

const DEFAULT_KIRO_SESSION_AFFINITY_MAX_ENTRIES: usize = 4_096;
const MAX_KIRO_SESSION_AFFINITY_MAX_ENTRIES: usize = 65_536;
const DEFAULT_KIRO_SESSION_AFFINITY_TTL_SECONDS: u64 = 6 * 60 * 60;
const MIN_KIRO_SESSION_AFFINITY_TTL_SECONDS: u64 = 60;
const MAX_KIRO_SESSION_AFFINITY_TTL_SECONDS: u64 = 24 * 60 * 60;

thread_local! {
    static LOOKUP_KEY_BUF: RefCell<String> = RefCell::new(String::with_capacity(128));
}

#[derive(Debug, Clone)]
struct KiroSessionAffinityEntry {
    account_name: Box<str>,
    updated_at: Instant,
}

#[derive(Debug)]
pub(super) struct KiroSessionAffinity {
    entries: Mutex<LruCache<Box<str>, KiroSessionAffinityEntry>>,
    ttl: Duration,
}

impl KiroSessionAffinity {
    pub(super) fn from_env() -> Self {
        let max_entries = std::env::var("LLM_ACCESS_KIRO_SESSION_AFFINITY_MAX_ENTRIES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .map(|value| value.clamp(1, MAX_KIRO_SESSION_AFFINITY_MAX_ENTRIES))
            .unwrap_or(DEFAULT_KIRO_SESSION_AFFINITY_MAX_ENTRIES);
        let ttl_seconds = std::env::var("LLM_ACCESS_KIRO_SESSION_AFFINITY_TTL_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .map(|value| {
                value.clamp(
                    MIN_KIRO_SESSION_AFFINITY_TTL_SECONDS,
                    MAX_KIRO_SESSION_AFFINITY_TTL_SECONDS,
                )
            })
            .unwrap_or(DEFAULT_KIRO_SESSION_AFFINITY_TTL_SECONDS);
        Self::new(max_entries, Duration::from_secs(ttl_seconds))
    }

    fn new(max_entries: usize, ttl: Duration) -> Self {
        let capacity = NonZeroUsize::new(max_entries.max(1)).expect("capacity is non-zero");
        Self {
            entries: Mutex::new(LruCache::new(capacity)),
            ttl,
        }
    }

    pub(super) fn remember(&self, key_id: &str, session_id: &str, account_name: &str) {
        self.remember_at(key_id, session_id, account_name, Instant::now());
    }

    fn remember_at(&self, key_id: &str, session_id: &str, account_name: &str, now: Instant) {
        let Some(key) = affinity_key(key_id, session_id) else { return };
        let Some(account_name) = trimmed_box(account_name) else {
            return;
        };
        self.entries
            .lock()
            .expect("kiro session affinity mutex")
            .put(key, KiroSessionAffinityEntry {
                account_name,
                updated_at: now,
            });
    }

    pub(super) fn lookup(&self, key_id: &str, session_id: &str) -> Option<String> {
        self.lookup_at(key_id, session_id, Instant::now())
    }

    fn lookup_at(&self, key_id: &str, session_id: &str, now: Instant) -> Option<String> {
        let (key_id, session_id) = trimmed_key_parts(key_id, session_id)?;
        LOOKUP_KEY_BUF.with(|buf| {
            let mut buf = buf.borrow_mut();
            build_affinity_key(&mut buf, key_id, session_id);

            let mut entries = self.entries.lock().expect("kiro session affinity mutex");
            let entry = entries.get(buf.as_str())?;
            if now.saturating_duration_since(entry.updated_at) > self.ttl {
                entries.pop(buf.as_str());
                return None;
            }
            Some(entry.account_name.to_string())
        })
    }
}

fn affinity_key(key_id: &str, session_id: &str) -> Option<Box<str>> {
    let (key_id, session_id) = trimmed_key_parts(key_id, session_id)?;
    let mut key = String::with_capacity(key_id.len() + session_id.len() + 20);
    build_affinity_key(&mut key, key_id, session_id);
    Some(key.into_boxed_str())
}

fn build_affinity_key(buf: &mut String, key_id: &str, session_id: &str) {
    buf.clear();
    let _ = write!(buf, "{}:", key_id.len());
    buf.push_str(key_id);
    buf.push_str(session_id);
}

fn trimmed_key_parts<'a>(key_id: &'a str, session_id: &'a str) -> Option<(&'a str, &'a str)> {
    let key_id = key_id.trim();
    let session_id = session_id.trim();
    (!key_id.is_empty() && !session_id.is_empty()).then_some((key_id, session_id))
}

fn trimmed_box(value: &str) -> Option<Box<str>> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| Box::<str>::from(trimmed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_returns_remembered_account_for_same_key_and_session() {
        let affinity = KiroSessionAffinity::new(4, Duration::from_secs(60));
        affinity.remember("key-a", "session-a", "account-a");

        assert_eq!(affinity.lookup("key-a", "session-a").as_deref(), Some("account-a"));
    }

    #[test]
    fn lookup_scopes_same_session_by_key_id() {
        let affinity = KiroSessionAffinity::new(4, Duration::from_secs(60));
        affinity.remember("key-a", "session-a", "account-a");
        affinity.remember("key-b", "session-a", "account-b");

        assert_eq!(affinity.lookup("key-a", "session-a").as_deref(), Some("account-a"));
        assert_eq!(affinity.lookup("key-b", "session-a").as_deref(), Some("account-b"));
    }

    #[test]
    fn lookup_distinguishes_separator_like_key_parts() {
        let affinity = KiroSessionAffinity::new(4, Duration::from_secs(60));
        affinity.remember("key:a", "bc", "account-a");
        affinity.remember("key", "a:bc", "account-b");

        assert_eq!(affinity.lookup("key:a", "bc").as_deref(), Some("account-a"));
        assert_eq!(affinity.lookup("key", "a:bc").as_deref(), Some("account-b"));
    }

    #[test]
    fn lookup_removes_expired_entry() {
        let affinity = KiroSessionAffinity::new(4, Duration::from_secs(60));
        let now = Instant::now();
        affinity.remember_at("key-a", "session-a", "account-a", now);

        assert_eq!(
            affinity
                .lookup_at("key-a", "session-a", now + Duration::from_secs(61))
                .as_deref(),
            None
        );
        assert_eq!(affinity.lookup_at("key-a", "session-a", now).as_deref(), None);
    }

    #[test]
    fn capacity_evicts_least_recently_used_entry() {
        let affinity = KiroSessionAffinity::new(2, Duration::from_secs(60));
        affinity.remember("key-a", "session-a", "account-a");
        affinity.remember("key-a", "session-b", "account-b");
        assert_eq!(affinity.lookup("key-a", "session-a").as_deref(), Some("account-a"));
        affinity.remember("key-a", "session-c", "account-c");

        assert_eq!(affinity.lookup("key-a", "session-b").as_deref(), None);
        assert_eq!(affinity.lookup("key-a", "session-a").as_deref(), Some("account-a"));
        assert_eq!(affinity.lookup("key-a", "session-c").as_deref(), Some("account-c"));
    }
}
