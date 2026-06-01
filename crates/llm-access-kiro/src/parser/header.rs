//! Typed header parsing for AWS event stream frames.
//!
//! Each header is a length-prefixed name followed by a one-byte type
//! discriminant and a type-specific value. This module decodes all ten
//! header value types defined by the AWS event stream specification.

use std::collections::HashMap;

use super::error::{ParseError, ParseResult};

/// One-byte discriminant identifying the wire type of a header value.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderValueType {
    BoolTrue = 0,
    BoolFalse = 1,
    Byte = 2,
    Short = 3,
    Integer = 4,
    Long = 5,
    ByteArray = 6,
    String = 7,
    Timestamp = 8,
    Uuid = 9,
}

impl TryFrom<u8> for HeaderValueType {
    type Error = ParseError;

    fn try_from(value: u8) -> ParseResult<Self> {
        match value {
            0 => Ok(Self::BoolTrue),
            1 => Ok(Self::BoolFalse),
            2 => Ok(Self::Byte),
            3 => Ok(Self::Short),
            4 => Ok(Self::Integer),
            5 => Ok(Self::Long),
            6 => Ok(Self::ByteArray),
            7 => Ok(Self::String),
            8 => Ok(Self::Timestamp),
            9 => Ok(Self::Uuid),
            other => Err(ParseError::InvalidHeaderType(other)),
        }
    }
}

/// Decoded header value, covering all ten AWS event stream header types.
#[derive(Debug, Clone, PartialEq)]
pub enum HeaderValue {
    Bool(bool),
    Byte(i8),
    Short(i16),
    Integer(i32),
    Long(i64),
    ByteArray(Vec<u8>),
    String(String),
    Timestamp(i64),
    Uuid([u8; 16]),
}

impl HeaderValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }
}

/// Name-to-value map of headers parsed from a single frame.
/// Provides convenience accessors for well-known header names
/// (`:message-type`, `:event-type`, `:exception-type`, `:error-code`).
#[derive(Debug, Clone, Default)]
pub struct Headers {
    inner: HashMap<String, HeaderValue>,
}

impl Headers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, name: String, value: HeaderValue) {
        self.inner.insert(name, value);
    }

    pub fn get(&self, name: &str) -> Option<&HeaderValue> {
        self.inner.get(name)
    }

    pub fn get_string(&self, name: &str) -> Option<&str> {
        self.get(name).and_then(HeaderValue::as_str)
    }

    pub fn message_type(&self) -> Option<&str> {
        self.get_string(":message-type")
    }

    pub fn event_type(&self) -> Option<&str> {
        self.get_string(":event-type")
    }

    pub fn exception_type(&self) -> Option<&str> {
        self.get_string(":exception-type")
    }

    pub fn error_code(&self) -> Option<&str> {
        self.get_string(":error-code")
    }
}

/// Parse the header section of a frame from raw bytes.
///
/// `header_length` is the byte count declared in the frame prelude.
pub fn parse_headers(data: &[u8], header_length: usize) -> ParseResult<Headers> {
    if data.len() < header_length {
        return Err(ParseError::Incomplete {
            needed: header_length,
            available: data.len(),
        });
    }

    let mut headers = Headers::new();
    let mut offset = 0usize;
    while offset < header_length {
        if offset >= data.len() {
            break;
        }
        let name_len = data[offset] as usize;
        offset += 1;
        if name_len == 0 {
            return Err(ParseError::HeaderParseFailed(
                "header name length cannot be zero".to_string(),
            ));
        }
        if offset + name_len > data.len() {
            return Err(ParseError::Incomplete {
                needed: name_len,
                available: data.len().saturating_sub(offset),
            });
        }
        let name = String::from_utf8_lossy(&data[offset..offset + name_len]).to_string();
        offset += name_len;
        if offset >= data.len() {
            return Err(ParseError::Incomplete {
                needed: 1,
                available: 0,
            });
        }
        let value_type = HeaderValueType::try_from(data[offset])?;
        offset += 1;
        let value = parse_header_value(&data[offset..], value_type, &mut offset)?;
        headers.insert(name, value);
    }

    Ok(headers)
}

fn parse_header_value(
    data: &[u8],
    value_type: HeaderValueType,
    global_offset: &mut usize,
) -> ParseResult<HeaderValue> {
    let mut local_offset = 0usize;
    let value = match value_type {
        HeaderValueType::BoolTrue => HeaderValue::Bool(true),
        HeaderValueType::BoolFalse => HeaderValue::Bool(false),
        HeaderValueType::Byte => {
            ensure_bytes(data, 1)?;
            local_offset = 1;
            HeaderValue::Byte(data[0] as i8)
        },
        HeaderValueType::Short => {
            ensure_bytes(data, 2)?;
            local_offset = 2;
            HeaderValue::Short(i16::from_be_bytes([data[0], data[1]]))
        },
        HeaderValueType::Integer => {
            ensure_bytes(data, 4)?;
            local_offset = 4;
            HeaderValue::Integer(i32::from_be_bytes([data[0], data[1], data[2], data[3]]))
        },
        HeaderValueType::Long | HeaderValueType::Timestamp => {
            ensure_bytes(data, 8)?;
            local_offset = 8;
            let value = i64::from_be_bytes([
                data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
            ]);
            if value_type == HeaderValueType::Timestamp {
                HeaderValue::Timestamp(value)
            } else {
                HeaderValue::Long(value)
            }
        },
        HeaderValueType::ByteArray | HeaderValueType::String => {
            ensure_bytes(data, 2)?;
            let len = u16::from_be_bytes([data[0], data[1]]) as usize;
            ensure_bytes(data, 2 + len)?;
            local_offset = 2 + len;
            if value_type == HeaderValueType::String {
                HeaderValue::String(String::from_utf8_lossy(&data[2..2 + len]).to_string())
            } else {
                HeaderValue::ByteArray(data[2..2 + len].to_vec())
            }
        },
        HeaderValueType::Uuid => {
            ensure_bytes(data, 16)?;
            local_offset = 16;
            let mut uuid = [0u8; 16];
            uuid.copy_from_slice(&data[..16]);
            HeaderValue::Uuid(uuid)
        },
    };
    *global_offset += local_offset;
    Ok(value)
}

fn ensure_bytes(data: &[u8], needed: usize) -> ParseResult<()> {
    if data.len() < needed {
        Err(ParseError::Incomplete {
            needed,
            available: data.len(),
        })
    } else {
        Ok(())
    }
}
