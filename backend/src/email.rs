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
use static_flow_shared::{
    article_request_store::ArticleRequestRecord,
    llm_gateway_store::{
        LlmGatewayAccountContributionRequestRecord, LlmGatewayKeyRecord,
        LlmGatewayTokenRequestRecord,
    },
    music_wish_store::MusicWishRecord,
};
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

#[derive(Debug)]
struct InlineEmailAsset {
    content_id: String,
    filename: String,
    bytes: Vec<u8>,
    content_type: ContentType,
}

#[derive(Debug)]
struct RenderedMarkdownEmail {
    html_fragment: String,
    inline_assets: Vec<InlineEmailAsset>,
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
        let body_markdown = format!(
            "## New music wish submitted\n\n- Wish ID: `{}`\n- Song: {}\n- Artist hint: {}\n- \
             Nickname: {}\n- Requester email: {}\n- Status: `{}`\n- Region: {}\n- Created at \
             (ms): `{}`\n\n### Message\n\n{}\n",
            wish.wish_id,
            wish.song_name,
            wish.artist_hint.as_deref().unwrap_or("-"),
            wish.nickname,
            wish.requester_email.as_deref().unwrap_or("-"),
            wish.status,
            wish.ip_region,
            wish.created_at,
            wish.wish_message,
        );
        self.send_markdown_email(&self.admin_recipient, &subject, &body_markdown)
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
        let link_markdown = match play_url {
            Some(url) => format!("- 播放链接: [{url}]({url})"),
            None => "- 播放链接: 暂不可用".to_string(),
        };
        let body_markdown = format!(
            "你好，{}：\n\n你的许愿任务已完成并入库。\n\n## 任务信息\n- 任务状态: `{}`\n- 任务ID: \
             `{}`\n- 歌曲: {}\n- 歌手提示: {}\n- 入库歌曲ID: `{}`\n\n## 完成内容\n\n{}\n\n## \
             播放\n{}\n",
            wish.nickname,
            wish.status,
            wish.wish_id,
            wish.song_name,
            wish.artist_hint.as_deref().unwrap_or("-"),
            wish.ingested_song_id.as_deref().unwrap_or("-"),
            wish.ai_reply.as_deref().unwrap_or("-"),
            link_markdown,
        );
        self.send_markdown_email(requester_email, &subject, &body_markdown)
            .await
    }

    pub async fn send_admin_new_article_request_notification(
        &self,
        req: &ArticleRequestRecord,
    ) -> Result<()> {
        let subject = format!(
            "[StaticFlow] New Article Request {} ({})",
            truncate_str(&req.article_url, 60),
            req.request_id
        );
        let body_markdown = format!(
            "## New article request submitted\n\n- Request ID: `{}`\n- URL: {}\n- Title hint: \
             {}\n- Nickname: {}\n- Requester email: {}\n- Status: `{}`\n- Region: {}\n- Created \
             at (ms): `{}`\n\n### Message\n\n{}\n",
            req.request_id,
            req.article_url,
            req.title_hint.as_deref().unwrap_or("-"),
            req.nickname,
            req.requester_email.as_deref().unwrap_or("-"),
            req.status,
            req.ip_region,
            req.created_at,
            req.request_message,
        );
        self.send_markdown_email(&self.admin_recipient, &subject, &body_markdown)
            .await
    }

    pub async fn send_user_article_request_done_notification(
        &self,
        req: &ArticleRequestRecord,
        article_detail_url: Option<&str>,
    ) -> Result<()> {
        let requester_email = req
            .requester_email
            .as_deref()
            .context("requester email missing for done notification")?;
        let subject =
            format!("[StaticFlow] 你的文章入库请求已完成：{}", truncate_str(&req.article_url, 60));
        let link_markdown = match article_detail_url {
            Some(url) => format!("- 文章链接: [{url}]({url})"),
            None => "- 文章链接: 暂不可用".to_string(),
        };
        let body_markdown = format!(
            "你好，{}：\n\n你的文章入库请求已完成。\n\n## 请求信息\n- 请求状态: `{}`\n- 请求ID: \
             `{}`\n- 原文链接: {}\n- 标题提示: {}\n- 入库文章ID: `{}`\n\n## 完成内容\n\n{}\n\n## \
             查看\n{}\n",
            req.nickname,
            req.status,
            req.request_id,
            req.article_url,
            req.title_hint.as_deref().unwrap_or("-"),
            req.ingested_article_id.as_deref().unwrap_or("-"),
            req.ai_reply.as_deref().unwrap_or("-"),
            link_markdown,
        );
        self.send_markdown_email(requester_email, &subject, &body_markdown)
            .await
    }

    pub async fn send_admin_new_llm_token_request_notification(
        &self,
        request: &LlmGatewayTokenRequestRecord,
    ) -> Result<()> {
        let subject = format!(
            "[StaticFlow] New LLM Token Wish {} ({})",
            request.requested_quota_billable_limit, request.request_id
        );
        let body_markdown = format!(
            "## New LLM token wish submitted\n\n- Request ID: `{}`\n- Requester email: {}\n- \
             Requested tokens: `{}`\n- Status: `{}`\n- Region: {}\n- Client IP: {}\n- Created at \
             (ms): `{}`\n- Frontend page: {}\n\n### Reason\n\n{}\n",
            request.request_id,
            request.requester_email,
            request.requested_quota_billable_limit,
            request.status,
            request.ip_region,
            request.client_ip,
            request.created_at,
            request.frontend_page_url.as_deref().unwrap_or("-"),
            request.request_reason,
        );
        self.send_markdown_email(&self.admin_recipient, &subject, &body_markdown)
            .await
    }

    pub async fn send_admin_new_llm_account_contribution_request_notification(
        &self,
        request: &LlmGatewayAccountContributionRequestRecord,
    ) -> Result<()> {
        let subject = format!(
            "[StaticFlow] New Codex Account Contribution {} ({})",
            request.account_name, request.request_id
        );
        let body_markdown = format!(
            "## New Codex account contribution submitted\n\n- Request ID: `{}`\n- Account name: \
             `{}`\n- Account ID: {}\n- Requester email: {}\n- GitHub ID: {}\n- Status: `{}`\n- \
             Region: {}\n- Client IP: {}\n- Created at (ms): `{}`\n- Frontend page: {}\n\n### \
             Message\n\n{}\n",
            request.request_id,
            request.account_name,
            request.account_id.as_deref().unwrap_or("-"),
            request.requester_email,
            request.github_id.as_deref().unwrap_or("-"),
            request.status,
            request.ip_region,
            request.client_ip,
            request.created_at,
            request.frontend_page_url.as_deref().unwrap_or("-"),
            request.contributor_message,
        );
        self.send_markdown_email(&self.admin_recipient, &subject, &body_markdown)
            .await
    }

    pub async fn send_user_llm_token_issued_notification(
        &self,
        request: &LlmGatewayTokenRequestRecord,
        key: &LlmGatewayKeyRecord,
        gateway_base_url: &str,
        llm_access_url: Option<&str>,
    ) -> Result<()> {
        let subject = "[StaticFlow] 你的 LLM Token 许愿已通过".to_string();
        let body_markdown = format!(
            "你好，\n\n你的 LLM Token 许愿已经审核通过，下面是已经为你创建好的访问凭证。\n\n## \
             申请信息\n- Request ID: `{}`\n- 状态: `{}`\n- 申请额度: `{}`\n- 实际发放 Key ID: \
             `{}`\n- Key 名称: {}\n\n## 使用信息\n- Base URL: `{}`\n- API Key: `{}`\n\n## \
             申请缘由\n\n{}\n\n{}\n\n请妥善保管这个 \
             key；如果后续需要调整额度或重新发放，请直接回复管理员。\n",
            request.request_id,
            request.status,
            request.requested_quota_billable_limit,
            key.id,
            key.name,
            gateway_base_url,
            key.secret,
            request.request_reason,
            llm_access_url
                .map(|url| format!("## 查看页面\n- LLM Access: [{url}]({url})"))
                .unwrap_or_default(),
        );
        self.send_markdown_email(&request.requester_email, &subject, &body_markdown)
            .await
    }

    pub async fn send_user_llm_account_contribution_issued_notification(
        &self,
        request: &LlmGatewayAccountContributionRequestRecord,
        key: &LlmGatewayKeyRecord,
        gateway_base_url: &str,
        llm_access_url: Option<&str>,
    ) -> Result<()> {
        let subject = "[StaticFlow] 你的 Codex 账号贡献已审核通过".to_string();
        let account_name = request
            .imported_account_name
            .as_deref()
            .unwrap_or(request.account_name.as_str());
        let body_markdown = format!(
            "你好，\n\n感谢你贡献 Codex \
             账号给站点共享池。你的申请已经审核通过，\
             系统已经导入账号并为你创建了一把绑定到该账号路由的新 token。\n\n## 贡献信息\n- \
             Request ID: `{}`\n- 状态: `{}`\n- 贡献账号: `{}`\n- Account ID: {}\n- GitHub ID: \
             {}\n- 发放 Key ID: `{}`\n- Key 名称: {}\n\n## 使用信息\n- Base URL: `{}`\n- API Key: \
             `{}`\n- 路由策略: `fixed`\n- 绑定账号: `{}`\n\n## \
             你的留言\n\n{}\n\n{}\n\n再次感谢你的贡献。以后如果这个账号需要下线、改名或重新发放 \
             key，请直接联系管理员。\n",
            request.request_id,
            request.status,
            account_name,
            request.account_id.as_deref().unwrap_or("-"),
            request.github_id.as_deref().unwrap_or("-"),
            key.id,
            key.name,
            gateway_base_url,
            key.secret,
            account_name,
            request.contributor_message,
            llm_access_url
                .map(|url| format!("## 查看页面\n- LLM Access: [{url}]({url})"))
                .unwrap_or_default(),
        );
        self.send_markdown_email(&request.requester_email, &subject, &body_markdown)
            .await
    }

    async fn send_markdown_email(
        &self,
        to: &str,
        subject: &str,
        markdown_body: &str,
    ) -> Result<()> {
        self.send_markdown_email_with_options(to, subject, markdown_body, None, None)
            .await
    }

    async fn send_markdown_email_with_options(
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
            let related = rendered.inline_assets.into_iter().fold(
                MultiPart::related().singlepart(html_part),
                |multipart, asset| {
                    multipart.singlepart(
                        Attachment::new_inline_with_name(asset.content_id, asset.filename)
                            .body(asset.bytes, asset.content_type),
                    )
                },
            );
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

fn render_markdown_email(
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

fn detect_inline_asset_content_type(path: &Path) -> Result<ContentType> {
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
    ContentType::parse(mime).context("failed to parse inline asset content type")
}

fn escape_html(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
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

pub fn build_article_detail_url(frontend_page_url: &str, article_id: &str) -> Result<String> {
    if article_id.trim().is_empty() {
        anyhow::bail!("article_id is required");
    }
    validate_frontend_url(frontend_page_url)?;

    let mut url = Url::parse(frontend_page_url).context("invalid frontend_page_url")?;
    let path = url.path();
    let has_static_flow_prefix = path == "/static_flow" || path.starts_with("/static_flow/");
    let encoded_id: String = url::form_urlencoded::byte_serialize(article_id.as_bytes()).collect();
    let target_path = if has_static_flow_prefix {
        format!("/static_flow/posts/{encoded_id}")
    } else {
        format!("/posts/{encoded_id}")
    };
    url.set_path(&target_path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.into())
}

pub fn build_llm_access_url(frontend_page_url: &str) -> Result<String> {
    validate_frontend_url(frontend_page_url)?;

    let mut url = Url::parse(frontend_page_url).context("invalid frontend_page_url")?;
    let path = url.path();
    let has_static_flow_prefix = path == "/static_flow" || path.starts_with("/static_flow/");
    let target_path =
        if has_static_flow_prefix { "/static_flow/llm-access" } else { "/llm-access" };
    url.set_path(target_path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.into())
}

pub fn build_llm_gateway_base_url(frontend_page_url: &str) -> Result<String> {
    validate_frontend_url(frontend_page_url)?;

    let mut url = Url::parse(frontend_page_url).context("invalid frontend_page_url")?;
    url.set_path("/api/llm-gateway/v1");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.into())
}

fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{truncated}...")
    }
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
    use std::fs;

    use super::{
        build_html_email_document, build_music_player_url, normalize_frontend_page_url_input,
        normalize_requester_email_input, render_markdown_email,
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

    #[test]
    fn render_markdown_to_html_keeps_links_and_images() {
        let markdown =
            "查看 [文档](https://example.com/docs)\n\n![cover](https://example.com/cover.png)";
        let rendered = render_markdown_email(markdown, None).expect("should render markdown");
        assert!(rendered
            .html_fragment
            .contains("href=\"https://example.com/docs\""));
        assert!(rendered
            .html_fragment
            .contains("src=\"https://example.com/cover.png\""));
        assert!(rendered.inline_assets.is_empty());
    }

    #[test]
    fn render_markdown_email_inlines_local_images() {
        let temp_dir =
            std::env::temp_dir().join(format!("static-flow-email-test-{}", std::process::id()));
        fs::create_dir_all(&temp_dir).expect("should create temp dir");
        let image_path = temp_dir.join("qr.png");
        fs::write(&image_path, b"fake-png-bytes").expect("should write fake image");

        let rendered = render_markdown_email("![qr](qr.png)", Some(&temp_dir))
            .expect("should inline local image");
        assert!(rendered.html_fragment.contains("src=\"cid:sf-inline-1\""));
        assert_eq!(rendered.inline_assets.len(), 1);
        assert_eq!(rendered.inline_assets[0].filename, "qr.png");

        fs::remove_file(&image_path).expect("should remove fake image");
        fs::remove_dir_all(&temp_dir).expect("should remove temp dir");
    }

    #[test]
    fn build_html_email_document_wraps_rendered_markdown() {
        let rendered = render_markdown_email("[播放链接](https://example.com/media/audio/1)", None)
            .expect("should render markdown");
        let html = build_html_email_document("测试主题", &rendered.html_fragment);
        assert!(html.contains("<!doctype html>"));
        assert!(html.contains("class=\"sf-content\""));
        assert!(html.contains("href=\"https://example.com/media/audio/1\""));
    }
}
