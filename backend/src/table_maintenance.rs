use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use parking_lot::RwLock;
use static_flow_shared::{
    article_request_store, comments_store, interactive_store,
    lancedb_api::CONTENT_BACKGROUND_COMPACTION_TABLE_NAMES,
    llm_gateway_store::LLM_GATEWAY_TABLE_NAMES,
    music_store, music_wish_store,
    optimize::{check_opened_table_and_compact, CompactAction, CompactConfig, CompactResult},
};
use tokio::sync::{mpsc, watch, OwnedSemaphorePermit, Semaphore};

use crate::state::{
    CompactionRuntimeConfig, TableCompactorStores, MAX_CONFIGURABLE_TABLE_COMPACT_WORKER_COUNT,
};

const STARTUP_DELAY_SECONDS: u64 = 60;
const DISPATCH_TICK_SECONDS: u64 = 1;
const MIN_MIMALLOC_COLLECTION_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MaintenanceDb {
    Content,
    Comments,
    Music,
    MusicWish,
    ArticleRequest,
    Interactive,
    LlmGateway,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskKind {
    Generic { db: MaintenanceDb, table_name: &'static str },
    ContentArticleViews,
    ContentApiBehavior,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TableMaintenanceTaskDefinition {
    id: String,
    db_label: &'static str,
    table_name: &'static str,
    kind: TaskKind,
    interval_override_seconds: Option<u64>,
    prune_older_than_hours_override: Option<i64>,
    compact_enabled: bool,
    optimize_dirty_indices: bool,
}

#[derive(Debug, Clone)]
struct ScheduledMaintenanceTask {
    definition: TableMaintenanceTaskDefinition,
    compact_config: CompactConfig,
    interval: Duration,
}

#[derive(Debug)]
struct MaintenanceTaskResult {
    task_id: String,
    scheduled_interval: Duration,
    result: CompactResult,
}

pub(crate) fn spawn_table_maintenance_loop(
    stores: TableCompactorStores,
    compaction_runtime_config: Arc<RwLock<CompactionRuntimeConfig>>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    tokio::spawn(async move {
        tokio::select! {
            biased;
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    tracing::info!("table maintenance cancelled during startup delay");
                    return;
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(STARTUP_DELAY_SECONDS)) => {}
        }

        let task_definitions = default_table_maintenance_tasks();
        let startup_runtime = compaction_runtime_config.read().clone();
        let worker_count = startup_runtime.worker_count.max(1);
        let concurrency_limit = Arc::new(Semaphore::new(worker_count));
        let active_tasks = Arc::new(AtomicUsize::new(0));
        let mut reserved_permits = Vec::new();
        tracing::info!(
            task_count = task_definitions.len(),
            worker_count,
            global_enabled = startup_runtime.enabled,
            global_scan_interval_seconds = startup_runtime.scan_interval_seconds,
            global_fragment_threshold = startup_runtime.fragment_threshold,
            global_prune_older_than_hours = startup_runtime.prune_older_than_hours,
            "table maintenance scheduler started"
        );

        let (task_tx, task_rx) =
            mpsc::channel::<ScheduledMaintenanceTask>((task_definitions.len().max(1)) * 2);
        let task_rx = Arc::new(tokio::sync::Mutex::new(task_rx));
        let (result_tx, mut result_rx) = mpsc::unbounded_channel::<MaintenanceTaskResult>();

        // Keep a fixed worker pool alive and use permits to control runtime
        // concurrency. That lets admin config changes take effect immediately
        // without tearing down and recreating background tasks.
        for worker_id in 0..MAX_CONFIGURABLE_TABLE_COMPACT_WORKER_COUNT {
            spawn_table_maintenance_worker(
                worker_id,
                stores.clone(),
                Arc::clone(&task_rx),
                result_tx.clone(),
                Arc::clone(&concurrency_limit),
                Arc::clone(&active_tasks),
                shutdown_rx.clone(),
            );
        }
        drop(result_tx);

        let mut next_due_at = task_definitions
            .iter()
            .map(|task| (task.id.clone(), Instant::now()))
            .collect::<HashMap<_, _>>();
        let mut in_flight = HashSet::new();
        let mut last_allocator_collection = None;

        let mut ticker = tokio::time::interval(Duration::from_secs(DISPATCH_TICK_SECONDS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                biased;
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        tracing::info!("table maintenance scheduler shutting down");
                        return;
                    }
                }
                Some(report) = result_rx.recv() => {
                    in_flight.remove(report.task_id.as_str());
                    next_due_at.insert(report.task_id.clone(), Instant::now() + report.scheduled_interval);
                    log_maintenance_result(&report.result);
                    if should_force_allocator_collection(
                        report.result.compacted,
                        &mut last_allocator_collection,
                        Instant::now(),
                    ) {
                        // SAFETY: this calls mimalloc's global collection hook only; it does not
                        // access borrowed Rust memory or violate aliasing rules.
                        unsafe {
                            better_mimalloc_sys::mi_collect(true);
                        }
                        tracing::info!(
                            table = report.result.table,
                            "table maintenance forced mimalloc collection after compaction"
                        );
                    }
                }
                _ = ticker.tick() => {
                    let global_runtime = compaction_runtime_config.read().clone();
                    adjust_worker_limit(
                        Arc::clone(&concurrency_limit),
                        active_tasks.load(Ordering::SeqCst),
                        &mut reserved_permits,
                        global_runtime.worker_count.max(1),
                    );
                    let now = Instant::now();
                    for definition in &task_definitions {
                        // Re-resolve config on every dispatch tick so the task
                        // registry stays static while timing and prune rules
                        // still track the latest admin settings.
                        let scheduled = resolve_scheduled_task(definition, &global_runtime);
                        let due_at = next_due_at
                            .get(definition.id.as_str())
                            .copied()
                            .unwrap_or(now);
                        if !scheduled.compact_config.enabled
                            || in_flight.contains(definition.id.as_str())
                            || now < due_at
                        {
                            continue;
                        }
                        match task_tx.try_send(scheduled.clone()) {
                            Ok(()) => {
                                in_flight.insert(definition.id.clone());
                            }
                            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                                tracing::debug!(
                                    task_id = definition.id,
                                    "table maintenance queue is full; will retry next tick"
                                );
                            }
                            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                tracing::warn!("table maintenance queue closed unexpectedly");
                                return;
                            }
                        }
                    }
                }
            }
        }
    });
}

fn should_force_allocator_collection(
    compacted: bool,
    last_collection_at: &mut Option<Instant>,
    now: Instant,
) -> bool {
    if !compacted {
        return false;
    }
    match last_collection_at {
        Some(previous) if now.duration_since(*previous) < MIN_MIMALLOC_COLLECTION_INTERVAL => false,
        _ => {
            *last_collection_at = Some(now);
            true
        },
    }
}

fn spawn_table_maintenance_worker(
    worker_id: usize,
    stores: TableCompactorStores,
    task_rx: Arc<tokio::sync::Mutex<mpsc::Receiver<ScheduledMaintenanceTask>>>,
    result_tx: mpsc::UnboundedSender<MaintenanceTaskResult>,
    concurrency_limit: Arc<Semaphore>,
    active_tasks: Arc<AtomicUsize>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    tokio::spawn(async move {
        tracing::info!(worker_id, "table maintenance worker started");
        loop {
            let next_task = tokio::select! {
                biased;
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        tracing::info!(worker_id, "table maintenance worker shutting down");
                        return;
                    }
                    continue;
                }
                task = async {
                    let mut receiver = task_rx.lock().await;
                    receiver.recv().await
                } => task,
            };
            let Some(task) = next_task else {
                tracing::info!(worker_id, "table maintenance worker exiting because queue closed");
                return;
            };

            let _permit = match tokio::select! {
                biased;
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        tracing::info!(worker_id, "table maintenance worker shutting down");
                        return;
                    }
                    continue;
                }
                permit = Arc::clone(&concurrency_limit).acquire_owned() => permit,
            } {
                Ok(permit) => permit,
                Err(_) => {
                    tracing::warn!(worker_id, "table maintenance semaphore closed");
                    return;
                },
            };

            active_tasks.fetch_add(1, Ordering::SeqCst);
            let result = execute_scheduled_task(&stores, &task).await;
            active_tasks.fetch_sub(1, Ordering::SeqCst);
            if result_tx
                .send(MaintenanceTaskResult {
                    task_id: task.definition.id.clone(),
                    scheduled_interval: task.interval,
                    result,
                })
                .is_err()
            {
                tracing::warn!(worker_id, "table maintenance result channel closed");
                return;
            }
        }
    });
}

fn default_table_maintenance_tasks() -> Vec<TableMaintenanceTaskDefinition> {
    let mut tasks = Vec::new();
    push_generic_tasks(
        &mut tasks,
        "content",
        MaintenanceDb::Content,
        CONTENT_BACKGROUND_COMPACTION_TABLE_NAMES,
    );
    tasks.push(TableMaintenanceTaskDefinition {
        id: "content:article_views".to_string(),
        db_label: "content",
        table_name: "article_views",
        kind: TaskKind::ContentArticleViews,
        interval_override_seconds: None,
        prune_older_than_hours_override: None,
        compact_enabled: true,
        optimize_dirty_indices: true,
    });
    tasks.push(TableMaintenanceTaskDefinition {
        id: "content:api_behavior_events".to_string(),
        db_label: "content",
        table_name: "api_behavior_events",
        kind: TaskKind::ContentApiBehavior,
        interval_override_seconds: None,
        prune_older_than_hours_override: None,
        compact_enabled: true,
        optimize_dirty_indices: true,
    });
    push_generic_tasks(
        &mut tasks,
        "content",
        MaintenanceDb::ArticleRequest,
        article_request_store::ARTICLE_REQUEST_TABLE_NAMES,
    );
    push_generic_tasks(
        &mut tasks,
        "content",
        MaintenanceDb::Interactive,
        interactive_store::INTERACTIVE_TABLE_NAMES,
    );
    push_generic_tasks(&mut tasks, "content", MaintenanceDb::LlmGateway, LLM_GATEWAY_TABLE_NAMES);
    push_generic_tasks(
        &mut tasks,
        "comments",
        MaintenanceDb::Comments,
        comments_store::COMMENT_TABLE_NAMES,
    );
    push_generic_tasks(&mut tasks, "music", MaintenanceDb::Music, music_store::MUSIC_TABLE_NAMES);
    push_generic_tasks(
        &mut tasks,
        "music",
        MaintenanceDb::MusicWish,
        music_wish_store::MUSIC_WISH_TABLE_NAMES,
    );
    tasks
}

fn push_generic_tasks(
    tasks: &mut Vec<TableMaintenanceTaskDefinition>,
    db_label: &'static str,
    db: MaintenanceDb,
    table_names: &[&'static str],
) {
    for &table_name in table_names {
        tasks.push(TableMaintenanceTaskDefinition {
            id: format!("{db_label}:{table_name}"),
            db_label,
            table_name,
            kind: TaskKind::Generic {
                db,
                table_name,
            },
            interval_override_seconds: None,
            prune_older_than_hours_override: None,
            compact_enabled: true,
            optimize_dirty_indices: true,
        });
    }
}

fn adjust_worker_limit(
    concurrency_limit: Arc<Semaphore>,
    active_tasks: usize,
    reserved_permits: &mut Vec<OwnedSemaphorePermit>,
    desired: usize,
) {
    // Lowering concurrency is implemented by reserving permits instead of
    // stopping workers. Existing jobs finish naturally and new jobs observe the
    // tighter limit on the next acquire.
    let accessible_permits = concurrency_limit.available_permits() + active_tasks;
    if desired > accessible_permits {
        let mut delta = desired - accessible_permits;
        while delta > 0 && !reserved_permits.is_empty() {
            reserved_permits.pop();
            delta -= 1;
        }
        if delta > 0 {
            concurrency_limit.add_permits(delta);
        }
        return;
    }

    let mut excess = accessible_permits.saturating_sub(desired);
    while excess > 0 {
        match Arc::clone(&concurrency_limit).try_acquire_owned() {
            Ok(permit) => {
                reserved_permits.push(permit);
                excess -= 1;
            },
            Err(_) => break,
        }
    }
}

fn resolve_scheduled_task(
    definition: &TableMaintenanceTaskDefinition,
    global_runtime: &CompactionRuntimeConfig,
) -> ScheduledMaintenanceTask {
    let enabled = global_runtime.enabled;
    let interval_seconds = definition
        .interval_override_seconds
        .unwrap_or(global_runtime.scan_interval_seconds);
    let fragment_threshold =
        if definition.compact_enabled { global_runtime.fragment_threshold } else { usize::MAX };
    let prune_older_than_hours = definition
        .prune_older_than_hours_override
        .unwrap_or(global_runtime.prune_older_than_hours);
    ScheduledMaintenanceTask {
        definition: definition.clone(),
        compact_config: CompactConfig {
            enabled,
            fragment_threshold,
            prune_older_than_hours,
            optimize_dirty_indices: definition.optimize_dirty_indices,
            skip_tables: HashSet::new(),
        },
        interval: Duration::from_secs(interval_seconds.max(1)),
    }
}

async fn execute_scheduled_task(
    stores: &TableCompactorStores,
    task: &ScheduledMaintenanceTask,
) -> CompactResult {
    match task.definition.kind {
        TaskKind::Generic {
            db,
            table_name,
        } => {
            let connection = match db {
                MaintenanceDb::Content => stores.content_store.connection(),
                MaintenanceDb::Comments => stores.comment_store.connection(),
                MaintenanceDb::Music => stores.music_store.connection(),
                MaintenanceDb::MusicWish => stores.music_wish_store.connection(),
                MaintenanceDb::ArticleRequest => stores.article_request_store.connection(),
                MaintenanceDb::Interactive => stores.interactive_store.connection(),
                MaintenanceDb::LlmGateway => stores.llm_gateway_store.connection(),
            };
            match connection.open_table(table_name).execute().await {
                Ok(table) => check_opened_table_and_compact(&table, &task.compact_config).await,
                Err(err) => CompactResult {
                    table: table_name.to_string(),
                    small_fragments: 0,
                    max_unindexed_rows: 0,
                    action: CompactAction::OpenFailed,
                    elapsed_ms: 0,
                    compacted: false,
                    pruned: false,
                    index_optimized: false,
                    error: Some(format!("open failed: {err:#}")),
                },
            }
        },
        TaskKind::ContentArticleViews => {
            stores
                .content_store
                .maintain_article_views_table(&task.compact_config)
                .await
        },
        TaskKind::ContentApiBehavior => {
            stores
                .content_store
                .maintain_api_behavior_table(&task.compact_config)
                .await
        },
    }
}

fn log_maintenance_result(result: &CompactResult) {
    if let Some(err) = &result.error {
        tracing::warn!(
            table = result.table,
            action = result.action.as_str(),
            compacted = result.compacted,
            pruned = result.pruned,
            index_optimized = result.index_optimized,
            small_fragments = result.small_fragments,
            max_unindexed_rows = result.max_unindexed_rows,
            elapsed_ms = result.elapsed_ms,
            error = err,
            "table maintenance task failed"
        );
    } else {
        tracing::info!(
            table = result.table,
            action = result.action.as_str(),
            compacted = result.compacted,
            pruned = result.pruned,
            index_optimized = result.index_optimized,
            small_fragments = result.small_fragments,
            max_unindexed_rows = result.max_unindexed_rows,
            elapsed_ms = result.elapsed_ms,
            "table maintenance task completed"
        );
    }
}

#[cfg(test)]
mod tests {
    use static_flow_shared::llm_gateway_store::LLM_GATEWAY_USAGE_EVENTS_TABLE;

    use super::*;
    use crate::state::{
        CompactionRuntimeConfig, DEFAULT_TABLE_COMPACT_FRAGMENT_THRESHOLD,
        DEFAULT_TABLE_COMPACT_PRUNE_OLDER_THAN_HOURS, DEFAULT_TABLE_COMPACT_SCAN_INTERVAL_SECS,
    };

    #[test]
    fn default_table_maintenance_tasks_have_unique_ids() {
        let tasks = default_table_maintenance_tasks();
        let unique_ids = tasks
            .iter()
            .map(|task| task.id.clone())
            .collect::<HashSet<_>>();
        assert_eq!(unique_ids.len(), tasks.len());
    }

    #[test]
    fn usage_event_task_inherits_global_compaction_defaults() {
        let task = default_table_maintenance_tasks()
            .into_iter()
            .find(|task| task.table_name == LLM_GATEWAY_USAGE_EVENTS_TABLE)
            .expect("usage event task exists");
        let global = CompactionRuntimeConfig {
            scan_interval_seconds: 321,
            prune_older_than_hours: 6,
            ..CompactionRuntimeConfig::default()
        };

        let scheduled = resolve_scheduled_task(&task, &global);

        assert!(scheduled.compact_config.enabled);
        assert_eq!(scheduled.interval, Duration::from_secs(321));
        assert_eq!(
            scheduled.compact_config.fragment_threshold,
            DEFAULT_TABLE_COMPACT_FRAGMENT_THRESHOLD
        );
        assert_eq!(scheduled.compact_config.prune_older_than_hours, 6);
    }

    #[test]
    fn non_usage_task_inherits_global_compaction_defaults() {
        let task = default_table_maintenance_tasks()
            .into_iter()
            .find(|task| task.table_name == "articles")
            .expect("articles maintenance task exists");
        let global = CompactionRuntimeConfig::default();
        let scheduled = resolve_scheduled_task(&task, &global);

        assert!(scheduled.compact_config.enabled);
        assert_eq!(
            scheduled.interval,
            Duration::from_secs(DEFAULT_TABLE_COMPACT_SCAN_INTERVAL_SECS)
        );
        assert_eq!(
            scheduled.compact_config.prune_older_than_hours,
            DEFAULT_TABLE_COMPACT_PRUNE_OLDER_THAN_HOURS
        );
    }

    #[test]
    fn compacted_results_rate_limit_global_allocator_collection() {
        let start = Instant::now();
        let mut last_collection = None;

        assert!(should_force_allocator_collection(true, &mut last_collection, start,));
        assert!(!should_force_allocator_collection(
            true,
            &mut last_collection,
            start + MIN_MIMALLOC_COLLECTION_INTERVAL - Duration::from_secs(1),
        ));
        assert!(should_force_allocator_collection(
            true,
            &mut last_collection,
            start + MIN_MIMALLOC_COLLECTION_INTERVAL,
        ));
    }

    #[test]
    fn non_compacted_results_do_not_force_allocator_collection() {
        let mut last_collection = None;
        assert!(!should_force_allocator_collection(false, &mut last_collection, Instant::now(),));
        assert!(last_collection.is_none());
    }
}
