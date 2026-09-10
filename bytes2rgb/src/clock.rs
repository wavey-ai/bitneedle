//! Clockface tones.
//!
//! A **clock** divides the disc into equal angular slots. Each slot is a
//! pocket, and each pocket is cut in its own tone. The slot of a pixel follows
//! from two values: its angle about the centre, in the frame of the record, and
//! its index in the groove. A decoder holds both values once it has walked the
//! spiral. The record therefore carries the wheel alone: the slot tones, the
//! start angle of slot zero, and the blend flag.
//!
//! A clock holds the capacity of a single-tone cut. Every slot palette carries
//! the same bits per pixel, and the bit stream runs continuously across the
//! slots, so the groove has the length of a single-tone cut. The palette that
//! each pixel is looked up in changes with the pocket.
//!
//! **Blending** selects between two existing palettes. A pixel between two slot
//! centres takes the tone of one neighbour or the other. A hash of its groove
//! index, against its distance toward the boundary, makes that choice. Every
//! toned pixel is noise, and the eye reads the mean of a palette, so a pixel by
//! pixel mix of two palettes gives a continuous surface from one slot tone to
//! the next. The decoder recomputes the same choice from the same index.

use crate::{ToneOrdering, TonedConfig, TonedPalette};
use anyhow::{bail, Context, Result};
use std::f64::consts::{FRAC_PI_2, TAU};
use std::sync::Arc;

pub const TONE_CLOCK_MIN_SLOTS: usize = 2;
pub const TONE_CLOCK_MAX_SLOTS: usize = 64;
/// Pockets in the whole wheel, across every ring.
pub const TONE_CLOCK_MAX_CELLS: usize = 256;
pub const TONE_CLOCK_MAX_RINGS: usize = 8;
/// Rotation is carried in hundredths of a degree so both sides derive the
/// same radians from the same integer.
pub const TONE_CLOCK_ROTATION_UNITS_PER_TURN: u32 = 36_000;
/// The band that the rings divide is carried in ten-thousandths of the
/// half-side, for the reason that the rotation is an integer: both sides derive
/// the same boundary from the same number. A float written twice can give two
/// values.
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
/// A wheel divides the groove band twice. **Rings** divide it by radius, into
/// equal bands. **Slots** divide each ring by angle, into equal wedges. A ring
/// near the rim has twice the circumference of a ring near the label, so it
/// holds twice the detail before its pockets are narrower than the art in them.
/// Each ring therefore carries its own slot count, and the house wheel is eight
/// inside and sixteen outside.
///
/// A wheel of one ring is a wheel of wedges over the whole depth of the band.
/// Every clock cut before rings existed has that form, and this code decides
/// each of its pixels as the earlier code did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToneClock {
    /// Where each ring's slot zero begins, clockwise from twelve o'clock, in
    /// hundredths of a degree, innermost first.
    ///
    /// The wheel carries one rotation per ring, because the rings turn at
    /// different rates. Two rings locked together read as one stamped shape at
    /// every rotation. A slower inner ring changes the alignment between the
    /// rings at every degree of spin, which gives each pressing its own sheen.
    ///
    /// A list shorter than `rings` is valid. The remaining rings take the last
    /// value given, so a wheel with one rotation states that value once.
    pub rotation_centidegrees: Vec<u16>,
    /// Whether a pixel near a boundary may take the tone of the adjacent
    /// pocket, in proportion to its distance from that boundary. With the flag
    /// clear, each pocket has a hard edge. The flag covers both boundaries: the
    /// angular boundary and the radial boundary.
    pub blend: bool,
    /// Shared by every pocket: the bit stream is one stream.
    pub bits_per_pixel: u32,
    pub ordering: ToneOrdering,
    /// The slots in each ring, innermost first. `[8, 16]` is eight pockets
    /// across the inside of the band and sixteen around the outside.
    pub rings: Vec<u32>,
    /// The band that the rings divide, in ten-thousandths of the half-side. It
    /// holds the start radius and the end radius of the groove. A pixel outside
    /// that band takes the nearest ring.
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
    /// The rings divide the band by radius, in equal widths. A ring is a band
    /// of the record, and its colour was read off a band of the picture of the
    /// same width. A pixel outside the band takes the nearest ring, because a
    /// spiral runs a short distance past its nominal ends.
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
            // The radial blend follows the angular blend. The ends stay
            // unwrapped: the outermost ring has no ring outside it, and the
            // label sits inside the innermost ring.
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
            // The value is zero at the centre of the slot and ±½ at its
            // edges. The chance of taking the neighbour on that side rises
            // linearly to even odds at the boundary, so both sides of a
            // boundary agree.
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
/// This value comes from the two numbers that [`pixel_angle`] uses. The encoder
/// and the decoder each hold the raster position of the pixel already, so a
/// ring lookup adds no further work.
pub fn pixel_radius(x: f64, y: f64, center_x: f64, center_y: f64) -> f64 {
    (x - center_x).hypot(center_y - y) / center_x.min(center_y).max(f64::EPSILON)
}

/// A unit in `[0, 1)` from a groove index, through the splitmix64 finalizer.
/// This function is frozen. It decides the side of a blend that a pixel lands
/// on, and a change to it makes every blended record already cut unreadable.
fn blend_unit(pixel_index: usize) -> f64 {
    let mut z = (pixel_index as u64).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

/// The same, for the ring a pixel lands in, and independent of it.
///
/// This function is a second stream. One draw for both axes would make a pixel
/// that took its angular neighbour take its radial neighbour also, and the two
/// boundaries would then blend along the diagonal. This function is frozen for
/// the reason that the first one is: it decides the pocket of a pixel, and a
/// record already cut holds its pixels.
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

/// The callback that a long job calls before each expensive step.
///
/// A wheel costs one palette per pocket, and a palette is a fold over the whole
/// sRGB cube. The encoder therefore calls this callback once per pocket. A
/// return of `true` stops the encode with [`CANCELLED`], which a caller
/// distinguishes from a failure.
pub type Stop<'a> = &'a (dyn Fn() -> bool + 'a);

/// The error that a stopped render returns. A caller that asked for a cut and
/// then asked for a different cut receives this error, and it reports the stop
/// rather than a failure.
pub const CANCELLED: &str = "render cancelled";

/// A [`Stop`] that always returns `false`.
pub fn never() -> impl Fn() -> bool {
    || false
}

/// Encodes `bytes` as clock-toned pixels. `angles[i]` is the angle at which
/// pixel `i` sits. See [`pixel_angle`]. A caller with fewer placed pixels than
/// the payload needs, which is an overflowing cut, passes those pixels and
/// receives those pixels. This function builds one palette at a time, over all
/// the pixels that use it, so one palette is resident at any moment.
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
/// This function calls `stop` once per pocket, before it builds that palette.
/// A wheel of twenty-four pockets therefore gives twenty-four stop points, and
/// a stop at the first point saves twenty-three palette builds.
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

/// Recovers the bytes from clock-toned pixels. `angles[i]` is the position of
/// pixel `i` of `rgba`. The caller lifts the groove pixels alone, so every
/// pixel here is opaque. `byte_length` truncates the padded tail.
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
    let words = decode_clock_words(rgba, clock, byte_length, |index| (angles[index], radii[index]))?;
    pack_clock_words(&words, clock.bits_per_pixel, byte_length)
}

/// The same, taking each pixel's index on the raster instead of its angle and
/// distance. The decode walks the spiral, so it already holds the raster
/// index; the angle and the radius are the two numbers that index is made of,
/// and reading them here spares the caller two scratch vectors the size of
/// the groove.
pub fn decode_toned_clock_raster(
    rgba: &[u8],
    clock: &ToneClock,
    width: usize,
    height: usize,
    pixel_indices: &[usize],
    byte_length: Option<usize>,
) -> Result<Vec<u8>> {
    clock.validate()?;
    if !rgba.len().is_multiple_of(4) {
        bail!("RGBA length must be divisible by 4");
    }
    let pixel_count = rgba.len() / 4;
    if pixel_indices.len() != pixel_count {
        bail!(
            "{} pixel indices given for {pixel_count} clock-toned pixels",
            pixel_indices.len()
        );
    }
    let center_x = width as f64 / 2.0;
    let center_y = height as f64 / 2.0;
    let words = decode_clock_words(rgba, clock, byte_length, |index| {
        let raster = pixel_indices[index];
        let x = (raster % width) as f64;
        let y = (raster / width) as f64;
        (
            pixel_angle(x, y, center_x, center_y),
            pixel_radius(x, y, center_x, center_y),
        )
    })?;
    pack_clock_words(&words, clock.bits_per_pixel, byte_length)
}

/// The wheel's words, in one pass.
///
/// `slot_at` gives the angle and the distance of groove pixel `i`. Each pixel
/// is looked up once, in the palette of its own pocket; the pocket's palette
/// is built the first time it is seen and kept for the rest of the pass. The
/// pockets are a fixed, small set, so the table is an array indexed by the key
/// the pixel already computes — no scan of the whole groove per pocket, no
/// hashing, no lock.
fn decode_clock_words<F>(
    rgba: &[u8],
    clock: &ToneClock,
    byte_length: Option<usize>,
    slot_at: F,
) -> Result<Vec<u32>>
where
    F: Fn(usize) -> (f64, f64),
{
    let pixel_count = rgba.len() / 4;
    if let Some(byte_length) = byte_length {
        if clock.pixel_count(byte_length) > pixel_count {
            bail!(
                "{byte_length} bytes need {} pixels at {} bits per pixel; only {pixel_count} were lifted",
                clock.pixel_count(byte_length),
                clock.bits_per_pixel
            );
        }
    }

    let mut palettes: Vec<Option<Arc<TonedPalette>>> = vec![None; 2 * TONE_CLOCK_MAX_CELLS];
    let mut words = vec![0u32; pixel_count];
    for (pixel_index, word) in words.iter_mut().enumerate() {
        let (angle, away) = slot_at(pixel_index);
        let cell = clock.cell_index(pixel_index, angle, away);
        let key = cell * 2 + usize::from(clock.is_gap(pixel_index));
        if palettes[key].is_none() {
            palettes[key] = Some(TonedPalette::shared(clock.config_for_key(key as u16))?);
        }
        let palette = palettes[key].as_ref().expect("palette was just built");
        let at = pixel_index * 4;
        let color = [rgba[at], rgba[at + 1], rgba[at + 2]];
        let Some(index) = palette.index_of(color) else {
            bail!(
                "pixel {pixel_index} #{:02X}{:02X}{:02X} is not in tone clock pocket {}'s {} palette",
                color[0],
                color[1],
                color[2],
                cell,
                if key % 2 == 1 { "gap" } else { "track" }
            );
        };
        *word = index;
    }
    Ok(words)
}

/// Packs `bits_per_pixel`-wide words back into bytes, then trims the padding
/// to the declared length.
fn pack_clock_words(
    words: &[u32],
    bits_per_pixel: u32,
    byte_length: Option<usize>,
) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(words.len() * bits_per_pixel as usize / 8 + 1);
    let mut acc = 0u64;
    let mut acc_bits = 0u32;
    for &word in words {
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
        // Small palettes keep the tests quick. The luma window holds 2^12
        // colours around each hue.
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
        // Angles for a whole spiral, over many turns, so the sweep visits
        // every slot.
        (0..count).map(|i| (i as f64 * 0.37).rem_euclid(TAU)).collect()
    }

    /// Radii for a whole spiral, from the rim inward, as a cut runs, so the
    /// sweep visits every ring of a wheel.
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

    /// The house wheel holds eight pockets across the inside of the band and
    /// sixteen around the outside. The payload round-trips through it.
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
        // Ring zero holds the first eight pockets. Ring one holds the next
        // sixteen, and its slot zero is pocket eight.
        assert_eq!(clock.ring_offset(0), 0);
        assert_eq!(clock.ring_offset(1), 8);
        assert_eq!(clock.cell_index(0, clock.rotation(1) + 0.01, just_outside), 8);
        // A spiral runs a short distance past its nominal ends. Those pixels
        // take the nearest pocket.
        assert_eq!(clock.ring_index(0, 0.0), 0);
        assert_eq!(clock.ring_index(0, 1.0), 1);
    }

    /// A wheel of one ring decides every pixel as the code decided it before
    /// rings existed, so a record already cut keeps its pixels.
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

    /// The two blends are independent. A pixel that takes its angular
    /// neighbour draws its radial neighbour separately.
    #[test]
    fn the_two_blends_do_not_move_together() {
        let together = (0..10_000)
            .filter(|&i| (blend_unit(i) < 0.5) == (blend_unit_across(i) < 0.5))
            .count();
        assert!((4_700..5_300).contains(&together), "{together} of 10000 agreed");
    }

    /// The radial blend matches the angular blend: even odds at the boundary
    /// between two rings.
    #[test]
    fn the_ring_blend_is_even_at_the_boundary() {
        let clock = ringed(vec![8, 16], true);
        let (inner, outer) = clock.band();
        let middle = (inner + outer) / 2.0;
        let crossed = (0..2_000).filter(|&i| clock.ring_index(i, middle) == 1).count();
        assert!((800..1_200).contains(&crossed), "{crossed} of 2000 crossed");
        // At the centre of a ring, every pixel keeps that ring.
        let heart = inner + (outer - inner) * 0.25;
        assert!((0..2_000).all(|i| clock.ring_index(i, heart) == 0));
    }

    /// The blend stays inside the wheel. The rim has no ring outside it, and
    /// the label has no ring inside it.
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
        // An angle above twelve o'clock is slot 0. An angle above three
        // o'clock is slot 1.
        assert_eq!(clock.slot_index(0, 0.01, 0), 0);
        assert_eq!(clock.slot_index(0, FRAC_PI_2 + 0.01, 0), 1);
        // A quarter turn of the wheel moves slot 1 to slot 0.
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

