//! Canonical (shortest-form) LEB128 varuint encoding, shared by the final
//! compact format's chunk framing and metadata tables.
//!
//! Encoding must be the shortest possible representation; decoders must
//! reject overlong encodings.

use anyhow::{bail, Context, Result};

/// Append the canonical varuint encoding of `value` to `out`.
pub fn encode_varuint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
}

/// Decode a canonical varuint starting at `bytes[offset]`.
///
/// Returns the decoded value and the number of bytes consumed. Rejects
/// truncated input, overlong (non-canonical) encodings, and encodings
/// longer than ten bytes.
pub fn decode_varuint(bytes: &[u8], offset: usize, label: &str) -> Result<(u64, usize)> {
    let mut value = 0u64;
    let mut shift = 0u32;

    for byte_index in 0..10 {
        let byte = *bytes
            .get(offset + byte_index)
            .with_context(|| format!("{label}: truncated varuint"))?;
        let payload = u64::from(byte & 0x7f);

        if shift == 63 && payload > 1 {
            bail!("{label}: varuint exceeds u64 range");
        }

        value |= payload
            .checked_shl(shift)
            .with_context(|| format!("{label}: varuint shift overflow"))?;

        if byte & 0x80 == 0 {
            let consumed = byte_index + 1;

            if consumed > 1 {
                let minimum = 1u64 << (7 * (consumed - 1));
                if value < minimum {
                    bail!("{label}: non-canonical overlong varuint encoding");
                }
            }

            return Ok((value, consumed));
        }

        shift += 7;

        if byte_index == 9 {
            bail!("{label}: varuint exceeds ten-byte limit");
        }
    }

    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(value: u64) {
        let mut out = Vec::new();
        encode_varuint(&mut out, value);
        let (decoded, consumed) = decode_varuint(&out, 0, "test").unwrap();
        assert_eq!(decoded, value);
        assert_eq!(consumed, out.len());
    }

    #[test]
    fn roundtrips_boundary_values() {
        for value in [0u64, 1, 127, 128, 16383, 16384, u32::MAX as u64, u64::MAX] {
            roundtrip(value);
        }
    }

    #[test]
    fn shortest_encoding_for_zero_is_one_byte() {
        let mut out = Vec::new();
        encode_varuint(&mut out, 0);
        assert_eq!(out, vec![0x00]);
    }

    #[test]
    fn rejects_truncated_varuint() {
        let err = decode_varuint(&[0x80], 0, "test").unwrap_err();
        assert!(err.to_string().contains("truncated"));
    }

    #[test]
    fn rejects_overlong_varuint() {
        // 1 encoded as two bytes (0x81, 0x00) instead of the canonical one byte (0x01).
        let err = decode_varuint(&[0x81, 0x00], 0, "test").unwrap_err();
        assert!(err.to_string().contains("non-canonical"));
    }

    #[test]
    fn rejects_eleven_byte_varuint() {
        let bytes = [0x80u8; 11];
        let err = decode_varuint(&bytes, 0, "test").unwrap_err();
        assert!(err.to_string().contains("ten-byte limit"));
    }

    #[test]
    fn decodes_at_nonzero_offset() {
        let mut out = vec![0xff, 0xff];
        encode_varuint(&mut out, 300);
        let (decoded, consumed) = decode_varuint(&out, 2, "test").unwrap();
        assert_eq!(decoded, 300);
        assert_eq!(consumed, 2);
    }
}
