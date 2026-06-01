//! Small pure helpers shared across the DuckDB submodules (time, gzip,
//! hashing, numeric casts).

use std::{
    io::{BufWriter, Read},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use sha2::{Digest, Sha256};


#[cfg(feature = "duckdb-runtime")]
pub fn i64_to_u64(value: i64) -> u64 {
    u64::try_from(value.max(0)).unwrap_or(u64::MAX)
}
#[cfg(feature = "duckdb-runtime")]
pub fn duckdb_string_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
#[cfg(feature = "duckdb-runtime")]
pub fn duckdb_mib_setting(value_mib: u64) -> String {
    format!("{}MB", value_mib.max(1))
}
#[cfg(feature = "duckdb-runtime")]
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}
#[cfg(feature = "duckdb-runtime")]
pub fn utc_date_parts(timestamp_ms: i64) -> (i32, u32, u32) {
    let datetime = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(timestamp_ms)
        .unwrap_or_else(|| chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).expect("epoch"));
    use chrono::Datelike;
    (datetime.year(), datetime.month(), datetime.day())
}
#[cfg(feature = "duckdb-runtime")]
pub fn gzip_json_bytes<T: serde::Serialize>(value: &T) -> anyhow::Result<Vec<u8>> {
    // Stream JSON straight into the encoder (no intermediate uncompressed Vec),
    // but through a BufWriter: serde_json emits many tiny writes and an
    // unbuffered GzEncoder would run the compressor on each one. Drain via
    // `into_inner()` rather than `flush()` so we don't inject a DEFLATE
    // sync-flush — the encoder then sees the exact same byte stream as a single
    // bulk write, keeping the output bytes identical.
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut writer = BufWriter::new(&mut encoder);
        serde_json::to_writer(&mut writer, value)
            .context("serialize and gzip usage detail json")?;
        writer
            .into_inner()
            .map_err(|err| anyhow::anyhow!("flush gzip usage detail payload: {err}"))?;
    }
    encoder.finish().context("finish gzip usage detail payload")
}
#[cfg(feature = "duckdb-runtime")]
pub fn gunzip_json_bytes<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> anyhow::Result<T> {
    let mut decoder = GzDecoder::new(bytes);
    let mut json = Vec::new();
    decoder
        .read_to_end(&mut json)
        .context("gunzip usage detail payload")?;
    serde_json::from_slice(&json).context("deserialize usage detail json")
}
#[cfg(feature = "duckdb-runtime")]
pub fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}
#[cfg(feature = "duckdb-runtime")]
pub fn i64_to_usize(value: i64) -> usize {
    usize::try_from(value.max(0)).unwrap_or(usize::MAX)
}
#[cfg(feature = "duckdb-runtime")]
pub fn usize_to_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}
