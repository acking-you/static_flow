//! Binary snapshot codec for `KiroCacheSimulator` state.
//!
//! Serializes the shared prefix tree and conversation-anchor index into a
//! compact, gzip-framed blob suitable for cross-node persistence in Valkey, and
//! restores them on startup. The on-wire layout, before gzip, is:
//!
//! ```text
//! [ header(24B) | prefix_section | anchor_section | crc32(4B) ]
//! ```
//!
//! Hash keys (`u128` page keys, 32-byte anchor hashes) dominate the size and do
//! not compress; the varint/UTF-8 structure does. All multi-byte integers are
//! little-endian; varints are LEB128, with zigzag for signed values.

use std::{
    io::{Read, Write},
    time::Duration,
};

use crc::{Crc, CRC_32_ISO_HDLC};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use serde::Serialize;

use super::{anchor_index::AnchorTokenCounts, projection::PREFIX_CACHE_PAGE_SIZE};

const MAGIC: [u8; 4] = *b"KCS1";
const FORMAT_VERSION: u16 = 1;
const HEADER_LEN: usize = 24;
const CRC32: Crc<u32> = Crc::<u32>::new(&CRC_32_ISO_HDLC);
/// Maximum compressed snapshot blob accepted from Valkey. The blob is external
/// persistent state (a corrupt or foreign key may sit in the shared namespace),
/// so an oversized input is refused before spending effort decompressing it.
/// Exposed so the Valkey read layer can skip oversized keys before fetching.
pub const MAX_COMPRESSED_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;
/// Maximum decompressed frame size. This is the real guard against a gzip bomb:
/// decompression is bounded so a corrupt or malicious blob cannot inflate into
/// an unbounded allocation and exhaust process memory.
const MAX_DECOMPRESSED_SNAPSHOT_BYTES: usize = 128 * 1024 * 1024;

/// Error raised while decoding or finalizing a snapshot blob.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    /// Magic, format version, or page size does not match this build; the
    /// snapshot must be treated as absent.
    #[error("snapshot is incompatible with this build")]
    Incompatible,
    /// The buffer is truncated, over-long, or otherwise structurally invalid.
    #[error("snapshot buffer is malformed")]
    Malformed,
    /// The trailing CRC32 did not match the decoded body.
    #[error("snapshot checksum mismatch")]
    ChecksumMismatch,
    /// Gzip compression or decompression failed.
    #[error("snapshot (de)compression failed: {0}")]
    Compression(String),
}
/// Optional size caps applied while encoding and restoring a snapshot. `None`
/// means "use the live runtime budget"; concrete values shrink the persisted
/// blob and the restored state.
#[derive(Debug, Clone, Copy, Default)]
pub struct SnapshotCaps {
    /// Upper bound on prefix-tree resident tokens to persist/restore.
    pub max_tokens: Option<u64>,
    /// Upper bound on anchor-index entries to persist/restore.
    pub max_anchor_entries: Option<usize>,
}

/// Result of importing one node's snapshot plus any peer snapshots.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct KiroSnapshotImportOutcome {
    /// Whether a prefix tree was recovered from the local node's snapshot.
    pub prefix_from_own: bool,
    /// Whether the prefix tree was seeded from a peer snapshot instead.
    pub prefix_from_peer: bool,
    /// Resident tokens in the restored prefix tree.
    pub prefix_resident_tokens: u64,
    /// Anchor entries installed after the cross-node union.
    pub anchor_entries: usize,
    /// Count of blobs (own + peers) that failed to decode and were skipped.
    pub decode_errors: usize,
}

/// Parsed snapshot header. Wall-clock `snapshot_unix_ms` anchors stop-time
/// aging; `resident_tokens` is advisory.
#[derive(Debug, Clone, Copy)]
pub struct SnapshotHeader {
    /// Wall-clock time the snapshot was produced, milliseconds since the epoch.
    pub snapshot_unix_ms: i64,
    /// Advisory resident-token count recorded at snapshot time.
    pub resident_tokens: u64,
}

impl SnapshotHeader {
    pub(super) fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&(PREFIX_CACHE_PAGE_SIZE as u16).to_le_bytes());
        out.extend_from_slice(&self.snapshot_unix_ms.to_le_bytes());
        out.extend_from_slice(&self.resident_tokens.to_le_bytes());
    }

    fn parse(bytes: &[u8]) -> Result<Self, SnapshotError> {
        if bytes.len() < HEADER_LEN {
            return Err(SnapshotError::Malformed);
        }
        if bytes[0..4] != MAGIC {
            return Err(SnapshotError::Incompatible);
        }
        let format_version = u16::from_le_bytes([bytes[4], bytes[5]]);
        let page_size = u16::from_le_bytes([bytes[6], bytes[7]]);
        if format_version != FORMAT_VERSION || usize::from(page_size) != PREFIX_CACHE_PAGE_SIZE {
            return Err(SnapshotError::Incompatible);
        }
        let snapshot_unix_ms = i64::from_le_bytes(
            bytes[8..16]
                .try_into()
                .map_err(|_| SnapshotError::Malformed)?,
        );
        let resident_tokens = u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| SnapshotError::Malformed)?,
        );
        Ok(Self {
            snapshot_unix_ms,
            resident_tokens,
        })
    }
}

/// A decoded snapshot frame: header plus the raw section bytes between the
/// header and the trailing CRC.
pub(super) struct DecodedFrame {
    pub(super) header: SnapshotHeader,
    pub(super) sections: Vec<u8>,
}

/// Gzip-compress a raw `[header | sections]` buffer after appending its CRC32.
pub(super) fn finalize_frame(mut raw: Vec<u8>) -> Result<Vec<u8>, SnapshotError> {
    let checksum = CRC32.checksum(&raw);
    raw.extend_from_slice(&checksum.to_le_bytes());
    gzip_compress(&raw)
}

/// Decompress and validate a snapshot blob, returning the header and sections.
pub(super) fn decode_frame(blob: &[u8]) -> Result<DecodedFrame, SnapshotError> {
    if blob.len() > MAX_COMPRESSED_SNAPSHOT_BYTES {
        return Err(SnapshotError::Malformed);
    }
    let raw = gzip_decompress(blob, MAX_DECOMPRESSED_SNAPSHOT_BYTES)?;
    if raw.len() < HEADER_LEN + 4 {
        return Err(SnapshotError::Malformed);
    }
    let body_len = raw.len() - 4;
    let stored_crc = u32::from_le_bytes(
        raw[body_len..]
            .try_into()
            .map_err(|_| SnapshotError::Malformed)?,
    );
    if CRC32.checksum(&raw[..body_len]) != stored_crc {
        return Err(SnapshotError::ChecksumMismatch);
    }
    let header = SnapshotHeader::parse(&raw[..HEADER_LEN])?;
    Ok(DecodedFrame {
        header,
        sections: raw[HEADER_LEN..body_len].to_vec(),
    })
}

/// Read only the header of a snapshot blob, validating the CRC first.
pub fn peek_header(blob: &[u8]) -> Result<SnapshotHeader, SnapshotError> {
    decode_frame(blob).map(|frame| frame.header)
}

fn gzip_compress(raw: &[u8]) -> Result<Vec<u8>, SnapshotError> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(raw)
        .map_err(|err| SnapshotError::Compression(err.to_string()))?;
    encoder
        .finish()
        .map_err(|err| SnapshotError::Compression(err.to_string()))
}

fn gzip_decompress(blob: &[u8], max_decompressed: usize) -> Result<Vec<u8>, SnapshotError> {
    // Bound the reader so a gzip bomb cannot inflate into an unbounded
    // allocation: read at most `max_decompressed + 1` bytes, then reject if the
    // extra byte materialized (i.e. the stream exceeded the cap).
    let limit = max_decompressed.saturating_add(1) as u64;
    let mut decoder = GzDecoder::new(blob).take(limit);
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(|err| SnapshotError::Compression(err.to_string()))?;
    if out.len() > max_decompressed {
        return Err(SnapshotError::Malformed);
    }
    Ok(out)
}
/// Bounds-checked forward cursor over snapshot section bytes.
pub(super) struct SnapshotReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> SnapshotReader<'a> {
    pub(super) fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
        }
    }

    pub(super) fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub(super) fn read_bytes(&mut self, len: usize) -> Result<&'a [u8], SnapshotError> {
        let end = self.pos.checked_add(len).ok_or(SnapshotError::Malformed)?;
        let slice = self
            .data
            .get(self.pos..end)
            .ok_or(SnapshotError::Malformed)?;
        self.pos = end;
        Ok(slice)
    }

    pub(super) fn read_u8(&mut self) -> Result<u8, SnapshotError> {
        Ok(self.read_bytes(1)?[0])
    }

    pub(super) fn read_u128_le(&mut self) -> Result<u128, SnapshotError> {
        let bytes = self.read_bytes(16)?;
        Ok(u128::from_le_bytes(bytes.try_into().map_err(|_| SnapshotError::Malformed)?))
    }

    /// Read an unsigned LEB128 varint.
    pub(super) fn read_varint(&mut self) -> Result<u64, SnapshotError> {
        let mut value: u64 = 0;
        let mut shift = 0u32;
        loop {
            let byte = self.read_u8()?;
            if shift >= 64 {
                return Err(SnapshotError::Malformed);
            }
            let chunk = u64::from(byte & 0x7f);
            let shifted = chunk << shift;
            // At shift == 63 only payloads 0/1 fit a u64; a larger payload would
            // silently drop high bits. Reject it as malformed rather than decode
            // a wrong value that later count/len/age checks would trust.
            if (shifted >> shift) != chunk {
                return Err(SnapshotError::Malformed);
            }
            value |= shifted;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
        }
    }

    /// Read a zigzag-encoded signed varint.
    pub(super) fn read_zigzag(&mut self) -> Result<i64, SnapshotError> {
        Ok(zigzag_decode(self.read_varint()?))
    }
}

/// Append an unsigned LEB128 varint.
pub(super) fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

/// Append a zigzag-encoded signed varint.
pub(super) fn write_zigzag(out: &mut Vec<u8>, value: i64) {
    write_varint(out, zigzag_encode(value));
}

fn zigzag_encode(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

fn zigzag_decode(value: u64) -> i64 {
    ((value >> 1) as i64) ^ -((value & 1) as i64)
}

/// One decoded anchor row, tagged with the originating snapshot wall-clock so
/// cross-node union can resolve recency consistently.
pub(super) struct DecodedAnchor {
    pub(super) hex: String,
    pub(super) conversation_id: String,
    pub(super) token_counts: Option<AnchorTokenCounts>,
    pub(super) age_secs: u64,
    pub(super) snapshot_unix_ms: i64,
}

/// One anchor row ready to be inserted into a rebuilt index, with its age
/// expressed relative to the current restore clock.
pub(super) struct RebuildRow {
    pub(super) hex: String,
    pub(super) conversation_id: String,
    pub(super) token_counts: Option<AnchorTokenCounts>,
    pub(super) eff_age_secs: u64,
}

/// Merge anchor rows from this node and peers into a recency-ordered, TTL- and
/// cap-bounded set. Newest `last_touched` wins per anchor; ties break on hex.
/// The returned rows are ordered oldest-first so an LRU rebuild preserves
/// recency, and every row's `eff_age_secs <= ttl`.
pub(super) fn union_anchor_rows(
    rows: Vec<DecodedAnchor>,
    now_unix_ms: i64,
    ttl: Duration,
    max_entries: usize,
) -> Vec<RebuildRow> {
    use std::collections::HashMap;

    // touched_ms is the absolute wall-clock of last touch; recency comparison
    // is independent of which snapshot a row came from.
    struct Candidate {
        conversation_id: String,
        token_counts: Option<AnchorTokenCounts>,
        touched_ms: i64,
    }

    let mut best: HashMap<String, Candidate> = HashMap::new();
    for row in rows {
        // `age_secs` is read from the wire as an unbounded varint; clamp the
        // millisecond conversion so a corrupt value cannot wrap negative and
        // make the entry look newer than its snapshot.
        let age_ms = i64::try_from(row.age_secs.saturating_mul(1000)).unwrap_or(i64::MAX);
        let touched_ms = row.snapshot_unix_ms.saturating_sub(age_ms);
        let candidate = Candidate {
            conversation_id: row.conversation_id,
            token_counts: row.token_counts,
            touched_ms,
        };
        match best.get(&row.hex) {
            Some(existing)
                if existing.touched_ms > touched_ms
                    || (existing.touched_ms == touched_ms
                        && existing.conversation_id >= candidate.conversation_id) => {},
            _ => {
                best.insert(row.hex, candidate);
            },
        }
    }

    let mut ordered: Vec<(String, Candidate)> = best.into_iter().collect();
    // Oldest first (ascending touched_ms); hex tiebreak for determinism.
    ordered.sort_by(|(left_hex, left), (right_hex, right)| {
        left.touched_ms
            .cmp(&right.touched_ms)
            .then_with(|| left_hex.cmp(right_hex))
    });
    if max_entries > 0 && ordered.len() > max_entries {
        let drop = ordered.len() - max_entries;
        ordered.drain(0..drop);
    }

    let ttl_secs = ttl.as_secs();
    ordered
        .into_iter()
        .filter_map(|(hex, candidate)| {
            let eff_age_ms = now_unix_ms.saturating_sub(candidate.touched_ms).max(0);
            let eff_age_secs = (eff_age_ms as u64) / 1000;
            if eff_age_secs > ttl_secs {
                return None;
            }
            Some(RebuildRow {
                hex,
                conversation_id: candidate.conversation_id,
                token_counts: candidate.token_counts,
                eff_age_secs,
            })
        })
        .collect()
}

/// Compute the effective age in seconds of a snapshot entry against the restore
/// clock: stop-time gap (wall clock) plus in-snapshot monotonic age.
pub(super) fn effective_age_secs(snapshot_unix_ms: i64, now_unix_ms: i64, age_secs: u64) -> u64 {
    let stop_gap_secs = now_unix_ms.saturating_sub(snapshot_unix_ms).max(0) as u64 / 1000;
    stop_gap_secs.saturating_add(age_secs)
}
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        decode_frame, finalize_frame, gzip_compress, gzip_decompress, peek_header,
        union_anchor_rows, write_varint, write_zigzag, DecodedAnchor, SnapshotError,
        SnapshotHeader, SnapshotReader, FORMAT_VERSION, MAGIC,
    };
    use crate::cache_sim::{anchor_index::AnchorTokenCounts, projection::PREFIX_CACHE_PAGE_SIZE};

    #[test]
    fn varint_round_trips_across_widths() {
        for value in [0u64, 1, 127, 128, 16_383, 16_384, u32::MAX as u64, u64::MAX] {
            let mut buf = Vec::new();
            write_varint(&mut buf, value);
            let mut reader = SnapshotReader::new(&buf);
            assert_eq!(reader.read_varint().expect("decode varint"), value);
            assert_eq!(reader.remaining(), 0);
        }
    }

    #[test]
    fn varint_rejects_overflowing_tenth_byte() {
        // u64::MAX is the largest legal varint: nine 0xFF groups then a final
        // 0x01 (payload 1 at shift 63). A final byte > 1 would drop high bits.
        let mut max_buf = Vec::new();
        write_varint(&mut max_buf, u64::MAX);
        assert_eq!(max_buf.len(), 10);
        assert_eq!(*max_buf.last().expect("last byte"), 0x01);

        // Same 10-byte shape but payload 2 in the last group: must be rejected
        // rather than silently truncated into a smaller value.
        let mut bad = vec![0xffu8; 9];
        bad.push(0x02);
        let mut reader = SnapshotReader::new(&bad);
        assert!(matches!(reader.read_varint(), Err(SnapshotError::Malformed)));
    }

    #[test]
    fn zigzag_round_trips_signed_values() {
        for value in [0i64, 1, -1, 2, -2, i32::MIN as i64, i32::MAX as i64] {
            let mut buf = Vec::new();
            write_zigzag(&mut buf, value);
            let mut reader = SnapshotReader::new(&buf);
            assert_eq!(reader.read_zigzag().expect("decode zigzag"), value);
        }
    }

    fn sample_header() -> SnapshotHeader {
        SnapshotHeader {
            snapshot_unix_ms: 1_700_000_000_000,
            resident_tokens: 4_096,
        }
    }

    #[test]
    fn header_round_trips() {
        let mut buf = Vec::new();
        sample_header().write(&mut buf);
        let parsed = SnapshotHeader::parse(&buf).expect("parse header");
        assert_eq!(parsed.snapshot_unix_ms, 1_700_000_000_000);
        assert_eq!(parsed.resident_tokens, 4_096);
    }

    #[test]
    fn header_rejects_incompatible_page_size_version_and_magic() {
        let mut buf = Vec::new();
        sample_header().write(&mut buf);
        // page_size lives at bytes 6..8.
        let mut bad_page = buf.clone();
        bad_page[6] = bad_page[6].wrapping_add(1);
        assert!(matches!(SnapshotHeader::parse(&bad_page), Err(SnapshotError::Incompatible)));
        // format_version lives at bytes 4..6.
        let mut bad_version = buf.clone();
        bad_version[4] = bad_version[4].wrapping_add(1);
        assert!(matches!(SnapshotHeader::parse(&bad_version), Err(SnapshotError::Incompatible)));
        let mut bad_magic = buf.clone();
        bad_magic[0] = b'X';
        assert!(matches!(SnapshotHeader::parse(&bad_magic), Err(SnapshotError::Incompatible)));
        assert_eq!(FORMAT_VERSION, 1);
        assert_eq!(MAGIC, *b"KCS1");
        assert_eq!(PREFIX_CACHE_PAGE_SIZE, 64);
    }

    #[test]
    fn frame_round_trips_and_detects_corruption() {
        let mut raw = Vec::new();
        sample_header().write(&mut raw);
        raw.extend_from_slice(b"prefix+anchor section payload");
        let blob = finalize_frame(raw.clone()).expect("finalize");

        let frame = decode_frame(&blob).expect("decode");
        assert_eq!(frame.header.resident_tokens, 4_096);
        assert_eq!(&frame.sections, b"prefix+anchor section payload");
        assert_eq!(peek_header(&blob).expect("peek").snapshot_unix_ms, 1_700_000_000_000);

        // Build a frame whose inner CRC is deliberately wrong but still
        // decompresses cleanly, exercising the inner checksum guard.
        let mut bad = raw;
        bad.extend_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        let bad_blob = gzip_compress(&bad).expect("compress");
        assert!(matches!(decode_frame(&bad_blob), Err(SnapshotError::ChecksumMismatch)));
    }

    #[test]
    fn gzip_decompress_rejects_blob_over_decompressed_limit() {
        // A highly compressible payload (zeros) shrinks to a tiny blob but would
        // inflate well past a small cap: the bounded reader must refuse it as
        // malformed instead of allocating the full expansion.
        let payload = vec![0u8; 4096];
        let blob = gzip_compress(&payload).expect("compress");
        assert!(blob.len() < payload.len());
        // Within the cap it round-trips.
        assert_eq!(gzip_decompress(&blob, 4096).expect("decompress").len(), 4096);
        // Below the decompressed size it is rejected, not expanded.
        assert!(matches!(gzip_decompress(&blob, 1024), Err(SnapshotError::Malformed)));
    }

    fn anchor_row(
        hex: &str,
        conv: &str,
        snapshot_unix_ms: i64,
        age_secs: u64,
        counts: Option<AnchorTokenCounts>,
    ) -> DecodedAnchor {
        DecodedAnchor {
            hex: hex.to_string(),
            conversation_id: conv.to_string(),
            token_counts: counts,
            age_secs,
            snapshot_unix_ms,
        }
    }

    #[test]
    fn union_keeps_newest_touch_per_anchor() {
        let now = 2_000_000_000_000i64;
        let rows = vec![
            // Same hex from two snapshots: the one touched more recently wins.
            anchor_row("aa", "stale-conv", now - 600_000, 300, None),
            anchor_row(
                "aa",
                "fresh-conv",
                now - 10_000,
                5,
                Some(AnchorTokenCounts {
                    real_input_tokens: 7,
                    local_input_tokens: 9,
                }),
            ),
        ];
        let merged = union_anchor_rows(rows, now, Duration::from_secs(86_400), 16);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].conversation_id, "fresh-conv");
        assert_eq!(merged[0].token_counts.map(|c| c.real_input_tokens), Some(7));
    }

    #[test]
    fn union_drops_oldest_when_over_cap_and_filters_ttl() {
        let now = 2_000_000_000_000i64;
        let rows = vec![
            anchor_row("a1", "c1", now - 30_000, 0, None),
            anchor_row("a2", "c2", now - 20_000, 0, None),
            anchor_row("a3", "c3", now - 10_000, 0, None),
            // Older than the TTL window; must be filtered out entirely.
            anchor_row("a4", "c4", now - 400_000, 0, None),
        ];
        let merged = union_anchor_rows(rows, now, Duration::from_secs(60), 2);
        // a4 filtered by TTL, then cap=2 keeps the two newest of {a1,a2,a3}.
        let ids: Vec<&str> = merged.iter().map(|r| r.conversation_id.as_str()).collect();
        assert_eq!(merged.len(), 2);
        assert!(ids.contains(&"c2"));
        assert!(ids.contains(&"c3"));
        assert!(!ids.contains(&"c4"));
    }
}
