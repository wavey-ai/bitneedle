// Copyright © Wavey, Inc.
// Licensed under the Wavey Artist Source Licence.
// Patent pending. All patent rights are reserved except as expressly granted by the licence.
// Commercial licensing: licence@yl.vin

//! Authoring-side construction of `GAP1` inter-track silence payloads.
//!
//! The wire format, decode-side validation, and sizing geometry live in
//! `record-core::gap`; this module builds the deterministic filler bytes and
//! the canonical entry that decoder crates read back.

use anyhow::{ensure, Result};
use record_core::gap::decode_gap_header;
use record_core::gap::{
    fill_gap_quiet_filler, gap_payload_byte_length, gap_sample_count_from_seconds,
    GapRenderContext, GAP_FLAG_PATTERNIZED, GAP_HEADER_LENGTH, GAP_MAGIC, GAP_VERSION,
};

/// Derive a deterministic GAP seed from stable construction context. Mixing all
/// available identifiers keeps successive gaps in one record visually distinct
/// while remaining fully reproducible — no process randomness is involved.
pub fn derive_gap_seed(
    record_identity: &[u8],
    musical_boundary_index: u64,
    gap_ordinal: u64,
    record_profile: &str,
    sample_count: u64,
) -> u32 {
    // 64-bit FNV-1a over the canonical construction context, folded to u32.
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = FNV_OFFSET;
    let mut mix = |bytes: &[u8]| {
        for &byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
    };

    mix(record_identity);
    mix(&musical_boundary_index.to_be_bytes());
    mix(&gap_ordinal.to_be_bytes());
    mix(record_profile.as_bytes());
    mix(&sample_count.to_be_bytes());

    let folded = (hash ^ (hash >> 32)) as u32;
    // Keep the seed away from the xorshift zero fixed point.
    if folded == 0 {
        0x9E37_79B9
    } else {
        folded
    }
}

/// Encode a canonical `GAP1` payload of exactly `payload_byte_length` bytes.
pub fn encode_gap_payload(
    sample_count: u64,
    payload_byte_length: usize,
    seed: u32,
) -> Result<Vec<u8>> {
    ensure!(
        sample_count > 0,
        "GAP sample count must be greater than zero"
    );
    ensure!(
        payload_byte_length >= GAP_HEADER_LENGTH,
        "GAP payload byte length must be at least the GAP1 header length"
    );

    let declared = payload_byte_length as u64;

    let mut out = Vec::with_capacity(payload_byte_length);
    out.extend_from_slice(GAP_MAGIC);
    out.push(GAP_VERSION);
    out.push(0); // flags
    out.extend_from_slice(&[0u8, 0u8]); // reserved
    out.extend_from_slice(&sample_count.to_be_bytes());
    out.extend_from_slice(&declared.to_be_bytes());
    out.extend_from_slice(&seed.to_be_bytes());

    debug_assert_eq!(out.len(), GAP_HEADER_LENGTH);

    out.resize(payload_byte_length, 0);
    fill_gap_quiet_filler(seed, &mut out[GAP_HEADER_LENGTH..]);

    Ok(out)
}

/// High-level constructor: build a canonical `GAP1` payload from a duration in
/// seconds and the per-record sizing context, deriving the sample count, total
/// byte length, and deterministic seed in one place.
#[allow(clippy::too_many_arguments)]
pub fn build_gap_payload(
    duration_seconds: f64,
    sample_rate: u32,
    record_profile: &str,
    render_context: &GapRenderContext,
    record_identity: &[u8],
    musical_boundary_index: u64,
    gap_ordinal: u64,
) -> Result<Vec<u8>> {
    let sample_count = gap_sample_count_from_seconds(duration_seconds, sample_rate)?;
    let payload_byte_length =
        gap_payload_byte_length(duration_seconds, record_profile, render_context)?;
    let seed = derive_gap_seed(
        record_identity,
        musical_boundary_index,
        gap_ordinal,
        record_profile,
        sample_count,
    );
    encode_gap_payload(sample_count, payload_byte_length, seed)
}

/// Mark an already-encoded `GAP1` payload as patternized in place, setting
/// `GAP_FLAG_PATTERNIZED` in its header. The filler bytes are left untouched;
/// the actual reordering is performed later on the toned pixels, and the chunk
/// CRC32 is (re)issued over the final bytes. Validates the header first so a
/// malformed buffer is rejected rather than silently stamped.
pub fn mark_payload_patternized(payload: &mut [u8]) -> Result<()> {
    use anyhow::Context;
    // Ensures magic/version/length are well-formed before mutating.
    decode_gap_header(payload).context("cannot mark a malformed GAP payload as patternized")?;
    payload[5] |= GAP_FLAG_PATTERNIZED;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use record_core::gap::validate_gap_payload;

    fn ctx() -> GapRenderContext {
        GapRenderContext::for_profile("single45").unwrap()
    }

    #[test]
    fn seed_derivation_is_deterministic_and_varies() {
        let a = derive_gap_seed(b"release-1", 0, 0, "single45", 96_000);
        let b = derive_gap_seed(b"release-1", 0, 0, "single45", 96_000);
        assert_eq!(a, b);
        let c = derive_gap_seed(b"release-1", 1, 1, "single45", 96_000);
        assert_ne!(a, c);
        assert_ne!(a, 0);
    }

    #[test]
    fn zero_samples_is_rejected() {
        assert!(encode_gap_payload(0, 64, 1).is_err());
    }

    #[test]
    fn build_gap_payload_validates() {
        let payload =
            build_gap_payload(2.0, 48_000, "single45", &ctx(), b"release-xyz", 0, 0).unwrap();
        let header = validate_gap_payload(&payload).unwrap();
        assert_eq!(header.sample_count, 96_000);
        // 2.0 s is 1.5 single45 revolutions wide.
        let expected = gap_payload_byte_length(2.0, "single45", &ctx()).unwrap();
        assert_eq!(payload.len(), expected);
    }

    #[test]
    fn mark_payload_patternized_rejects_malformed() {
        let mut not_a_gap = vec![0u8; GAP_HEADER_LENGTH];
        assert!(mark_payload_patternized(&mut not_a_gap).is_err());
    }

    #[test]
    fn mark_payload_patternized_round_trips() {
        let mut payload = encode_gap_payload(96_000, 256, 9).unwrap();
        payload[GAP_HEADER_LENGTH..].reverse();
        assert!(validate_gap_payload(&payload).is_err());

        mark_payload_patternized(&mut payload).unwrap();
        let header = validate_gap_payload(&payload).unwrap();
        assert!(header.is_patternized());
    }
}
