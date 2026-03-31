//! Shared library surface for the `sf-cli` binary.

/// CLI schema types that are shared across commands and tests.
#[allow(
    missing_docs,
    reason = "The schema module exports many storage DTOs; enforcing item-level docs there would \
              be a larger follow-up pass."
)]
pub mod schema;
/// Utility helpers reused by multiple `sf-cli` commands.
#[allow(
    missing_docs,
    reason = "The module is public for reuse in tests and command code, while detailed item docs \
              will be added module-by-module."
)]
pub mod utils;
