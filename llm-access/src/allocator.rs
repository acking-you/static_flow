//! Process allocator tuning for the standalone LLM access binaries.

use better_mimalloc_rs::{MiMalloc, MiMallocConfig};

/// Configure mimalloc for low steady-state RSS.
///
/// The service is control-plane heavy and cost-sensitive; keeping freed pages
/// committed for throughput is a bad tradeoff here. This should be called as
/// the first operation in each binary's `main`.
pub fn configure_process_allocator_for_low_rss() {
    let config = MiMallocConfig {
        eager_commit: Some(false),
        eager_commit_delay: Some(0),
        arena_eager_commit: Some(0),
        purge_decommits: Some(true),
        purge_delay: Some(0),
        arena_purge_mult: Some(1),
        purge_extend_delay: Some(0),
        generic_collect: Some(1_000),
    };
    MiMalloc::init_with(&config);
    // SAFETY: this only sets a process-local mimalloc option before the
    // service runtime starts allocating heavily.
    unsafe {
        better_mimalloc_sys::mi_option_set_enabled(better_mimalloc_sys::mi_option_allow_thp, false);
    }
}

/// Ask mimalloc to return unused pages after allocation-heavy maintenance work.
pub fn collect_process_allocator() {
    // SAFETY: this is a leaf allocator maintenance hook; it does not touch
    // application pointers.
    unsafe {
        better_mimalloc_sys::mi_collect(true);
    }
}
