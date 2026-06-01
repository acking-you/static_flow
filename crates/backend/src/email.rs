use anyhow::{Context, Result};
use static_flow_shared::{
    article_request_store::ArticleRequestRecord,
    llm_gateway_store::Gpt2ApiAccountContributionRequestRecord, music_wish_store::MusicWishRecord,
};
use url::Url;

#[derive(Clone)]
pub struct EmailNotifier {
    inner: static_flow_email::EmailNotifier,
}

impl EmailNotifier {
    pub fn from_env() -> Result<Option<Self>> {
        static_flow_email::EmailNotifier::from_env().map(|notifier| {
            notifier.map(|inner| Self {
                inner,
            })
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
        self.send_markdown_email(self.inner.admin_recipient(), &subject, &body_markdown)
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
        self.send_markdown_email(self.inner.admin_recipient(), &subject, &body_markdown)
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

    pub async fn send_admin_new_gpt2api_account_contribution_request_notification(
        &self,
        request: &Gpt2ApiAccountContributionRequestRecord,
    ) -> Result<()> {
        let subject = format!(
            "[StaticFlow] New GPT Account Contribution {} ({})",
            request.account_name, request.request_id
        );
        let supplied = match (
            request
                .access_token
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty()),
            request
                .session_json
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty()),
        ) {
            (Some(_), Some(_)) => "access token + session JSON",
            (Some(_), None) => "access token",
            (None, Some(_)) => "session JSON",
            (None, None) => "none",
        };
        let body_markdown = format!(
            "## New GPT account contribution submitted\n\n- Request ID: `{}`\n- Display name: \
             `{}`\n- Credential type: {}\n- Requester email: {}\n- GitHub ID: {}\n- Status: \
             `{}`\n- Region: {}\n- Client IP: {}\n- Created at (ms): `{}`\n- Frontend page: \
             {}\n\n### Message\n\n{}\n",
            request.request_id,
            request.account_name,
            supplied,
            request.requester_email,
            request.github_id.as_deref().unwrap_or("-"),
            request.status,
            request.ip_region,
            request.client_ip,
            request.created_at,
            request.frontend_page_url.as_deref().unwrap_or("-"),
            request.contributor_message,
        );
        self.send_markdown_email(self.inner.admin_recipient(), &subject, &body_markdown)
            .await
    }

    pub async fn send_user_gpt2api_account_contribution_issued_notification(
        &self,
        request: &Gpt2ApiAccountContributionRequestRecord,
        key_id: &str,
        key_name: &str,
        api_key: &str,
        login_url: &str,
    ) -> Result<()> {
        let subject = "[StaticFlow] 你的 GPT 生图账号贡献已审核通过".to_string();
        let account_name = request
            .imported_account_name
            .as_deref()
            .unwrap_or(request.account_name.as_str());
        let body_markdown = format!(
            "你好，\n\n感谢你贡献 GPT \
             生图账号给站点共享池。你的申请已经审核通过，\
             系统已经导入账号并为你创建了一把绑定到该账号的新 key；这个 key \
             已经绑定你的邮箱，后续生图完成提醒会发送到这个邮箱。\n\n## 贡献信息\n- Request ID: \
             `{}`\n- 状态: `{}`\n- 贡献账号: `{}`\n- GitHub ID: {}\n- 发放 Key ID: `{}`\n- Key \
             名称: {}\n\n## 使用信息\n- 登录页面: [{}]({})\n- API Key: `{}`\n- 路由策略: \
             `fixed`\n- 绑定账号: `{}`\n- 邮件提醒: `enabled`\n\n## \
             你的留言\n\n{}\n\n再次感谢你的贡献。以后如果这个账号需要下线、改名或重新发放 \
             key，请直接联系管理员。\n",
            request.request_id,
            request.status,
            account_name,
            request.github_id.as_deref().unwrap_or("-"),
            key_id,
            key_name,
            login_url,
            login_url,
            api_key,
            account_name,
            request.contributor_message,
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
        self.inner
            .send_markdown_email(to, subject, markdown_body)
            .await
    }
}

pub fn normalize_requester_email_input(value: Option<String>) -> Result<Option<String>> {
    match normalize_optional_string(value) {
        Some(raw) => {
            if raw.chars().count() > 254 {
                anyhow::bail!("`requester_email` must be <= 254 chars");
            }
            Ok(Some(static_flow_email::normalize_email(raw)?))
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

pub fn build_gpt2api_login_url(frontend_page_url: &str) -> Result<String> {
    validate_frontend_url(frontend_page_url)?;

    let mut url = Url::parse(frontend_page_url).context("invalid frontend_page_url")?;
    url.set_path("/gpt2api/login");
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

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
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
        build_gpt2api_login_url, build_music_player_url, normalize_frontend_page_url_input,
        normalize_requester_email_input,
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
    fn build_gpt2api_login_url_points_to_product_login() {
        let url = build_gpt2api_login_url("https://example.com/gpt2api?x=1#top")
            .expect("should build login URL");
        assert_eq!(url, "https://example.com/gpt2api/login");
    }

    #[test]
    fn normalize_frontend_page_url_rejects_non_http_scheme() {
        let err = normalize_frontend_page_url_input(Some("javascript:alert(1)".to_string()));
        assert!(err.is_err());
    }
}
