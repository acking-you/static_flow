//! Image format detection from explicit media types, base64 prefixes, and
//! raw magic bytes.

use base64::Engine as _;

pub fn get_image_from_source(
    source: &crate::anthropic::types::ImageSource,
) -> Result<Option<ValidatedImageSource>, ImageDataError> {
    let bytes = decode_base64_image_data(&source.data)?;
    let declared_format = get_image_format(&source.media_type);
    if let Some(format) = detect_image_format_from_bytes(&bytes)
        .or_else(|| truncated_declared_format(declared_format, &bytes))
    {
        validate_image_bytes(format, &bytes)?;
        return Ok(Some(ValidatedImageSource {
            format: format.to_string(),
            data: base64::engine::general_purpose::STANDARD.encode(&bytes),
        }));
    }
    if declared_format.is_some() {
        return Err(ImageDataError::new("base64 data does not contain supported image bytes"));
    }
    Ok(None)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedImageSource {
    pub format: String,
    pub data: String,
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
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(padded.as_bytes()))
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

fn truncated_declared_format(
    declared_format: Option<&'static str>,
    bytes: &[u8],
) -> Option<&'static str> {
    (declared_format == Some("webp") && bytes.starts_with(b"RIFF")).then_some("webp")
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
    if !bytes.ends_with(&[0xff, 0xd9]) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::types::ImageSource;

    const SAMPLE_PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/\
                                     x8AAwMCAO+/p9sAAAAASUVORK5CYII=";
    const SAMPLE_PNG_BYTES: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4,
        0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 252, 255, 31, 0, 3, 3,
        2, 0, 239, 191, 167, 219, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];

    fn source(media_type: &str, data: impl Into<String>) -> ImageSource {
        ImageSource {
            source_type: "base64".to_string(),
            media_type: media_type.to_string(),
            data: data.into(),
        }
    }

    fn encode(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn format_from_source(source: &ImageSource) -> Result<Option<String>, ImageDataError> {
        Ok(get_image_from_source(source)?.map(|image| image.format))
    }

    #[test]
    fn accepts_png_with_trailing_bytes_after_iend() {
        let mut bytes = SAMPLE_PNG_BYTES.to_vec();
        bytes.extend_from_slice(b"trailing");

        let format = format_from_source(&source("image/png", encode(&bytes)))
            .expect("trailing bytes should be accepted")
            .expect("format");

        assert_eq!(format, "png");
    }

    #[test]
    fn rejects_supported_media_type_without_supported_magic_bytes() {
        let err = format_from_source(&source("image/png", encode(b"not an image")))
            .expect_err("declared image without magic bytes should be rejected");

        assert!(err.to_string().contains("supported image bytes"));
    }

    #[test]
    fn rejects_jpeg_when_eoi_is_not_at_end() {
        let bytes = [0xff, 0xd8, 0xff, 0x00, 0xff, 0xd9, 0x00];
        let err = format_from_source(&source("image/jpeg", encode(&bytes)))
            .expect_err("jpeg with non-terminal EOI should be rejected");

        assert!(err.to_string().contains("end-of-image"));
    }

    #[test]
    fn rejects_gif_missing_trailer() {
        let err = format_from_source(&source("image/gif", encode(b"GIF89aabc")))
            .expect_err("gif without trailer should be rejected");

        assert!(err.to_string().contains("trailer"));
    }

    #[test]
    fn rejects_truncated_webp() {
        let err = format_from_source(&source("image/webp", encode(b"RIFF")))
            .expect_err("short webp should be rejected");

        assert!(err.to_string().contains("webp data is truncated"));
    }

    #[test]
    fn rejects_webp_with_oversized_riff_length() {
        let bytes = b"RIFF\x08\x00\x00\x00WEBP";
        let err = format_from_source(&source("image/webp", encode(bytes)))
            .expect_err("webp with oversized riff length should be rejected");

        assert!(err.to_string().contains("webp data is truncated"));
    }

    #[test]
    fn accepts_url_safe_base64_image_data() {
        let data = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(SAMPLE_PNG_BYTES);

        let image = get_image_from_source(&source("image/png", data))
            .expect("url-safe image data should be accepted")
            .expect("image");

        assert_eq!(image.format, "png");
        assert_eq!(image.data, SAMPLE_PNG_BASE64);
    }

    #[test]
    fn accepts_valid_png_fixture() {
        let format = format_from_source(&source("image/png", SAMPLE_PNG_BASE64))
            .expect("png fixture should be accepted")
            .expect("format");

        assert_eq!(format, "png");
    }
}
