//! Final BRS1 v2 transport chunk framing (section 3).
//!
//! Chunks carry no signature, index, count, or descriptor information —
//! those are derived from chunk order and BRS1 metadata/revolution table.
//! Chunks exist purely for framing and corruption detection (CRC-32).

use crate::varuint::{decode_varuint, encode_varuint};
use crate::{crc32_ieee, read_u32be};
use anyhow::{bail, Result};

pub const ENCRYPTED_CHUNK_NONCE_LENGTH: usize = 12;

/// A decoded chunk's byte ranges within the chunk-section buffer it was
/// parsed from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkRanges {
    pub chunk_start: usize,
    pub chunk_end: usize,
    pub crc32: u32,
    pub nonce: Option<[u8; ENCRYPTED_CHUNK_NONCE_LENGTH]>,
    pub payload_start: usize,
    pub payload_end: usize,
}

/// Encode one unencrypted chunk (section 3.1): `varuint length, crc32, payload`.
pub fn encode_unencrypted_chunk(payload: &[u8]) -> Result<Vec<u8>> {
    if payload.is_empty() {
        bail!("chunk payload must not be empty");
    }
    let mut out = Vec::with_capacity(payload.len() + 8);
    encode_varuint(&mut out, payload.len() as u64);
    out.extend_from_slice(&crc32_ieee(payload).to_be_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

/// Encode one encrypted chunk (section 3.2): `varuint length, crc32, nonce, ciphertext`.
///
/// `crc32` and `length` are computed over `ciphertext`, per section 1.6.
pub fn encode_encrypted_chunk(
    nonce: [u8; ENCRYPTED_CHUNK_NONCE_LENGTH],
    ciphertext: &[u8],
) -> Result<Vec<u8>> {
    if ciphertext.is_empty() {
        bail!("chunk payload must not be empty");
    }
    let mut out = Vec::with_capacity(ciphertext.len() + ENCRYPTED_CHUNK_NONCE_LENGTH + 8);
    encode_varuint(&mut out, ciphertext.len() as u64);
    out.extend_from_slice(&crc32_ieee(ciphertext).to_be_bytes());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(ciphertext);
    Ok(out)
}

/// Parse every chunk in `bytes[offset..]`, requiring the chunks to consume
/// `bytes` exactly to its end. Rejects truncated chunks, overlong varuints,
/// zero-length chunks, and trailing bytes.
pub fn parse_chunk_section(bytes: &[u8], offset: usize, encrypted: bool) -> Result<Vec<ChunkRanges>> {
    let mut ranges = Vec::new();
    let mut cursor = offset;

    while cursor < bytes.len() {
        let (payload_length, varuint_len) = decode_varuint(bytes, cursor, "chunk payload length")?;
        if payload_length == 0 {
            bail!("chunk at byte {cursor} has zero payload length");
        }
        let crc32_start = cursor + varuint_len;
        let crc32 = read_u32be(bytes, crc32_start, "chunk CRC32")?;

        let (nonce, payload_start) = if encrypted {
            let nonce_start = crc32_start + 4;
            let nonce_end = nonce_start + ENCRYPTED_CHUNK_NONCE_LENGTH;
            let nonce_bytes = bytes
                .get(nonce_start..nonce_end)
                .ok_or_else(|| anyhow::anyhow!("chunk at byte {cursor} nonce is truncated"))?;
            let mut nonce = [0u8; ENCRYPTED_CHUNK_NONCE_LENGTH];
            nonce.copy_from_slice(nonce_bytes);
            (Some(nonce), nonce_end)
        } else {
            (None, crc32_start + 4)
        };

        let payload_length = payload_length as usize;
        let payload_end = payload_start
            .checked_add(payload_length)
            .ok_or_else(|| anyhow::anyhow!("chunk at byte {cursor} length overflows"))?;
        if payload_end > bytes.len() {
            bail!("chunk at byte {cursor} payload is truncated");
        }

        ranges.push(ChunkRanges {
            chunk_start: cursor,
            chunk_end: payload_end,
            crc32,
            nonce,
            payload_start,
            payload_end,
        });

        cursor = payload_end;
    }

    Ok(ranges)
}

/// Verify a parsed chunk's CRC-32 against the bytes it covers (ciphertext
/// for encrypted chunks, plaintext payload for unencrypted ones).
pub fn verify_chunk_crc32(bytes: &[u8], chunk: &ChunkRanges) -> bool {
    crc32_ieee(&bytes[chunk.payload_start..chunk.payload_end]) == chunk.crc32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unencrypted_chunk_has_no_signature_bytes() {
        let payload = b"hello revolution";
        let encoded = encode_unencrypted_chunk(payload).unwrap();
        // varuint(16) = 1 byte + crc32 4 bytes + 16 payload bytes = 21 bytes total.
        assert_eq!(encoded.len(), 1 + 4 + payload.len());
    }

    #[test]
    fn chunk_order_determines_index() {
        let a = encode_unencrypted_chunk(b"first").unwrap();
        let b = encode_unencrypted_chunk(b"second").unwrap();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&a);
        bytes.extend_from_slice(&b);

        let chunks = parse_chunk_section(&bytes, 0, false).unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(&bytes[chunks[0].payload_start..chunks[0].payload_end], b"first");
        assert_eq!(&bytes[chunks[1].payload_start..chunks[1].payload_end], b"second");
    }

    #[test]
    fn parser_rejects_trailing_bytes() {
        let mut bytes = encode_unencrypted_chunk(b"payload").unwrap();
        // A trailing 0x01 looks like the start of a new chunk's varuint
        // length, but there are no bytes left for its CRC-32 or payload.
        bytes.push(0x01);
        let err = parse_chunk_section(&bytes, 0, false).unwrap_err();
        assert!(err.to_string().contains("truncated"));
    }

    #[test]
    fn parser_rejects_truncated_varuint() {
        let bytes = [0x80u8];
        let err = parse_chunk_section(&bytes, 0, false).unwrap_err();
        assert!(err.to_string().contains("truncated") || err.to_string().contains("varuint"));
    }

    #[test]
    fn parser_rejects_overlong_varuint() {
        let mut bytes = vec![0x81, 0x00]; // overlong encoding of 1
        bytes.extend_from_slice(&crc32_ieee(b"x").to_be_bytes());
        bytes.push(b'x');
        let err = parse_chunk_section(&bytes, 0, false).unwrap_err();
        assert!(err.to_string().contains("non-canonical"));
    }

    #[test]
    fn parser_rejects_zero_length_chunk() {
        let mut bytes = Vec::new();
        encode_varuint(&mut bytes, 0);
        bytes.extend_from_slice(&0u32.to_be_bytes());
        let err = parse_chunk_section(&bytes, 0, false).unwrap_err();
        assert!(err.to_string().contains("zero payload length"));
    }

    #[test]
    fn encode_rejects_zero_length_payload() {
        assert!(encode_unencrypted_chunk(b"").is_err());
    }

    #[test]
    fn crc_detects_modified_payload() {
        let mut bytes = encode_unencrypted_chunk(b"original").unwrap();
        let chunks = parse_chunk_section(&bytes, 0, false).unwrap();
        assert!(verify_chunk_crc32(&bytes, &chunks[0]));

        let payload_start = chunks[0].payload_start;
        bytes[payload_start] ^= 0xff;
        let chunks = parse_chunk_section(&bytes, 0, false).unwrap();
        assert!(!verify_chunk_crc32(&bytes, &chunks[0]));
    }

    #[test]
    fn encrypted_crc_covers_ciphertext() {
        let nonce = [7u8; ENCRYPTED_CHUNK_NONCE_LENGTH];
        let ciphertext = b"opaque ciphertext bytes";
        let bytes = encode_encrypted_chunk(nonce, ciphertext).unwrap();

        let chunks = parse_chunk_section(&bytes, 0, true).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].nonce, Some(nonce));
        assert_eq!(
            &bytes[chunks[0].payload_start..chunks[0].payload_end],
            ciphertext
        );
        assert!(verify_chunk_crc32(&bytes, &chunks[0]));
        assert_eq!(chunks[0].crc32, crc32_ieee(ciphertext));
    }

    #[test]
    fn parser_rejects_truncated_chunk_payload() {
        let mut bytes = encode_unencrypted_chunk(b"original payload").unwrap();
        bytes.truncate(bytes.len() - 3);
        let err = parse_chunk_section(&bytes, 0, false).unwrap_err();
        assert!(err.to_string().contains("truncated"));
    }
}
