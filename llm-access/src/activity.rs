//! Live provider request activity tracking.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
    time::Instant,
};

/// Current live request activity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RequestActivitySnapshot {
    pub(crate) rpm: u32,
    pub(crate) in_flight: u32,
}

#[derive(Debug, Clone, Copy, Default)]
struct SlidingSecondBucket {
    tick_sec: u64,
    count: u32,
}

#[derive(Debug, Clone)]
struct SlidingSecondWindow {
    buckets: [SlidingSecondBucket; 60],
}

impl Default for SlidingSecondWindow {
    fn default() -> Self {
        Self {
            buckets: [SlidingSecondBucket::default(); 60],
        }
    }
}

impl SlidingSecondWindow {
    fn record_at(&mut self, tick_sec: u64) {
        let idx = (tick_sec % self.buckets.len() as u64) as usize;
        let bucket = &mut self.buckets[idx];
        if bucket.tick_sec != tick_sec {
            bucket.tick_sec = tick_sec;
            bucket.count = 0;
        }
        bucket.count = bucket.count.saturating_add(1);
    }

    fn rpm_at(&self, now_sec: u64) -> u32 {
        self.buckets
            .iter()
            .filter(|bucket| bucket.count > 0 && now_sec.saturating_sub(bucket.tick_sec) < 60)
            .map(|bucket| bucket.count)
            .sum()
    }
}

#[derive(Debug, Clone, Default)]
struct ActivityState {
    in_flight: u32,
    rpm_window: SlidingSecondWindow,
}

impl ActivityState {
    fn record_start(&mut self, tick_sec: u64) {
        self.in_flight = self.in_flight.saturating_add(1);
        self.rpm_window.record_at(tick_sec);
    }

    fn finish(&mut self) {
        self.in_flight = self.in_flight.saturating_sub(1);
    }

    fn snapshot(&self, now_sec: u64) -> RequestActivitySnapshot {
        RequestActivitySnapshot {
            rpm: self.rpm_window.rpm_at(now_sec),
            in_flight: self.in_flight,
        }
    }
}

#[derive(Debug, Default)]
struct RequestActivityInner {
    total: ActivityState,
    per_key: HashMap<String, ActivityState>,
}

/// Tracks accepted provider request rate and in-flight counts.
#[derive(Debug)]
pub(crate) struct RequestActivityTracker {
    started_at: Instant,
    inner: Mutex<RequestActivityInner>,
}

impl RequestActivityTracker {
    pub(crate) fn new() -> Self {
        Self {
            started_at: Instant::now(),
            inner: Mutex::new(RequestActivityInner::default()),
        }
    }

    pub(crate) fn snapshot(&self, key_id: Option<&str>) -> RequestActivitySnapshot {
        self.snapshot_at(key_id, self.current_tick_sec())
    }

    fn current_tick_sec(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }

    fn snapshot_at(&self, key_id: Option<&str>, tick_sec: u64) -> RequestActivitySnapshot {
        let Ok(inner) = self.inner.lock() else {
            return RequestActivitySnapshot::default();
        };
        match key_id {
            Some(key_id) => inner
                .per_key
                .get(key_id)
                .map(|state| state.snapshot(tick_sec))
                .unwrap_or_default(),
            None => inner.total.snapshot(tick_sec),
        }
    }

    fn finish(&self, key_id: &str) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        inner.total.finish();
        if let Some(state) = inner.per_key.get_mut(key_id) {
            state.finish();
        }
    }

    fn start_at(self: &Arc<Self>, key_id: &str, tick_sec: u64) -> RequestActivityGuard {
        if let Ok(mut inner) = self.inner.lock() {
            inner.total.record_start(tick_sec);
            inner
                .per_key
                .entry(key_id.to_string())
                .or_default()
                .record_start(tick_sec);
        }
        RequestActivityGuard {
            tracker: Arc::downgrade(self),
            key_id: key_id.to_string(),
        }
    }

    pub(crate) fn start(self: &Arc<Self>, key_id: &str) -> RequestActivityGuard {
        self.start_at(key_id, self.current_tick_sec())
    }
}

pub(crate) struct RequestActivityGuard {
    tracker: Weak<RequestActivityTracker>,
    key_id: String,
}

impl Drop for RequestActivityGuard {
    fn drop(&mut self) {
        if let Some(tracker) = self.tracker.upgrade() {
            tracker.finish(&self.key_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{RequestActivityTracker, SlidingSecondWindow};

    #[test]
    fn sliding_second_window_reuses_slots_after_sixty_seconds() {
        let mut window = SlidingSecondWindow::default();

        window.record_at(100);
        window.record_at(100);
        assert_eq!(window.rpm_at(100), 2);

        window.record_at(160);
        assert_eq!(window.rpm_at(160), 1);
    }

    #[test]
    fn request_activity_tracker_counts_total_and_key_in_flight() {
        let tracker = Arc::new(RequestActivityTracker::new());
        let guard_a = tracker.start_at("key-a", 100);
        let guard_b = tracker.start_at("key-a", 101);

        assert_eq!(tracker.snapshot_at(None, 101).in_flight, 2);
        assert_eq!(tracker.snapshot_at(Some("key-a"), 101).in_flight, 2);
        assert_eq!(tracker.snapshot_at(Some("key-a"), 101).rpm, 2);

        drop(guard_a);
        drop(guard_b);

        assert_eq!(tracker.snapshot_at(Some("key-a"), 101).in_flight, 0);
    }
}
