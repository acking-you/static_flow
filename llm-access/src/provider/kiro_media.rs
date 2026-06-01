//! Kiro remote-media (image/document) resolution, validation, and fetch.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use async_trait::async_trait;
use base64::Engine as _;
use llm_access_kiro::anthropic::types::MessagesRequest;

use super::{
    KiroRemoteMediaFetcher, KiroRemoteMediaKind, KiroRemoteMediaRequest,
    KiroRemoteMediaResolutionError, PendingKiroRemoteMediaSource, ReqwestKiroRemoteMediaFetcher,
    ResolvedKiroRemoteMedia, StrippedKiroRemoteMediaSource, KIRO_REMOTE_DOCUMENT_MAX_BYTES,
    KIRO_REMOTE_IMAGE_MAX_BYTES, KIRO_REMOTE_MEDIA_CLIENT,
};

impl KiroRemoteMediaResolutionError {
    pub(super) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    fn with_context(self, context: impl AsRef<str>) -> Self {
        Self {
            message: format!("{}: {}", context.as_ref(), self.message),
        }
    }
}
impl std::fmt::Display for KiroRemoteMediaResolutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}
#[async_trait]
impl KiroRemoteMediaFetcher for ReqwestKiroRemoteMediaFetcher {
    async fn fetch(
        &self,
        request: KiroRemoteMediaRequest<'_>,
    ) -> Result<ResolvedKiroRemoteMedia, KiroRemoteMediaResolutionError> {
        let url = validate_kiro_remote_media_url(request.url)?;
        validate_kiro_remote_media_resolved_addresses(&url).await?;
        let max_bytes = match request.kind {
            KiroRemoteMediaKind::Image => KIRO_REMOTE_IMAGE_MAX_BYTES,
            KiroRemoteMediaKind::Document => KIRO_REMOTE_DOCUMENT_MAX_BYTES,
        };
        let response = self
            .client
            .get(url.clone())
            .header(reqwest::header::ACCEPT, kiro_remote_media_accept_header(request.kind))
            .send()
            .await
            .map_err(|err| {
                KiroRemoteMediaResolutionError::new(format!("failed to fetch URL source: {err}"))
            })?;
        if !response.status().is_success() {
            return Err(KiroRemoteMediaResolutionError::new(format!(
                "URL source returned HTTP {}",
                response.status()
            )));
        }
        if response
            .content_length()
            .is_some_and(|length| length > max_bytes as u64)
        {
            return Err(KiroRemoteMediaResolutionError::new(format!(
                "URL source exceeds {} byte limit",
                max_bytes
            )));
        }
        let media_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(normalize_media_type);
        let bytes = response.bytes().await.map_err(|err| {
            KiroRemoteMediaResolutionError::new(format!("failed to read URL source body: {err}"))
        })?;
        if bytes.len() > max_bytes {
            return Err(KiroRemoteMediaResolutionError::new(format!(
                "URL source exceeds {} byte limit",
                max_bytes
            )));
        }
        if bytes.is_empty() {
            return Err(KiroRemoteMediaResolutionError::new("URL source body is empty"));
        }
        Ok(ResolvedKiroRemoteMedia {
            media_type,
            bytes,
        })
    }
}
fn kiro_remote_media_accept_header(kind: KiroRemoteMediaKind) -> &'static str {
    match kind {
        KiroRemoteMediaKind::Image => "image/jpeg,image/png,image/gif,image/webp",
        KiroRemoteMediaKind::Document => {
            "application/pdf,text/csv,application/msword,application/vnd.\
             openxmlformats-officedocument.wordprocessingml.document,application/vnd.ms-excel,\
             application/vnd.openxmlformats-officedocument.spreadsheetml.sheet,text/html,text/\
             plain,text/markdown"
        },
    }
}
pub async fn resolve_kiro_remote_media_sources(
    payload: &mut MessagesRequest,
) -> Result<(), KiroRemoteMediaResolutionError> {
    if !payload_has_kiro_remote_media_sources(payload) {
        return Ok(());
    }
    let fetcher = ReqwestKiroRemoteMediaFetcher {
        client: KIRO_REMOTE_MEDIA_CLIENT.clone(),
    };
    resolve_kiro_remote_media_sources_with_fetcher(payload, &fetcher).await
}
fn payload_has_kiro_remote_media_sources(payload: &MessagesRequest) -> bool {
    payload.messages.iter().any(|message| {
        message.role == "user"
            && message
                .content
                .as_array()
                .is_some_and(|items| items.iter().any(is_kiro_remote_media_source_block))
    })
}
pub fn strip_kiro_remote_media_sources(
    payload: &mut MessagesRequest,
) -> Vec<StrippedKiroRemoteMediaSource> {
    let mut removed = Vec::new();
    for (message_index, message) in payload.messages.iter_mut().enumerate() {
        if message.role != "user" {
            continue;
        }
        let Some(items) = message.content.as_array_mut() else {
            continue;
        };
        let mut retained = Vec::with_capacity(items.len());
        for (block_index, item) in std::mem::take(items).into_iter().enumerate() {
            if let Some(stripped) =
                stripped_kiro_remote_media_source(&item, message_index, block_index)
            {
                removed.push(stripped);
            } else {
                retained.push(item);
            }
        }
        *items = retained;
    }
    removed
}
fn stripped_kiro_remote_media_source(
    item: &serde_json::Value,
    message_index: usize,
    block_index: usize,
) -> Option<StrippedKiroRemoteMediaSource> {
    let object = item.as_object()?;
    let block_type = object
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)?;
    if !matches!(block_type, "image" | "document") {
        return None;
    }
    let source = object
        .get("source")
        .and_then(serde_json::Value::as_object)?;
    if source
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        != Some("url")
    {
        return None;
    }
    let url_summary = source
        .get("url")
        .and_then(serde_json::Value::as_str)
        .map(summarize_kiro_remote_media_url)
        .unwrap_or_else(|| "(missing url)".to_string());
    Some(StrippedKiroRemoteMediaSource {
        message_index,
        block_index,
        block_type: block_type.to_string(),
        url_summary,
    })
}
fn summarize_kiro_remote_media_url(raw_url: &str) -> String {
    let trimmed = raw_url.trim();
    if trimmed.is_empty() {
        return "(empty url)".to_string();
    }
    if let Ok(mut parsed) = url::Url::parse(trimmed) {
        parsed.set_query(None);
        parsed.set_fragment(None);
        return parsed.to_string();
    }
    trimmed.chars().take(160).collect()
}
fn is_kiro_remote_media_source_block(item: &serde_json::Value) -> bool {
    let Some(object) = item.as_object() else {
        return false;
    };
    let Some("image" | "document") = object.get("type").and_then(serde_json::Value::as_str) else {
        return false;
    };
    object
        .get("source")
        .and_then(serde_json::Value::as_object)
        .and_then(|source| source.get("type"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        == Some("url")
}
pub async fn resolve_kiro_remote_media_sources_with_fetcher(
    payload: &mut MessagesRequest,
    fetcher: &(dyn KiroRemoteMediaFetcher + Sync),
) -> Result<(), KiroRemoteMediaResolutionError> {
    for (message_index, message) in payload.messages.iter_mut().enumerate() {
        if message.role != "user" {
            continue;
        }
        let Some(items) = message.content.as_array_mut() else {
            continue;
        };
        for (block_index, item) in items.iter_mut().enumerate() {
            let Some(source) = pending_kiro_remote_media_source(item, message_index, block_index)?
            else {
                continue;
            };
            let remote = fetcher
                .fetch(KiroRemoteMediaRequest {
                    url: &source.url,
                    kind: source.kind,
                })
                .await
                .map_err(|err| {
                    err.with_context(format!(
                        "message {message_index} {} block {block_index}",
                        source.block_type
                    ))
                })?;
            let replacement = match source.kind {
                KiroRemoteMediaKind::Image => build_kiro_remote_image_source(
                    source.source_media_type.as_deref(),
                    remote.media_type.as_deref(),
                    &source.url,
                    &remote.bytes,
                )?,
                KiroRemoteMediaKind::Document => build_kiro_remote_document_source(
                    source.source_media_type.as_deref(),
                    remote.media_type.as_deref(),
                    &source.url,
                    &remote.bytes,
                )?,
            };
            if let Some(object) = item.as_object_mut() {
                object.insert("source".to_string(), replacement);
            }
        }
    }
    Ok(())
}
fn pending_kiro_remote_media_source(
    item: &serde_json::Value,
    message_index: usize,
    block_index: usize,
) -> Result<Option<PendingKiroRemoteMediaSource>, KiroRemoteMediaResolutionError> {
    let Some(object) = item.as_object() else {
        return Ok(None);
    };
    let Some(block_type) = object.get("type").and_then(serde_json::Value::as_str) else {
        return Ok(None);
    };
    let (kind, block_type) = match block_type {
        "image" => (KiroRemoteMediaKind::Image, "image"),
        "document" => (KiroRemoteMediaKind::Document, "document"),
        _ => return Ok(None),
    };
    let Some(source) = object.get("source").and_then(serde_json::Value::as_object) else {
        return Ok(None);
    };
    let Some(source_type) = source
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
    else {
        return Ok(None);
    };
    if source_type != "url" {
        return Ok(None);
    }
    let Some(url) = source
        .get("url")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Err(KiroRemoteMediaResolutionError::new(format!(
            "message {message_index} {block_type} block {block_index} URL source is missing url"
        )));
    };
    Ok(Some(PendingKiroRemoteMediaSource {
        kind,
        block_type,
        url: url.to_string(),
        source_media_type: source
            .get("media_type")
            .and_then(serde_json::Value::as_str)
            .and_then(normalize_media_type),
    }))
}
fn build_kiro_remote_image_source(
    source_media_type: Option<&str>,
    response_media_type: Option<&str>,
    url: &str,
    bytes: &[u8],
) -> Result<serde_json::Value, KiroRemoteMediaResolutionError> {
    if bytes.is_empty() {
        return Err(KiroRemoteMediaResolutionError::new("URL source body is empty"));
    }
    let media_type = response_media_type
        .and_then(canonical_image_media_type)
        .or_else(|| source_media_type.and_then(canonical_image_media_type))
        .or_else(|| image_media_type_from_url(url))
        .ok_or_else(|| {
            KiroRemoteMediaResolutionError::new(
                "URL image source must resolve to image/jpeg, image/png, image/gif, or image/webp",
            )
        })?;
    Ok(serde_json::json!({
        "type": "base64",
        "media_type": media_type,
        "data": base64::engine::general_purpose::STANDARD.encode(bytes)
    }))
}
fn build_kiro_remote_document_source(
    source_media_type: Option<&str>,
    response_media_type: Option<&str>,
    url: &str,
    bytes: &[u8],
) -> Result<serde_json::Value, KiroRemoteMediaResolutionError> {
    if bytes.is_empty() {
        return Err(KiroRemoteMediaResolutionError::new("URL source body is empty"));
    }
    let media_type = response_media_type
        .and_then(canonical_document_media_type)
        .or_else(|| source_media_type.and_then(canonical_document_media_type))
        .or_else(|| document_media_type_from_url(url))
        .ok_or_else(|| {
            KiroRemoteMediaResolutionError::new(
                "URL document source must resolve to a supported document type",
            )
        })?;
    match media_type {
        "application/pdf"
        | "application/msword"
        | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        | "application/vnd.ms-excel"
        | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
            Ok(serde_json::json!({
                "type": "base64",
                "media_type": media_type,
                "data": base64::engine::general_purpose::STANDARD.encode(bytes)
            }))
        },
        "text/plain" | "text/markdown" | "text/html" | "text/csv" => {
            let text = std::str::from_utf8(bytes).map_err(|err| {
                KiroRemoteMediaResolutionError::new(format!(
                    "URL text document source is not valid UTF-8: {err}"
                ))
            })?;
            Ok(serde_json::json!({
                "type": "text",
                "media_type": media_type,
                "data": text
            }))
        },
        _ => unreachable!("document media type is normalized to the supported set"),
    }
}
fn normalize_media_type(value: &str) -> Option<String> {
    value
        .split(';')
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}
fn canonical_image_media_type(media_type: &str) -> Option<&'static str> {
    match media_type {
        "image/jpeg" | "image/jpg" => Some("image/jpeg"),
        "image/png" => Some("image/png"),
        "image/gif" => Some("image/gif"),
        "image/webp" => Some("image/webp"),
        _ => None,
    }
}
fn canonical_document_media_type(media_type: &str) -> Option<&'static str> {
    match media_type {
        "application/pdf" => Some("application/pdf"),
        "text/csv" => Some("text/csv"),
        "application/msword" => Some("application/msword"),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document")
        },
        "application/vnd.ms-excel" => Some("application/vnd.ms-excel"),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
            Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")
        },
        "text/html" => Some("text/html"),
        "text/plain" => Some("text/plain"),
        "text/markdown" | "text/md" | "text/x-markdown" => Some("text/markdown"),
        _ => None,
    }
}
fn image_media_type_from_url(url: &str) -> Option<&'static str> {
    match lower_url_path_extension(url).as_deref() {
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("png") => Some("image/png"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        _ => None,
    }
}
fn document_media_type_from_url(url: &str) -> Option<&'static str> {
    match lower_url_path_extension(url).as_deref() {
        Some("pdf") => Some("application/pdf"),
        Some("csv") => Some("text/csv"),
        Some("doc") => Some("application/msword"),
        Some("docx") => {
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document")
        },
        Some("xls") => Some("application/vnd.ms-excel"),
        Some("xlsx") => Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
        Some("html" | "htm") => Some("text/html"),
        Some("txt") => Some("text/plain"),
        Some("md" | "markdown") => Some("text/markdown"),
        _ => None,
    }
}
fn lower_url_path_extension(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    parsed
        .path_segments()
        .and_then(Iterator::last)
        .and_then(|name| {
            name.rsplit_once('.')
                .map(|(_, ext)| ext.to_ascii_lowercase())
        })
}
fn validate_kiro_remote_media_url(
    raw_url: &str,
) -> Result<url::Url, KiroRemoteMediaResolutionError> {
    let url = url::Url::parse(raw_url)
        .map_err(|err| KiroRemoteMediaResolutionError::new(format!("invalid URL source: {err}")))?;
    match url.scheme() {
        "http" | "https" => {},
        _ => {
            return Err(KiroRemoteMediaResolutionError::new(
                "URL source scheme must be http or https",
            ))
        },
    }
    // Match the parsed host enum, NOT `host_str()`: the latter returns IPv6
    // literals bracketed (`[::1]`), which `parse::<IpAddr>()` rejects, letting a
    // literal-IP URL skip the private-address gate entirely (reqwest dials the
    // literal without ever calling the filtering resolver).
    match url
        .host()
        .ok_or_else(|| KiroRemoteMediaResolutionError::new("URL source is missing host"))?
    {
        url::Host::Domain(domain) => {
            if domain.eq_ignore_ascii_case("localhost") || domain.ends_with(".localhost") {
                return Err(KiroRemoteMediaResolutionError::new(
                    "URL source host must not be localhost",
                ));
            }
        },
        url::Host::Ipv4(ip) => reject_private_kiro_remote_media_ip(IpAddr::V4(ip))?,
        url::Host::Ipv6(ip) => reject_private_kiro_remote_media_ip(IpAddr::V6(ip))?,
    }
    Ok(url)
}
async fn validate_kiro_remote_media_resolved_addresses(
    url: &url::Url,
) -> Result<(), KiroRemoteMediaResolutionError> {
    // Literal IPs are already vetted by `validate_kiro_remote_media_url` and are
    // dialed directly (no resolver); only domains need a pre-flight lookup. Use
    // the host enum so bracketed IPv6 literals aren't misread as domains.
    let domain = match url
        .host()
        .ok_or_else(|| KiroRemoteMediaResolutionError::new("URL source is missing host"))?
    {
        url::Host::Domain(domain) => domain.to_string(),
        url::Host::Ipv4(_) | url::Host::Ipv6(_) => return Ok(()),
    };
    let port = url
        .port_or_known_default()
        .ok_or_else(|| KiroRemoteMediaResolutionError::new("URL source is missing port"))?;
    let addresses = tokio::net::lookup_host((domain.as_str(), port))
        .await
        .map_err(|err| {
            KiroRemoteMediaResolutionError::new(format!("failed to resolve URL source host: {err}"))
        })?;
    let mut resolved_any = false;
    for address in addresses {
        resolved_any = true;
        reject_private_kiro_remote_media_ip(address.ip())?;
    }
    if !resolved_any {
        return Err(KiroRemoteMediaResolutionError::new(
            "URL source host resolved to no addresses",
        ));
    }
    Ok(())
}
fn reject_private_kiro_remote_media_ip(ip: IpAddr) -> Result<(), KiroRemoteMediaResolutionError> {
    if is_private_kiro_remote_media_ip(ip) {
        Err(KiroRemoteMediaResolutionError::new(
            "URL source host resolves to a private or local address",
        ))
    } else {
        Ok(())
    }
}
fn is_private_kiro_remote_media_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_private_kiro_remote_media_ipv4(ip),
        IpAddr::V6(ip) => is_private_kiro_remote_media_ipv6(ip),
    }
}
fn is_private_kiro_remote_media_ipv4(ip: Ipv4Addr) -> bool {
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip == Ipv4Addr::UNSPECIFIED
}
fn is_private_kiro_remote_media_ipv6(ip: Ipv6Addr) -> bool {
    // IPv4-mapped (`::ffff:a.b.c.d`) and IPv4-compatible (`::a.b.c.d`) addresses
    // are dialed as their embedded IPv4 by dual-stack sockets, so an attacker
    // could smuggle a private IPv4 target (e.g. `::ffff:127.0.0.1`) past the
    // native-IPv6 checks below. Re-check the embedded IPv4 against the v4 rules.
    if let Some(mapped) = ip.to_ipv4() {
        if is_private_kiro_remote_media_ipv4(mapped) {
            return true;
        }
    }
    ip.is_loopback()
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
        || ip.is_unspecified()
        || matches!(ip.segments(), [0x2001, 0x0db8, _, _, _, _, _, _])
}

/// Drops private/local addresses from a resolved set, rejecting the lookup if
/// none remain. Used by [`PrivateFilteringDnsResolver`] so the address that is
/// vetted is exactly the address reqwest dials.
fn filter_public_kiro_remote_media_addrs(
    host: &str,
    addrs: Vec<SocketAddr>,
) -> Result<Vec<SocketAddr>, String> {
    let public = addrs
        .into_iter()
        .filter(|addr| !is_private_kiro_remote_media_ip(addr.ip()))
        .collect::<Vec<_>>();
    if public.is_empty() {
        return Err(format!(
            "URL source host `{host}` resolved only to private or local addresses"
        ));
    }
    Ok(public)
}

/// Custom reqwest DNS resolver for the Kiro remote-media client that filters
/// out private/local addresses at resolution time.
///
/// This is the actual SSRF guard against DNS rebinding: reqwest dials the
/// addresses this resolver returns, so the IP that is checked is the IP that is
/// connected to — closing the time-of-check/time-of-use gap that a separate
/// pre-flight `lookup_host` check cannot (reqwest performs its own resolution
/// at connect time, which a hostile resolver can answer differently).
/// IP-literal URLs (never sent to a resolver) remain covered by the literal-IP
/// gate in [`validate_kiro_remote_media_url`].
pub(super) struct PrivateFilteringDnsResolver;

impl reqwest::dns::Resolve for PrivateFilteringDnsResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let resolved = tokio::net::lookup_host((host.as_str(), 0u16))
                .await
                .map_err(|err| Box::new(err) as Box<dyn std::error::Error + Send + Sync>)?
                .collect::<Vec<SocketAddr>>();
            let public = filter_public_kiro_remote_media_addrs(&host, resolved)
                .map_err(Box::<dyn std::error::Error + Send + Sync>::from)?;
            Ok(Box::new(public.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sa(addr: &str) -> SocketAddr {
        format!("{addr}:443").parse().expect("valid socket addr")
    }

    #[test]
    fn filter_public_addrs_keeps_public_drops_private() {
        let out = filter_public_kiro_remote_media_addrs("example.test", vec![
            sa("8.8.8.8"),
            sa("10.0.0.5"),
            sa("1.1.1.1"),
            sa("127.0.0.1"),
        ])
        .expect("public addresses remain");
        assert_eq!(out, vec![sa("8.8.8.8"), sa("1.1.1.1")]);
    }

    #[test]
    fn filter_public_addrs_rejects_all_private() {
        let err = filter_public_kiro_remote_media_addrs("rebind.test", vec![
            sa("127.0.0.1"),
            sa("169.254.169.254"),
            sa("10.1.2.3"),
        ])
        .expect_err("an all-private resolution must be rejected");
        assert!(err.contains("private or local"), "unexpected message: {err}");
    }

    #[test]
    fn filter_public_addrs_rejects_empty() {
        let err = filter_public_kiro_remote_media_addrs("empty.test", Vec::new())
            .expect_err("an empty resolution must be rejected");
        assert!(err.contains("private or local"), "unexpected message: {err}");
    }

    #[tokio::test]
    async fn resolver_rejects_loopback_hostname() {
        use std::str::FromStr;

        use reqwest::dns::Resolve;
        // `localhost` resolves to loopback (RFC 6761), so the resolver must drop
        // every address and error — proving rebinding to loopback is blocked.
        let name = reqwest::dns::Name::from_str("localhost").expect("valid dns name");
        let result = PrivateFilteringDnsResolver.resolve(name).await;
        assert!(result.is_err(), "localhost must be rejected by the filtering resolver");
    }

    #[test]
    fn filter_public_addrs_rejects_ipv4_mapped_private() {
        // Dual-stack sockets dial `::ffff:a.b.c.d` as the embedded IPv4, so a
        // hostile resolver could smuggle a private IPv4 target as IPv4-mapped
        // IPv6. The embedded address must be re-checked against the v4 rules.
        let err = filter_public_kiro_remote_media_addrs("rebind.test", vec![
            sa("[::ffff:127.0.0.1]"),
            sa("[::ffff:10.1.2.3]"),
            sa("[::ffff:169.254.169.254]"),
        ])
        .expect_err("IPv4-mapped private addresses must be rejected");
        assert!(err.contains("private or local"), "unexpected message: {err}");
    }

    #[test]
    fn filter_public_addrs_keeps_ipv4_mapped_public() {
        let out = filter_public_kiro_remote_media_addrs("example.test", vec![
            sa("[::ffff:8.8.8.8]"),
            sa("[::ffff:10.0.0.1]"),
        ])
        .expect("the mapped-public address remains");
        assert_eq!(out, vec![sa("[::ffff:8.8.8.8]")]);
    }

    #[test]
    fn validate_url_rejects_literal_loopback_variants() {
        // host_str() brackets IPv6 literals, so a `parse::<IpAddr>()` gate would
        // skip them; matching url.host() must reject these (incl. IPv4-mapped and
        // url-normalized decimal IPv4).
        for raw in [
            "http://[::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://[::ffff:10.0.0.1]/",
            "http://127.0.0.1/",
            "http://2130706433/",
        ] {
            let err = validate_kiro_remote_media_url(raw)
                .err()
                .unwrap_or_else(|| panic!("{raw} must be rejected"));
            assert!(err.to_string().contains("private or local"), "raw={raw} msg={err}");
        }
    }

    #[test]
    fn validate_url_allows_public_literal_ipv6() {
        validate_kiro_remote_media_url("http://[2606:4700:4700::1111]/")
            .expect("public literal IPv6 must be allowed");
    }
}
