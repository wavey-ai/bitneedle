//! Clockface tones.
//!
//! A **clock** divides the disc into equal angular slots, each cut in its
//! own tone, the way a roulette wheel is divided into pockets. Which slot a
//! pixel belongs to is a function of where it sits on the disc — its angle
//! about the centre, in the record's own frame — and of its index in the
//! groove. Both are things a decoder has once it has walked the spiral, so
//! nothing about the assignment is written down beyond the wheel itself:
//! the slot tones, where slot zero starts, and whether neighbours blend.
//!
//! Capacity is untouched. Every slot's palette carries the same bits per
//! pixel and the bit stream runs continuously across slots, so the groove is
//! exactly as long as a single-tone cut; only the colour each pixel is
//! looked up in changes.
//!
//! **Blending** never makes a new palette. A pixel between two slot centres
//! takes one neighbour's tone or the other, chosen by a hash of its groove
//! index against how far toward the boundary it sits. Every toned pixel is
//! noise anyway — the eye reads the palette's mean — so mixing two palettes
//! pixel by pixel reads as a continuum from one slot's tone to the next,
//! and the decoder recomputes the same choice from the same index.

use crate::{ToneOrdering, TonedConfig, TonedPalette};
use anyhow::{bail, Context, Result};
use std::f64::consts::{FRAC_PI_2, TAU};

pub const TONE_CLOCK_MIN_SLOTS: usize = 2;
pub const TONE_CLOCK_MAX_SLOTS: usize = 64;
/// Pockets in the whole wheel, across every ring.
pub const TONE_CLOCK_MAX_CELLS: usize = 256;
pub const TONE_CLOCK_MAX_RINGS: usize = 8;
/// Rotation is carried in hundredths of a degree so both sides derive the
/// same radians from the same integer.
pub const TONE_CLOCK_ROTATION_UNITS_PER_TURN: u32 = 36_000;
/// The band the rings divide is carried in ten-thousandths of the half-side,
/// for the same reason the rotation is an integer: both sides have to derive
/// the same boundary from the same number, and a float written down twice is
/// two numbers.
pub const TONE_CLOCK_SPAN_UNITS: u32 = 10_000;

/// One pocket of the wheel: the tone the track is cut in there, and the
/// lighter tone its track gaps are cut in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClockSlot {
    pub base: [u8; 3],
    pub luma_tolerance: u8,
    pub gap_base: [u8; 3],
    pub gap_luma_tolerance: u8,
}

/// The wheel: everything a decoder needs to put each pixel in its pocket.
///
/// A wheel divides the groove band twice. **Rings** cut it across, into
/// equal bands of radius; **slots** cut each ring around, into equal wedges.
/// A ring near the rim has twice the circumference of one near the label and
/// can hold twice the detail before its pockets are narrower than the art in
/// them, so each ring carries its own slot count — the house wheel is eight
/// inside and sixteen outside.
///
/// A wheel of one ring is a wheel of wedges running the whole depth of the
/// band, which is every clock cut before rings existed, and it is decided
/// pixel for pixel exactly as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToneClock {
    /// Where each ring's slot zero begins, clockwise from twelve o'clock, in
    /// hundredths of a degree, innermost first.
    ///
    /// One per ring rather than one for the wheel, because the rings are
    /// turned at different rates: a reflection is not a rigid stencil laid
    /// over a record, and two rings locked together read as one stamped
    /// shape however far it is spun. Turning the inner ring slower means the
    /// alignment between the rings changes with every degree of spin, which
    /// is what makes one pressing's sheen unlike another's.
    ///
    /// Shorter than `rings` is allowed and means the rest take the last
    /// given, so a wheel that wants one rotation can say so once.
    pub rotation_centidegrees: Vec<u16>,
    /// Whether a pixel near a boundary may take the neighbouring pocket's
    /// tone, in proportion to how near it is. Off, pockets are hard-edged.
    /// Both boundaries: around, and across.
    pub blend: bool,
    /// Shared by every pocket: the bit stream is one stream.
    pub bits_per_pixel: u32,
    pub ordering: ToneOrdering,
    /// The slots in each ring, innermost first. `[8, 16]` is eight pockets
    /// across the inside of the band and sixteen around the outside.
    pub rings: Vec<u32>,
    /// The band the rings divide, in ten-thousandths of the half-side:
    /// where the groove starts and where it ends. Outside it a pixel takes
    /// the nearest ring, which is what the ends of a spiral want anyway.
    pub span: (u16, u16),
    /// Every pocket's tones: innermost ring first, and clockwise from
    /// twelve within each ring. `rings` says where each ring's run begins.
    pub slots: Vec<ClockSlot>,
    /// Byte offsets at which the groove alternates between track and gap
    /// tone, starting in track tone at offset zero. Strictly increasing.
    pub gap_switch_offsets: Vec<usize>,
}

impl ToneClock {
    pub fn validate(&self) -> Result<()> {
        if self.rings.is_empty() || self.rings.len() > TONE_CLOCK_MAX_RINGS {
            bail!(
                "tone clock needs between 1 and {TONE_CLOCK_MAX_RINGS} rings, got {}",
                self.rings.len()
            );
        }
        for (index, &slots) in self.rings.iter().enumerate() {
            if !(TONE_CLOCK_MIN_SLOTS..=TONE_CLOCK_MAX_SLOTS).contains(&(slots as usize)) {
                bail!(
                    "tone clock ring {index} needs between {TONE_CLOCK_MIN_SLOTS} and \
                     {TONE_CLOCK_MAX_SLOTS} slots, got {slots}"
                );
            }
        }
        let cells: usize = self.rings.iter().map(|&slots| slots as usize).sum();
        if cells > TONE_CLOCK_MAX_CELLS {
            bail!("tone clock has {cells} pockets, more than {TONE_CLOCK_MAX_CELLS}");
        }
        if self.slots.len() != cells {
            bail!(
                "tone clock has {} tones for {cells} pockets; every pocket takes one",
                self.slots.len()
            );
        }
        if u32::from(self.span.0) >= u32::from(self.span.1) {
            bail!(
                "tone clock band runs from {} to {}, which is not a band",
                self.span.0, self.span.1
            );
        }
        if u32::from(self.span.1) > TONE_CLOCK_SPAN_UNITS {
            bail!("tone clock band ends at {}, past the record", self.span.1);
        }
        if self.rotation_centidegrees.is_empty() {
            bail!("tone clock needs a rotation");
        }
        if self.rotation_centidegrees.len() > self.rings.len() {
            bail!(
                "tone clock has {} rotations for {} rings",
                self.rotation_centidegrees.len(),
                self.rings.len()
            );
        }
        for &turn in &self.rotation_centidegrees {
            if u32::from(turn) >= TONE_CLOCK_ROTATION_UNITS_PER_TURN {
                bail!("tone clock rotation {turn} is a full turn or more");
            }
        }
        if !(1..=24).contains(&self.bits_per_pixel) {
            bail!("tone clock bits per pixel must be between 1 and 24");
        }
        for pair in self.gap_switch_offsets.windows(2) {
            if pair[1] <= pair[0] {
                bail!("tone clock gap switch offsets must be strictly increasing");
            }
        }
        if self.gap_switch_offsets.first() == Some(&0) {
            bail!("tone clock cannot switch to gap tone at byte offset zero");
        }
        Ok(())
    }

    /// Pixels a payload of `byte_length` bytes occupies.
    pub fn pixel_count(&self, byte_length: usize) -> usize {
        (byte_length * 8).div_ceil(self.bits_per_pixel as usize)
    }

    /// A ring's slot zero, in radians clockwise from twelve.
    pub fn rotation(&self, ring: usize) -> f64 {
        let at = ring.min(self.rotation_centidegrees.len() - 1);
        f64::from(self.rotation_centidegrees[at])
            / f64::from(TONE_CLOCK_ROTATION_UNITS_PER_TURN)
            * TAU
    }

    /// The band the rings divide, as fractions of the half-side.
    pub fn band(&self) -> (f64, f64) {
        (
            f64::from(self.span.0) / f64::from(TONE_CLOCK_SPAN_UNITS),
            f64::from(self.span.1) / f64::from(TONE_CLOCK_SPAN_UNITS),
        )
    }

    /// Where each ring's pockets begin in [`ToneClock::slots`].
    pub fn ring_offset(&self, ring: usize) -> usize {
        self.rings[..ring].iter().map(|&slots| slots as usize).sum()
    }

    /// The ring the pixel at groove index `pixel_index`, sitting `away` from
    /// the centre (see [`pixel_radius`]), is cut in.
    ///
    /// The rings divide the band by radius, in equal widths: a ring is a
    /// band of the record, and the colour it is cut in was read off a band
    /// of the picture the same width. Outside the band a pixel takes the
    /// nearest ring — a spiral overruns its nominal ends by a hair, and a
    /// hair is not a reason to have no pocket.
    pub fn ring_index(&self, pixel_index: usize, away: f64) -> usize {
        let count = self.rings.len();
        if count == 1 {
            return 0;
        }
        let (inner, outer) = self.band();
        let across = (outer - inner).max(f64::EPSILON);
        let position = ((away - inner) / across).clamp(0.0, 1.0) * count as f64;
        let mut ring = (position.floor() as usize).min(count - 1);
        if self.blend {
            // As around, so across — but the ends do not wrap: there is no
            // ring outside the outermost, and the innermost is the label.
            let toward_edge = position - ring as f64 - 0.5;
            if blend_unit_across(pixel_index) < toward_edge.abs() {
                if toward_edge > 0.0 && ring + 1 < count {
                    ring += 1;
                } else if toward_edge < 0.0 && ring > 0 {
                    ring -= 1;
                }
            }
        }
        ring
    }

    /// The slot of `ring` the pixel at groove index `pixel_index`, sitting
    /// at `angle` (see [`pixel_angle`]), is cut in.
    pub fn slot_index(&self, pixel_index: usize, angle: f64, ring: usize) -> usize {
        let count = self.rings[ring] as usize;
        let width = TAU / count as f64;
        let position = (angle - self.rotation(ring)).rem_euclid(TAU) / width;
        let mut slot = (position.floor() as usize).min(count - 1);
        if self.blend {
            // Zero at the slot's centre, ±½ at its edges; the chance of
            // taking the neighbour on that side grows linearly to even odds
            // at the boundary, so the two sides of a boundary agree.
            let toward_edge = position - slot as f64 - 0.5;
            if blend_unit(pixel_index) < toward_edge.abs() {
                slot = if toward_edge > 0.0 {
                    (slot + 1) % count
                } else {
                    (slot + count - 1) % count
                };
            }
        }
        slot
    }

    /// The pocket the pixel at groove index `pixel_index` is cut in: its
    /// ring first, then its slot within that ring, as an index into
    /// [`ToneClock::slots`].
    pub fn cell_index(&self, pixel_index: usize, angle: f64, away: f64) -> usize {
        let ring = self.ring_index(pixel_index, away);
        self.ring_offset(ring) + self.slot_index(pixel_index, angle, ring)
    }

    /// Whether the pixel at `pixel_index` falls in a track gap: the state
    /// of the byte its first bit belongs to.
    pub fn is_gap(&self, pixel_index: usize) -> bool {
        let byte_offset = pixel_index * self.bits_per_pixel as usize / 8;
        let switches_passed = self
            .gap_switch_offsets
            .partition_point(|&offset| offset <= byte_offset);
        switches_passed % 2 == 1
    }

    /// The palette a pocket's track or gap pixels are looked up in.
    pub fn config(&self, cell: usize, gap: bool) -> TonedConfig {
        let pocket = self.slots[cell];
        let (base, luma_tolerance) = if gap {
            (pocket.gap_base, pocket.gap_luma_tolerance)
        } else {
            (pocket.base, pocket.luma_tolerance)
        };
        TonedConfig {
            base,
            luma_tolerance,
            bits_per_pixel: self.bits_per_pixel,
            ordering: self.ordering,
        }
    }

    /// Which palette each of the first `angles.len()` pixels uses, as
    /// `cell * 2 + gap`.
    fn palette_keys(&self, angles: &[f64], radii: &[f64]) -> Vec<u16> {
        angles
            .iter()
            .zip(radii)
            .enumerate()
            .map(|(pixel_index, (&angle, &away))| {
                (self.cell_index(pixel_index, angle, away) * 2
                    + usize::from(self.is_gap(pixel_index))) as u16
            })
            .collect()
    }

    fn config_for_key(&self, key: u16) -> TonedConfig {
        self.config(usize::from(key / 2), key % 2 == 1)
    }
}

/// A pixel's angle about the disc's centre, clockwise from twelve o'clock,
/// in `0..TAU`. Raster coordinates: `y` grows downward. The centre is the
/// raster's, `(width / 2, height / 2)`, the same point the spiral is traced
/// about.
pub fn pixel_angle(x: f64, y: f64, center_x: f64, center_y: f64) -> f64 {
    (FRAC_PI_2 - (center_y - y).atan2(x - center_x)).rem_euclid(TAU)
}

/// A pixel's distance from the disc's centre, as a fraction of the
/// half-side — the same frame [`ToneClock::span`] is written in.
///
/// The same two numbers [`pixel_angle`] is derived from, read the other way,
/// which is why rings cost the encoder and the decoder nothing to find: both
/// already had the pixel's place on the raster in hand.
pub fn pixel_radius(x: f64, y: f64, center_x: f64, center_y: f64) -> f64 {
    (x - center_x).hypot(center_y - y) / center_x.min(center_y).max(f64::EPSILON)
}

/// A unit in `[0, 1)` from a groove index. splitmix64's finaliser, frozen:
/// this decides which side of a blend a pixel lands on, so it can never
/// change without every blended record already cut becoming unreadable.
fn blend_unit(pixel_index: usize) -> f64 {
    let mut z = (pixel_index as u64).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

/// The same, for the ring a pixel lands in, and independent of it.
///
/// A second stream rather than the same one: if one draw decided both axes,
/// a pixel that took its neighbour around would take its neighbour across as
/// well, and the two boundaries would blend along the diagonal instead of
/// each in its own direction. Frozen for the same reason as the first — it
/// decides which pocket a pixel is in, and a record already cut cannot be
/// asked to change its mind.
fn blend_unit_across(pixel_index: usize) -> f64 {
    let mut z = (pixel_index as u64).wrapping_add(0xD1B5_4A32_D192_ED03);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

/// The `bits_per_pixel`-bit words of `bytes`, in stream order, the last one
/// zero-padded.
fn pack_words(bytes: &[u8], bits_per_pixel: u32) -> Vec<u32> {
    let mask = (1u64 << bits_per_pixel) - 1;
    let mut words = Vec::with_capacity((bytes.len() * 8).div_ceil(bits_per_pixel as usize));
    let mut acc = 0u64;
    let mut acc_bits = 0u32;
    for &byte in bytes {
        acc = (acc << 8) | u64::from(byte);
        acc_bits += 8;
        while acc_bits >= bits_per_pixel {
            words.push(((acc >> (acc_bits - bits_per_pixel)) & mask) as u32);
            acc_bits -= bits_per_pixel;
            acc &= (1 << acc_bits) - 1;
        }
    }
    if acc_bits > 0 {
        words.push(((acc << (bits_per_pixel - acc_bits)) & mask) as u32);
    }
    words
}

/// What a long job asks before it does the next expensive thing.
///
/// A wheel's cost is one palette per pocket, and a palette is a fold over
/// the whole sRGB cube — so a pocket is the natural place to ask whether
/// anybody still wants this. Returning `true` stops the encode with
/// [`CANCELLED`], which callers can tell apart from a real failure.
pub type Stop<'a> = &'a (dyn Fn() -> bool + 'a);

/// What a stopped render says. A cancellation is not a fault: a caller that
/// asked for a cut and then asked for a different one wants this and should
/// not log it as an error.
pub const CANCELLED: &str = "render cancelled";

/// A [`Stop`] that never stops.
pub fn never() -> impl Fn() -> bool {
    || false
}

/// Encodes `bytes` as clock-toned pixels. `angles[i]` is the angle at which
/// pixel `i` will sit (see [`pixel_angle`]); a caller that has placed fewer
/// pixels than the payload needs — an overflowing cut — passes only those,
/// and gets only those back. Palettes are built one at a time, each over
/// all the pixels that use it, so a wheel of many pockets never has more
/// than one palette live.
pub fn encode_toned_clock(
    bytes: &[u8],
    clock: &ToneClock,
    angles: &[f64],
    radii: &[f64],
) -> Result<Vec<u8>> {
    encode_toned_clock_until(bytes, clock, angles, radii, &never())
}

/// The same, stopping when `stop` says to.
///
/// Asked once per pocket, before its palette is built. A wheel of
/// twenty-four pockets is twenty-four chances to give up, which on a hand
/// that has moved on is twenty-three palettes not built.
pub fn encode_toned_clock_until(
    bytes: &[u8],
    clock: &ToneClock,
    angles: &[f64],
    radii: &[f64],
    stop: Stop<'_>,
) -> Result<Vec<u8>> {
    clock.validate()?;
    let pixel_count = clock.pixel_count(bytes.len());
    if angles.len() > pixel_count {
        bail!(
            "{} pixel angles given for a payload of {pixel_count} pixels",
            angles.len()
        );
    }
    if radii.len() != angles.len() {
        bail!(
            "{} pixel radii given for {} pixel angles",
            radii.len(),
            angles.len()
        );
    }
    let words = pack_words(bytes, clock.bits_per_pixel);
    let keys = clock.palette_keys(angles, radii);
    let mut rgba = vec![0u8; angles.len() * 4];

    for key in distinct_keys(&keys) {
        if stop() {
            bail!("{CANCELLED}");
        }
        let palette = TonedPalette::shared(clock.config_for_key(key)).with_context(|| {
            format!(
                "tone clock pocket {} {} tone cannot make a palette at {} bits per pixel",
                key / 2,
                if key % 2 == 1 { "gap" } else { "track" },
                clock.bits_per_pixel
            )
        })?;
        for (pixel_index, _) in keys.iter().enumerate().filter(|(_, &k)| k == key) {
            let color = palette.color(words[pixel_index] as usize);
            let at = pixel_index * 4;
            rgba[at..at + 3].copy_from_slice(&color);
            rgba[at + 3] = 255;
        }
    }
    Ok(rgba)
}

/// Recovers the bytes from clock-toned pixels. `angles[i]` is where pixel
/// `i` of `rgba` sits; fully transparent pixels are not expected here — the
/// caller has already lifted exactly the groove's pixels. `byte_length`
/// truncates the padded tail.
pub fn decode_toned_clock(
    rgba: &[u8],
    clock: &ToneClock,
    angles: &[f64],
    radii: &[f64],
    byte_length: Option<usize>,
) -> Result<Vec<u8>> {
    clock.validate()?;
    if !rgba.len().is_multiple_of(4) {
        bail!("RGBA length must be divisible by 4");
    }
    let pixel_count = rgba.len() / 4;
    if angles.len() != pixel_count {
        bail!(
            "{} pixel angles given for {pixel_count} clock-toned pixels",
            angles.len()
        );
    }
    if radii.len() != pixel_count {
        bail!(
            "{} pixel radii given for {pixel_count} clock-toned pixels",
            radii.len()
        );
    }
    if let Some(byte_length) = byte_length {
        if clock.pixel_count(byte_length) > pixel_count {
            bail!(
                "{byte_length} bytes need {} pixels at {} bits per pixel; only {pixel_count} were lifted",
                clock.pixel_count(byte_length),
                clock.bits_per_pixel
            );
        }
    }

    let keys = clock.palette_keys(angles, radii);
    let mut words = vec![0u32; pixel_count];
    for key in distinct_keys(&keys) {
        let palette = TonedPalette::shared(clock.config_for_key(key))?;
        for (pixel_index, _) in keys.iter().enumerate().filter(|(_, &k)| k == key) {
            let at = pixel_index * 4;
            let color = [rgba[at], rgba[at + 1], rgba[at + 2]];
            let Some(index) = palette.index_of(color) else {
                bail!(
                    "pixel {pixel_index} #{:02X}{:02X}{:02X} is not in tone clock pocket {}'s {} palette",
                    color[0],
                    color[1],
                    color[2],
                    key / 2,
                    if key % 2 == 1 { "gap" } else { "track" }
                );
            };
            words[pixel_index] = index;
        }
    }

    let bits_per_pixel = clock.bits_per_pixel;
    let mut bytes = Vec::with_capacity(pixel_count * bits_per_pixel as usize / 8 + 1);
    let mut acc = 0u64;
    let mut acc_bits = 0u32;
    for word in words {
        acc = (acc << bits_per_pixel) | u64::from(word);
        acc_bits += bits_per_pixel;
        while acc_bits >= 8 {
            bytes.push((acc >> (acc_bits - 8)) as u8);
            acc_bits -= 8;
            acc &= (1 << acc_bits) - 1;
        }
    }
    if let Some(byte_length) = byte_length {
        if byte_length > bytes.len() {
            bail!("requested byte length exceeds decoded clock-toned payload");
        }
        bytes.truncate(byte_length);
    }
    Ok(bytes)
}

fn distinct_keys(keys: &[u16]) -> Vec<u16> {
    let mut seen = [false; 2 * TONE_CLOCK_MAX_CELLS];
    for &key in keys {
        seen[usize::from(key)] = true;
    }
    (0..seen.len() as u16).filter(|&k| seen[usize::from(k)]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wheel(slots: usize, blend: bool) -> ToneClock {
        // Small palettes so the tests stay quick: a luma window wide enough
        // for 2^12 colours around each hue.
        let hues: Vec<[u8; 3]> = (0..slots)
            .map(|k| {
                let t = k as f64 / slots as f64;
                [
                    (128.0 + 100.0 * (TAU * t).cos()) as u8,
                    (128.0 + 100.0 * (TAU * t + 2.0).cos()) as u8,
                    (128.0 + 100.0 * (TAU * t + 4.0).cos()) as u8,
                ]
            })
            .collect();
        ToneClock {
            rotation_centidegrees: vec![1_250],
            blend,
            bits_per_pixel: 12,
            ordering: ToneOrdering::ChromaProximity,
            rings: vec![slots as u32],
            span: (3_000, 9_700),
            slots: hues
                .iter()
                .map(|&base| ClockSlot {
                    base,
                    luma_tolerance: 24,
                    gap_base: [base[0].saturating_add(30), base[1].saturating_add(30), base[2]],
                    gap_luma_tolerance: 24,
                })
                .collect(),
            gap_switch_offsets: vec![700, 900],
        }
    }

    fn payload(len: usize) -> Vec<u8> {
        let mut state = 0xfeed_face_cafe_beefu64;
        (0..len)
            .map(|_| {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                (state >> 56) as u8
            })
            .collect()
    }

    fn angles(count: usize) -> Vec<f64> {
        // A spiral's worth of angles: many turns, so every slot is visited.
        (0..count).map(|i| (i as f64 * 0.37).rem_euclid(TAU)).collect()
    }

    /// A spiral's worth of radii: from the rim inward, as a cut runs, so a
    /// wheel with rings has every one of them visited.
    fn radii(count: usize, clock: &ToneClock) -> Vec<f64> {
        let (inner, outer) = clock.band();
        (0..count)
            .map(|i| outer - (outer - inner) * (i as f64 / count.max(1) as f64))
            .collect()
    }

    /// A wheel of rings, each with its own slot count.
    fn ringed(rings: Vec<u32>, blend: bool) -> ToneClock {
        let cells: u32 = rings.iter().sum();
        let mut clock = wheel(cells as usize, blend);
        clock.rings = rings;
        clock
    }

    #[test]
    fn round_trips_with_gaps_and_blend() {
        for blend in [false, true] {
            let clock = wheel(8, blend);
            let bytes = payload(2_003);
            let angles = angles(clock.pixel_count(bytes.len()));
            let radii = radii(angles.len(), &clock);
            let rgba = encode_toned_clock(&bytes, &clock, &angles, &radii).unwrap();
            assert_eq!(rgba.len() / 4, angles.len());
            let back =
                decode_toned_clock(&rgba, &clock, &angles, &radii, Some(bytes.len())).unwrap();
            assert_eq!(back, bytes, "blend={blend}");
        }
    }

    /// The house wheel: eight pockets across the inside of the band and
    /// sixteen around the outside, and the payload comes back out of it.
    #[test]
    fn a_ringed_wheel_round_trips() {
        for blend in [false, true] {
            let clock = ringed(vec![8, 16], blend);
            let bytes = payload(3_001);
            let angles = angles(clock.pixel_count(bytes.len()));
            let radii = radii(angles.len(), &clock);
            let rgba = encode_toned_clock(&bytes, &clock, &angles, &radii).unwrap();
            let back =
                decode_toned_clock(&rgba, &clock, &angles, &radii, Some(bytes.len())).unwrap();
            assert_eq!(back, bytes, "blend={blend}");
        }
    }

    /// Every pocket of a ringed wheel is reached, and the rings are told
    /// apart by radius rather than by angle.
    #[test]
    fn rings_are_chosen_by_radius() {
        let clock = ringed(vec![8, 16], false);
        let (inner, outer) = clock.band();
        let just_inside = inner + (outer - inner) * 0.01;
        let just_outside = outer - (outer - inner) * 0.01;
        assert_eq!(clock.ring_index(0, just_inside), 0);
        assert_eq!(clock.ring_index(0, just_outside), 1);
        // Ring zero's pockets are the first eight; ring one's the next
        // sixteen, and its slot zero is pocket eight.
        assert_eq!(clock.ring_offset(0), 0);
        assert_eq!(clock.ring_offset(1), 8);
        assert_eq!(clock.cell_index(0, clock.rotation(1) + 0.01, just_outside), 8);
        // A spiral runs past its nominal ends by a hair; the hair still has
        // a pocket, and it is the nearest one.
        assert_eq!(clock.ring_index(0, 0.0), 0);
        assert_eq!(clock.ring_index(0, 1.0), 1);
    }

    /// A wheel of one ring decides every pixel exactly as it did before
    /// rings existed — the record already cut cannot change its mind.
    #[test]
    fn one_ring_is_the_old_wheel_pixel_for_pixel() {
        let clock = wheel(8, true);
        let angles = angles(4_000);
        for (pixel_index, &angle) in angles.iter().enumerate() {
            let ring = clock.slot_index(pixel_index, angle, 0);
            for away in [0.0, 0.31, 0.5, 0.97, 1.0] {
                assert_eq!(clock.cell_index(pixel_index, angle, away), ring);
            }
        }
    }

    /// The two blends are independent: a pixel that takes its neighbour
    /// around does not thereby take its neighbour across.
    #[test]
    fn the_two_blends_do_not_move_together() {
        let together = (0..10_000)
            .filter(|&i| (blend_unit(i) < 0.5) == (blend_unit_across(i) < 0.5))
            .count();
        assert!((4_700..5_300).contains(&together), "{together} of 10000 agreed");
    }

    /// Across, as around: even odds at the boundary between two rings.
    #[test]
    fn the_ring_blend_is_even_at_the_boundary() {
        let clock = ringed(vec![8, 16], true);
        let (inner, outer) = clock.band();
        let middle = (inner + outer) / 2.0;
        let crossed = (0..2_000).filter(|&i| clock.ring_index(i, middle) == 1).count();
        assert!((800..1_200).contains(&crossed), "{crossed} of 2000 crossed");
        // And nothing crosses at a ring's own centre.
        let heart = inner + (outer - inner) * 0.25;
        assert!((0..2_000).all(|i| clock.ring_index(i, heart) == 0));
    }

    /// Nothing blends off the outside of the wheel: there is no ring past
    /// the rim to take a pixel, and none inside the label either.
    #[test]
    fn the_ends_of_the_band_do_not_wrap() {
        let clock = ringed(vec![4, 4, 4], true);
        let (inner, outer) = clock.band();
        assert!((0..2_000).all(|i| clock.ring_index(i, inner) == 0));
        assert!((0..2_000).all(|i| clock.ring_index(i, outer) == 2));
    }

    #[test]
    fn every_slot_is_used_and_gaps_toggle() {
        let clock = wheel(16, true);
        let angles = angles(4_000);
        let radii = radii(angles.len(), &clock);
        let keys = clock.palette_keys(&angles, &radii);
        let used = distinct_keys(&keys);
        assert_eq!(used.len(), 32, "16 slots × track/gap");
        assert!(!clock.is_gap(0));
        assert!(clock.is_gap(clock.pixel_count(700)));
        assert!(!clock.is_gap(clock.pixel_count(900)));
    }

    #[test]
    fn hard_edges_follow_rotation() {
        let mut clock = wheel(4, false);
        clock.rotation_centidegrees = vec![0];
        // Just past twelve is slot 0; just past three is slot 1.
        assert_eq!(clock.slot_index(0, 0.01, 0), 0);
        assert_eq!(clock.slot_index(0, FRAC_PI_2 + 0.01, 0), 1);
        // Spin the wheel a quarter turn: what was slot 1 is now slot 0.
        clock.rotation_centidegrees = vec![9_000];
        assert_eq!(clock.slot_index(0, FRAC_PI_2 + 0.01, 0), 0);
        assert_eq!(clock.slot_index(0, 0.01, 0), 3);
    }

    #[test]
    fn blend_is_even_at_the_boundary_and_absent_at_the_centre() {
        let mut clock = wheel(4, true);
        clock.rotation_centidegrees = vec![0];
        let centre = FRAC_PI_2 / 2.0;
        assert!((0..2_000).all(|i| clock.slot_index(i, centre, 0) == 0));
        let at_boundary = FRAC_PI_2 - 1e-9;
        let flipped = (0..2_000)
            .filter(|&i| clock.slot_index(i, at_boundary, 0) == 1)
            .count();
        assert!((800..1_200).contains(&flipped), "{flipped} of 2000 crossed");
    }

    #[test]
    fn pixel_angle_runs_clockwise_from_twelve() {
        let (cx, cy) = (288.0, 288.0);
        assert!(pixel_angle(288.0, 100.0, cx, cy).abs() < 1e-12);
        assert!((pixel_angle(400.0, 288.0, cx, cy) - FRAC_PI_2).abs() < 1e-12);
        assert!((pixel_angle(288.0, 400.0, cx, cy) - 2.0 * FRAC_PI_2).abs() < 1e-12);
        assert!((pixel_angle(100.0, 288.0, cx, cy) - 3.0 * FRAC_PI_2).abs() < 1e-12);
    }

    #[test]
    fn blend_unit_is_frozen() {
        assert_eq!(blend_unit(0).to_bits(), 0x3FEC_4415_072F_63B9);
        assert!((0..10_000).all(|i| (0.0..1.0).contains(&blend_unit(i))));
        assert!((0..10_000).all(|i| (0.0..1.0).contains(&blend_unit_across(i))));
    }

    #[test]
    fn pixel_radius_is_a_fraction_of_the_half_side() {
        let (cx, cy) = (288.0, 288.0);
        assert!(pixel_radius(288.0, 288.0, cx, cy).abs() < 1e-12);
        assert!((pixel_radius(576.0, 288.0, cx, cy) - 1.0).abs() < 1e-12);
        assert!((pixel_radius(288.0, 0.0, cx, cy) - 1.0).abs() < 1e-12);
    }
}

