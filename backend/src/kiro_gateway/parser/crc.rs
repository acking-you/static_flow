//! CRC-32 checksum (ISO HDLC / CRC-32C) used by the AWS event stream frame
//! parser.
//!
//! Both the 8-byte prelude and the full message carry CRC-32 checksums that
//! must be validated before the frame payload is trusted.

use crc::{Crc, CRC_32_ISO_HDLC};

const CRC32: Crc<u32> = Crc::<u32>::new(&CRC_32_ISO_HDLC);

pub fn crc32(data: &[u8]) -> u32 {
    CRC32.checksum(data)
}
