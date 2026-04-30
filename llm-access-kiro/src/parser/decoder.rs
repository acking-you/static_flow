//! Incremental event stream decoder with error recovery.
//!
//! [`EventStreamDecoder`] accumulates raw bytes via
//! [`feed`](EventStreamDecoder::feed) and yields parsed [`Frame`]s via
//! [`decode`](EventStreamDecoder::decode) or the convenience
//! [`decode_iter`](EventStreamDecoder::decode_iter). On parse errors the
//! decoder attempts byte-level recovery (skipping the bad frame)
//! and stops permanently after [`DEFAULT_MAX_ERRORS`] consecutive failures.

use bytes::{Buf, BytesMut};

use super::{
    error::{ParseError, ParseResult},
    frame::{parse_frame, Frame, PRELUDE_SIZE},
};

pub const DEFAULT_MAX_BUFFER_SIZE: usize = 16 * 1024 * 1024;
pub const DEFAULT_MAX_ERRORS: usize = 5;
pub const DEFAULT_BUFFER_CAPACITY: usize = 8192;

/// Current phase of the decoder state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecoderState {
    /// Idle, ready to accept more data or attempt a decode.
    Ready,
    /// Actively parsing a frame from the buffer.
    Parsing,
    /// Last decode hit an error; recovery was attempted, awaiting new data.
    Recovering,
    /// Too many consecutive errors; decoder will not produce further frames.
    Stopped,
}

/// Incremental, fault-tolerant decoder for AWS event stream binary frames.
///
/// Buffers incoming bytes and yields complete [`Frame`]s. On CRC or header
/// errors the decoder skips forward and continues; after
/// [`DEFAULT_MAX_ERRORS`] consecutive failures it transitions to
/// [`DecoderState::Stopped`].
pub struct EventStreamDecoder {
    buffer: BytesMut,
    state: DecoderState,
    error_count: usize,
    max_errors: usize,
    max_buffer_size: usize,
    bytes_skipped: usize,
}

impl Default for EventStreamDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl EventStreamDecoder {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_BUFFER_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buffer: BytesMut::with_capacity(capacity),
            state: DecoderState::Ready,
            error_count: 0,
            max_errors: DEFAULT_MAX_ERRORS,
            max_buffer_size: DEFAULT_MAX_BUFFER_SIZE,
            bytes_skipped: 0,
        }
    }

    /// Append raw bytes to the internal buffer. Returns an error if the
    /// buffer would exceed [`DEFAULT_MAX_BUFFER_SIZE`].
    pub fn feed(&mut self, data: &[u8]) -> ParseResult<()> {
        let new_size = self.buffer.len() + data.len();
        if new_size > self.max_buffer_size {
            return Err(ParseError::BufferOverflow {
                size: new_size,
                max: self.max_buffer_size,
            });
        }
        self.buffer.extend_from_slice(data);
        if self.state == DecoderState::Recovering {
            self.state = DecoderState::Ready;
        }
        Ok(())
    }

    /// Try to decode one frame from the buffer. Returns `Ok(None)` when
    /// there is not enough data for a complete frame.
    pub fn decode(&mut self) -> ParseResult<Option<Frame>> {
        if self.state == DecoderState::Stopped {
            return Err(ParseError::TooManyErrors {
                count: self.error_count,
                last_error: "decoder stopped".to_string(),
            });
        }
        if self.buffer.is_empty() {
            self.state = DecoderState::Ready;
            return Ok(None);
        }
        self.state = DecoderState::Parsing;
        match parse_frame(&self.buffer) {
            Ok(Some((frame, consumed))) => {
                self.buffer.advance(consumed);
                self.state = DecoderState::Ready;
                self.error_count = 0;
                Ok(Some(frame))
            },
            Ok(None) => {
                self.state = DecoderState::Ready;
                Ok(None)
            },
            Err(err) => {
                self.error_count += 1;
                if self.error_count >= self.max_errors {
                    self.state = DecoderState::Stopped;
                    return Err(ParseError::TooManyErrors {
                        count: self.error_count,
                        last_error: err.to_string(),
                    });
                }
                self.try_recover(&err);
                self.state = DecoderState::Recovering;
                Err(err)
            },
        }
    }

    /// Return an iterator that drains all complete frames from the buffer,
    /// stopping on incomplete data, recovery state, or decoder shutdown.
    pub fn decode_iter(&mut self) -> DecodeIter<'_> {
        DecodeIter {
            decoder: self,
        }
    }

    // Attempt to skip past a corrupted frame so the decoder can resync.
    // For prelude-level errors we advance one byte; for message-level errors
    // we try to skip the entire declared message length.
    fn try_recover(&mut self, error: &ParseError) {
        if self.buffer.is_empty() {
            return;
        }
        match error {
            ParseError::PreludeCrcMismatch {
                ..
            }
            | ParseError::MessageTooSmall {
                ..
            }
            | ParseError::MessageTooLarge {
                ..
            } => {
                self.buffer.advance(1);
                self.bytes_skipped += 1;
            },
            ParseError::MessageCrcMismatch {
                ..
            }
            | ParseError::HeaderParseFailed(_) => {
                if self.buffer.len() >= PRELUDE_SIZE {
                    let total_length = u32::from_be_bytes([
                        self.buffer[0],
                        self.buffer[1],
                        self.buffer[2],
                        self.buffer[3],
                    ]) as usize;
                    if total_length >= 16 && total_length <= self.buffer.len() {
                        self.buffer.advance(total_length);
                        self.bytes_skipped += total_length;
                        return;
                    }
                }
                self.buffer.advance(1);
                self.bytes_skipped += 1;
            },
            _ => {
                self.buffer.advance(1);
                self.bytes_skipped += 1;
            },
        }
    }
}

/// Borrowing iterator over frames available in the decoder's buffer.
pub struct DecodeIter<'a> {
    decoder: &'a mut EventStreamDecoder,
}

impl Iterator for DecodeIter<'_> {
    type Item = ParseResult<Frame>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.decoder.state {
            DecoderState::Stopped | DecoderState::Recovering => None,
            _ => match self.decoder.decode() {
                Ok(Some(frame)) => Some(Ok(frame)),
                Ok(None) => None,
                Err(err) => Some(Err(err)),
            },
        }
    }
}
