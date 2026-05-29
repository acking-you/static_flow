//! Stable hashing helpers: segment hashing, incremental hasher updates,
//! SHA-256 hex, and canonical segment serialization.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) fn hash_segments(segments: &[String]) -> String {
    let mut hasher = Sha256::new();
    update_hash_segments(&mut hasher, segments.iter());
    format!("{:x}", hasher.finalize())
}
pub(crate) fn update_hash_segments<'a>(
    hasher: &mut Sha256,
    segments: impl IntoIterator<Item = &'a String>,
) {
    for segment in segments {
        update_hash_segment(hasher, segment);
    }
}
pub(crate) fn update_hash_segment(hasher: &mut Sha256, segment: &str) {
    let len = segment.len() as u64;
    hasher.update(len.to_le_bytes());
    hasher.update(segment.as_bytes());
}
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}
pub(crate) fn serialize_canonical_segment<T: Serialize>(segment: &T) -> String {
    serde_json::to_string(segment).expect("canonical segments should serialize")
}
