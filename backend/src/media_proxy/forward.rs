use anyhow::{Context, Result};
use axum::{
    body::Body,
    http::{header, HeaderMap, StatusCode},
    response::Response,
};
use bytes::Bytes;
use static_flow_media_types::{PosterQuery, RawPlaybackQuery, UploadChunkQuery};

fn join_internal_url(base_url: &reqwest::Url, relative: &str) -> Result<reqwest::Url> {
    base_url
        .join(relative)
        .with_context(|| format!("failed to join upstream path {relative}"))
}

fn with_range_header(
    mut request: reqwest::RequestBuilder,
    headers: &HeaderMap,
) -> reqwest::RequestBuilder {
    if let Some(value) = headers.get(header::RANGE).cloned() {
        request = request.header(header::RANGE, value);
    }
    request
}

pub async fn forward(upstream: reqwest::RequestBuilder) -> Result<Response> {
    let upstream = upstream
        .send()
        .await
        .context("failed to send upstream request")?;
    let status = StatusCode::from_u16(upstream.status().as_u16())
        .context("upstream returned invalid HTTP status code")?;
    let mut builder = Response::builder().status(status);
    for name in [
        header::CONTENT_TYPE,
        header::CONTENT_LENGTH,
        header::CONTENT_RANGE,
        header::ACCEPT_RANGES,
        header::CACHE_CONTROL,
    ] {
        if let Some(value) = upstream.headers().get(&name) {
            builder = builder.header(name, value.clone());
        }
    }
    builder
        .body(Body::from_stream(upstream.bytes_stream()))
        .context("failed to build proxied streaming response")
}

pub async fn forward_raw_request(
    client: &reqwest::Client,
    base_url: &reqwest::Url,
    file: &str,
    headers: &HeaderMap,
) -> Result<Response> {
    let upstream = client
        .get(join_internal_url(base_url, "internal/local-media/playback/raw")?)
        .query(&RawPlaybackQuery {
            file: file.to_string(),
        });
    forward(with_range_header(upstream, headers)).await
}

pub async fn forward_hls_request(
    client: &reqwest::Client,
    base_url: &reqwest::Url,
    job_id: &str,
    file_name: &str,
    headers: &HeaderMap,
) -> Result<Response> {
    let relative = format!("internal/local-media/playback/hls/{job_id}/{file_name}");
    let upstream = client.get(join_internal_url(base_url, &relative)?);
    forward(with_range_header(upstream, headers)).await
}

pub async fn forward_mp4_request(
    client: &reqwest::Client,
    base_url: &reqwest::Url,
    job_id: &str,
    file_name: &str,
    headers: &HeaderMap,
) -> Result<Response> {
    let relative = format!("internal/local-media/playback/mp4/{job_id}/{file_name}");
    let upstream = client.get(join_internal_url(base_url, &relative)?);
    forward(with_range_header(upstream, headers)).await
}

pub async fn forward_poster_request(
    client: &reqwest::Client,
    base_url: &reqwest::Url,
    file: &str,
) -> Result<Response> {
    let upstream = client
        .get(join_internal_url(base_url, "internal/local-media/poster")?)
        .query(&PosterQuery {
            file: file.to_string(),
        });
    forward(upstream).await
}

pub async fn forward_upload_chunk_request(
    client: &reqwest::Client,
    base_url: &reqwest::Url,
    task_id: &str,
    offset: u64,
    content_type: reqwest::header::HeaderValue,
    body: Bytes,
) -> Result<Response> {
    let relative = format!("internal/local-media/uploads/tasks/{task_id}/chunks");
    let upstream = client
        .put(join_internal_url(base_url, &relative)?)
        .query(&UploadChunkQuery {
            offset,
        })
        .header(reqwest::header::CONTENT_TYPE, content_type)
        .body(body);
    forward(upstream).await
}

#[cfg(test)]
mod tests {
    use axum::{
        body::to_bytes,
        http::{header, HeaderMap},
    };
    use bytes::Bytes;

    use super::{forward_raw_request, forward_upload_chunk_request};

    #[tokio::test]
    async fn forward_raw_request_preserves_range_header() {
        let upstream = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/internal/local-media/playback/raw"))
            .and(wiremock::matchers::header("range", "bytes=0-15"))
            .respond_with(
                wiremock::ResponseTemplate::new(206)
                    .insert_header("content-range", "bytes 0-15/100")
                    .set_body_bytes(b"0123456789abcdef"),
            )
            .mount(&upstream)
            .await;

        let mut headers = HeaderMap::new();
        headers.insert(header::RANGE, header::HeaderValue::from_static("bytes=0-15"));

        let response = forward_raw_request(
            &reqwest::Client::new(),
            &reqwest::Url::parse(&upstream.uri()).expect("base url"),
            "video.mkv",
            &headers,
        )
        .await
        .expect("forward response");

        assert_eq!(response.status(), axum::http::StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response.headers().get(header::CONTENT_RANGE),
            Some(&header::HeaderValue::from_static("bytes 0-15/100"))
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        assert_eq!(&body[..], b"0123456789abcdef");
    }

    #[tokio::test]
    async fn forward_upload_chunk_preserves_body_and_content_type() {
        let upstream = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path(
                "/internal/local-media/uploads/tasks/task-1/chunks",
            ))
            .and(wiremock::matchers::query_param("offset", "4"))
            .and(wiremock::matchers::header(
                "content-type",
                "application/octet-stream",
            ))
            .and(wiremock::matchers::body_bytes(b"chunk".to_vec()))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_raw(
                r#"{"task":{"task_id":"task-1","resume_key":"k","status":"partial","target_dir":"","source_file_name":"clip.mp4","target_file_name":"clip.mp4","target_relative_path":"clip.mp4","file_size":10,"uploaded_bytes":9,"last_modified_ms":1,"mime_type":null,"error":null,"created_at_ms":1,"updated_at_ms":1}}"#,
                "application/json",
            ))
            .mount(&upstream)
            .await;

        let response = forward_upload_chunk_request(
            &reqwest::Client::new(),
            &reqwest::Url::parse(&upstream.uri()).expect("base url"),
            "task-1",
            4,
            reqwest::header::HeaderValue::from_static("application/octet-stream"),
            Bytes::from_static(b"chunk"),
        )
        .await
        .expect("forward response");

        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }
}
