use std::{env, path::PathBuf, str::FromStr};

use anyhow::{Context, Result};
use lettre::{
    message::Mailbox, transport::smtp::authentication::Credentials, AsyncSmtpTransport,
    AsyncTransport, Message, Tokio1Executor,
};
use serde::Deserialize;
use static_flow_shared::music_wish_store::MusicWishRecord;
use url::Url;

const DEFAULT_EMAIL_ACCOUNTS_FILE: &str = "backend/.local/email_accounts.json";
const FALLBACK_EMAIL_ACCOUNTS_FILE: &str = ".local/email_accounts.json";
const DEFAULT_SMTP_HOST: &str = "smtp.gmail.com";
const DEFAULT_SMTP_PORT: u16 = 587;

#[derive(Debug, Clone, Deserialize)]
struct EmailAccountsConfig {
    public_mailbox: PublicMailboxConfig,
    admin_mailbox: AdminMailboxConfig,
    #[serde(default)]
    admin_recipient: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct PublicMailboxConfig {
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
struct AdminMailboxConfig {
    username: String,
    app_password: String,
}

#[derive(Clone)]
pub struct EmailNotifier {
    from_mailbox: Mailbox,
    admin_recipient: String,
    mailer: AsyncSmtpTransport<Tokio1Executor>,
}

impl EmailNotifier {
    pub fn from_env() -> Result<Option<Self>> {
        let path = resolve_email_accounts_file_path();
        if !path.exists() {
            tracing::warn!(
                "email notifier disabled: credentials file not found at {}",
                path.display()
            );
            return Ok(None);
        }

        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read email accounts file {}", path.display()))?;
        let config: EmailAccountsConfig = serde_json::from_str(&raw)
            .with_context(|| format!("invalid email accounts JSON: {}", path.display()))?;
        let notifier = Self::build(config)?;
        tracing::info!("email notifier enabled using credentials file {}", path.display());
        Ok(Some(notifier))
    }

    fn build(config: EmailAccountsConfig) -> Result<Self> {
        let public_username =
            normalize_required_string(config.public_mailbox.username, "public_mailbox.username")?;
        let public_password = normalize_app_password(
            config.public_mailbox.app_password,
            "public_mailbox.app_password",
        )?;
        let smtp_host = config
            .public_mailbox
            .smtp_host
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_SMTP_HOST.to_string());
        let smtp_port = config.public_mailbox.smtp_port.unwrap_or(DEFAULT_SMTP_PORT);

        let admin_username =
            normalize_required_string(config.admin_mailbox.username, "admin_mailbox.username")?;
        // Keep admin mailbox password validated in config even if current flow doesn't
        // send from it.
        let _admin_password = normalize_app_password(
            config.admin_mailbox.app_password,
            "admin_mailbox.app_password",
        )?;
        let admin_recipient = match normalize_optional_string(config.admin_recipient) {
            Some(value) => normalize_email(value)?,
            None => normalize_email(admin_username)?,
        };

        let sender_email = normalize_email(public_username)?;
        let display_name = normalize_optional_string(config.public_mailbox.display_name)
            .unwrap_or_else(|| "StaticFlow".to_string());
        let from_mailbox = Mailbox::from_str(&format!("{display_name} <{sender_email}>"))
            .context("invalid sender mailbox")?;

        let credentials = Credentials::new(sender_email.clone(), public_password);
        let builder = if smtp_port == 465 {
            AsyncSmtpTransport::<Tokio1Executor>::relay(&smtp_host)
                .with_context(|| format!("invalid smtp relay host: {smtp_host}"))?
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&smtp_host)
                .with_context(|| format!("invalid smtp starttls host: {smtp_host}"))?
        };
        let mailer = builder.port(smtp_port).credentials(credentials).build();

        Ok(Self {
            from_mailbox,
            admin_recipient,
            mailer,
        })
    }

    pub async fn send_admin_new_wish_notification(&self, wish: &MusicWishRecord) -> Result<()> {
        let subject = format!("[StaticFlow] New Music Wish {} ({})", wish.song_name, wish.wish_id);
        let body = format!(
            "New music wish submitted.\n\nWish ID: {}\nSong: {}\nArtist hint: {}\nNickname: \
             {}\nRequester email: {}\nMessage:\n{}\n\nStatus: {}\nRegion: {}\nCreated at (ms): \
             {}\n",
            wish.wish_id,
            wish.song_name,
            wish.artist_hint.as_deref().unwrap_or("-"),
            wish.nickname,
            wish.requester_email.as_deref().unwrap_or("-"),
            wish.wish_message,
            wish.status,
            wish.ip_region,
            wish.created_at
        );
        self.send_plain_text_email(&self.admin_recipient, &subject, &body)
            .await
    }

    pub async fn send_user_wish_done_notification(
        &self,
        wish: &MusicWishRecord,
        play_url: Option<&str>,
    ) -> Result<()> {
        let requester_email = wish
            .requester_email
            .as_deref()
            .context("requester email missing for done notification")?;
        let subject = format!("[StaticFlow] 你的点歌已完成：{}", wish.song_name);
        let link_line = match play_url {
            Some(url) => format!("播放链接: {url}"),
            None => "播放链接: 暂不可用".to_string(),
        };
        let body = format!(
            "你好，{}：\n\n你的许愿任务已完成并入库。\n\n任务状态: {}\n任务ID: {}\n歌曲: \
             {}\n歌手提示: {}\n入库歌曲ID: {}\n\n完成内容:\n{}\n\n{}\n",
            wish.nickname,
            wish.status,
            wish.wish_id,
            wish.song_name,
            wish.artist_hint.as_deref().unwrap_or("-"),
            wish.ingested_song_id.as_deref().unwrap_or("-"),
            wish.ai_reply.as_deref().unwrap_or("-"),
            link_line
        );
        self.send_plain_text_email(requester_email, &subject, &body)
            .await
    }

    async fn send_plain_text_email(&self, to: &str, subject: &str, body: &str) -> Result<()> {
        let to_mailbox =
            Mailbox::from_str(to).with_context(|| format!("invalid recipient: {to}"))?;
        let email = Message::builder()
            .from(self.from_mailbox.clone())
            .to(to_mailbox)
            .subject(subject)
            .body(body.to_string())
            .context("failed to build email message")?;
        self.mailer
            .send(email)
            .await
            .context("failed to send email via SMTP")?;
        Ok(())
    }
}

pub fn normalize_requester_email_input(value: Option<String>) -> Result<Option<String>> {
    match normalize_optional_string(value) {
        Some(raw) => {
            if raw.chars().count() > 254 {
                anyhow::bail!("`requester_email` must be <= 254 chars");
            }
            Ok(Some(normalize_email(raw)?))
        },
        None => Ok(None),
    }
}

pub fn normalize_frontend_page_url_input(value: Option<String>) -> Result<Option<String>> {
    match normalize_optional_string(value) {
        Some(raw) => {
            if raw.chars().count() > 2000 {
                anyhow::bail!("`frontend_page_url` must be <= 2000 chars");
            }
            validate_frontend_url(&raw)?;
            Ok(Some(raw))
        },
        None => Ok(None),
    }
}

pub fn build_music_player_url(frontend_page_url: &str, song_id: &str) -> Result<String> {
    if song_id.trim().is_empty() {
        anyhow::bail!("song_id is required");
    }
    validate_frontend_url(frontend_page_url)?;

    let mut url = Url::parse(frontend_page_url).context("invalid frontend_page_url")?;
    let path = url.path();
    let has_static_flow_prefix = path == "/static_flow" || path.starts_with("/static_flow/");
    let encoded_song_id: String =
        url::form_urlencoded::byte_serialize(song_id.as_bytes()).collect();
    let target_path = if has_static_flow_prefix {
        format!("/static_flow/media/audio/{encoded_song_id}")
    } else {
        format!("/media/audio/{encoded_song_id}")
    };
    url.set_path(&target_path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.into())
}

fn resolve_email_accounts_file_path() -> PathBuf {
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

    default_path
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

fn normalize_email(value: String) -> Result<String> {
    let trimmed = value.trim();
    Mailbox::from_str(trimmed).with_context(|| format!("invalid email address: {trimmed}"))?;
    Ok(trimmed.to_string())
}

fn normalize_app_password(value: String, field_name: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        anyhow::bail!("{field_name} is required");
    }
    let compact: String = trimmed.chars().filter(|ch| !ch.is_whitespace()).collect();
    if compact.is_empty() {
        anyhow::bail!("{field_name} is required");
    }
    Ok(compact)
}

fn validate_frontend_url(raw: &str) -> Result<()> {
    let parsed = Url::parse(raw).with_context(|| format!("invalid URL: {raw}"))?;
    match parsed.scheme() {
        "http" | "https" => {},
        _ => anyhow::bail!("`frontend_page_url` must use http or https"),
    }
    if parsed.host_str().is_none() {
        anyhow::bail!("`frontend_page_url` must include a host");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        build_music_player_url, normalize_frontend_page_url_input, normalize_requester_email_input,
    };

    #[test]
    fn build_music_player_url_keeps_same_origin() {
        let output =
            build_music_player_url("https://example.com/media/audio?tab=library#top", "song-001")
                .expect("should build URL");
        assert_eq!(output, "https://example.com/media/audio/song-001");
    }

    #[test]
    fn build_music_player_url_supports_static_flow_prefix() {
        let output =
            build_music_player_url("https://example.com/static_flow/media/audio?s=1", "song-001")
                .expect("should build URL");
        assert_eq!(output, "https://example.com/static_flow/media/audio/song-001");
    }

    #[test]
    fn normalize_requester_email_accepts_valid_email() {
        let value = normalize_requester_email_input(Some("user@example.com".to_string()))
            .expect("should normalize");
        assert_eq!(value, Some("user@example.com".to_string()));
    }

    #[test]
    fn normalize_requester_email_rejects_invalid_email() {
        let err = normalize_requester_email_input(Some("not-email".to_string()));
        assert!(err.is_err());
    }

    #[test]
    fn normalize_frontend_page_url_rejects_non_http_scheme() {
        let err = normalize_frontend_page_url_input(Some("javascript:alert(1)".to_string()));
        assert!(err.is_err());
    }
}
