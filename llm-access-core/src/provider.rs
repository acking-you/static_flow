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

impl ProviderType {
    /// Parse the canonical provider string stored in control-plane records.
    pub fn from_storage_str(value: &str) -> Option<Self> {
        match value {
            "codex" => Some(Self::Codex),
            "kiro" => Some(Self::Kiro),
            _ => None,
        }
    }

    /// Return the canonical provider string stored in control-plane records.
    pub fn as_storage_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Kiro => "kiro",
        }
    }
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

impl ProtocolFamily {
    /// Parse the canonical protocol string stored in control-plane records.
    pub fn from_storage_str(value: &str) -> Option<Self> {
        match value {
            "openai" => Some(Self::OpenAi),
            "anthropic" => Some(Self::Anthropic),
            _ => None,
        }
    }

    /// Return the canonical protocol string stored in control-plane records.
    pub fn as_storage_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
        }
    }
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

impl RouteStrategy {
    /// Parse the canonical route strategy string stored in control-plane
    /// records.
    pub fn from_storage_str(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "fixed" => Some(Self::Fixed),
            _ => None,
        }
    }

    /// Return the canonical route strategy string stored in control-plane
    /// records.
    pub fn as_storage_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Fixed => "fixed",
        }
    }
}
