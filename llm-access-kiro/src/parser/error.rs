//! Error types for the AWS event stream binary parser.
//!
//! [`ParseError`] covers every failure mode from incomplete data and CRC
//! mismatches to header/payload deserialization errors and decoder-level
//! circuit-breaking after too many consecutive failures.

/// Errors that can occur while parsing an AWS event stream frame.
#[derive(Debug)]
pub enum ParseError {
    /// Not enough bytes in the buffer to complete the current parse step.
    Incomplete { needed: usize, available: usize },
    /// The 8-byte prelude CRC does not match the computed value.
    PreludeCrcMismatch { expected: u32, actual: u32 },
    /// The trailing message CRC does not match the computed value.
    MessageCrcMismatch { expected: u32, actual: u32 },
    /// Header type discriminant byte is not a known
    /// [`HeaderValueType`](super::header::HeaderValueType).
    InvalidHeaderType(u8),
    /// Structural error while walking the header key-value pairs.
    HeaderParseFailed(String),
    /// Total message length exceeds the configured maximum.
    MessageTooLarge { length: u32, max: u32 },
    /// Total message length is below the minimum valid size.
    MessageTooSmall { length: u32, min: u32 },
    /// The `:message-type` header contains an unrecognized value.
    InvalidMessageType(String),
    /// JSON deserialization of the frame payload failed.
    PayloadDeserialize(serde_json::Error),
    /// Underlying I/O error.
    Io(std::io::Error),
    /// The decoder has hit its consecutive-error limit and stopped.
    TooManyErrors { count: usize, last_error: String },
    /// The internal buffer would exceed its size cap after appending new data.
    BufferOverflow { size: usize, max: usize },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Incomplete {
                needed,
                available,
            } => {
                write!(f, "incomplete data: need {needed} bytes, have {available}")
            },
            Self::PreludeCrcMismatch {
                expected,
                actual,
            } => {
                write!(f, "prelude crc mismatch: expected {expected:#x}, got {actual:#x}")
            },
            Self::MessageCrcMismatch {
                expected,
                actual,
            } => {
                write!(f, "message crc mismatch: expected {expected:#x}, got {actual:#x}")
            },
            Self::InvalidHeaderType(value) => write!(f, "invalid header type: {value}"),
            Self::HeaderParseFailed(message) => write!(f, "header parse failed: {message}"),
            Self::MessageTooLarge {
                length,
                max,
            } => {
                write!(f, "message too large: {length} > {max}")
            },
            Self::MessageTooSmall {
                length,
                min,
            } => {
                write!(f, "message too small: {length} < {min}")
            },
            Self::InvalidMessageType(value) => write!(f, "invalid message type: {value}"),
            Self::PayloadDeserialize(err) => write!(f, "payload deserialize failed: {err}"),
            Self::Io(err) => write!(f, "io error: {err}"),
            Self::TooManyErrors {
                count,
                last_error,
            } => {
                write!(f, "too many parse errors ({count}): {last_error}")
            },
            Self::BufferOverflow {
                size,
                max,
            } => {
                write!(f, "buffer overflow: {size} > {max}")
            },
        }
    }
}

impl std::error::Error for ParseError {}

impl From<std::io::Error> for ParseError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for ParseError {
    fn from(value: serde_json::Error) -> Self {
        Self::PayloadDeserialize(value)
    }
}

pub type ParseResult<T> = Result<T, ParseError>;
