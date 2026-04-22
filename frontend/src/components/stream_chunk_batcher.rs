//! SSE chunk batching for admin stream pages.
//!
//! Live LLM streams emit one SSE event per token. Calling
//! `UseStateHandle::set` on every event renders the entire chunk list that
//! many times, which dominates the frame budget once the transcript grows
//! past a few hundred chunks. This helper batches arrivals so the UI
//! commits at most ~10 times per second regardless of incoming rate.
//!
//! Usage pattern (inside a `use_effect_with` that opens an `EventSource`):
//!
//! ```ignore
//! let batcher = ChunkBatcher::new(state, |c| c.chunk_id.clone(), |c| c.batch_index);
//! let batcher_for_msg = batcher.clone();          // cheap: Rc bump
//! let onmessage = Closure::new(move |_| {
//!     batcher_for_msg.push(chunk);
//! });
//! // store `batcher` alongside source + closures; in cleanup call batcher.cancel().
//! ```
//!
//! Duplicate dedup keys are dropped against the *current* state on flush,
//! so bursts within a window still collapse cleanly even if the state
//! already contains earlier copies. If the flush drains only duplicates,
//! the `set` call is skipped so Yew does not schedule a render for a no-op.
//!
//! Design note: the batcher intentionally does NOT cancel its timer on
//! `Drop`. A cloned handle captured by a closure might outlive the owning
//! `StreamHandle`, and killing the timer on every clone's drop would defeat
//! the batching. Call `cancel()` explicitly from the effect cleanup path
//! (or from the owner's `Drop`) to abort a pending flush.

use std::cell::RefCell;
use std::rc::Rc;

use gloo_timers::callback::Timeout;
use yew::UseStateHandle;

/// Default interval between flushes.
pub const DEFAULT_FLUSH_INTERVAL_MS: u32 = 100;

/// Batches SSE chunks and flushes them into a `Vec<T>` state at a fixed cadence.
///
/// Cheap to clone (internal `Rc`); all clones share the same pending buffer
/// and timer slot.
pub struct ChunkBatcher<T, K>
where
    T: Clone + 'static,
    K: PartialEq + 'static,
{
    inner: Rc<Inner<T, K>>,
}

impl<T, K> Clone for ChunkBatcher<T, K>
where
    T: Clone + 'static,
    K: PartialEq + 'static,
{
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

struct Inner<T, K>
where
    T: Clone + 'static,
    K: PartialEq + 'static,
{
    state: UseStateHandle<Vec<T>>,
    pending: RefCell<Vec<T>>,
    timer: RefCell<Option<Timeout>>,
    dedup_key: Box<dyn Fn(&T) -> K>,
    sort_key: Box<dyn Fn(&T) -> i32>,
    flush_interval_ms: u32,
}

impl<T, K> ChunkBatcher<T, K>
where
    T: Clone + 'static,
    K: PartialEq + 'static,
{
    /// Create a new batcher bound to `state`.
    pub fn new(
        state: UseStateHandle<Vec<T>>,
        dedup_key: impl Fn(&T) -> K + 'static,
        sort_key: impl Fn(&T) -> i32 + 'static,
    ) -> Self {
        Self::with_interval(state, dedup_key, sort_key, DEFAULT_FLUSH_INTERVAL_MS)
    }

    pub fn with_interval(
        state: UseStateHandle<Vec<T>>,
        dedup_key: impl Fn(&T) -> K + 'static,
        sort_key: impl Fn(&T) -> i32 + 'static,
        flush_interval_ms: u32,
    ) -> Self {
        Self {
            inner: Rc::new(Inner {
                state,
                pending: RefCell::new(Vec::new()),
                timer: RefCell::new(None),
                dedup_key: Box::new(dedup_key),
                sort_key: Box::new(sort_key),
                flush_interval_ms,
            }),
        }
    }

    /// Push a chunk into the pending buffer. Arms a one-shot Timeout if none
    /// is queued; otherwise returns immediately — the existing timer will
    /// pick up this chunk along with the rest.
    pub fn push(&self, chunk: T) {
        self.inner.pending.borrow_mut().push(chunk);
        if self.inner.timer.borrow().is_some() {
            return;
        }
        let inner = self.inner.clone();
        let t = Timeout::new(self.inner.flush_interval_ms, move || {
            inner.timer.borrow_mut().take();
            let drained: Vec<T> = inner.pending.borrow_mut().drain(..).collect();
            if drained.is_empty() {
                return;
            }
            let mut next = (*inner.state).clone();
            let before = next.len();
            for chunk in drained {
                let key = (inner.dedup_key)(&chunk);
                if !next.iter().any(|existing| (inner.dedup_key)(existing) == key) {
                    next.push(chunk);
                }
            }
            if next.len() == before {
                return; // every drained chunk was already known; skip render
            }
            next.sort_by_key(|chunk| (inner.sort_key)(chunk));
            inner.state.set(next);
        });
        *self.inner.timer.borrow_mut() = Some(t);
    }

    /// Cancel a pending flush and drop any buffered chunks. Intended for
    /// effect cleanup / owner drop paths; see the module docs.
    pub fn cancel(&self) {
        self.inner.timer.borrow_mut().take();
        self.inner.pending.borrow_mut().clear();
    }
}
