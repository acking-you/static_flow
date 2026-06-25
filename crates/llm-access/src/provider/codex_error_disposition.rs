//! Dispatch policy derived from Codex upstream error classes.

use std::time::Duration;

use super::{
    codex_upstream_error::{CodexClassifiedUpstreamError, CodexUpstreamErrorClass},
    CODEX_QUOTA_EXHAUSTION_COOLDOWN,
};

pub(crate) const CODEX_SERVER_OVERLOADED_COOLDOWN_MIN: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodexErrorDisposition {
    ReturnToClient { strict_session_block: bool },
    RetrySameAccount { retry_after: Option<Duration> },
    FailoverWithCooldown { cooldown: Duration },
    Failover,
}

impl CodexErrorDisposition {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ReturnToClient {
                strict_session_block: true,
            } => "return_to_client_block_session",
            Self::ReturnToClient {
                strict_session_block: false,
            } => "return_to_client",
            Self::RetrySameAccount {
                ..
            } => "retry_same_account",
            Self::FailoverWithCooldown {
                ..
            } => "failover_with_cooldown",
            Self::Failover => "failover",
        }
    }
}

pub(crate) fn codex_error_disposition(
    error: &CodexClassifiedUpstreamError,
) -> CodexErrorDisposition {
    match error.class {
        CodexUpstreamErrorClass::ContextWindowExceeded
        | CodexUpstreamErrorClass::CyberPolicy
        | CodexUpstreamErrorClass::InvalidRequest => CodexErrorDisposition::ReturnToClient {
            strict_session_block: true,
        },
        CodexUpstreamErrorClass::UsageNotIncluded => CodexErrorDisposition::ReturnToClient {
            strict_session_block: false,
        },
        CodexUpstreamErrorClass::QuotaExceeded => CodexErrorDisposition::FailoverWithCooldown {
            cooldown: CODEX_QUOTA_EXHAUSTION_COOLDOWN,
        },
        CodexUpstreamErrorClass::ServerOverloaded => CodexErrorDisposition::FailoverWithCooldown {
            cooldown: CODEX_SERVER_OVERLOADED_COOLDOWN_MIN,
        },
        CodexUpstreamErrorClass::Retryable => CodexErrorDisposition::RetrySameAccount {
            retry_after: error.retry_after,
        },
        CodexUpstreamErrorClass::Stream | CodexUpstreamErrorClass::UnexpectedStatus => {
            CodexErrorDisposition::Failover
        },
    }
}
