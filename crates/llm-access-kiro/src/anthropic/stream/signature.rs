//! Anthropic-shaped thinking-signature synthesis.
//!
//! Kiro exposes summarized thinking text but not Anthropic's encrypted
//! signature. This module emits a deterministic protobuf envelope matching the
//! observed Anthropic/Claude Code field layout.

use std::sync::Arc;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use sha2::{Digest, Sha512};

const THINKING_SIGNATURE_DOMAIN: &[u8] =
    b"staticflow-kiro-anthropic-thinking-signature-anthropic-shape-v6\0";
const PROTECTED_THINKING_SIGNATURE_DOMAIN: &[u8] =
    b"staticflow-kiro-anthropic-protected-thinking-signature-v1\0";
const SHA512_BLOCK_LEN: usize = 128;
const SHA512_OUTPUT_LEN: usize = 64;
/// Protobuf header field-1 value identifying the signature kind.
pub const THINKING_SIGNATURE_HEADER_KIND: u64 = 14;
/// Protobuf header field-3 value identifying the signature mode.
pub const THINKING_SIGNATURE_HEADER_MODE: u64 = 2;
/// Byte length of the header field-5 body block.
pub const THINKING_SIGNATURE_HEADER_BODY_LEN: usize = 64;
/// Byte length of the header field-8 trace block.
pub const THINKING_SIGNATURE_HEADER_TRACE_LEN: usize = 8;
/// Byte length of the inner nonce fields (2 and 3).
pub const THINKING_SIGNATURE_HEADER_NONCE_LEN: usize = 12;
/// Byte length of the inner proof field (4).
pub const THINKING_SIGNATURE_HEADER_PROOF_LEN: usize = 48;
const THINKING_SIGNATURE_BODY_SHORT_LEN: usize = 140;
const THINKING_SIGNATURE_BODY_LONG_LEN: usize = 425;
const THINKING_SIGNATURE_BODY_LONG_THRESHOLD: usize = 192;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThinkingSignatureContext {
    key_id: Arc<str>,
    secret: Arc<str>,
}

impl ThinkingSignatureContext {
    pub fn new(key_id: impl Into<Arc<str>>, secret: impl Into<Arc<str>>) -> Self {
        Self {
            key_id: key_id.into(),
            secret: secret.into(),
        }
    }

    pub fn signature(&self, model: &str, thinking: &str) -> String {
        protected_thinking_signature(model, thinking, self.key_id.as_ref(), self.secret.as_ref())
    }
}

fn encode_proto_varint(mut value: u64, out: &mut Vec<u8>) {
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

fn encode_proto_key(field_number: u32, wire_type: u8, out: &mut Vec<u8>) {
    encode_proto_varint(((field_number as u64) << 3) | u64::from(wire_type), out);
}

fn proto_varint_len(mut value: usize) -> usize {
    let mut len = 1usize;
    while value >= 0x80 {
        value >>= 7;
        len += 1;
    }
    len
}

fn proto_bytes_field_encoded_len(field_number: u32, content_len: usize) -> usize {
    proto_varint_len(((field_number as usize) << 3) | 2)
        + proto_varint_len(content_len)
        + content_len
}

fn encode_proto_varint_field(field_number: u32, value: u64, out: &mut Vec<u8>) {
    encode_proto_key(field_number, 0, out);
    encode_proto_varint(value, out);
}

fn encode_proto_bytes_field(field_number: u32, value: &[u8], out: &mut Vec<u8>) {
    encode_proto_key(field_number, 2, out);
    encode_proto_varint(value.len() as u64, out);
    out.extend_from_slice(value);
}

fn derive_deterministic_signature_bytes(
    model: &str,
    thinking: &str,
    label: &[u8],
    len: usize,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut counter = 0u32;
    while out.len() < len {
        let mut hasher = Sha512::new();
        hasher.update(THINKING_SIGNATURE_DOMAIN);
        hasher.update(label);
        hasher.update([0]);
        hasher.update(model.as_bytes());
        hasher.update([0]);
        hasher.update(thinking.as_bytes());
        hasher.update(counter.to_le_bytes());
        out.extend_from_slice(&hasher.finalize());
        counter = counter.wrapping_add(1);
    }
    out.truncate(len);
    out
}

fn hmac_sha512(secret: &[u8], chunks: &[&[u8]]) -> [u8; SHA512_OUTPUT_LEN] {
    let mut key_block = [0u8; SHA512_BLOCK_LEN];
    if secret.len() > SHA512_BLOCK_LEN {
        let mut hasher = Sha512::new();
        hasher.update(secret);
        key_block[..SHA512_OUTPUT_LEN].copy_from_slice(&hasher.finalize());
    } else {
        key_block[..secret.len()].copy_from_slice(secret);
    }

    let mut inner_pad = [0x36u8; SHA512_BLOCK_LEN];
    let mut outer_pad = [0x5cu8; SHA512_BLOCK_LEN];
    for index in 0..SHA512_BLOCK_LEN {
        inner_pad[index] ^= key_block[index];
        outer_pad[index] ^= key_block[index];
    }

    let mut inner = Sha512::new();
    inner.update(inner_pad);
    for chunk in chunks {
        inner.update(chunk);
    }
    let inner_hash = inner.finalize();

    let mut outer = Sha512::new();
    outer.update(outer_pad);
    outer.update(inner_hash);
    outer.finalize().into()
}

fn derive_protected_signature_bytes(
    model: &str,
    thinking: &str,
    key_id: &str,
    secret: &str,
    label: &[u8],
    len: usize,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut counter = 0u32;
    while out.len() < len {
        let counter_bytes = counter.to_le_bytes();
        let digest = hmac_sha512(secret.as_bytes(), &[
            PROTECTED_THINKING_SIGNATURE_DOMAIN,
            label,
            b"\0",
            key_id.as_bytes(),
            b"\0",
            model.as_bytes(),
            b"\0",
            thinking.as_bytes(),
            &counter_bytes,
        ]);
        out.extend_from_slice(&digest);
        counter = counter.wrapping_add(1);
    }
    out.truncate(len);
    out
}

fn signature_body_target_len(thinking: &str) -> usize {
    if thinking.len() <= THINKING_SIGNATURE_BODY_LONG_THRESHOLD {
        THINKING_SIGNATURE_BODY_SHORT_LEN
    } else {
        THINKING_SIGNATURE_BODY_LONG_LEN
    }
}

fn build_signature_envelope<F>(model: &str, thinking: &str, mut derive: F) -> String
where
    F: FnMut(&str, &str, &[u8], usize) -> Vec<u8>,
{
    let mut header = Vec::new();
    encode_proto_varint_field(1, THINKING_SIGNATURE_HEADER_KIND, &mut header);
    encode_proto_varint_field(3, THINKING_SIGNATURE_HEADER_MODE, &mut header);
    let header_body = derive(model, thinking, b"header-body", THINKING_SIGNATURE_HEADER_BODY_LEN);
    encode_proto_bytes_field(5, &header_body, &mut header);
    encode_proto_bytes_field(6, model.as_bytes(), &mut header);
    encode_proto_varint_field(7, 0, &mut header);
    let header_trace =
        derive(model, thinking, b"header-trace", THINKING_SIGNATURE_HEADER_TRACE_LEN);
    encode_proto_bytes_field(8, &header_trace, &mut header);

    let field_2 = derive(model, thinking, b"field-2", THINKING_SIGNATURE_HEADER_NONCE_LEN);
    let field_3 = derive(model, thinking, b"field-3", THINKING_SIGNATURE_HEADER_NONCE_LEN);
    let field_4 = derive(model, thinking, b"field-4", THINKING_SIGNATURE_HEADER_PROOF_LEN);
    let body_len = signature_body_target_len(thinking);
    let field_5 = derive(model, thinking, b"field-5", body_len);
    let fixed_payload_len = proto_bytes_field_encoded_len(1, header.len())
        + proto_bytes_field_encoded_len(2, field_2.len())
        + proto_bytes_field_encoded_len(3, field_3.len())
        + proto_bytes_field_encoded_len(4, field_4.len())
        + proto_bytes_field_encoded_len(5, field_5.len());

    let mut payload = Vec::new();
    encode_proto_bytes_field(1, &header, &mut payload);
    encode_proto_bytes_field(2, &field_2, &mut payload);
    encode_proto_bytes_field(3, &field_3, &mut payload);
    encode_proto_bytes_field(4, &field_4, &mut payload);
    encode_proto_bytes_field(5, &field_5, &mut payload);
    debug_assert_eq!(payload.len(), fixed_payload_len);

    let mut envelope = Vec::new();
    encode_proto_bytes_field(2, &payload, &mut envelope);
    encode_proto_varint_field(3, 1, &mut envelope);

    STANDARD.encode(envelope)
}

/// Build a deterministic protobuf envelope matching the field layout of recent
/// Anthropic/Claude Code signatures observed locally:
/// outer field-2 payload + outer field-3=1; inner fields 1/2/3/4/5; header
/// fields 1=14, 3=2, 5=64-byte body, 6=model string, 7=0, 8=8-byte trace.
///
/// Kiro exposes summarized thinking text but not Anthropic's encrypted
/// signature. This remains synthetic and is not a cryptographically valid
/// signature.
pub fn synthetic_thinking_signature(model: &str, thinking: &str) -> String {
    build_signature_envelope(model, thinking, |model, thinking, label, len| {
        derive_deterministic_signature_bytes(model, thinking, label, len)
    })
}

/// Build a service-authenticated thinking signature. The output uses the same
/// canonical envelope, while the bytes are derived from a server secret and
/// bound to one StaticFlow key id.
pub fn protected_thinking_signature(
    model: &str,
    thinking: &str,
    key_id: &str,
    secret: &str,
) -> String {
    build_signature_envelope(model, thinking, |model, thinking, label, len| {
        derive_protected_signature_bytes(model, thinking, key_id, secret, label, len)
    })
}

pub fn verify_protected_thinking_signature(
    model: &str,
    thinking: &str,
    key_id: &str,
    secret: &str,
    signature: &str,
) -> bool {
    let expected = protected_thinking_signature(model, thinking, key_id, secret);
    constant_time_eq(expected.as_bytes(), signature.as_bytes())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (left_byte, right_byte) in left.iter().zip(right.iter()) {
        diff |= left_byte ^ right_byte;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::{protected_thinking_signature, verify_protected_thinking_signature};

    #[test]
    fn protected_signature_verifies_only_exact_key_model_and_thinking() {
        let signature = protected_thinking_signature(
            "claude-opus-4-8",
            "private reasoning",
            "kiro-key-1",
            "server-secret",
        );

        assert!(verify_protected_thinking_signature(
            "claude-opus-4-8",
            "private reasoning",
            "kiro-key-1",
            "server-secret",
            &signature,
        ));
        assert!(!verify_protected_thinking_signature(
            "claude-opus-4-8",
            "tampered reasoning",
            "kiro-key-1",
            "server-secret",
            &signature,
        ));
        assert!(!verify_protected_thinking_signature(
            "claude-opus-4-7",
            "private reasoning",
            "kiro-key-1",
            "server-secret",
            &signature,
        ));
        assert!(!verify_protected_thinking_signature(
            "claude-opus-4-8",
            "private reasoning",
            "kiro-key-2",
            "server-secret",
            &signature,
        ));
    }
}
