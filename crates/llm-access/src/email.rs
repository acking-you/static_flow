//! LLM-access email notification workflows.

use std::path::Path;

use anyhow::Result;
use llm_access_core::store::{AdminAccountContributionRequest, AdminTokenRequest, NewAdminKey};
use url::Url;

#[derive(Clone)]
pub(crate) struct EmailNotifier {
    inner: static_flow_email::EmailNotifier,
}

impl EmailNotifier {
    pub(crate) fn from_env() -> Result<Option<Self>> {
        static_flow_email::EmailNotifier::from_env().map(|notifier| {
            notifier.map(|inner| Self {
                inner,
            })
        })
    }

    pub(crate) async fn send_user_llm_token_issued_notification(
        &self,
        request: &AdminTokenRequest,
        key: &NewAdminKey,
    ) -> Result<()> {
        let gateway_base_url = llm_gateway_base_url(request.frontend_page_url.as_deref());
        let llm_access_url = llm_access_url(request.frontend_page_url.as_deref());
        let subject = "[StaticFlow] 你的 LLM Token 许愿已通过";
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
        self.inner
            .send_markdown_email(&request.requester_email, subject, &body_markdown)
            .await
    }

    pub(crate) async fn send_user_llm_account_contribution_issued_notification(
        &self,
        request: &AdminAccountContributionRequest,
        key: &NewAdminKey,
    ) -> Result<()> {
        let gateway_base_url = llm_gateway_base_url(request.frontend_page_url.as_deref());
        let llm_access_url = llm_access_url(request.frontend_page_url.as_deref());
        let subject = "[StaticFlow] 你的 Codex 账号贡献已审核通过";
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
        self.inner
            .send_markdown_email(&request.requester_email, subject, &body_markdown)
            .await
    }

    pub(crate) async fn send_llm_sponsor_payment_instructions(
        &self,
        requester_email: &str,
        subject: &str,
        markdown_body: &str,
        asset_base_dir: &Path,
        reply_to: Option<&str>,
    ) -> Result<()> {
        self.inner
            .send_markdown_email_with_options(
                requester_email,
                subject,
                markdown_body,
                Some(asset_base_dir),
                reply_to,
            )
            .await
    }
}

fn llm_gateway_base_url(frontend_page_url: Option<&str>) -> String {
    frontend_page_url
        .and_then(|url| build_llm_gateway_base_url(url).ok())
        .or_else(|| {
            std::env::var("SITE_BASE_URL")
                .ok()
                .map(|base| format!("{}/api/llm-gateway/v1", base.trim_end_matches('/')))
        })
        .unwrap_or_else(|| "/api/llm-gateway/v1".to_string())
}

fn llm_access_url(frontend_page_url: Option<&str>) -> Option<String> {
    frontend_page_url.and_then(|url| build_llm_access_url(url).ok())
}

fn build_llm_gateway_base_url(frontend_page_url: &str) -> Result<String> {
    validate_frontend_url(frontend_page_url)?;

    let mut url = Url::parse(frontend_page_url)?;
    url.set_path("/api/llm-gateway/v1");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.into())
}

fn build_llm_access_url(frontend_page_url: &str) -> Result<String> {
    validate_frontend_url(frontend_page_url)?;

    let mut url = Url::parse(frontend_page_url)?;
    let path = url.path();
    let has_static_flow_prefix = path == "/static_flow" || path.starts_with("/static_flow/");
    let target_path =
        if has_static_flow_prefix { "/static_flow/llm-access" } else { "/llm-access" };
    url.set_path(target_path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.into())
}

fn validate_frontend_url(raw: &str) -> Result<()> {
    let parsed = Url::parse(raw)?;
    match parsed.scheme() {
        "http" | "https" => {},
        _ => anyhow::bail!("frontend page URL must use http or https"),
    }
    if parsed.host_str().is_none() {
        anyhow::bail!("frontend page URL must include a host");
    }
    Ok(())
}
