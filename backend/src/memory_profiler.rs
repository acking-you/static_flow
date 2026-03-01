use std::{
    alloc::{GlobalAlloc, Layout},
    cell::Cell,
    collections::{hash_map::DefaultHasher, HashMap, HashSet},
    env,
    ffi::c_void,
    hash::{Hash, Hasher},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        OnceLock,
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use backtrace::{resolve, trace_unsynchronized};
use better_mimalloc_rs::MiMalloc;
use dashmap::DashMap;
use rustc_demangle::try_demangle;
use serde::{Deserialize, Serialize};

const MAX_STACK_DEPTH: usize = 24;
const DEFAULT_SAMPLE_RATE: u64 = 32;
const DEFAULT_MIN_ALLOC_BYTES: usize = 256;
const DEFAULT_MAX_TRACKED_ALLOCATIONS: usize = 200_000;
const DEFAULT_STACK_SKIP: usize = 8;
const DEFAULT_TOP_LIMIT: usize = 50;
const MAX_TOP_LIMIT: usize = 500;

#[derive(Clone, Copy, Hash, Eq, PartialEq)]
struct StackKey {
    len: u8,
    frames: [usize; MAX_STACK_DEPTH],
}

impl Default for StackKey {
    fn default() -> Self {
        Self {
            len: 0,
            frames: [0; MAX_STACK_DEPTH],
        }
    }
}

impl StackKey {
    fn frames(&self) -> &[usize] {
        &self.frames[..self.len as usize]
    }
}

#[derive(Clone, Copy)]
struct AllocationMeta {
    stack: StackKey,
    weighted_bytes: u64,
}

#[derive(Clone, Copy, Default)]
struct StackStats {
    live_bytes: u64,
    alloc_bytes_total: u64,
    alloc_count: u64,
    free_count: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Default)]
pub struct MiProcessMemoryInfo {
    pub elapsed_millis: u64,
    pub user_millis: u64,
    pub system_millis: u64,
    pub current_rss_bytes: u64,
    pub peak_rss_bytes: u64,
    pub current_commit_bytes: u64,
    pub peak_commit_bytes: u64,
    pub page_faults: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryProfilerConfigSnapshot {
    pub enabled: bool,
    pub sample_rate: u64,
    pub min_alloc_bytes: usize,
    pub max_tracked_allocations: usize,
    pub stack_skip: usize,
    pub max_stack_depth: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MemoryProfilerConfigUpdate {
    pub enabled: Option<bool>,
    pub sample_rate: Option<u64>,
    pub min_alloc_bytes: Option<usize>,
    pub max_tracked_allocations: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryProfilerOverview {
    pub generated_at_ms: i64,
    pub config: MemoryProfilerConfigSnapshot,
    pub process_uptime_secs: u64,
    pub tracked_allocations: usize,
    pub distinct_stacks: usize,
    pub dropped_allocations: u64,
    pub sampled_alloc_events: u64,
    pub sampled_dealloc_events: u64,
    pub sampled_realloc_events: u64,
    pub total_live_bytes_estimate: u64,
    pub total_alloc_bytes_estimate: u64,
    pub process_rss_bytes: u64,
    pub process_virtual_bytes: u64,
    pub mimalloc: MiProcessMemoryInfo,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryStackEntry {
    pub stack_id: String,
    pub live_bytes_estimate: u64,
    pub alloc_bytes_total_estimate: u64,
    pub alloc_count: u64,
    pub free_count: u64,
    pub live_ratio_heap: f64,
    pub live_ratio_rss: f64,
    pub frames: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryStackReport {
    pub generated_at_ms: i64,
    pub top: usize,
    pub total_live_bytes_estimate: u64,
    pub process_rss_bytes: u64,
    pub entries: Vec<MemoryStackEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryFunctionEntry {
    pub function: String,
    pub module: String,
    pub live_bytes_estimate: u64,
    pub alloc_bytes_total_estimate: u64,
    pub stack_count: usize,
    pub alloc_count: u64,
    pub free_count: u64,
    pub live_ratio_heap: f64,
    pub live_ratio_rss: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryFunctionReport {
    pub generated_at_ms: i64,
    pub top: usize,
    pub total_live_bytes_estimate: u64,
    pub process_rss_bytes: u64,
    pub entries: Vec<MemoryFunctionEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryModuleEntry {
    pub module: String,
    pub live_bytes_estimate: u64,
    pub alloc_bytes_total_estimate: u64,
    pub function_count: usize,
    pub stack_count: usize,
    pub alloc_count: u64,
    pub free_count: u64,
    pub live_ratio_heap: f64,
    pub live_ratio_rss: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryModuleReport {
    pub generated_at_ms: i64,
    pub top: usize,
    pub total_live_bytes_estimate: u64,
    pub process_rss_bytes: u64,
    pub entries: Vec<MemoryModuleEntry>,
}

struct FunctionAggregate {
    function: String,
    module: String,
    live_bytes: u64,
    alloc_bytes_total: u64,
    stack_count: usize,
    alloc_count: u64,
    free_count: u64,
}

struct ModuleAggregate {
    module: String,
    live_bytes: u64,
    alloc_bytes_total: u64,
    function_names: HashSet<String>,
    stack_count: usize,
    alloc_count: u64,
    free_count: u64,
}

pub struct MemoryProfiler {
    enabled: AtomicBool,
    sample_rate: AtomicU64,
    min_alloc_bytes: AtomicUsize,
    max_tracked_allocations: AtomicUsize,
    stack_skip: usize,
    max_stack_depth: usize,
    started_at: Instant,
    allocation_seq: AtomicU64,
    tracked_allocations: AtomicUsize,
    dropped_allocations: AtomicU64,
    sampled_alloc_events: AtomicU64,
    sampled_dealloc_events: AtomicU64,
    sampled_realloc_events: AtomicU64,
    allocations: DashMap<usize, AllocationMeta>,
    stacks: DashMap<StackKey, StackStats>,
    symbols: DashMap<usize, String>,
}

impl MemoryProfiler {
    fn from_env() -> Self {
        let enabled = parse_bool_env("MEM_PROF_ENABLED", false);
        let sample_rate = env::var("MEM_PROF_SAMPLE_RATE")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_SAMPLE_RATE);
        let min_alloc_bytes = env::var("MEM_PROF_MIN_ALLOC_BYTES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_MIN_ALLOC_BYTES);
        let max_tracked_allocations = env::var("MEM_PROF_MAX_TRACKED_ALLOCATIONS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_MAX_TRACKED_ALLOCATIONS);
        let stack_skip = env::var("MEM_PROF_STACK_SKIP")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(DEFAULT_STACK_SKIP);

        Self {
            enabled: AtomicBool::new(enabled),
            sample_rate: AtomicU64::new(sample_rate),
            min_alloc_bytes: AtomicUsize::new(min_alloc_bytes),
            max_tracked_allocations: AtomicUsize::new(max_tracked_allocations),
            stack_skip,
            max_stack_depth: MAX_STACK_DEPTH,
            started_at: Instant::now(),
            allocation_seq: AtomicU64::new(0),
            tracked_allocations: AtomicUsize::new(0),
            dropped_allocations: AtomicU64::new(0),
            sampled_alloc_events: AtomicU64::new(0),
            sampled_dealloc_events: AtomicU64::new(0),
            sampled_realloc_events: AtomicU64::new(0),
            allocations: DashMap::new(),
            stacks: DashMap::new(),
            symbols: DashMap::new(),
        }
    }

    pub fn config_snapshot(&self) -> MemoryProfilerConfigSnapshot {
        MemoryProfilerConfigSnapshot {
            enabled: self.enabled.load(Ordering::Relaxed),
            sample_rate: self.sample_rate.load(Ordering::Relaxed),
            min_alloc_bytes: self.min_alloc_bytes.load(Ordering::Relaxed),
            max_tracked_allocations: self.max_tracked_allocations.load(Ordering::Relaxed),
            stack_skip: self.stack_skip,
            max_stack_depth: self.max_stack_depth,
        }
    }

    pub fn update_config(
        &self,
        update: MemoryProfilerConfigUpdate,
    ) -> Result<MemoryProfilerConfigSnapshot, String> {
        if let Some(enabled) = update.enabled {
            self.enabled.store(enabled, Ordering::Relaxed);
        }
        if let Some(sample_rate) = update.sample_rate {
            if sample_rate == 0 || sample_rate > 10_000 {
                return Err("`sample_rate` must be between 1 and 10000".to_string());
            }
            self.sample_rate.store(sample_rate, Ordering::Relaxed);
        }
        if let Some(min_alloc_bytes) = update.min_alloc_bytes {
            if min_alloc_bytes == 0 {
                return Err("`min_alloc_bytes` must be > 0".to_string());
            }
            self.min_alloc_bytes
                .store(min_alloc_bytes, Ordering::Relaxed);
        }
        if let Some(max_tracked_allocations) = update.max_tracked_allocations {
            if max_tracked_allocations == 0 {
                return Err("`max_tracked_allocations` must be > 0".to_string());
            }
            self.max_tracked_allocations
                .store(max_tracked_allocations, Ordering::Relaxed);
        }
        Ok(self.config_snapshot())
    }

    pub fn reset(&self) {
        let Some(_guard) = ReentryGuard::enter() else {
            return;
        };
        self.allocations.clear();
        self.stacks.clear();
        self.symbols.clear();
        self.allocation_seq.store(0, Ordering::Relaxed);
        self.tracked_allocations.store(0, Ordering::Relaxed);
        self.dropped_allocations.store(0, Ordering::Relaxed);
        self.sampled_alloc_events.store(0, Ordering::Relaxed);
        self.sampled_dealloc_events.store(0, Ordering::Relaxed);
        self.sampled_realloc_events.store(0, Ordering::Relaxed);
    }

    pub fn overview(&self) -> MemoryProfilerOverview {
        let Some(_guard) = ReentryGuard::enter() else {
            return MemoryProfilerOverview {
                generated_at_ms: now_ms(),
                config: self.config_snapshot(),
                process_uptime_secs: self.started_at.elapsed().as_secs(),
                tracked_allocations: self.tracked_allocations.load(Ordering::Relaxed),
                distinct_stacks: self.stacks.len(),
                dropped_allocations: self.dropped_allocations.load(Ordering::Relaxed),
                sampled_alloc_events: self.sampled_alloc_events.load(Ordering::Relaxed),
                sampled_dealloc_events: self.sampled_dealloc_events.load(Ordering::Relaxed),
                sampled_realloc_events: self.sampled_realloc_events.load(Ordering::Relaxed),
                total_live_bytes_estimate: 0,
                total_alloc_bytes_estimate: 0,
                process_rss_bytes: 0,
                process_virtual_bytes: 0,
                mimalloc: MiProcessMemoryInfo::default(),
            };
        };
        let (total_live_bytes_estimate, total_alloc_bytes_estimate) = self.total_bytes();
        let (process_rss_bytes, process_virtual_bytes) = read_process_memory();
        let mimalloc = read_mimalloc_process_info();

        MemoryProfilerOverview {
            generated_at_ms: now_ms(),
            config: self.config_snapshot(),
            process_uptime_secs: self.started_at.elapsed().as_secs(),
            tracked_allocations: self.tracked_allocations.load(Ordering::Relaxed),
            distinct_stacks: self.stacks.len(),
            dropped_allocations: self.dropped_allocations.load(Ordering::Relaxed),
            sampled_alloc_events: self.sampled_alloc_events.load(Ordering::Relaxed),
            sampled_dealloc_events: self.sampled_dealloc_events.load(Ordering::Relaxed),
            sampled_realloc_events: self.sampled_realloc_events.load(Ordering::Relaxed),
            total_live_bytes_estimate,
            total_alloc_bytes_estimate,
            process_rss_bytes,
            process_virtual_bytes,
            mimalloc,
        }
    }

    pub fn stacks_report(&self, top: usize) -> MemoryStackReport {
        let Some(_guard) = ReentryGuard::enter() else {
            return MemoryStackReport {
                generated_at_ms: now_ms(),
                top: normalize_top(top),
                total_live_bytes_estimate: 0,
                process_rss_bytes: 0,
                entries: Vec::new(),
            };
        };
        let top = normalize_top(top);
        let (process_rss_bytes, _) = read_process_memory();
        let (total_live_bytes, mut rows) = self.collect_stack_rows();
        rows.sort_by(|left, right| right.1.live_bytes.cmp(&left.1.live_bytes));

        let entries = rows
            .into_iter()
            .take(top)
            .map(|(stack, stats)| {
                let stack_id = format!("{:016x}", hash_stack_key(&stack));
                let live_ratio_heap = ratio(stats.live_bytes, total_live_bytes);
                let live_ratio_rss = ratio(stats.live_bytes, process_rss_bytes);
                let frames = stack
                    .frames()
                    .iter()
                    .map(|addr| self.resolve_symbol(*addr))
                    .collect::<Vec<_>>();
                MemoryStackEntry {
                    stack_id,
                    live_bytes_estimate: stats.live_bytes,
                    alloc_bytes_total_estimate: stats.alloc_bytes_total,
                    alloc_count: stats.alloc_count,
                    free_count: stats.free_count,
                    live_ratio_heap,
                    live_ratio_rss,
                    frames,
                }
            })
            .collect();

        MemoryStackReport {
            generated_at_ms: now_ms(),
            top,
            total_live_bytes_estimate: total_live_bytes,
            process_rss_bytes,
            entries,
        }
    }

    pub fn functions_report(&self, top: usize) -> MemoryFunctionReport {
        let Some(_guard) = ReentryGuard::enter() else {
            return MemoryFunctionReport {
                generated_at_ms: now_ms(),
                top: normalize_top(top),
                total_live_bytes_estimate: 0,
                process_rss_bytes: 0,
                entries: Vec::new(),
            };
        };
        let top = normalize_top(top);
        let (process_rss_bytes, _) = read_process_memory();
        let (total_live_bytes, rows) = self.collect_stack_rows();

        let mut agg: HashMap<String, FunctionAggregate> = HashMap::new();
        for (stack, stats) in rows {
            let function = self.resolve_function_for_stack(&stack);
            let module = module_name_from_function(&function);
            let entry = agg
                .entry(function.clone())
                .or_insert_with(|| FunctionAggregate {
                    function,
                    module,
                    live_bytes: 0,
                    alloc_bytes_total: 0,
                    stack_count: 0,
                    alloc_count: 0,
                    free_count: 0,
                });
            entry.live_bytes = entry.live_bytes.saturating_add(stats.live_bytes);
            entry.alloc_bytes_total = entry
                .alloc_bytes_total
                .saturating_add(stats.alloc_bytes_total);
            entry.stack_count = entry.stack_count.saturating_add(1);
            entry.alloc_count = entry.alloc_count.saturating_add(stats.alloc_count);
            entry.free_count = entry.free_count.saturating_add(stats.free_count);
        }

        let mut values = agg.into_values().collect::<Vec<_>>();
        values.sort_by(|left, right| right.live_bytes.cmp(&left.live_bytes));

        let entries = values
            .into_iter()
            .take(top)
            .map(|item| MemoryFunctionEntry {
                function: item.function,
                module: item.module,
                live_bytes_estimate: item.live_bytes,
                alloc_bytes_total_estimate: item.alloc_bytes_total,
                stack_count: item.stack_count,
                alloc_count: item.alloc_count,
                free_count: item.free_count,
                live_ratio_heap: ratio(item.live_bytes, total_live_bytes),
                live_ratio_rss: ratio(item.live_bytes, process_rss_bytes),
            })
            .collect();

        MemoryFunctionReport {
            generated_at_ms: now_ms(),
            top,
            total_live_bytes_estimate: total_live_bytes,
            process_rss_bytes,
            entries,
        }
    }

    pub fn modules_report(&self, top: usize) -> MemoryModuleReport {
        let Some(_guard) = ReentryGuard::enter() else {
            return MemoryModuleReport {
                generated_at_ms: now_ms(),
                top: normalize_top(top),
                total_live_bytes_estimate: 0,
                process_rss_bytes: 0,
                entries: Vec::new(),
            };
        };
        let top = normalize_top(top);
        let (process_rss_bytes, _) = read_process_memory();
        let (total_live_bytes, rows) = self.collect_stack_rows();

        let mut agg: HashMap<String, ModuleAggregate> = HashMap::new();
        for (stack, stats) in rows {
            let function = self.resolve_function_for_stack(&stack);
            let module = module_name_from_function(&function);
            let entry = agg
                .entry(module.clone())
                .or_insert_with(|| ModuleAggregate {
                    module,
                    live_bytes: 0,
                    alloc_bytes_total: 0,
                    function_names: HashSet::new(),
                    stack_count: 0,
                    alloc_count: 0,
                    free_count: 0,
                });
            entry.live_bytes = entry.live_bytes.saturating_add(stats.live_bytes);
            entry.alloc_bytes_total = entry
                .alloc_bytes_total
                .saturating_add(stats.alloc_bytes_total);
            entry.function_names.insert(function);
            entry.stack_count = entry.stack_count.saturating_add(1);
            entry.alloc_count = entry.alloc_count.saturating_add(stats.alloc_count);
            entry.free_count = entry.free_count.saturating_add(stats.free_count);
        }

        let mut values = agg.into_values().collect::<Vec<_>>();
        values.sort_by(|left, right| right.live_bytes.cmp(&left.live_bytes));

        let entries = values
            .into_iter()
            .take(top)
            .map(|item| MemoryModuleEntry {
                module: item.module,
                live_bytes_estimate: item.live_bytes,
                alloc_bytes_total_estimate: item.alloc_bytes_total,
                function_count: item.function_names.len(),
                stack_count: item.stack_count,
                alloc_count: item.alloc_count,
                free_count: item.free_count,
                live_ratio_heap: ratio(item.live_bytes, total_live_bytes),
                live_ratio_rss: ratio(item.live_bytes, process_rss_bytes),
            })
            .collect();

        MemoryModuleReport {
            generated_at_ms: now_ms(),
            top,
            total_live_bytes_estimate: total_live_bytes,
            process_rss_bytes,
            entries,
        }
    }

    fn collect_stack_rows(&self) -> (u64, Vec<(StackKey, StackStats)>) {
        let mut total_live_bytes = 0_u64;
        let mut rows = Vec::with_capacity(self.stacks.len());
        for entry in &self.stacks {
            let stats = *entry.value();
            if stats.live_bytes == 0 && stats.alloc_bytes_total == 0 {
                continue;
            }
            total_live_bytes = total_live_bytes.saturating_add(stats.live_bytes);
            rows.push((*entry.key(), stats));
        }
        (total_live_bytes, rows)
    }

    fn total_bytes(&self) -> (u64, u64) {
        let mut live = 0_u64;
        let mut allocated = 0_u64;
        for entry in &self.stacks {
            let stats = *entry.value();
            live = live.saturating_add(stats.live_bytes);
            allocated = allocated.saturating_add(stats.alloc_bytes_total);
        }
        (live, allocated)
    }

    fn on_alloc(&self, ptr: *mut u8, usable_bytes: usize) {
        if ptr.is_null() || !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let Some(_guard) = ReentryGuard::enter() else {
            return;
        };
        self.record_allocation(ptr as usize, usable_bytes);
    }

    fn on_dealloc(&self, ptr: *mut u8) {
        if ptr.is_null() {
            return;
        }
        let Some(_guard) = ReentryGuard::enter() else {
            return;
        };
        if self.remove_allocation(ptr as usize) {
            self.sampled_dealloc_events.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn on_realloc(&self, old_ptr: *mut u8, new_ptr: *mut u8, usable_bytes: usize) {
        if new_ptr.is_null() {
            return;
        }
        let Some(_guard) = ReentryGuard::enter() else {
            return;
        };

        if !old_ptr.is_null() && self.remove_allocation(old_ptr as usize) {
            self.sampled_dealloc_events.fetch_add(1, Ordering::Relaxed);
        }
        if self.enabled.load(Ordering::Relaxed) {
            self.record_allocation(new_ptr as usize, usable_bytes);
        }
        self.sampled_realloc_events.fetch_add(1, Ordering::Relaxed);
    }

    fn record_allocation(&self, ptr: usize, usable_bytes: usize) {
        if usable_bytes < self.min_alloc_bytes.load(Ordering::Relaxed) {
            return;
        }

        let sample_rate = self.sample_rate.load(Ordering::Relaxed).max(1);
        if !self.should_sample(sample_rate) {
            return;
        }

        if self.tracked_allocations.load(Ordering::Relaxed)
            >= self.max_tracked_allocations.load(Ordering::Relaxed)
        {
            self.dropped_allocations.fetch_add(1, Ordering::Relaxed);
            return;
        }

        // Defensive cleanup in case allocator address got re-used while a stale
        // entry is still tracked.
        self.remove_allocation(ptr);

        let stack = self.capture_stack();
        let weighted_bytes = (usable_bytes as u64).saturating_mul(sample_rate);

        self.allocations.insert(ptr, AllocationMeta {
            stack,
            weighted_bytes,
        });
        self.tracked_allocations.fetch_add(1, Ordering::Relaxed);
        self.sampled_alloc_events.fetch_add(1, Ordering::Relaxed);

        let mut stats = self.stacks.entry(stack).or_default();
        stats.live_bytes = stats.live_bytes.saturating_add(weighted_bytes);
        stats.alloc_bytes_total = stats.alloc_bytes_total.saturating_add(weighted_bytes);
        stats.alloc_count = stats.alloc_count.saturating_add(1);
    }

    fn remove_allocation(&self, ptr: usize) -> bool {
        let Some((_, meta)) = self.allocations.remove(&ptr) else {
            return false;
        };
        self.tracked_allocations.fetch_sub(1, Ordering::Relaxed);
        if let Some(mut stats) = self.stacks.get_mut(&meta.stack) {
            stats.live_bytes = stats.live_bytes.saturating_sub(meta.weighted_bytes);
            stats.free_count = stats.free_count.saturating_add(1);
        }
        true
    }

    fn capture_stack(&self) -> StackKey {
        let mut key = StackKey::default();
        let mut index = 0_usize;
        let mut skip = self.stack_skip;
        let max_depth = self.max_stack_depth.min(MAX_STACK_DEPTH);
        // SAFETY: trace_unsynchronized is safe for sampling the current
        // thread's stack as long as we avoid concurrent symbolization here.
        unsafe {
            trace_unsynchronized(|frame| {
                let ip = frame.ip() as usize;
                if ip == 0 {
                    return true;
                }
                if skip > 0 {
                    skip -= 1;
                    return true;
                }
                if index >= max_depth {
                    return false;
                }
                key.frames[index] = ip;
                index += 1;
                true
            });
        }
        key.len = index as u8;
        key
    }

    fn resolve_symbol(&self, addr: usize) -> String {
        if let Some(existing) = self.symbols.get(&addr) {
            return existing.clone();
        }
        let mut resolved = format!("0x{addr:016x}");
        resolve(addr as *mut c_void, |symbol| {
            let Some(name) = symbol.name() else {
                return;
            };
            let raw = name.to_string();
            let demangled = try_demangle(&raw)
                .map(|value| value.to_string())
                .unwrap_or(raw);
            resolved = strip_rust_symbol_hash(&demangled);
        });
        self.symbols.insert(addr, resolved.clone());
        resolved
    }

    fn resolve_function_for_stack(&self, stack: &StackKey) -> String {
        for addr in stack.frames() {
            let symbol = self.resolve_symbol(*addr);
            if !is_profiler_internal_symbol(&symbol) {
                return symbol;
            }
        }
        stack
            .frames()
            .first()
            .map(|addr| self.resolve_symbol(*addr))
            .unwrap_or_else(|| "unknown".to_string())
    }

    fn should_sample(&self, sample_rate: u64) -> bool {
        if sample_rate <= 1 {
            return true;
        }
        let seq = self.allocation_seq.fetch_add(1, Ordering::Relaxed);
        seq.is_multiple_of(sample_rate)
    }
}

pub struct ProfiledMiMalloc {
    inner: MiMalloc,
}

impl ProfiledMiMalloc {
    pub const fn new(inner: MiMalloc) -> Self {
        Self {
            inner,
        }
    }
}

unsafe impl GlobalAlloc for ProfiledMiMalloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = self.inner.alloc(layout);
        if !ptr.is_null() {
            let usable = self.inner.usable_size(ptr);
            if let Some(profiler) = profiler() {
                profiler.on_alloc(ptr, usable);
            }
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = self.inner.alloc_zeroed(layout);
        if !ptr.is_null() {
            let usable = self.inner.usable_size(ptr);
            if let Some(profiler) = profiler() {
                profiler.on_alloc(ptr, usable);
            }
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if let Some(profiler) = profiler() {
            profiler.on_dealloc(ptr);
        }
        self.inner.dealloc(ptr, layout);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let next_ptr = self.inner.realloc(ptr, layout, new_size);
        if !next_ptr.is_null() {
            let usable = self.inner.usable_size(next_ptr);
            if let Some(profiler) = profiler() {
                profiler.on_realloc(ptr, next_ptr, usable);
            }
        }
        next_ptr
    }
}

thread_local! {
    static IN_PROFILER_HOOK: Cell<bool> = const { Cell::new(false) };
}

struct ReentryGuard;

impl ReentryGuard {
    fn enter() -> Option<Self> {
        IN_PROFILER_HOOK.with(|flag| {
            if flag.get() {
                None
            } else {
                flag.set(true);
                Some(Self)
            }
        })
    }
}

impl Drop for ReentryGuard {
    fn drop(&mut self) {
        IN_PROFILER_HOOK.with(|flag| flag.set(false));
    }
}

static PROFILER: OnceLock<MemoryProfiler> = OnceLock::new();

pub fn init_from_env() -> &'static MemoryProfiler {
    PROFILER.get_or_init(MemoryProfiler::from_env)
}

pub fn profiler() -> Option<&'static MemoryProfiler> {
    PROFILER.get().filter(|p| p.enabled.load(Ordering::Relaxed))
}

fn parse_bool_env(key: &str, default_value: bool) -> bool {
    env::var(key)
        .ok()
        .map(|value| {
            matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(default_value)
}

fn read_process_memory() -> (u64, u64) {
    if let Some(stats) = memory_stats::memory_stats() {
        return (stats.physical_mem as u64, stats.virtual_mem as u64);
    }
    (0, 0)
}

fn read_mimalloc_process_info() -> MiProcessMemoryInfo {
    let mut elapsed = 0_usize;
    let mut user = 0_usize;
    let mut system = 0_usize;
    let mut current_rss = 0_usize;
    let mut peak_rss = 0_usize;
    let mut current_commit = 0_usize;
    let mut peak_commit = 0_usize;
    let mut page_faults = 0_usize;
    // SAFETY: all out-params are valid writable pointers.
    unsafe {
        better_mimalloc_sys::mi_process_info(
            &mut elapsed,
            &mut user,
            &mut system,
            &mut current_rss,
            &mut peak_rss,
            &mut current_commit,
            &mut peak_commit,
            &mut page_faults,
        );
    }
    // On Linux, mimalloc's committed.current is int64_t internally.
    // Under concurrent alloc/dealloc it can momentarily go negative,
    // and the cast to size_t produces a huge value (e.g. 16777216 TB).
    // Also, Linux mi_process_info sets current_rss = current_commit
    // (no /proc/self/statm read), so both fields can be bogus.
    // Clamp to 0 when the value exceeds a reasonable threshold (1 TB).
    const MAX_REASONABLE: usize = 1 << 40; // 1 TB
    let clamp = |v: usize| -> u64 {
        if v > MAX_REASONABLE {
            0
        } else {
            v as u64
        }
    };
    MiProcessMemoryInfo {
        elapsed_millis: elapsed as u64,
        user_millis: user as u64,
        system_millis: system as u64,
        current_rss_bytes: clamp(current_rss),
        peak_rss_bytes: clamp(peak_rss),
        current_commit_bytes: clamp(current_commit),
        peak_commit_bytes: clamp(peak_commit),
        page_faults: page_faults as u64,
    }
}

fn strip_rust_symbol_hash(value: &str) -> String {
    if let Some(index) = value.rfind("::h") {
        let hash = &value[index + 3..];
        if hash.len() == 16 && hash.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return value[..index].to_string();
        }
    }
    value.to_string()
}

fn is_profiler_internal_symbol(symbol: &str) -> bool {
    const INTERNAL_PREFIXES: [&str; 10] = [
        "static_flow_backend::memory_profiler::",
        "core::alloc::",
        "alloc::alloc::",
        "alloc::raw_vec::",
        "std::alloc::",
        "backtrace::",
        "rustc_demangle::",
        "dashmap::",
        "hashbrown::",
        "__rust_",
    ];
    INTERNAL_PREFIXES
        .iter()
        .any(|prefix| symbol.starts_with(prefix))
}

fn module_name_from_function(function: &str) -> String {
    let mut parts = function.split("::").collect::<Vec<_>>();
    if parts.len() <= 1 {
        return function.to_string();
    }
    parts.pop();
    parts.join("::")
}

fn hash_stack_key(stack: &StackKey) -> u64 {
    let mut hasher = DefaultHasher::new();
    stack.hash(&mut hasher);
    hasher.finish()
}

fn ratio(value: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        value as f64 / total as f64
    }
}

fn normalize_top(top: usize) -> usize {
    top.clamp(1, MAX_TOP_LIMIT)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as i64)
        .unwrap_or_default()
}

pub fn normalized_top_or_default(top: Option<usize>) -> usize {
    normalize_top(top.unwrap_or(DEFAULT_TOP_LIMIT))
}
