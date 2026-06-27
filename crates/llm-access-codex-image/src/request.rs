use axum::http::StatusCode;
use serde_json::{Map, Value};

const DEFAULT_MODEL: &str = "gpt-image-2";
const ALLOWED_FIELDS_GENERATIONS: &[&str] =
    &["model", "prompt", "n", "size", "quality", "background"];
const ALLOWED_FIELDS_EDITS: &[&str] =
    &["model", "prompt", "n", "size", "quality", "background", "images"];
const ALLOWED_SIZES: &[&str] =
    &["auto", "1024x1024", "1536x1024", "1024x1536", "2048x1152", "1152x2048"];
const ALLOWED_QUALITIES: &[&str] = &["auto", "low", "medium", "high"];
const ALLOWED_BACKGROUNDS: &[&str] = &["auto", "transparent", "opaque"];
const ALLOWED_DATA_IMAGE_PREFIXES: &[&str] = &[
    "data:image/png;base64,",
    "data:image/jpeg;base64,",
    "data:image/jpg;base64,",
    "data:image/webp;base64,",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Supported image API endpoints after public path normalization.
pub enum CodexImageEndpoint {
    /// Image generation endpoint.
    Generations,
    /// Image edit endpoint.
    Edits,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Validated image request accepted by the standalone gateway.
pub struct CodexImageRequest {
    /// OpenAI image model name. Currently always `gpt-image-2`.
    pub model: String,
    /// User prompt passed to the upstream image API.
    pub prompt: String,
    /// Requested number of images.
    pub n: u64,
    /// Optional image size parameter.
    pub size: Option<String>,
    /// Optional quality parameter.
    pub quality: Option<String>,
    /// Optional background parameter.
    pub background: Option<String>,
    /// Edit input image sources.
    pub images: Vec<String>,
    /// Sanitized original JSON object forwarded to upstream.
    pub raw: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Validation error returned before any upstream request is attempted.
pub struct CodexImageRequestError {
    /// HTTP status to return to the client.
    pub status: StatusCode,
    /// Human-readable error message.
    pub message: String,
}

impl CodexImageRequestError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }
}

/// Maps all supported public aliases to the internal image endpoint enum.
pub fn normalize_image_gateway_path(path: &str) -> Option<CodexImageEndpoint> {
    match path {
        "/v1/images/generations"
        | "/api/codex-gateway/images/generations"
        | "/api/codex-gateway/v1/images/generations"
        | "/api/llm-gateway/v1/images/generations" => Some(CodexImageEndpoint::Generations),
        "/v1/images/edits"
        | "/api/codex-gateway/images/edits"
        | "/api/codex-gateway/v1/images/edits"
        | "/api/llm-gateway/v1/images/edits" => Some(CodexImageEndpoint::Edits),
        _ => None,
    }
}

/// Returns the upstream image API path for a normalized endpoint.
pub fn upstream_image_path(endpoint: CodexImageEndpoint) -> &'static str {
    match endpoint {
        CodexImageEndpoint::Generations => "/v1/images/generations",
        CodexImageEndpoint::Edits => "/v1/images/edits",
    }
}

/// Validates a JSON image request and rejects unsupported parameters.
pub fn parse_image_request(
    endpoint: CodexImageEndpoint,
    payload: Value,
) -> Result<CodexImageRequest, CodexImageRequestError> {
    let object = payload
        .as_object()
        .ok_or_else(|| CodexImageRequestError::bad_request("request body must be a JSON object"))?;
    let allowed_fields = match endpoint {
        CodexImageEndpoint::Generations => ALLOWED_FIELDS_GENERATIONS,
        CodexImageEndpoint::Edits => ALLOWED_FIELDS_EDITS,
    };
    reject_unknown_fields(object, allowed_fields)?;

    let model = optional_string(object, "model")?.unwrap_or_else(|| DEFAULT_MODEL.to_string());
    if model != DEFAULT_MODEL {
        return Err(CodexImageRequestError::bad_request("model must be gpt-image-2"));
    }

    let prompt = required_string(object, "prompt")?;
    if prompt.trim().is_empty() {
        return Err(CodexImageRequestError::bad_request("prompt is required"));
    }

    let n = optional_u64(object, "n")?.unwrap_or(1);
    if !(1..=4).contains(&n) {
        return Err(CodexImageRequestError::bad_request("n must be between 1 and 4"));
    }

    let size = optional_enum_string(object, "size", ALLOWED_SIZES)?;
    let quality = optional_enum_string(object, "quality", ALLOWED_QUALITIES)?;
    let background = optional_enum_string(object, "background", ALLOWED_BACKGROUNDS)?;
    let images = parse_edit_images(endpoint, object)?;

    Ok(CodexImageRequest {
        model,
        prompt,
        n,
        size,
        quality,
        background,
        images,
        raw: Value::Object(object.clone()),
    })
}

fn reject_unknown_fields(
    object: &Map<String, Value>,
    allowed_fields: &[&str],
) -> Result<(), CodexImageRequestError> {
    for key in object.keys() {
        if !allowed_fields.contains(&key.as_str()) {
            return Err(CodexImageRequestError::bad_request(format!("unknown field `{key}`")));
        }
    }
    Ok(())
}

fn required_string(
    object: &Map<String, Value>,
    field: &str,
) -> Result<String, CodexImageRequestError> {
    optional_string(object, field)?
        .ok_or_else(|| CodexImageRequestError::bad_request(format!("{field} is required")))
}

fn optional_string(
    object: &Map<String, Value>,
    field: &str,
) -> Result<Option<String>, CodexImageRequestError> {
    match object.get(field) {
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(CodexImageRequestError::bad_request(format!("{field} must be a string"))),
        None => Ok(None),
    }
}

fn optional_u64(
    object: &Map<String, Value>,
    field: &str,
) -> Result<Option<u64>, CodexImageRequestError> {
    match object.get(field) {
        Some(Value::Number(value)) => value.as_u64().map(Some).ok_or_else(|| {
            CodexImageRequestError::bad_request(format!("{field} must be an integer"))
        }),
        Some(_) => Err(CodexImageRequestError::bad_request(format!("{field} must be an integer"))),
        None => Ok(None),
    }
}

fn optional_enum_string(
    object: &Map<String, Value>,
    field: &str,
    allowed: &[&str],
) -> Result<Option<String>, CodexImageRequestError> {
    let Some(value) = optional_string(object, field)? else {
        return Ok(None);
    };
    if allowed.contains(&value.as_str()) {
        Ok(Some(value))
    } else {
        Err(CodexImageRequestError::bad_request(format!("{field} has unsupported value")))
    }
}

fn parse_edit_images(
    endpoint: CodexImageEndpoint,
    object: &Map<String, Value>,
) -> Result<Vec<String>, CodexImageRequestError> {
    if endpoint == CodexImageEndpoint::Generations {
        return Ok(Vec::new());
    }
    let Some(value) = object.get("images") else {
        return Err(CodexImageRequestError::bad_request("images is required"));
    };
    let images = value
        .as_array()
        .ok_or_else(|| CodexImageRequestError::bad_request("images must be an array"))?;
    if images.is_empty() || images.len() > 5 {
        return Err(CodexImageRequestError::bad_request(
            "images must contain between 1 and 5 entries",
        ));
    }
    images
        .iter()
        .map(|value| {
            let source = value.as_str().ok_or_else(|| {
                CodexImageRequestError::bad_request("image source must be a string")
            })?;
            validate_image_source(source)?;
            Ok(source.to_string())
        })
        .collect()
}

fn validate_image_source(source: &str) -> Result<(), CodexImageRequestError> {
    let lower = source.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Ok(());
    }
    if ALLOWED_DATA_IMAGE_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
    {
        return Ok(());
    }
    Err(CodexImageRequestError::bad_request(
        "image source must be an http(s) URL or supported image data URL",
    ))
}
