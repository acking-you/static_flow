//! LanceDB storage layer for StaticFlow content, comments, music, and the
//! legacy LLM gateway tables.
//!
//! Everything here is host-only: this crate owns the vendored lance/lancedb
//! dependency tree, so wasm consumers (the frontend) depend on
//! `static-flow-shared` for plain types and never compile this crate.

/// Comment moderation storage models and persistence helpers.
#[allow(
    missing_docs,
    reason = "Store modules expose large DTO surfaces; the module contract is documented here \
              while inner items are documented separately."
)]
pub mod comments_store;

/// Content database queries and article/image API data structures.
#[allow(
    missing_docs,
    reason = "This storage module has a large public API surface that needs a dedicated \
              documentation pass."
)]
pub mod lancedb_api;

/// Music storage records and related query helpers.
#[allow(
    missing_docs,
    reason = "The module remains public for multiple crates, but documenting every exported \
              record belongs in the module itself."
)]
pub mod music_store;

/// Music wish workflow storage records and helper functions.
#[allow(
    missing_docs,
    reason = "The module exports many workflow DTOs; only the top-level contract is enforced in \
              this pass."
)]
pub mod music_wish_store;

/// Article request worker storage models and status helpers.
#[allow(
    missing_docs,
    reason = "The module is intentionally public for cross-crate reuse, but its item-level docs \
              are deferred to a focused follow-up."
)]
pub mod article_request_store;

/// Interactive page mirror storage and asset management helpers.
#[allow(
    missing_docs,
    reason = "The interactive store exports many records and helper methods; documenting every \
              item is deferred."
)]
pub mod interactive_store;

/// Shared persistence types for the LLM and Kiro gateway features.
#[allow(
    missing_docs,
    reason = "The gateway store module has a broad DTO surface that is better documented in place."
)]
pub mod llm_gateway_store;

/// LanceDB compaction and optimization helpers.
#[allow(
    missing_docs,
    reason = "Optimization helpers expose a compact but still multi-type surface that will be \
              documented in the module itself."
)]
pub mod optimize;

mod lance_schema_encoding;

/// Maintenance routines for rebuilding image embedding vectors.
#[allow(
    missing_docs,
    reason = "The module remains public for backend and CLI reuse, while detailed item docs are \
              deferred."
)]
pub mod image_vector_maintenance;
