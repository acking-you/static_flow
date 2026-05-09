use anyhow::anyhow;
use llm_access_kiro::{
    auth_file::{
        KiroAuthRecord, DEFAULT_KIRO_VERSION, DEFAULT_NODE_VERSION, DEFAULT_SYSTEM_VERSION,
    },
    machine_id,
};

#[derive(Clone, Copy)]
pub(crate) enum KiroAwsService {
    Streaming,
    Runtime,
}

impl KiroAwsService {
    fn api_name(self) -> &'static str {
        match self {
            Self::Streaming => "codewhispererstreaming",
            Self::Runtime => "codewhispererruntime",
        }
    }
}

pub(crate) struct KiroHeaderConfig<'a> {
    pub upstream_host: &'a str,
    pub access_token: &'a str,
    pub service: KiroAwsService,
    pub client_version: &'a str,
    pub sdk_request: &'a str,
    pub content_type: Option<&'a str>,
    pub accept: Option<&'a str>,
    pub connection_close: bool,
    pub agent_mode: Option<&'a str>,
    pub include_opt_out: bool,
}

struct KiroUserAgents {
    x_amz_user_agent: String,
    user_agent: String,
}

pub(crate) fn add_kiro_headers(
    mut request: reqwest::RequestBuilder,
    auth: &KiroAuthRecord,
    config: KiroHeaderConfig<'_>,
) -> anyhow::Result<reqwest::RequestBuilder> {
    let machine_id = machine_id::generate_from_auth(auth)
        .ok_or_else(|| anyhow!("failed to derive kiro machine id"))?;
    let user_agents = kiro_user_agents(config.service, config.client_version, &machine_id);

    if let Some(content_type) = config.content_type {
        request = request.header(reqwest::header::CONTENT_TYPE, content_type);
    }
    if let Some(accept) = config.accept {
        request = request.header(reqwest::header::ACCEPT, accept);
    }
    if config.connection_close {
        request = request.header(reqwest::header::CONNECTION, "close");
    }
    if let Some(agent_mode) = config.agent_mode {
        request = request.header("x-amzn-kiro-agent-mode", agent_mode);
    }
    if config.include_opt_out {
        request = request.header("x-amzn-codewhisperer-optout", "true");
    }
    if auth.auth_method() == "external_idp" {
        request = request.header("TokenType", "EXTERNAL_IDP");
    }
    if auth
        .provider
        .as_deref()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("internal"))
    {
        request = request.header("redirect-for-internal", "true");
    }

    Ok(request
        .header("x-amz-user-agent", user_agents.x_amz_user_agent)
        .header(reqwest::header::USER_AGENT, user_agents.user_agent)
        .header("host", config.upstream_host)
        .header("amz-sdk-invocation-id", uuid::Uuid::new_v4().to_string())
        .header("amz-sdk-request", config.sdk_request)
        .header("authorization", format!("Bearer {}", config.access_token)))
}

fn kiro_user_agents(
    service: KiroAwsService,
    client_version: &str,
    machine_id: &str,
) -> KiroUserAgents {
    KiroUserAgents {
        x_amz_user_agent: format!(
            "aws-sdk-js/{client_version} KiroIDE-{DEFAULT_KIRO_VERSION}-{machine_id}"
        ),
        user_agent: format!(
            "aws-sdk-js/{client_version} ua/2.1 os/{DEFAULT_SYSTEM_VERSION} lang/js \
             md/nodejs#{DEFAULT_NODE_VERSION} api/{}#{client_version} \
             KiroIDE-{DEFAULT_KIRO_VERSION}-{machine_id}",
            service.api_name(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use llm_access_kiro::auth_file::KiroAuthRecord;

    #[test]
    fn external_idp_internal_headers_are_detected_from_auth_record() {
        let mut auth = KiroAuthRecord {
            auth_method: Some("external_idp".to_string()),
            provider: Some("Internal".to_string()),
            machine_id: Some("a".repeat(64)),
            ..Default::default()
        };
        auth = auth.canonicalize();

        let request = super::add_kiro_headers(
            reqwest::Client::new().get("http://127.0.0.1:19090/test"),
            &auth,
            super::KiroHeaderConfig {
                upstream_host: "127.0.0.1:19090",
                access_token: "token",
                service: super::KiroAwsService::Runtime,
                client_version: "1.0.0",
                sdk_request: "attempt=1; max=1",
                content_type: None,
                accept: None,
                connection_close: false,
                agent_mode: None,
                include_opt_out: false,
            },
        )
        .expect("headers");
        let built = request.build().expect("request");

        assert_eq!(
            built
                .headers()
                .get("TokenType")
                .and_then(|v| v.to_str().ok()),
            Some("EXTERNAL_IDP")
        );
        assert_eq!(
            built
                .headers()
                .get("redirect-for-internal")
                .and_then(|v| v.to_str().ok()),
            Some("true")
        );
    }
}
