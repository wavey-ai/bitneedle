//! First-class inter-track silence: the `GAP1` codec.
//!
//! A GAP is intentional PCM silence in the playable record timeline. It is a
//! payload entry and a rendered groove region. A GAP carries no title and no
//! track number, and every [`crate::TrackDescriptor`] range excludes it.
//!
//! A GAP entry carries a self-describing, versioned `GAP1` payload. That payload
//! is authoritative for the sample count, for its own total byte length, and for
//! a deterministic filler seed. The earlier representation was a bare `u64be`
//! sample count, and the descriptor geometry then held the gap duration. The
//! filler makes the GAP occupy carrier bytes, and therefore groove pixels. It
//! produces a narrow, quiet band that round-trips exactly through the PNG.
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

/// The `flags` bit that marks a GAP entry with a *patternized* filler. A
/// patternized filler has its pixels reordered after toning, for visual effect,
/// so its bytes differ from the raw `xorshift32(seed)` keystream. With this bit
/// set, the enclosing chunk CRC32 carries the integrity of the entry, as it does
/// for every chunk payload outside a GAP, and [`validate_gap_payload`] skips its
/// keystream-equality check. The seed then records the construction of the
/// entry.
pub const GAP_FLAG_PATTERNIZED: u8 = 0x01;

/// Mask of every `flags` bit that this version reads. Any other bit set marks
/// an entry from a later writer, and a reader must reject that entry.
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

/// Upper bound on the visible width of a GAP, in groove revolutions. This cap
/// keeps a long gap inside a part of the record.
pub const MAX_GAP_REVOLUTIONS: f64 = 8.0;

/// Lower bound on the visible width, in groove revolutions. This floor renders
/// a short gap as a visible ring.
pub const MIN_GAP_REVOLUTIONS: f64 = 0.25;

/// The GAP band is painted in one quiet tone. That tone is the median of the
/// surrounding high-entropy audio carrier, which averages to mid-grey, so the
/// band reads as a smooth inter-track boundary. The filler bytes carry this
/// tone, centered here with a small deterministic dither. The band is therefore
/// quiet and even, and it round-trips exactly through the PNG.
pub const GAP_QUIET_TONE: u8 = 128;
/// Peak deterministic deviation, per channel byte, from [`GAP_QUIET_TONE`].
pub const GAP_QUIET_VARIATION: u8 = 6;

/// Upper bound on the sample count of one GAP. This bound keeps a long duration
/// from requesting a carrier size beyond the record. The value is one hour per
/// channel at 192 kHz, which is above every inter-track gap in use.
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

/// Profile-derived geometry that sizes the visible band of a GAP, so that its
/// width follows playback time. A gap of one revolution in duration occupies one
/// turn of groove at every radius.
///
/// * `seconds_per_revolution` is the physical revolution duration of the
///   profile: 4/3 s for single45, and 9/5 s for lp. The rotation clock uses the
///   same constant.
/// * `pixels_per_revolution` is the carrier-pixel count in one turn at the mean
///   payload radius, which is `2π·r_mid`. The spiral rasterizes about one pixel
///   per unit of arc length, so this figure estimates one turn at every
///   position.
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

/// Physical revolution duration, in seconds, for a record profile. One turn of
/// groove holds this much playback time. single45 spins at 45 RPM, which gives
/// 4/3 s. lp spins at 33⅓ RPM, which gives 9/5 s.
pub fn seconds_per_revolution(record_profile: &str) -> Result<f64> {
    match record_profile {
        "single45" => Ok(4.0 / 3.0),
        // A 10 in is cut at 45 RPM, so it shares the 7 in revolution clock and
        // the 1333 ms encoder bundle that goes with it.
        "ten" => Ok(4.0 / 3.0),
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
/// This function is the source of truth for the duration-to-width mapping, and
/// the preview UI calls it through WASM. A gap of one revolution in duration is
/// one turn wide. [`MAX_GAP_REVOLUTIONS`] caps the result.
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
/// given record profile. The band spans `gap_revolutions` turns of groove, so
/// the byte budget is `revolutions × pixels_per_revolution × 3`. The result has
/// a floor of [`GAP_HEADER_LENGTH`], so the header fits the smallest gap.
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

/// The `xorshift32` deterministic byte generator. It is cheap, it runs from the
/// seed alone, and it reproduces identical bytes for identical seeds.
struct XorShift32 {
    state: u32,
}

impl XorShift32 {
    fn new(seed: u32) -> Self {
        // Move off the zero fixed point, which emits an all-zero stream.
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

    /// Fill `out` with a quiet, even tone centered on [`GAP_QUIET_TONE`], with
    /// at most ±[`GAP_QUIET_VARIATION`] of deterministic per-byte dither. The
    /// result reads as a smooth mid-grey band against the audio carrier, and the
    /// seed reproduces it exactly.
    fn fill_quiet(&mut self, out: &mut [u8]) {
        let span = u32::from(GAP_QUIET_VARIATION) * 2 + 1;
        for byte in out.iter_mut() {
            let delta = (self.next_u32() % span) as i32 - i32::from(GAP_QUIET_VARIATION);
            *byte = (i32::from(GAP_QUIET_TONE) + delta).clamp(0, 255) as u8;
        }
    }
}

/// Fill `out` with the deterministic quiet-tone filler for the given seed. This
/// function is public so that authoring code, such as `record-cut`, produces
/// filler bytes that [`validate_gap_payload`] accepts. The comment above
/// documents the keystream.
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

    // The filler of a patternized GAP is a post-toning reordering of the
    // keystream, so it differs from `xorshift32(seed)`. The enclosing chunk
    // CRC32 carries the integrity of such an entry, as it does for every other
    // chunk payload. This function checks the quiet-keystream form byte for
    // byte.
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

    /// A test-only `GAP1` encoder that mirrors
    /// `record-cut::gap::encode_gap_payload`. It lives here so that the
    /// decode-side tests of this crate run without an authoring dependency.
    /// Mirror every change to the wire layout in both places.
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
        // A value of exactly half a sample rounds up: 0.5 s * 3 Hz = 1.5 -> 2.
        assert_eq!(gap_sample_count_from_seconds(0.5, 3).unwrap(), 2);
        // A value below half a sample rounds down: 1.4 -> 1.
        assert_eq!(gap_sample_count_from_seconds(1.4, 1).unwrap(), 1);
        // A value above half a sample rounds up: 2.6 -> 3.
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
        // A duration of one revolution, which is 4/3 s for single45, gives one
        // turn.
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
        // A gap of one revolution in duration occupies about one turn of
        // carrier pixels.
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
        // The lenient header decode still succeeds.
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

        // The structural checks still run on a patternized entry, so a
        // declared-length mismatch is rejected.
        assert!(validate_gap_payload(&payload[..255]).is_err());
        // A corrupted magic is rejected.
        let mut broken = payload.clone();
        broken[0] = b'X';
        assert!(decode_gap_header(&broken).is_err());
    }

    #[test]
    fn unknown_flag_bits_are_rejected() {
        let mut payload = test_encode_gap_payload(96_000, 64, 1);
        payload[5] = 0x80; // a bit this version does not understand
        assert!(decode_gap_header(&payload).is_err());
        // The defined patternized bit is accepted.
        payload[5] = GAP_FLAG_PATTERNIZED;
        assert!(decode_gap_header(&payload).is_ok());
    }

    #[test]
    fn filler_never_starts_with_ecdc_magic() {
        // The payload begins with GAP1, so a reader distinguishes an entry from
        // a standalone ECDC stream.
        let payload = test_encode_gap_payload(96_000, 3_000, 0x4543_4443 /* "ECDC" */);
        assert_eq!(&payload[0..4], GAP_MAGIC);
        assert_ne!(&payload[0..4], b"ECDC");
    }
}
