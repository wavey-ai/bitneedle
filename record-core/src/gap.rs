//! First-class inter-track silence: the `GAP1` codec.
//!
//! A GAP is intentional PCM silence in the playable record timeline. It is a
//! first-class payload entry and a first-class rendered groove region, but it is
//! not a musical track: it has no title and no track number, and is never
//! covered by a [`crate::TrackDescriptor`] range.
//!
//! Unlike the placeholder representation it replaces (a bare `u64be` sample
//! count, with the gap duration effectively living in descriptor geometry), a
//! GAP entry carries a self-describing, versioned `GAP1` payload. The payload is
//! authoritative for the sample count, its own total byte length, and a
//! deterministic filler seed. The filler exists so the GAP occupies real carrier
//! bytes — and therefore real groove pixels — producing a narrow, visually quiet
//! band that round-trips exactly through the PNG.
//!
//! The canonical layout is:
//!
//! ```text
//! magic                 4 bytes   "GAP1"
//! version               1 byte    1
//! flags                 1 byte    0
//! reserved              2 bytes   0
//! sample_count          8 bytes   u64be   (PCM samples per channel)
//! payload_byte_length   8 bytes   u64be   (total entry length, header + filler)
//! seed                  4 bytes   u32be   (deterministic filler seed)
//! filler                remaining bytes    (xorshift32(seed) keystream)
//! ```

use anyhow::{bail, ensure, Context, Result};

/// Four-byte payload magic identifying a versioned GAP entry.
pub const GAP_MAGIC: &[u8; 4] = b"GAP1";
/// Current GAP payload version.
pub const GAP_VERSION: u8 = 1;

/// `flags` bit marking a GAP entry whose filler has been *patternized* — i.e.
/// the filler pixels were reordered after toning for visual effect, so the
/// filler bytes no longer equal the raw `xorshift32(seed)` keystream. When set,
/// the entry's integrity is guaranteed by its enclosing chunk CRC32 alone
/// (exactly like every other, non-GAP chunk's payload), and the deterministic
/// keystream-equality check in [`validate_gap_payload`] is intentionally
/// skipped. The seed is retained as construction provenance, not as an
/// integrity assertion.
pub const GAP_FLAG_PATTERNIZED: u8 = 0x01;

/// Mask of every `flags` bit this version understands; any other bit set means
/// the entry was produced by a newer writer and must be rejected.
const GAP_KNOWN_FLAGS: u8 = GAP_FLAG_PATTERNIZED;

/// Byte length of the fixed `GAP1` header preceding the deterministic filler.
pub const GAP_HEADER_LENGTH: usize = 4 // magic
    + 1 // version
    + 1 // flags
    + 2 // reserved
    + 8 // sample_count
    + 8 // payload_byte_length
    + 4; // seed

/// RGB24 carrier packing: three payload bytes per groove pixel.
pub const GAP_BYTES_PER_PIXEL: f64 = 3.0;

/// Defensive cap on a GAP's visible width, in groove revolutions, so a
/// pathologically long gap cannot swallow the whole record.
pub const MAX_GAP_REVOLUTIONS: f64 = 8.0;

/// Minimum visible width, in groove revolutions, so a very short but non-zero
/// gap still renders a clearly perceptible ring rather than a hairline.
pub const MIN_GAP_REVOLUTIONS: f64 = 0.25;

/// The GAP band is painted as a single quiet tone — the median of the
/// surrounding high-entropy audio carrier (which averages to mid-grey) — so it
/// reads as a smooth inter-track boundary like a real record. The filler bytes
/// *are* this tone (centered here, with a small deterministic dither), so the
/// band is quiet and consistent while still round-tripping exactly through the
/// PNG.
pub const GAP_QUIET_TONE: u8 = 128;
/// Peak deterministic deviation, per channel byte, from [`GAP_QUIET_TONE`].
pub const GAP_QUIET_VARIATION: u8 = 6;

/// Defensive upper bound on a single GAP's sample count, guarding against
/// pathological durations producing impossible carrier sizes. One hour per
/// channel at 192 kHz is far beyond any musically reasonable inter-track gap.
pub const MAX_GAP_SAMPLE_COUNT: u64 = 192_000 * 3_600;

/// Parsed `GAP1` header fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GapHeader {
    /// PCM samples of silence per channel.
    pub sample_count: u64,
    /// Declared total length of the GAP payload entry, header plus filler.
    pub payload_byte_length: u64,
    /// Deterministic filler seed.
    pub seed: u32,
    /// Entry flags (see [`GAP_FLAG_PATTERNIZED`]).
    pub flags: u8,
}

impl GapHeader {
    /// Whether this entry's filler has been patternized (see
    /// [`GAP_FLAG_PATTERNIZED`]).
    pub fn is_patternized(&self) -> bool {
        self.flags & GAP_FLAG_PATTERNIZED != 0
    }
}

/// Profile-derived geometry used to size a GAP's visible band so that its width
/// tracks playback time: a gap of one revolution's duration occupies one full
/// turn of groove, wherever it falls.
///
/// * `seconds_per_revolution` is the profile's physical revolution duration
///   (single45 = 4/3 s, lp = 9/5 s) — the same constant the rotation clock uses.
/// * `pixels_per_revolution` is the carrier-pixel count in one full turn at the
///   mean payload radius (`2π·r_mid`); the spiral rasterizes ~one pixel per unit
///   arc length, so this is a position-independent estimate of one turn.
#[derive(Debug, Clone, Copy)]
pub struct GapRenderContext {
    pub seconds_per_revolution: f64,
    pub pixels_per_revolution: f64,
}

impl GapRenderContext {
    /// Build the render context for a known record profile from its groove
    /// geometry.
    pub fn for_profile(record_profile: &str) -> Result<Self> {
        let geometry = crate::describe_record_profile(record_profile)?;
        let r_mid = (f64::from(geometry.payload_inner_radius)
            + f64::from(geometry.payload_outer_radius))
            / 2.0;
        Ok(Self {
            seconds_per_revolution: seconds_per_revolution(record_profile)?,
            pixels_per_revolution: 2.0 * std::f64::consts::PI * r_mid,
        })
    }
}

/// Physical revolution duration, in seconds, for a record profile. One full turn
/// of groove represents exactly this much playback time (single45 spins at
/// 45 RPM → 4/3 s; lp at 33⅓ RPM → 9/5 s).
pub fn seconds_per_revolution(record_profile: &str) -> Result<f64> {
    match record_profile {
        "single45" => Ok(4.0 / 3.0),
        "lp" => Ok(9.0 / 5.0),
        other => bail!("unknown record profile: {other}"),
    }
}

/// Convert a user-supplied gap duration in seconds to an exact PCM sample count
/// using the canonical round-half-up rule.
pub fn gap_sample_count_from_seconds(duration_seconds: f64, sample_rate: u32) -> Result<u64> {
    ensure!(
        duration_seconds.is_finite(),
        "GAP duration must be a finite number of seconds"
    );
    ensure!(
        duration_seconds > 0.0,
        "GAP duration must be greater than zero seconds"
    );
    ensure!(
        sample_rate > 0,
        "record sample rate must be greater than zero"
    );

    // Canonical rounding rule across all crates: round half up.
    let exact = duration_seconds * f64::from(sample_rate);
    let rounded = (exact + 0.5).floor();

    ensure!(
        rounded.is_finite() && rounded >= 1.0,
        "GAP duration rounds to zero samples"
    );
    ensure!(
        rounded <= MAX_GAP_SAMPLE_COUNT as f64,
        "GAP duration exceeds the maximum supported sample count"
    );

    Ok(rounded as u64)
}

/// Visible width of a GAP band, in groove revolutions, for a given duration.
/// This is the single source of truth for the duration→width mapping; the
/// preview UI consumes it through WASM rather than reimplementing it. A gap of
/// one revolution's duration is exactly one turn wide; the result is capped at
/// [`MAX_GAP_REVOLUTIONS`].
pub fn gap_revolutions(duration_seconds: f64, render_context: &GapRenderContext) -> Result<f64> {
    ensure!(
        duration_seconds.is_finite() && duration_seconds > 0.0,
        "GAP duration must be a positive finite number of seconds"
    );
    ensure!(
        render_context.seconds_per_revolution > 0.0,
        "render context has a non-positive revolution duration"
    );

    let revolutions = duration_seconds / render_context.seconds_per_revolution;
    Ok(revolutions.clamp(MIN_GAP_REVOLUTIONS, MAX_GAP_REVOLUTIONS))
}

/// Canonical total payload byte length for a GAP of the given duration on the
/// given record profile. The band spans `gap_revolutions` full turns of groove,
/// so the byte budget is `revolutions × pixels_per_revolution × 3`. Always at
/// least [`GAP_HEADER_LENGTH`] so the header fits even for the smallest gaps.
pub fn gap_payload_byte_length(
    duration_seconds: f64,
    _record_profile: &str,
    render_context: &GapRenderContext,
) -> Result<usize> {
    let revolutions = gap_revolutions(duration_seconds, render_context)?;
    let pixels = (revolutions * render_context.pixels_per_revolution).round();
    let budget = (pixels * GAP_BYTES_PER_PIXEL).round();

    ensure!(
        budget.is_finite() && budget >= 0.0,
        "GAP payload budget is not representable"
    );

    Ok((budget as usize).max(GAP_HEADER_LENGTH))
}

/// `xorshift32` deterministic byte generator. Cheap, requires no external
/// randomness, and reproduces identical bytes for identical seeds.
struct XorShift32 {
    state: u32,
}

impl XorShift32 {
    fn new(seed: u32) -> Self {
        // Avoid the zero fixed point, which would emit an all-zero stream.
        Self {
            state: if seed == 0 { 0x9E37_79B9 } else { seed },
        }
    }

    fn next_u32(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }

    /// Fill `out` with a quiet, consistent tone centered on [`GAP_QUIET_TONE`]
    /// with at most ±[`GAP_QUIET_VARIATION`] deterministic per-byte dither. The
    /// result reads as a smooth mid-grey band against the colourful audio
    /// carrier, yet is fully reproducible from the seed.
    fn fill_quiet(&mut self, out: &mut [u8]) {
        let span = u32::from(GAP_QUIET_VARIATION) * 2 + 1;
        for byte in out.iter_mut() {
            let delta = (self.next_u32() % span) as i32 - i32::from(GAP_QUIET_VARIATION);
            *byte = (i32::from(GAP_QUIET_TONE) + delta).clamp(0, 255) as u8;
        }
    }
}

/// Fill `out` with the deterministic quiet-tone filler for the given seed.
/// Exposed so authoring code (see `record-cut`) can produce filler bytes that
/// this crate's [`validate_gap_payload`] will accept; the keystream itself is
/// already documented above and carries no additional disclosure.
pub fn fill_gap_quiet_filler(seed: u32, out: &mut [u8]) {
    XorShift32::new(seed).fill_quiet(out);
}

/// Parse and validate the `GAP1` header without re-deriving the filler.
pub fn decode_gap_header(bytes: &[u8]) -> Result<GapHeader> {
    ensure!(
        bytes.len() >= GAP_HEADER_LENGTH,
        "GAP payload is shorter than the GAP1 header"
    );
    ensure!(&bytes[0..4] == GAP_MAGIC, "GAP payload magic is invalid");

    let version = bytes[4];
    ensure!(
        version == GAP_VERSION,
        "unsupported GAP payload version {version}"
    );

    let flags = bytes[5];
    ensure!(
        flags & !GAP_KNOWN_FLAGS == 0,
        "GAP payload flags has unsupported bits set: {flags:#04x}"
    );
    ensure!(
        bytes[6] == 0 && bytes[7] == 0,
        "GAP payload reserved bytes must be zero"
    );

    let sample_count = u64::from_be_bytes(bytes[8..16].try_into().expect("length checked"));
    ensure!(
        sample_count > 0,
        "GAP sample count must be greater than zero"
    );

    let payload_byte_length = u64::from_be_bytes(bytes[16..24].try_into().expect("length checked"));

    let seed = u32::from_be_bytes(bytes[24..28].try_into().expect("length checked"));

    Ok(GapHeader {
        sample_count,
        payload_byte_length,
        seed,
        flags,
    })
}

/// Fully validate a `GAP1` payload: header well-formedness, declared length
/// matching the actual entry length, and deterministic filler integrity.
pub fn validate_gap_payload(bytes: &[u8]) -> Result<GapHeader> {
    let header = decode_gap_header(bytes)?;

    ensure!(
        header.payload_byte_length == bytes.len() as u64,
        "GAP declared payload length {} does not match entry length {}",
        header.payload_byte_length,
        bytes.len()
    );

    // A patternized GAP's filler is a post-toning reordering of the keystream,
    // so it deliberately no longer equals `xorshift32(seed)`. Such an entry is
    // trusted exactly like every other chunk's payload: by the enclosing chunk
    // CRC32, with no extra determinism assertion. Only the original
    // quiet-keystream form is verified byte-for-byte here.
    if !header.is_patternized() {
        // Verify the deterministic filler so a strict parser rejects tampered or
        // corrupted carrier bytes that the enclosing integrity check might miss.
        let mut generator = XorShift32::new(header.seed);
        let mut expected = vec![0u8; bytes.len() - GAP_HEADER_LENGTH];
        generator.fill_quiet(&mut expected);
        ensure!(
            &bytes[GAP_HEADER_LENGTH..] == expected.as_slice(),
            "GAP deterministic filler does not match the declared seed"
        );
    }

    Ok(header)
}

/// Read a GAP entry's silence sample count from its payload, validating the
/// header. Used by pre-decode duration inspection.
pub fn gap_sample_count(bytes: &[u8]) -> Result<u64> {
    Ok(decode_gap_header(bytes)
        .context("invalid GAP payload")?
        .sample_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> GapRenderContext {
        GapRenderContext::for_profile("single45").unwrap()
    }

    /// Test-only `GAP1` encoder mirroring `record-cut::gap::encode_gap_payload`,
    /// kept here so this crate's decode-side tests don't need an authoring
    /// dependency. Any change to the wire layout must be mirrored there.
    fn test_encode_gap_payload(
        sample_count: u64,
        payload_byte_length: usize,
        seed: u32,
    ) -> Vec<u8> {
        assert!(sample_count > 0);
        assert!(payload_byte_length >= GAP_HEADER_LENGTH);

        let declared = payload_byte_length as u64;
        let mut out = Vec::with_capacity(payload_byte_length);
        out.extend_from_slice(GAP_MAGIC);
        out.push(GAP_VERSION);
        out.push(0); // flags
        out.extend_from_slice(&[0u8, 0u8]); // reserved
        out.extend_from_slice(&sample_count.to_be_bytes());
        out.extend_from_slice(&declared.to_be_bytes());
        out.extend_from_slice(&seed.to_be_bytes());

        out.resize(payload_byte_length, 0);
        fill_gap_quiet_filler(seed, &mut out[GAP_HEADER_LENGTH..]);
        out
    }

    fn test_mark_payload_patternized(payload: &mut [u8]) {
        decode_gap_header(payload).unwrap();
        payload[5] |= GAP_FLAG_PATTERNIZED;
    }

    #[test]
    fn seconds_to_samples_round_half_up() {
        // 2.0 s at 48 kHz is exact.
        assert_eq!(gap_sample_count_from_seconds(2.0, 48_000).unwrap(), 96_000);
        // Exactly half a sample rounds up: 0.5 s * 3 Hz = 1.5 -> 2.
        assert_eq!(gap_sample_count_from_seconds(0.5, 3).unwrap(), 2);
        // Just below half a sample rounds down: 1.4 -> 1.
        assert_eq!(gap_sample_count_from_seconds(1.4, 1).unwrap(), 1);
        // Just above half a sample rounds up: 2.6 -> 3.
        assert_eq!(gap_sample_count_from_seconds(2.6, 1).unwrap(), 3);
    }

    #[test]
    fn seconds_to_samples_rejects_bad_input() {
        assert!(gap_sample_count_from_seconds(f64::NAN, 48_000).is_err());
        assert!(gap_sample_count_from_seconds(f64::INFINITY, 48_000).is_err());
        assert!(gap_sample_count_from_seconds(0.0, 48_000).is_err());
        assert!(gap_sample_count_from_seconds(-1.0, 48_000).is_err());
        assert!(gap_sample_count_from_seconds(2.0, 0).is_err());
        // Rounds to zero samples.
        assert!(gap_sample_count_from_seconds(1e-9, 1).is_err());
    }

    #[test]
    fn gap_width_tracks_time_as_revolutions() {
        let c = ctx();
        // One revolution's duration (4/3 s for single45) is exactly one turn.
        let one_rev = gap_revolutions(4.0 / 3.0, &c).unwrap();
        assert!((one_rev - 1.0).abs() < 1e-9);
        // 2 s is 1.5 turns; 2/3 s is half a turn.
        assert!((gap_revolutions(2.0, &c).unwrap() - 1.5).abs() < 1e-9);
        assert!((gap_revolutions(2.0 / 3.0, &c).unwrap() - 0.5).abs() < 1e-9);
        // Long gaps are capped.
        assert_eq!(gap_revolutions(1_000.0, &c).unwrap(), MAX_GAP_REVOLUTIONS);
    }

    #[test]
    fn payload_byte_length_is_one_revolution_of_pixels_per_revolution_duration() {
        let c = ctx();
        // A one-revolution-duration gap occupies ~one turn of carrier pixels.
        let bytes = gap_payload_byte_length(4.0 / 3.0, "single45", &c).unwrap();
        let expected = (c.pixels_per_revolution * GAP_BYTES_PER_PIXEL).round() as usize;
        assert!(bytes.abs_diff(expected) <= 3, "{bytes} ~= {expected}");

        // Width scales with duration and is bounded.
        let half = gap_payload_byte_length(2.0 / 3.0, "single45", &c).unwrap();
        let long = gap_payload_byte_length(1_000.0, "single45", &c).unwrap();
        assert!(half < bytes, "shorter gap is narrower");
        assert!(long > bytes, "longer gap is wider");
        let cap =
            (MAX_GAP_REVOLUTIONS * c.pixels_per_revolution * GAP_BYTES_PER_PIXEL).round() as usize;
        assert!(long.abs_diff(cap) <= 3, "{long} ~= cap {cap}");
        assert!(half >= GAP_HEADER_LENGTH);
    }

    #[test]
    fn gap_filler_is_a_quiet_consistent_tone() {
        let payload = test_encode_gap_payload(96_000, 3_000, 0);
        for &byte in &payload[GAP_HEADER_LENGTH..] {
            let delta = (i32::from(byte) - i32::from(GAP_QUIET_TONE)).unsigned_abs();
            assert!(
                delta <= u32::from(GAP_QUIET_VARIATION),
                "byte {byte} within quiet band"
            );
        }
    }

    #[test]
    fn encode_decode_round_trip() {
        let payload = test_encode_gap_payload(96_000, 3_000, 0xABCD_1234);
        assert_eq!(payload.len(), 3_000);
        assert_eq!(&payload[0..4], GAP_MAGIC);

        let header = validate_gap_payload(&payload).unwrap();
        assert_eq!(header.sample_count, 96_000);
        assert_eq!(header.payload_byte_length, 3_000);
        assert_eq!(header.seed, 0xABCD_1234);
    }

    #[test]
    fn deterministic_filler_is_reproducible() {
        let a = test_encode_gap_payload(1_000, 512, 42);
        let b = test_encode_gap_payload(1_000, 512, 42);
        assert_eq!(a, b);
        // Different seed yields different filler.
        let c = test_encode_gap_payload(1_000, 512, 43);
        assert_ne!(a, c);
    }

    #[test]
    fn header_only_payload_has_no_filler() {
        let payload = test_encode_gap_payload(10, GAP_HEADER_LENGTH, 7);
        assert_eq!(payload.len(), GAP_HEADER_LENGTH);
        validate_gap_payload(&payload).unwrap();
    }

    #[test]
    fn malformed_magic_is_rejected() {
        let mut payload = test_encode_gap_payload(96_000, 64, 1);
        payload[0] = b'X';
        assert!(decode_gap_header(&payload).is_err());
        assert!(validate_gap_payload(&payload).is_err());
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let mut payload = test_encode_gap_payload(96_000, 64, 1);
        payload[4] = 2;
        assert!(decode_gap_header(&payload).is_err());
    }

    #[test]
    fn zero_samples_is_rejected() {
        // A hand-built header with zero samples fails validation.
        let mut payload = test_encode_gap_payload(1, 64, 1);
        payload[8..16].copy_from_slice(&0u64.to_be_bytes());
        assert!(decode_gap_header(&payload).is_err());
    }

    #[test]
    fn declared_length_mismatch_is_rejected() {
        let payload = test_encode_gap_payload(96_000, 128, 5);
        assert!(validate_gap_payload(&payload[..127]).is_err());
        // Truncating below the header is also rejected.
        assert!(decode_gap_header(&payload[..GAP_HEADER_LENGTH - 1]).is_err());
    }

    #[test]
    fn corrupted_filler_is_rejected() {
        let mut payload = test_encode_gap_payload(96_000, 256, 9);
        let last = payload.len() - 1;
        payload[last] ^= 0xFF;
        assert!(validate_gap_payload(&payload).is_err());
        // ...but the lenient header decode still succeeds.
        assert!(decode_gap_header(&payload).is_ok());
    }

    #[test]
    fn patternized_flag_skips_keystream_check_but_keeps_structural_checks() {
        // A patternized entry: reorder the filler so it no longer equals the
        // keystream, then stamp the flag.
        let mut payload = test_encode_gap_payload(96_000, 256, 9);
        payload[GAP_HEADER_LENGTH..].reverse();
        // Without the flag, the reordered filler is rejected.
        assert!(validate_gap_payload(&payload).is_err());

        test_mark_payload_patternized(&mut payload);
        let header = validate_gap_payload(&payload).unwrap();
        assert!(header.is_patternized());
        assert_eq!(header.flags, GAP_FLAG_PATTERNIZED);

        // Structural checks still bite even when patternized: a declared-length
        // mismatch is still rejected.
        assert!(validate_gap_payload(&payload[..255]).is_err());
        // ...and a corrupted magic is still rejected.
        let mut broken = payload.clone();
        broken[0] = b'X';
        assert!(decode_gap_header(&broken).is_err());
    }

    #[test]
    fn unknown_flag_bits_are_rejected() {
        let mut payload = test_encode_gap_payload(96_000, 64, 1);
        payload[5] = 0x80; // a bit this version does not understand
        assert!(decode_gap_header(&payload).is_err());
        // The defined patternized bit, by contrast, is accepted.
        payload[5] = GAP_FLAG_PATTERNIZED;
        assert!(decode_gap_header(&payload).is_ok());
    }

    #[test]
    fn filler_never_starts_with_ecdc_magic() {
        // The payload always begins with GAP1, so an entry can never be mistaken
        // for a standalone ECDC stream.
        let payload = test_encode_gap_payload(96_000, 3_000, 0x4543_4443 /* "ECDC" */);
        assert_eq!(&payload[0..4], GAP_MAGIC);
        assert_ne!(&payload[0..4], b"ECDC");
    }
}
