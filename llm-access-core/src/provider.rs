//! Provider-neutral request and routing contracts.

use serde::{Deserialize, Serialize};

/// LLM provider family used by keys, accounts, usage events, and routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    /// Codex/OpenAI-compatible provider path.
    Codex,
    /// Kiro/Claude-compatible provider path.
    Kiro,
}

/// Client-facing protocol family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolFamily {
    /// OpenAI-compatible API surface.
    OpenAi,
    /// Anthropic/Claude-compatible API surface.
    Anthropic,
}

/// Account routing strategy stored on a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteStrategy {
    /// Let the runtime choose from eligible accounts.
    Auto,
    /// Force a single account.
    Fixed,
}
