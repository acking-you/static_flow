//! Runtime startup validation for the standalone LLM access service.

use anyhow::{anyhow, Context};

use crate::config::StorageConfig;

/// Validate and prepare the persistent state root before storage is opened.
pub fn validate_state_root(config: &StorageConfig) -> anyhow::Result<()> {
    let metadata = std::fs::metadata(&config.state_root).with_context(|| {
        format!("state root `{}` is not accessible", config.state_root.display())
    })?;
    if !metadata.is_dir() {
        return Err(anyhow!("state root `{}` is not a directory", config.state_root.display()));
    }
    for dir in [&config.kiro_auths_dir, &config.codex_auths_dir, &config.cdc_dir, &config.logs_dir]
    {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create `{}`", dir.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn validate_state_root_creates_expected_subdirectories() {
        let root =
            std::env::temp_dir().join(format!("llm-access-state-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create root");
        let config = crate::config::StorageConfig {
            state_root: root.clone(),
            sqlite_control: root.join("control/llm-access.sqlite3"),
            duckdb: root.join("analytics/usage.duckdb"),
            kiro_auths_dir: root.join("auths/kiro"),
            codex_auths_dir: root.join("auths/codex"),
            cdc_dir: root.join("cdc"),
            logs_dir: root.join("logs"),
        };

        super::validate_state_root(&config).expect("validate root");

        assert!(config.kiro_auths_dir.is_dir());
        assert!(config.codex_auths_dir.is_dir());
        assert!(config.cdc_dir.is_dir());
        assert!(config.logs_dir.is_dir());
        std::fs::remove_dir_all(&root).expect("cleanup");
    }
}
