/// AWS Event Stream binary message protocol parser.
///
/// Implements decoding of the event stream wire format used by AWS services
/// (e.g., Bedrock Runtime `InvokeModelWithResponseStream`). The format is a
/// sequence of length-prefixed, CRC-protected binary frames, each carrying
/// typed headers and an opaque payload.
pub mod crc;
pub mod decoder;
pub mod error;
pub mod frame;
pub mod header;
