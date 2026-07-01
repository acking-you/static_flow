//! Generic background refresh scheduler for account-scoped maintenance tasks.
//!
//! ```text
//!         tick
//!          |
//!          v
//!   +---------------+       due task        +------------------+
//!   | dispatcher    | --------------------> | bounded channel  |
//!   | list providers|                       +------------------+
//!   +-------+-------+                                |
//!           | dedupe queued/running                  v
//!           v                              +--------------------+
//!   +---------------+                      | 32+ worker tasks    |
//!   | SchedulerState| <------------------- | refresh one account |
//!   +---------------+   next_due update    +--------------------+
//! ```
//!
//! The scheduler owns only timing, de-duplication, and worker fan-out.
//! Provider modules own account listing, refresh execution, and next-delay
//! policy, so Kiro/Codex behavior stays in their respective modules.

use std::{
    any::Any,
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use tokio::sync::{mpsc, Mutex};

const DEFAULT_BACKGROUND_REFRESH_WORKERS: usize = 32;
const MAX_BACKGROUND_REFRESH_WORKERS: usize = 512;
const DEFAULT_DISPATCH_TICK: Duration = Duration::from_secs(5);
const DEFAULT_CHANNEL_MULTIPLIER: usize = 4;
const FALLBACK_REFRESH_DELAY: Duration = Duration::from_secs(240);
const BACKGROUND_REFRESH_WORKERS_ENV: &str = "LLM_ACCESS_BACKGROUND_REFRESH_WORKERS";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct RefreshTaskKey {
    provider: &'static str,
    account_name: String,
}

impl RefreshTaskKey {
    fn new(provider: &'static str, account_name: impl Into<String>) -> Self {
        Self {
            provider,
            account_name: account_name.into(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct RefreshTask {
    key: RefreshTaskKey,
    payload: Arc<dyn Any + Send + Sync>,
}

impl RefreshTask {
    #[cfg(test)]
    pub(crate) fn new(provider: &'static str, account_name: impl Into<String>) -> Self {
        Self {
            key: RefreshTaskKey::new(provider, account_name),
            payload: Arc::new(()),
        }
    }

    pub(crate) fn with_payload<T>(
        provider: &'static str,
        account_name: impl Into<String>,
        payload: T,
    ) -> Self
    where
        T: Any + Send + Sync,
    {
        Self {
            key: RefreshTaskKey::new(provider, account_name),
            payload: Arc::new(payload),
        }
    }

    pub(crate) fn payload<T: Any>(&self) -> Option<&T> {
        self.payload.downcast_ref::<T>()
    }
}

#[async_trait]
pub(crate) trait RefreshTaskProvider: Send + Sync + 'static {
    fn name(&self) -> &'static str;

    async fn list_tasks(&self) -> anyhow::Result<Vec<RefreshTask>>;

    async fn refresh_task(&self, task: &RefreshTask) -> anyhow::Result<()>;

    async fn next_delay(&self) -> anyhow::Result<Duration>;
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RefreshSchedulerConfig {
    worker_count: usize,
    channel_capacity: usize,
    dispatch_tick: Duration,
}

impl RefreshSchedulerConfig {
    pub(crate) fn from_env() -> Self {
        let worker_count = normalize_worker_count(
            std::env::var(BACKGROUND_REFRESH_WORKERS_ENV)
                .ok()
                .as_deref(),
        );
        Self {
            worker_count,
            channel_capacity: worker_count * DEFAULT_CHANNEL_MULTIPLIER,
            dispatch_tick: DEFAULT_DISPATCH_TICK,
        }
    }
}

#[derive(Debug, Default)]
struct SchedulerState {
    // Absolute Unix milliseconds for the next eligible refresh per account.
    next_due_at_ms: HashMap<RefreshTaskKey, i64>,
    // Protects upstreams and storage from duplicate work while a task is
    // already queued or being refreshed by one worker.
    queued_or_running: HashSet<RefreshTaskKey>,
}

impl SchedulerState {
    fn claim_if_due(&mut self, task: &RefreshTask, now_ms: i64) -> bool {
        if self.queued_or_running.contains(&task.key) {
            return false;
        }
        if self
            .next_due_at_ms
            .get(&task.key)
            .is_some_and(|next_due| *next_due > now_ms)
        {
            return false;
        }
        self.queued_or_running.insert(task.key.clone())
    }

    fn release(&mut self, key: &RefreshTaskKey) {
        self.queued_or_running.remove(key);
    }

    fn complete(&mut self, key: &RefreshTaskKey, next_due_at_ms: i64) {
        self.queued_or_running.remove(key);
        self.next_due_at_ms.insert(key.clone(), next_due_at_ms);
    }
}

struct QueuedRefreshTask {
    provider: Arc<dyn RefreshTaskProvider>,
    task: RefreshTask,
}

pub(crate) fn spawn_refresh_scheduler(
    providers: Vec<Arc<dyn RefreshTaskProvider>>,
    config: RefreshSchedulerConfig,
) {
    if providers.is_empty() {
        tracing::warn!("background refresh scheduler has no providers");
        return;
    }
    tracing::info!(
        worker_count = config.worker_count,
        channel_capacity = config.channel_capacity,
        dispatch_tick_ms = config.dispatch_tick.as_millis() as u64,
        "starting background refresh scheduler"
    );
    let state = Arc::new(Mutex::new(SchedulerState::default()));
    let (sender, receiver) = mpsc::channel(config.channel_capacity.max(config.worker_count));
    let receiver = Arc::new(Mutex::new(receiver));
    for worker_index in 0..config.worker_count {
        tokio::spawn(refresh_worker_loop(worker_index, Arc::clone(&receiver), Arc::clone(&state)));
    }
    tokio::spawn(dispatcher_loop(providers, sender, state, config.dispatch_tick));
}

async fn dispatcher_loop(
    providers: Vec<Arc<dyn RefreshTaskProvider>>,
    sender: mpsc::Sender<QueuedRefreshTask>,
    state: Arc<Mutex<SchedulerState>>,
    dispatch_tick: Duration,
) {
    loop {
        dispatch_due_tasks_once(&providers, &sender, &state).await;
        tokio::time::sleep(dispatch_tick).await;
    }
}

async fn dispatch_due_tasks_once(
    providers: &[Arc<dyn RefreshTaskProvider>],
    sender: &mpsc::Sender<QueuedRefreshTask>,
    state: &Arc<Mutex<SchedulerState>>,
) {
    let now = now_ms();
    for provider in providers {
        let mut queued_count = 0usize;
        let mut skipped_count = 0usize;
        let tasks = match provider.list_tasks().await {
            Ok(tasks) => tasks,
            Err(err) => {
                tracing::warn!(
                    provider = provider.name(),
                    "failed to list background refresh tasks: {err:#}"
                );
                continue;
            },
        };
        let listed_count = tasks.len();
        for task in tasks {
            let claimed = {
                let mut state = state.lock().await;
                state.claim_if_due(&task, now)
            };
            if !claimed {
                skipped_count += 1;
                continue;
            }
            let key = task.key.clone();
            let queued = QueuedRefreshTask {
                provider: Arc::clone(provider),
                task,
            };
            if let Err(err) = sender.try_send(queued) {
                let mut state = state.lock().await;
                state.release(&key);
                skipped_count += 1;
                tracing::warn!(
                    provider = key.provider,
                    account_name = %key.account_name,
                    "background refresh queue is full or closed: {err}"
                );
            } else {
                queued_count += 1;
            }
        }
        tracing::debug!(
            provider = provider.name(),
            listed_count,
            queued_count,
            skipped_count,
            "background refresh dispatch tick completed for provider"
        );
    }
}

async fn refresh_worker_loop(
    worker_index: usize,
    receiver: Arc<Mutex<mpsc::Receiver<QueuedRefreshTask>>>,
    state: Arc<Mutex<SchedulerState>>,
) {
    loop {
        let Some(job) = receive_next_job(&receiver).await else {
            return;
        };
        let key = job.task.key.clone();
        if let Err(err) = job.provider.refresh_task(&job.task).await {
            tracing::warn!(
                worker_index,
                provider = key.provider,
                account_name = %key.account_name,
                "background refresh task failed: {err:#}"
            );
        }
        let delay = job.provider.next_delay().await.unwrap_or_else(|err| {
            tracing::warn!(
                worker_index,
                provider = key.provider,
                account_name = %key.account_name,
                "failed to load background refresh delay: {err:#}"
            );
            FALLBACK_REFRESH_DELAY
        });
        let next_due = now_ms().saturating_add(duration_ms(delay));
        let mut state = state.lock().await;
        state.complete(&key, next_due);
        tracing::debug!(
            worker_index,
            provider = key.provider,
            account_name = %key.account_name,
            next_delay_ms = duration_ms(delay),
            next_due_at_ms = next_due,
            "background refresh task completed"
        );
        crate::allocator::collect_process_allocator();
    }
}

async fn receive_next_job(
    receiver: &Arc<Mutex<mpsc::Receiver<QueuedRefreshTask>>>,
) -> Option<QueuedRefreshTask> {
    let mut receiver = receiver.lock().await;
    receiver.recv().await
}

fn duration_ms(duration: Duration) -> i64 {
    duration.as_millis().min(i64::MAX as u128) as i64
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn normalize_worker_count(raw: Option<&str>) -> usize {
    raw.and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(DEFAULT_BACKGROUND_REFRESH_WORKERS)
        .clamp(DEFAULT_BACKGROUND_REFRESH_WORKERS, MAX_BACKGROUND_REFRESH_WORKERS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_count_defaults_to_at_least_32() {
        assert_eq!(normalize_worker_count(None), 32);
        assert_eq!(normalize_worker_count(Some("4")), 32);
        assert_eq!(normalize_worker_count(Some("32")), 32);
        assert_eq!(normalize_worker_count(Some("64")), 64);
    }

    #[test]
    fn scheduler_state_deduplicates_queued_or_running_tasks_until_next_due() {
        let mut state = SchedulerState::default();
        let task = RefreshTask::new("kiro", "kiro-a");

        assert!(state.claim_if_due(&task, 1_000));
        assert!(!state.claim_if_due(&task, 1_000));

        state.complete(&task.key, 1_500);
        assert!(!state.claim_if_due(&task, 1_499));
        assert!(state.claim_if_due(&task, 1_500));
    }
}
