use std::sync::Arc;

use tokio::sync::RwLock;

use crate::types::{PlaybackJobStatusResponse, PlaybackMode, PlaybackStatus};

#[derive(Debug)]
pub struct PlaybackJobHandle {
    job_id: String,
    snapshot: RwLock<PlaybackJobStatusResponse>,
}

impl PlaybackJobHandle {
    pub fn new(job_id: String, mode: PlaybackMode) -> Arc<Self> {
        Arc::new(Self {
            snapshot: RwLock::new(PlaybackJobStatusResponse {
                job_id: job_id.clone(),
                status: PlaybackStatus::Preparing,
                mode: Some(mode),
                player_url: None,
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
