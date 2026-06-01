//! Process and cgroup memory snapshots for admin diagnostics.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Memory stats for one running process.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(crate) struct ProcessMemoryStats {
    pub(crate) rss_bytes: Option<u64>,
    pub(crate) virtual_bytes: Option<u64>,
    pub(crate) cgroup_current_bytes: Option<u64>,
    pub(crate) cgroup_peak_bytes: Option<u64>,
    pub(crate) cgroup_high_bytes: Option<u64>,
    pub(crate) cgroup_max_bytes: Option<u64>,
    pub(crate) cgroup_swap_current_bytes: Option<u64>,
    pub(crate) cgroup_swap_max_bytes: Option<u64>,
}

/// Read memory stats for the current process.
pub(crate) fn read_current_process_memory_stats() -> ProcessMemoryStats {
    read_process_memory_stats_from_paths(Path::new("/proc/self"), Path::new("/sys/fs/cgroup"))
}

fn read_process_memory_stats_from_paths(proc_dir: &Path, cgroup_root: &Path) -> ProcessMemoryStats {
    let mut stats = read_proc_status_memory(proc_dir);
    if let Some(cgroup_dir) = read_cgroup_v2_dir(proc_dir, cgroup_root) {
        stats.cgroup_current_bytes = read_cgroup_bytes(&cgroup_dir.join("memory.current"));
        stats.cgroup_peak_bytes = read_cgroup_bytes(&cgroup_dir.join("memory.peak"));
        stats.cgroup_high_bytes = read_cgroup_bytes(&cgroup_dir.join("memory.high"));
        stats.cgroup_max_bytes = read_cgroup_bytes(&cgroup_dir.join("memory.max"));
        stats.cgroup_swap_current_bytes =
            read_cgroup_bytes(&cgroup_dir.join("memory.swap.current"));
        stats.cgroup_swap_max_bytes = read_cgroup_bytes(&cgroup_dir.join("memory.swap.max"));
    }
    stats
}

fn read_proc_status_memory(proc_dir: &Path) -> ProcessMemoryStats {
    let Ok(status) = std::fs::read_to_string(proc_dir.join("status")) else {
        return ProcessMemoryStats::default();
    };
    let mut stats = ProcessMemoryStats::default();
    for line in status.lines() {
        if let Some(bytes) = parse_proc_status_kib_line(line, "VmRSS:") {
            stats.rss_bytes = Some(bytes);
        } else if let Some(bytes) = parse_proc_status_kib_line(line, "VmSize:") {
            stats.virtual_bytes = Some(bytes);
        }
    }
    stats
}

fn read_cgroup_v2_dir(proc_dir: &Path, cgroup_root: &Path) -> Option<PathBuf> {
    let cgroup = std::fs::read_to_string(proc_dir.join("cgroup")).ok()?;
    let relative = parse_cgroup_v2_relative_path(&cgroup)?;
    let relative = relative.strip_prefix('/').unwrap_or(relative);
    Some(cgroup_root.join(relative))
}

fn read_cgroup_bytes(path: &Path) -> Option<u64> {
    let raw = std::fs::read_to_string(path).ok()?;
    parse_cgroup_bytes_value(&raw)
}

fn parse_proc_status_kib_line(line: &str, prefix: &str) -> Option<u64> {
    let rest = line.strip_prefix(prefix)?.trim();
    let raw_kib = rest.split_whitespace().next()?.parse::<u64>().ok()?;
    raw_kib.checked_mul(1024)
}

fn parse_cgroup_v2_relative_path(raw: &str) -> Option<&str> {
    raw.lines().find_map(|line| {
        let mut parts = line.splitn(3, ':');
        let hierarchy = parts.next()?;
        let controllers = parts.next()?;
        let path = parts.next()?;
        (hierarchy == "0" && controllers.is_empty()).then_some(path)
    })
}

fn parse_cgroup_bytes_value(raw: &str) -> Option<u64> {
    let value = raw.trim();
    if value == "max" || value.is_empty() {
        return None;
    }
    value.parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::{
        parse_cgroup_bytes_value, parse_cgroup_v2_relative_path, parse_proc_status_kib_line,
    };

    #[test]
    fn proc_status_kib_line_converts_to_bytes() {
        assert_eq!(parse_proc_status_kib_line("VmRSS:\t  1234 kB", "VmRSS:"), Some(1_263_616));
        assert_eq!(parse_proc_status_kib_line("VmSize: none", "VmRSS:"), None);
    }

    #[test]
    fn cgroup_bytes_value_treats_max_as_unlimited() {
        assert_eq!(parse_cgroup_bytes_value("3221225472\n"), Some(3_221_225_472));
        assert_eq!(parse_cgroup_bytes_value("max\n"), None);
    }

    #[test]
    fn cgroup_v2_path_uses_unified_hierarchy() {
        let raw = "0::/system.slice/llm-access-usage-worker.service\n";
        assert_eq!(
            parse_cgroup_v2_relative_path(raw),
            Some("/system.slice/llm-access-usage-worker.service")
        );
    }
}
