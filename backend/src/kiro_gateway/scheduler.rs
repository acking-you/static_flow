//! Re-exported Kiro scheduler from the standalone LLM access runtime.

pub(crate) use llm_access_kiro::scheduler::*;

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[test]
    fn backend_scheduler_reexport_preserves_basic_acquire_path() {
        let scheduler = KiroRequestScheduler::new();
        let started = Instant::now();
        let lease = scheduler
            .try_acquire("alpha", 1, 0, started)
            .expect("first acquire should succeed");
        let blocked = scheduler
            .try_acquire("alpha", 1, 0, started)
            .expect_err("second acquire should hit the re-exported concurrency limiter");
        assert_eq!(blocked.reason, "local_concurrency_limit");
        drop(lease);
        scheduler
            .try_acquire("alpha", 1, 0, started)
            .expect("acquire should succeed after release");
    }
}
