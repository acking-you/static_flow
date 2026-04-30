//! Provider-neutral upstream proxy selection contracts.

use serde::{Deserialize, Serialize};

/// Account-level proxy override mode stored alongside provider account
/// settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountProxyMode {
    /// Reuse the existing provider-level binding or environment fallback.
    #[default]
    Inherit,
    /// Bypass the shared upstream proxy and connect directly.
    Direct,
    /// Pin this account to one reusable shared proxy config.
    Fixed,
}

impl AccountProxyMode {
    /// Stable serialized representation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Direct => "direct",
            Self::Fixed => "fixed",
        }
    }
}

/// Account-level proxy selection persisted on provider account records.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AccountProxySelection {
    /// Account-level proxy override mode.
    #[serde(default)]
    pub proxy_mode: AccountProxyMode,
    /// Shared proxy-config id used when `proxy_mode` is
    /// [`AccountProxyMode::Fixed`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_config_id: Option<String>,
}

impl AccountProxySelection {
    /// Normalize fixed-mode ids and clear ids for non-fixed modes.
    pub fn canonicalize(mut self) -> Self {
        self.proxy_config_id = self
            .proxy_config_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if self.proxy_mode != AccountProxyMode::Fixed {
            self.proxy_config_id = None;
        }
        self
    }

    /// Whether the selection inherits provider-level proxy behavior.
    pub fn is_default(&self) -> bool {
        self.proxy_mode == AccountProxyMode::Inherit && self.proxy_config_id.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::{AccountProxyMode, AccountProxySelection};

    #[test]
    fn clears_proxy_config_id_unless_mode_is_fixed() {
        let selection = AccountProxySelection {
            proxy_mode: AccountProxyMode::Direct,
            proxy_config_id: Some("proxy-1".to_string()),
        }
        .canonicalize();

        assert_eq!(selection.proxy_mode, AccountProxyMode::Direct);
        assert_eq!(selection.proxy_config_id, None);
    }

    #[test]
    fn trims_fixed_proxy_config_id() {
        let selection = AccountProxySelection {
            proxy_mode: AccountProxyMode::Fixed,
            proxy_config_id: Some(" proxy-1 ".to_string()),
        }
        .canonicalize();

        assert_eq!(selection.proxy_config_id.as_deref(), Some("proxy-1"));
    }
}
