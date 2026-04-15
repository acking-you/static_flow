use std::sync::Arc;

use tokio::sync::RwLock;

use crate::types::{PlaybackJobStatusResponse, PlaybackMode, PlaybackStatus};

#[derive(Debug)]
pub struct PlaybackJobHandle {
    job_id: String,
    snapshot: RwLock<PlaybackJobStatusResponse>,
}

impl PlaybackJobHandle {
    pub fn new(
        job_id: String,
        mode: PlaybackMode,
        duration_seconds: Option<f64>,
        detail: Option<String>,
    ) -> Arc<Self> {
        Arc::new(Self {
            snapshot: RwLock::new(PlaybackJobStatusResponse {
                job_id: job_id.clone(),
                status: PlaybackStatus::Preparing,
                mode: Some(mode),
                player_url: None,
                duration_seconds,
                detail,
                error: None,
            }),
            job_id,
        })
    }

    pub fn job_id(&self) -> &str {
        &self.job_id
    }

    pub async fn snapshot(&self) -> PlaybackJobStatusResponse {
        self.snapshot.read().await.clone()
    }

    pub async fn mark_preparing(&self, detail: impl Into<String>) {
        let mut snapshot = self.snapshot.write().await;
        snapshot.status = PlaybackStatus::Preparing;
        snapshot.detail = Some(detail.into());
        snapshot.error = None;
    }

    pub async fn mark_ready(&self, player_url: String) {
        let mut snapshot = self.snapshot.write().await;
        snapshot.status = PlaybackStatus::Ready;
        snapshot.player_url = Some(player_url);
        snapshot.error = None;
    }

    pub async fn mark_failed(&self, error: impl Into<String>) {
        let mut snapshot = self.snapshot.write().await;
        snapshot.status = PlaybackStatus::Failed;
        snapshot.error = Some(error.into());
        snapshot.player_url = None;
    }
}

#[cfg(test)]
mod tests {
    use super::PlaybackJobHandle;
    use crate::types::PlaybackMode;

    #[tokio::test]
    async fn mark_preparing_updates_detail_without_losing_duration() {
        let job = PlaybackJobHandle::new(
            "job-1".to_string(),
            PlaybackMode::Raw,
            Some(42.5),
            Some("queued".to_string()),
        );

        job.mark_preparing("copying streams").await;

        let snapshot = job.snapshot().await;
        assert_eq!(snapshot.duration_seconds, Some(42.5));
        assert_eq!(snapshot.detail.as_deref(), Some("copying streams"));
        assert!(snapshot.error.is_none());
    }
}
