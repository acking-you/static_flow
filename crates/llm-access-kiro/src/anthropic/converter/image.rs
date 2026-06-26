//! Image format detection from explicit media types, base64 prefixes, and
//! raw magic bytes.

use base64::Engine as _;

pub fn get_image_format_from_source(
    source: &crate::anthropic::types::ImageSource,
) -> Result<Option<String>, ImageDataError> {
    let bytes = decode_base64_image_data(&source.data)?;
    if let Some(format) = detect_image_format_from_bytes(&bytes) {
        validate_image_bytes(format, &bytes)?;
        return Ok(Some(format.to_string()));
    }
    if get_image_format(&source.media_type).is_some() {
        return Err(ImageDataError::new("base64 data does not contain supported image bytes"));
    }
    Ok(None)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageDataError {
    message: String,
}

impl ImageDataError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ImageDataError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ImageDataError {}

fn decode_base64_image_data(data: &str) -> Result<Vec<u8>, ImageDataError> {
    let compact = data
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace())
        .collect::<String>();
    if compact.is_empty() {
        return Err(ImageDataError::new("image data is empty"));
    }
    if compact.len() % 4 == 1 {
        return Err(ImageDataError::new("base64 data has invalid length"));
    }
    let mut padded = compact;
    while padded.len() % 4 != 0 {
        padded.push('=');
    }
    base64::engine::general_purpose::STANDARD
        .decode(padded.as_bytes())
        .map_err(|_| ImageDataError::new("base64 data is invalid"))
}

fn detect_image_format_from_bytes(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("jpeg");
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("png");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("webp");
    }
    None
}

fn get_image_format(media_type: &str) -> Option<&'static str> {
    match media_type {
        "image/jpeg" => Some("jpeg"),
        "image/png" => Some("png"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        _ => None,
    }
}

fn validate_image_bytes(format: &str, bytes: &[u8]) -> Result<(), ImageDataError> {
    match format {
        "jpeg" => validate_jpeg(bytes),
        "png" => validate_png(bytes),
        "gif" => validate_gif(bytes),
        "webp" => validate_webp(bytes),
        _ => Ok(()),
    }
}

fn validate_jpeg(bytes: &[u8]) -> Result<(), ImageDataError> {
    if !bytes.windows(2).any(|window| window == [0xff, 0xd9]) {
        return Err(ImageDataError::new("jpeg data is missing end-of-image marker"));
    }
    Ok(())
}

fn validate_png(bytes: &[u8]) -> Result<(), ImageDataError> {
    let mut offset = 8usize;
    let mut saw_ihdr = false;
    let mut saw_iend = false;
    while offset + 8 <= bytes.len() {
        let length = u32::from_be_bytes(
            bytes[offset..offset + 4]
                .try_into()
                .expect("slice length is checked"),
        ) as usize;
        let chunk_type = &bytes[offset + 4..offset + 8];
        if !saw_ihdr && chunk_type != b"IHDR" {
            return Err(ImageDataError::new("png first chunk is not IHDR"));
        }
        if chunk_type == b"IHDR" {
            if length != 13 {
                return Err(ImageDataError::new("png IHDR chunk has invalid length"));
            }
            saw_ihdr = true;
        }
        let Some(chunk_end) = offset
            .checked_add(12)
            .and_then(|value| value.checked_add(length))
        else {
            return Err(ImageDataError::new("png chunk length overflows"));
        };
        if chunk_end > bytes.len() {
            return Err(ImageDataError::new(format!(
                "truncated png chunk `{}`",
                String::from_utf8_lossy(chunk_type)
            )));
        }
        offset = chunk_end;
        if chunk_type == b"IEND" {
            if length != 0 {
                return Err(ImageDataError::new("png IEND chunk has invalid length"));
            }
            saw_iend = true;
            break;
        }
    }
    if !saw_ihdr {
        return Err(ImageDataError::new("png data is missing IHDR chunk"));
    }
    if !saw_iend {
        return Err(ImageDataError::new("png data is missing IEND chunk"));
    }
    if offset != bytes.len() {
        return Err(ImageDataError::new("png data has trailing bytes after IEND"));
    }
    Ok(())
}

fn validate_gif(bytes: &[u8]) -> Result<(), ImageDataError> {
    if !bytes.ends_with(&[0x3b]) {
        return Err(ImageDataError::new("gif data is missing trailer"));
    }
    Ok(())
}

fn validate_webp(bytes: &[u8]) -> Result<(), ImageDataError> {
    if bytes.len() < 12 {
        return Err(ImageDataError::new("webp data is truncated"));
    }
    let riff_size =
        u32::from_le_bytes(bytes[4..8].try_into().expect("slice length is checked")) as usize;
    let Some(expected_len) = riff_size.checked_add(8) else {
        return Err(ImageDataError::new("webp RIFF size overflows"));
    };
    if bytes.len() < expected_len {
        return Err(ImageDataError::new("webp data is truncated"));
    }
    Ok(())
}
