use std::{
    env,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::Deserialize;

const DEFAULT_SUPPORT_DIR: &str = "backend/.local/llm_access_support";
const FALLBACK_SUPPORT_DIR: &str = ".local/llm_access_support";
const SUPPORT_CONFIG_FILE: &str = "config.json";
const PAYMENT_EMAIL_TEMPLATE_FILE: &str = "payment_email.md";
pub(crate) const ALIPAY_QR_FILE: &str = "alipay_qr.png";
pub(crate) const WECHAT_QR_FILE: &str = "wechat_qr.png";
pub(crate) const QQ_GROUP_QR_FILE: &str = "qq_group_qr.png";

#[derive(Debug, Clone)]
pub(crate) struct LlmAccessSupportConfig {
    pub base_dir: PathBuf,
    pub owner_display_name: String,
    pub sponsor_title: String,
    pub sponsor_intro: String,
    pub group_name: String,
    pub qq_group_number: String,
    pub group_invite_text: String,
    pub payment_email_subject: String,
    pub payment_email_signature: String,
    pub reply_to_email: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawLlmAccessSupportConfig {
    owner_display_name: String,
    sponsor_title: String,
    sponsor_intro: String,
    group_name: String,
    qq_group_number: String,
    group_invite_text: String,
    payment_email_subject: String,
    payment_email_signature: String,
    #[serde(default)]
    reply_to_email: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct SupportAsset {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
}

impl LlmAccessSupportConfig {
    pub fn payment_template_path(&self) -> PathBuf {
        self.base_dir.join(PAYMENT_EMAIL_TEMPLATE_FILE)
    }

    pub fn asset_path(&self, file_name: &str) -> PathBuf {
        self.base_dir.join(file_name)
    }

    pub fn has_group_qr(&self) -> bool {
        self.asset_path(QQ_GROUP_QR_FILE).exists()
    }
}

pub(crate) fn resolve_support_dir() -> PathBuf {
    if let Ok(raw) = env::var("LLM_ACCESS_SUPPORT_DIR") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    let default_path = PathBuf::from(DEFAULT_SUPPORT_DIR);
    if default_path.exists() {
        return default_path;
    }

    let fallback_path = PathBuf::from(FALLBACK_SUPPORT_DIR);
    if fallback_path.exists() {
        return fallback_path;
    }

    default_path
}

pub(crate) fn load_support_config() -> Result<LlmAccessSupportConfig> {
    let base_dir = resolve_support_dir();
    let config_path = base_dir.join(SUPPORT_CONFIG_FILE);
    let raw = std::fs::read_to_string(&config_path).with_context(|| {
        format!("failed to read llm access support config {}", config_path.display())
    })?;
    let parsed: RawLlmAccessSupportConfig = serde_json::from_str(&raw).with_context(|| {
        format!("invalid llm access support config JSON: {}", config_path.display())
    })?;
    Ok(LlmAccessSupportConfig {
        base_dir,
        owner_display_name: normalize_required(parsed.owner_display_name, "owner_display_name")?,
        sponsor_title: normalize_required(parsed.sponsor_title, "sponsor_title")?,
        sponsor_intro: normalize_required(parsed.sponsor_intro, "sponsor_intro")?,
        group_name: normalize_required(parsed.group_name, "group_name")?,
        qq_group_number: normalize_required(parsed.qq_group_number, "qq_group_number")?,
        group_invite_text: normalize_required(parsed.group_invite_text, "group_invite_text")?,
        payment_email_subject: normalize_required(
            parsed.payment_email_subject,
            "payment_email_subject",
        )?,
        payment_email_signature: normalize_required(
            parsed.payment_email_signature,
            "payment_email_signature",
        )?,
        reply_to_email: parsed
            .reply_to_email
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
    })
}

pub(crate) fn render_payment_email_markdown(config: &LlmAccessSupportConfig) -> Result<String> {
    let template_path = config.payment_template_path();
    let mut markdown = std::fs::read_to_string(&template_path).with_context(|| {
        format!("failed to read llm access payment email template {}", template_path.display())
    })?;
    for (placeholder, value) in [
        ("{{ owner_display_name }}", config.owner_display_name.as_str()),
        ("{{ sponsor_title }}", config.sponsor_title.as_str()),
        ("{{ sponsor_intro }}", config.sponsor_intro.as_str()),
        ("{{ group_name }}", config.group_name.as_str()),
        ("{{ qq_group_number }}", config.qq_group_number.as_str()),
        ("{{ group_invite_text }}", config.group_invite_text.as_str()),
        ("{{ payment_email_signature }}", config.payment_email_signature.as_str()),
    ] {
        markdown = markdown.replace(placeholder, value);
    }
    Ok(markdown)
}

pub(crate) fn load_support_asset(
    config: &LlmAccessSupportConfig,
    file_name: &str,
) -> Result<SupportAsset> {
    let (file_name, content_type) = match file_name {
        ALIPAY_QR_FILE => (ALIPAY_QR_FILE, "image/png"),
        WECHAT_QR_FILE => (WECHAT_QR_FILE, "image/png"),
        QQ_GROUP_QR_FILE => (QQ_GROUP_QR_FILE, "image/png"),
        _ => anyhow::bail!("unsupported support asset: {file_name}"),
    };
    let asset_path = config.asset_path(file_name);
    let bytes = std::fs::read(&asset_path)
        .with_context(|| format!("failed to read support asset {}", asset_path.display()))?;
    Ok(SupportAsset {
        bytes,
        content_type,
    })
}

fn normalize_required(value: String, field_name: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        anyhow::bail!("{field_name} is required");
    }
    Ok(trimmed.to_string())
}

#[allow(
    dead_code,
    reason = "This helper is kept around for future path-hardening work and targeted tests."
)]
fn _is_within_base_dir(base_dir: &Path, candidate: &Path) -> bool {
    candidate.starts_with(base_dir)
}
