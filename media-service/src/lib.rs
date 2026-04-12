//! Standalone admin local-media service for StaticFlow.
#![allow(
    missing_docs,
    reason = "internal service crate keeps implementation details private to the workspace"
)]

/// Cache path management and cache key generation.
pub mod cache;
/// Media service configuration loaded from the environment.
pub mod config;
/// FFmpeg and ffprobe discovery plus command construction.
pub mod ffmpeg;
/// Filesystem browsing for the local media root.
pub mod fs;
/// HTTP handlers exposed by the media service.
pub mod handlers;
/// Playback job state and coordination.
pub mod jobs;
/// Path confinement for media-root-relative requests.
pub mod path_guard;
/// Playback opening, raw streaming, and HLS serving.
pub mod playback;
/// Poster extraction and streaming.
pub mod poster;
/// Media probing and playback-mode decisions.
pub mod probe;
/// Route registration for the internal media service API.
pub mod routes;
/// Shared service state and initialization.
pub mod state;
/// Shared request and response types.
pub mod types;

/// Re-exported local-media state for the media service.
pub use state::LocalMediaState;
