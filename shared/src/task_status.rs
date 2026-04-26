//! Shared task lifecycle status for wish / request / comment workflows.

use serde::{Deserialize, Serialize};

/// Task lifecycle status shared across music-wish, article-request, and
/// comment workflows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Awaiting review.
    Pending,
    /// Approved for processing.
    Approved,
    /// Currently being processed by an AI worker.
    Running,
    /// Successfully completed.
    Done,
    /// Processing failed (may be retried).
    Failed,
    /// Rejected by admin.
    Rejected,
}

impl TaskStatus {
    /// Parse the LanceDB stored status value.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "approved" => Some(Self::Approved),
            "running" => Some(Self::Running),
            "done" => Some(Self::Done),
            "failed" => Some(Self::Failed),
            "rejected" => Some(Self::Rejected),
            _ => None,
        }
    }

    /// String representation matching the LanceDB stored value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Rejected => "rejected",
        }
    }
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Validate a generic task status transition.
///
/// The transition table is identical across wish / request / comment
/// workflows (with one exception: wish and request allow `Failed -> Done`,
/// comments do not). Use `allow_failed_to_done` to control that edge.
pub fn validate_task_transition(
    current: TaskStatus,
    next: TaskStatus,
    allow_failed_to_done: bool,
) -> anyhow::Result<()> {
    if current == next {
        anyhow::bail!("{current} -> {next}");
    }

    let ok = matches!(
        (current, next),
        (TaskStatus::Pending, TaskStatus::Approved | TaskStatus::Running | TaskStatus::Rejected)
            | (TaskStatus::Approved, TaskStatus::Running | TaskStatus::Rejected)
            | (TaskStatus::Running, TaskStatus::Done | TaskStatus::Failed)
            | (
                TaskStatus::Failed,
                TaskStatus::Approved | TaskStatus::Running | TaskStatus::Rejected
            )
    ) || (allow_failed_to_done
        && current == TaskStatus::Failed
        && next == TaskStatus::Done);

    if ok {
        Ok(())
    } else {
        anyhow::bail!("{current} -> {next}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_roundtrip() {
        let s = serde_json::to_string(&TaskStatus::Pending).expect("serialize");
        assert_eq!(s, "\"pending\"");
        let v: TaskStatus = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(v, TaskStatus::Pending);
    }

    #[test]
    fn parse_stored_status_values() {
        assert_eq!(TaskStatus::parse("pending"), Some(TaskStatus::Pending));
        assert_eq!(TaskStatus::parse("approved"), Some(TaskStatus::Approved));
        assert_eq!(TaskStatus::parse("running"), Some(TaskStatus::Running));
        assert_eq!(TaskStatus::parse("done"), Some(TaskStatus::Done));
        assert_eq!(TaskStatus::parse("failed"), Some(TaskStatus::Failed));
        assert_eq!(TaskStatus::parse("rejected"), Some(TaskStatus::Rejected));
        assert_eq!(TaskStatus::parse("unknown"), None);
    }

    #[test]
    fn display_matches_as_str() {
        assert_eq!(TaskStatus::Done.to_string(), "done");
    }

    #[test]
    fn valid_transitions() {
        assert!(validate_task_transition(TaskStatus::Pending, TaskStatus::Approved, false).is_ok());
        assert!(validate_task_transition(TaskStatus::Running, TaskStatus::Done, false).is_ok());
        assert!(validate_task_transition(TaskStatus::Failed, TaskStatus::Done, true).is_ok());
    }

    #[test]
    fn invalid_transitions() {
        assert!(validate_task_transition(TaskStatus::Done, TaskStatus::Pending, false).is_err());
        assert!(validate_task_transition(TaskStatus::Pending, TaskStatus::Pending, false).is_err());
        assert!(validate_task_transition(TaskStatus::Failed, TaskStatus::Done, false).is_err());
    }
}
