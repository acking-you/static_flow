//! AWS event stream binary frame parser: prelude, headers, payload, CRC
//! validation.
//!
//! Each frame consists of a 12-byte prelude (total length, header length,
//! prelude CRC), followed by typed headers, a variable-length payload, and a
//! trailing 4-byte message CRC. See the [AWS event stream spec] for details.
//!
//! [AWS event stream spec]: https://docs.aws.amazon.com/transcribe/latest/dg/event-stream.html

use super::{
    crc::crc32,
    error::{ParseError, ParseResult},
    header::{parse_headers, Headers},
};

/// Size of the prelude: 4 (total_length) + 4 (header_length) + 4 (prelude_crc).
pub const PRELUDE_SIZE: usize = 12;
/// Minimum valid message: prelude + trailing message CRC.
pub const MIN_MESSAGE_SIZE: usize = PRELUDE_SIZE + 4;
/// Hard upper bound on a single message (16 MiB).
pub const MAX_MESSAGE_SIZE: u32 = 16 * 1024 * 1024;

/// A parsed AWS event stream frame containing typed headers and a raw payload.
#[derive(Debug, Clone)]
pub struct Frame {
    pub headers: Headers,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn message_type(&self) -> Option<&str> {
        self.headers.message_type()
    }

    pub fn event_type(&self) -> Option<&str> {
        self.headers.event_type()
    }

    pub fn payload_as_json<T: serde::de::DeserializeOwned>(&self) -> ParseResult<T> {
        serde_json::from_slice(&self.payload).map_err(ParseError::PayloadDeserialize)
    }

    pub fn payload_as_str(&self) -> String {
        String::from_utf8_lossy(&self.payload).to_string()
    }
}

/// Try to parse one complete frame from `buffer`.
///
/// Returns `Ok(None)` when the buffer does not yet contain enough bytes.
/// On success returns the parsed [`Frame`] and the number of bytes consumed.
pub fn parse_frame(buffer: &[u8]) -> ParseResult<Option<(Frame, usize)>> {
    if buffer.len() < PRELUDE_SIZE {
        return Ok(None);
    }

    let total_length = u32::from_be_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]);
    let header_length = u32::from_be_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]);
    let prelude_crc = u32::from_be_bytes([buffer[8], buffer[9], buffer[10], buffer[11]]);

    if total_length < MIN_MESSAGE_SIZE as u32 {
        return Err(ParseError::MessageTooSmall {
            length: total_length,
            min: MIN_MESSAGE_SIZE as u32,
        });
    }
    if total_length > MAX_MESSAGE_SIZE {
        return Err(ParseError::MessageTooLarge {
            length: total_length,
            max: MAX_MESSAGE_SIZE,
        });
    }

    let total_length = total_length as usize;
    let header_length = header_length as usize;
    if buffer.len() < total_length {
        return Ok(None);
    }

    let actual_prelude_crc = crc32(&buffer[..8]);
    if actual_prelude_crc != prelude_crc {
        return Err(ParseError::PreludeCrcMismatch {
            expected: prelude_crc,
            actual: actual_prelude_crc,
        });
    }

    let message_crc = u32::from_be_bytes([
        buffer[total_length - 4],
        buffer[total_length - 3],
        buffer[total_length - 2],
        buffer[total_length - 1],
    ]);
    let actual_message_crc = crc32(&buffer[..total_length - 4]);
    if actual_message_crc != message_crc {
        return Err(ParseError::MessageCrcMismatch {
            expected: message_crc,
            actual: actual_message_crc,
        });
    }

    let headers_start = PRELUDE_SIZE;
    let headers_end = headers_start + header_length;
    if headers_end > total_length - 4 {
        return Err(ParseError::HeaderParseFailed(
            "header length exceeds message boundary".to_string(),
        ));
    }
    let headers = parse_headers(&buffer[headers_start..headers_end], header_length)?;
    let payload = buffer[headers_end..total_length - 4].to_vec();

    Ok(Some((
        Frame {
            headers,
            payload,
        },
        total_length,
    )))
}
