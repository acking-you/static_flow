//! Image format detection from explicit media types, base64 prefixes, and
//! raw magic bytes.

use base64::Engine as _;

pub fn get_image_format_from_source(
    source: &crate::anthropic::types::ImageSource,
) -> Option<String> {
    detect_image_format_from_base64(&source.data)
        .or_else(|| get_image_format(&source.media_type))
        .map(str::to_string)
}

fn detect_image_format_from_base64(data: &str) -> Option<&'static str> {
    let mut prefix = data
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace())
        .take(64)
        .collect::<String>();
    if prefix.is_empty() {
        return None;
    }
    while prefix.len() % 4 != 0 {
        prefix.push('=');
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(prefix.as_bytes())
        .ok()?;
    detect_image_format_from_bytes(&bytes)
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
