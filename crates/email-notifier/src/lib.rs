//! Shared StaticFlow email notification utilities.

use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result};
use lettre::{
    message::{header::ContentType, Attachment, Mailbox, MultiPart, SinglePart},
    transport::smtp::authentication::Credentials,
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use pulldown_cmark::{html, CowStr, Event, Options, Parser, Tag};
use serde::Deserialize;

const DEFAULT_EMAIL_ACCOUNTS_FILE: &str = "crates/backend/.local/email_accounts.json";
const FALLBACK_EMAIL_ACCOUNTS_FILE: &str = ".local/email_accounts.json";
const DEFAULT_SMTP_HOST: &str = "smtp.gmail.com";
const DEFAULT_SMTP_PORT: u16 = 587;

#[derive(Debug, Clone, Deserialize)]
struct RawEmailAccounts {
    public_mailbox: RawPublicMailbox,
    admin_mailbox: RawAdminMailbox,
    #[serde(default)]
    admin_recipient: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawPublicMailbox {
    #[serde(default)]
    smtp_host: Option<String>,
    #[serde(default)]
    smtp_port: Option<u16>,
    username: String,
    app_password: String,
    #[serde(default)]
    display_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawAdminMailbox {
    username: String,
    app_password: String,
}

/// Parsed email account configuration.
#[derive(Debug, Clone)]
pub struct EmailAccounts {
    /// SMTP host used by the public sender mailbox.
    pub smtp_host: String,
    /// SMTP port used by the public sender mailbox.
    pub smtp_port: u16,
    /// Public sender email address.
    pub public_username: String,
    /// Public sender app password.
    pub public_app_password: String,
    /// Public sender display name.
    pub public_display_name: String,
    /// Admin mailbox username.
    pub admin_username: String,
    /// Admin mailbox app password.
    pub admin_app_password: String,
    /// Admin recipient address.
    pub admin_recipient: String,
}

/// Rendered inline asset.
#[derive(Debug, Clone)]
pub struct InlineEmailAsset {
    /// Content-ID used in the rendered HTML.
    pub content_id: String,
    /// Source file name.
    pub filename: String,
    /// Raw bytes.
    pub bytes: Vec<u8>,
    /// MIME content type.
    pub content_type: String,
}

/// Rendered markdown email body.
#[derive(Debug, Clone)]
pub struct RenderedMarkdownEmail {
    /// HTML fragment rendered from markdown.
    pub html_fragment: String,
    /// Inline local image assets.
    pub inline_assets: Vec<InlineEmailAsset>,
}

/// SMTP email notifier.
#[derive(Clone)]
pub struct EmailNotifier {
    admin_recipient: String,
    from_mailbox: Mailbox,
    mailer: AsyncSmtpTransport<Tokio1Executor>,
}

impl EmailAccounts {
    /// Parse email accounts JSON.
    pub fn from_json(raw: &str) -> Result<Self> {
        let parsed: RawEmailAccounts =
            serde_json::from_str(raw).context("invalid email accounts JSON")?;
        Self::from_raw(parsed)
    }

    fn from_raw(raw: RawEmailAccounts) -> Result<Self> {
        let public_username =
            normalize_required_string(raw.public_mailbox.username, "public_mailbox.username")?;
        let public_app_password =
            normalize_app_password(raw.public_mailbox.app_password, "public_mailbox.app_password")?;
        let smtp_host = raw
            .public_mailbox
            .smtp_host
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_SMTP_HOST.to_string());
        let smtp_port = raw.public_mailbox.smtp_port.unwrap_or(DEFAULT_SMTP_PORT);
        let public_display_name = normalize_optional_string(raw.public_mailbox.display_name)
            .unwrap_or_else(|| "StaticFlow".to_string());

        let admin_username =
            normalize_required_string(raw.admin_mailbox.username, "admin_mailbox.username")?;
        let admin_app_password =
            normalize_app_password(raw.admin_mailbox.app_password, "admin_mailbox.app_password")?;
        let admin_recipient = match normalize_optional_string(raw.admin_recipient) {
            Some(value) => normalize_email(value)?,
            None => normalize_email(admin_username.clone())?,
        };

        Ok(Self {
            smtp_host,
            smtp_port,
            public_username: normalize_email(public_username)?,
            public_app_password,
            public_display_name,
            admin_username: normalize_email(admin_username)?,
            admin_app_password,
            admin_recipient,
        })
    }
}

impl EmailNotifier {
    /// Load an optional notifier from environment.
    pub fn from_env() -> Result<Option<Self>> {
        let path = resolve_email_accounts_file_path();
        if !path.exists() {
            tracing::warn!(
                "email notifier disabled: credentials file not found at {}",
                path.display()
            );
            return Ok(None);
        }
        Self::from_path(path).map(Some)
    }

    /// Load a notifier from a specific JSON file.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read email accounts file {}", path.display()))?;
        let accounts = EmailAccounts::from_json(&raw)
            .with_context(|| format!("invalid email accounts JSON: {}", path.display()))?;
        let notifier = Self::from_accounts(accounts)?;
        tracing::info!("email notifier enabled using credentials file {}", path.display());
        Ok(notifier)
    }

    /// Build a notifier from parsed account configuration.
    pub fn from_accounts(accounts: EmailAccounts) -> Result<Self> {
        let from_mailbox = Mailbox::from_str(&format!(
            "{} <{}>",
            accounts.public_display_name, accounts.public_username
        ))
        .context("invalid sender mailbox")?;
        let credentials = Credentials::new(
            accounts.public_username.clone(),
            accounts.public_app_password.clone(),
        );
        let builder = if accounts.smtp_port == 465 {
            AsyncSmtpTransport::<Tokio1Executor>::relay(&accounts.smtp_host)
                .with_context(|| format!("invalid smtp relay host: {}", accounts.smtp_host))?
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&accounts.smtp_host)
                .with_context(|| format!("invalid smtp starttls host: {}", accounts.smtp_host))?
        };
        let mailer = builder
            .port(accounts.smtp_port)
            .credentials(credentials)
            .build();
        Ok(Self {
            admin_recipient: accounts.admin_recipient,
            from_mailbox,
            mailer,
        })
    }

    /// Admin recipient address.
    pub fn admin_recipient(&self) -> &str {
        &self.admin_recipient
    }

    /// Send one markdown email.
    pub async fn send_markdown_email(
        &self,
        to: &str,
        subject: &str,
        markdown_body: &str,
    ) -> Result<()> {
        self.send_markdown_email_with_options(to, subject, markdown_body, None, None)
            .await
    }

    /// Send one markdown email with optional inline local assets and reply-to.
    pub async fn send_markdown_email_with_options(
        &self,
        to: &str,
        subject: &str,
        markdown_body: &str,
        asset_base_dir: Option<&Path>,
        reply_to: Option<&str>,
    ) -> Result<()> {
        let to_mailbox =
            Mailbox::from_str(to).with_context(|| format!("invalid recipient: {to}"))?;
        let rendered = render_markdown_email(markdown_body, asset_base_dir)?;
        let html_body = build_html_email_document(subject, &rendered.html_fragment);
        let plain_part = SinglePart::builder()
            .header(ContentType::TEXT_PLAIN)
            .body(markdown_body.to_string());
        let html_part = SinglePart::builder()
            .header(ContentType::TEXT_HTML)
            .body(html_body);
        let multipart = if rendered.inline_assets.is_empty() {
            MultiPart::alternative()
                .singlepart(plain_part)
                .singlepart(html_part)
        } else {
            let related = rendered.inline_assets.into_iter().try_fold(
                MultiPart::related().singlepart(html_part),
                |multipart, asset| {
                    let content_type = ContentType::parse(&asset.content_type)
                        .context("invalid asset MIME type")?;
                    Ok::<_, anyhow::Error>(
                        multipart.singlepart(
                            Attachment::new_inline_with_name(asset.content_id, asset.filename)
                                .body(asset.bytes, content_type),
                        ),
                    )
                },
            )?;
            MultiPart::alternative()
                .singlepart(plain_part)
                .multipart(related)
        };
        let mut builder = Message::builder()
            .from(self.from_mailbox.clone())
            .to(to_mailbox)
            .subject(subject);
        if let Some(reply_to) = reply_to {
            let reply_to_mailbox = Mailbox::from_str(reply_to)
                .with_context(|| format!("invalid reply-to recipient: {reply_to}"))?;
            builder = builder.reply_to(reply_to_mailbox);
        }
        let email = builder
            .multipart(multipart)
            .context("failed to build email message")?;
        self.mailer
            .send(email)
            .await
            .context("failed to send email via SMTP")?;
        Ok(())
    }
}

/// Resolve the configured email accounts file path.
pub fn resolve_email_accounts_file_path() -> PathBuf {
    if let Ok(raw) = env::var("EMAIL_ACCOUNTS_FILE") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    let default_path = PathBuf::from(DEFAULT_EMAIL_ACCOUNTS_FILE);
    if default_path.exists() {
        return default_path;
    }

    let fallback_path = PathBuf::from(FALLBACK_EMAIL_ACCOUNTS_FILE);
    if fallback_path.exists() {
        return fallback_path;
    }

    PathBuf::from("crates/backend/.local/email_accounts.json")
}

/// Normalize and validate one email address.
pub fn normalize_email(value: String) -> Result<String> {
    let trimmed = value.trim();
    Mailbox::from_str(trimmed).with_context(|| format!("invalid email address: {trimmed}"))?;
    Ok(trimmed.to_string())
}

/// Render markdown to email HTML and inline local assets.
pub fn render_markdown_email(
    markdown: &str,
    asset_base_dir: Option<&Path>,
) -> Result<RenderedMarkdownEmail> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_GFM);
    let mut inline_assets = Vec::new();
    let mut inline_asset_ids = HashMap::<PathBuf, String>::new();
    let mut render_error = None::<anyhow::Error>;
    let parser = Parser::new_ext(markdown, options).map(|event| match event {
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) => {
            if render_error.is_some() {
                return Event::Start(Tag::Image {
                    link_type,
                    dest_url,
                    title,
                    id,
                });
            }
            let mut resolved_dest_url = dest_url;
            if let Some(base_dir) = asset_base_dir {
                match maybe_register_inline_asset(
                    base_dir,
                    resolved_dest_url.as_ref(),
                    &mut inline_assets,
                    &mut inline_asset_ids,
                ) {
                    Ok(Some(content_id)) => {
                        resolved_dest_url =
                            CowStr::Boxed(format!("cid:{content_id}").into_boxed_str());
                    },
                    Ok(None) => {},
                    Err(err) => render_error = Some(err),
                }
            }
            Event::Start(Tag::Image {
                link_type,
                dest_url: resolved_dest_url,
                title,
                id,
            })
        },
        other => other,
    });
    let mut output = String::new();
    html::push_html(&mut output, parser);
    if let Some(err) = render_error {
        return Err(err);
    }
    Ok(RenderedMarkdownEmail {
        html_fragment: output,
        inline_assets,
    })
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
}

fn normalize_required_string(value: String, field_name: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        anyhow::bail!("{field_name} is required");
    }
    Ok(trimmed.to_string())
}

fn normalize_app_password(value: String, field_name: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        anyhow::bail!("{field_name} is required");
    }
    Ok(trimmed.split_whitespace().collect())
}

fn build_html_email_document(subject: &str, content_html: &str) -> String {
    let escaped_subject = escape_html(subject);
    format!(
        r#"<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{}</title>
  <style>
    .sf-content a {{
      color: #2563eb;
      text-decoration: underline;
      word-break: break-all;
    }}
    .sf-content img {{
      max-width: 100%;
      height: auto;
      border-radius: 8px;
      display: block;
      margin: 12px 0;
    }}
    .sf-content pre {{
      white-space: pre-wrap;
      background: #f8fafc;
      border: 1px solid #e5e7eb;
      border-radius: 8px;
      padding: 10px;
      overflow-x: auto;
    }}
    .sf-content code {{
      background: #f3f4f6;
      border-radius: 4px;
      padding: 2px 4px;
    }}
  </style>
</head>
<body style="margin:0;padding:0;background:#f5f7fb;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,'PingFang SC','Hiragino Sans GB','Microsoft YaHei',sans-serif;color:#1f2937;">
  <table role="presentation" width="100%" cellpadding="0" cellspacing="0" style="padding:24px 12px;">
    <tr>
      <td align="center">
        <table role="presentation" width="100%" cellpadding="0" cellspacing="0" style="max-width:720px;background:#ffffff;border:1px solid #e5e7eb;border-radius:14px;padding:22px;">
          <tr>
            <td style="font-size:20px;font-weight:700;color:#111827;padding-bottom:14px;border-bottom:1px solid #eef2f7;">{}</td>
          </tr>
          <tr>
            <td style="padding-top:18px;font-size:15px;line-height:1.65;">
              <div class="sf-content" style="word-break:break-word;">
                {}
              </div>
            </td>
          </tr>
        </table>
      </td>
    </tr>
  </table>
</body>
</html>"#,
        escaped_subject, escaped_subject, content_html
    )
}

fn maybe_register_inline_asset(
    base_dir: &Path,
    dest_url: &str,
    inline_assets: &mut Vec<InlineEmailAsset>,
    inline_asset_ids: &mut HashMap<PathBuf, String>,
) -> Result<Option<String>> {
    if !should_inline_local_image_reference(dest_url) {
        return Ok(None);
    }
    let clean_ref = dest_url.split(['#', '?']).next().unwrap_or(dest_url).trim();
    if clean_ref.is_empty() {
        return Ok(None);
    }
    let candidate = Path::new(clean_ref);
    let resolved_path =
        if candidate.is_absolute() { candidate.to_path_buf() } else { base_dir.join(candidate) };
    let canonical_path = resolved_path.canonicalize().with_context(|| {
        format!("failed to resolve local email image asset {}", resolved_path.display())
    })?;
    if let Some(existing_content_id) = inline_asset_ids.get(&canonical_path) {
        return Ok(Some(existing_content_id.clone()));
    }

    let bytes = std::fs::read(&canonical_path).with_context(|| {
        format!("failed to read local email image {}", canonical_path.display())
    })?;
    let filename = canonical_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .with_context(|| format!("invalid local email image path {}", canonical_path.display()))?;
    let content_type = detect_inline_asset_content_type(&canonical_path)?;
    let content_id = format!("sf-inline-{}", inline_assets.len() + 1);
    inline_assets.push(InlineEmailAsset {
        content_id: content_id.clone(),
        filename,
        bytes,
        content_type,
    });
    inline_asset_ids.insert(canonical_path, content_id.clone());
    Ok(Some(content_id))
}

fn should_inline_local_image_reference(dest_url: &str) -> bool {
    let trimmed = dest_url.trim();
    !(trimmed.is_empty()
        || trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("cid:")
        || trimmed.starts_with("data:")
        || trimmed.starts_with("mailto:")
        || trimmed.starts_with('#'))
}

fn detect_inline_asset_content_type(path: &Path) -> Result<String> {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        anyhow::bail!("local email image {} has no file extension", path.display());
    };
    let mime = match ext.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => {
            anyhow::bail!("local email image {} has unsupported extension .{}", path.display(), ext)
        },
    };
    Ok(mime.to_string())
}

fn escape_html(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_email_accounts_with_gmail_defaults_and_admin_fallback() {
        let raw = r#"{
            "public_mailbox": {
                "username": " public@example.com ",
                "app_password": " abcd efgh ijkl mnop ",
                "display_name": " StaticFlow Mail "
            },
            "admin_mailbox": {
                "username": " admin@example.com ",
                "app_password": " zzzz yyyy xxxx wwww "
            }
        }"#;

        let config = EmailAccounts::from_json(raw).expect("parse email accounts");

        assert_eq!(config.smtp_host, "smtp.gmail.com");
        assert_eq!(config.smtp_port, 587);
        assert_eq!(config.public_username, "public@example.com");
        assert_eq!(config.public_app_password, "abcdefghijklmnop");
        assert_eq!(config.public_display_name, "StaticFlow Mail");
        assert_eq!(config.admin_username, "admin@example.com");
        assert_eq!(config.admin_app_password, "zzzzyyyyxxxxwwww");
        assert_eq!(config.admin_recipient, "admin@example.com");
    }

    #[test]
    fn markdown_renderer_inlines_relative_local_images() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::write(root.path().join("qr.png"), b"not-a-real-png").expect("write test image");

        let rendered = render_markdown_email("![QR](qr.png)", Some(root.path()))
            .expect("render markdown email");

        assert!(rendered.html_fragment.contains("cid:sf-inline-1"));
        assert_eq!(rendered.inline_assets.len(), 1);
        assert_eq!(rendered.inline_assets[0].content_id, "sf-inline-1");
        assert_eq!(rendered.inline_assets[0].filename, "qr.png");
        assert_eq!(rendered.inline_assets[0].content_type, "image/png");
        assert_eq!(rendered.inline_assets[0].bytes, b"not-a-real-png");
    }
}
