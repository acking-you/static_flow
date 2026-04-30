//! Kiro provider runtime extracted for standalone LLM access.

#[allow(
    missing_docs,
    reason = "Extracted Kiro modules preserve the existing backend runtime surface during \
              migration."
)]
pub mod billable_multipliers;
#[allow(
    missing_docs,
    reason = "Extracted Kiro modules preserve the existing backend runtime surface during \
              migration."
)]
pub mod cache_policy;
#[allow(
    missing_docs,
    reason = "Extracted Kiro modules preserve the existing backend runtime surface during \
              migration."
)]
pub mod cache_sim;
#[allow(
    missing_docs,
    reason = "Extracted Kiro modules preserve the existing backend runtime surface during \
              migration."
)]
pub mod parser;
#[allow(
    missing_docs,
    reason = "Extracted Kiro modules preserve the existing backend runtime surface during \
              migration."
)]
pub mod scheduler;
#[allow(
    missing_docs,
    reason = "Extracted Kiro modules preserve the existing backend runtime surface during \
              migration."
)]
pub mod wire;
