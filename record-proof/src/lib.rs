//! Print-proof post-processing for bitneedle picture records.
//!
//! A record PNG is a 576×576 RGBA image whose disc is opaque and whose four
//! corners are fully transparent. [`add_print_calibration`] leaves every disc
//! pixel untouched and paints colour-calibration targets into the transparent
//! corners so that a high-quality print of the record can later be scanned
//! and decoded despite the printer's colour rendition.
//!
//! Layout (`proof-v1`): each corner holds a **mini record** — a disc of
//! radius [`MINI_OUTER_RADIUS`] centred [`MINI_CENTER`] px in from the image
//! corner, mirrored so every corner reads the same from its own corner:
//!
//! * a transparent **spindle hole** in the middle;
//! * a white **label** ring carrying a 1 px black **crosshair** — the
//!   registration centre and axes;
//! * six concentric **groove rings**, each [`RING_WIDTH`] px wide and split
//!   into ~[`RING_WIDTH`]-px sectors: the swatch cells. Cell order runs from
//!   the innermost ring outward, sectors clockwise from the +x axis: first
//!   the structural set (black, white, greys, RGBCMY), then a deterministic
//!   sample of each distinct tone span's palette in palette-index order
//!   (production palettes hold ~2^20 colours, so `samples_per_span` indices
//!   are taken at an even stride from nearest-to-base to farthest; small
//!   palettes are enumerated in full);
//! * a black **rim** with four white notches on the axes — scale and
//!   orientation for a scanner.
//!
//! The top-left mini record carries, instead of grooves, a QR code on its
//! label with [`ProofConfig`]: the layout parameters and every tone span's
//! palette config. A scanner can rebuild the exact expected colour of every
//! cell from this alone, without first decoding the descriptor spiral. The
//! other three corners are identical swatch replicas, letting a scanner fit
//! a spatial colour-drift model across the plate.
//!
//! Everything painted is deterministic in the record descriptor, so
//! [`ProofLayout`] can be regenerated on the decode side.

use anyhow::{bail, Context, Result};
use qrcode::{EcLevel, QrCode};
use record_core::describe_record_profile;
use record_decode::decode_record_descriptor_from_png;
use record_descriptor::{RecordDescriptor, ToneOrdering, ToneSpanDescriptor};
use record_groove::{ToneOrdering as GrooveToneOrdering, TonedConfig, TonedPalette};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

pub const PROOF_LAYOUT_VERSION: u8 = 1;
pub const PROOF_MAGIC: &[u8; 4] = b"BNPF";
pub const RECORD_SIZE: usize = 576;
/// Mini-record centre, in px from the image corner along both axes (a
/// continuous coordinate; pixel `x` is sampled at `x + 0.5`).
pub const MINI_CENTER: f64 = 49.5;
/// Mini-record outer radius. Its innermost point sits at
/// `(288 − 49.5)·√2 − 48 ≈ 289.3` px from the record centre, clear of the
/// radius-287 disc.
pub const MINI_OUTER_RADIUS: f64 = 48.0;
/// Rim (black ring with notches) spans `[RIM_INNER_RADIUS, MINI_OUTER_RADIUS)`.
pub const RIM_INNER_RADIUS: f64 = 45.0;
/// Groove rings span `[GROOVE_INNER_RADIUS, RIM_INNER_RADIUS)`.
pub const GROOVE_INNER_RADIUS: f64 = 9.0;
/// Label (white, crosshair) spans `[SPINDLE_RADIUS, GROOVE_INNER_RADIUS)`;
/// inside `SPINDLE_RADIUS` is left transparent.
pub const SPINDLE_RADIUS: f64 = 4.0;
/// Groove ring width and target sector arc length, px.
pub const RING_WIDTH: usize = 6;
/// Half-width of the rim notches, px.
const NOTCH_HALF_WIDTH: f64 = 2.0;
/// Mini-record bounding square, corner-local.
const MINI_SIDE: usize = 98;
/// Structural swatches painted before the palette: neutral ramp + primaries.
pub const STRUCTURAL_SWATCHES: [[u8; 3]; 11] = [
    [0, 0, 0],
    [255, 255, 255],
    [64, 64, 64],
    [128, 128, 128],
    [192, 192, 192],
    [255, 0, 0],
    [0, 255, 0],
    [0, 0, 255],
    [0, 255, 255],
    [255, 0, 255],
    [255, 255, 0],
];

/// Fallback palette for records without toned grooves: a 4-level RGB cube.
fn rgb_cube_swatches() -> Vec<[u8; 3]> {
    let levels = [0u8, 85, 170, 255];
    let mut out = Vec::with_capacity(64);
    for &r in &levels {
        for &g in &levels {
            for &b in &levels {
                out.push([r, g, b]);
            }
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    pub const SWATCH_CORNERS: [Corner; 3] =
        [Corner::TopRight, Corner::BottomLeft, Corner::BottomRight];

    /// Maps a corner-local coordinate (0,0 at the corner, growing inward) to
    /// image pixel coordinates.
    pub fn to_image(self, x: usize, y: usize) -> (usize, usize) {
        let flip = |v: usize| RECORD_SIZE - 1 - v;
        match self {
            Corner::TopLeft => (x, y),
            Corner::TopRight => (flip(x), y),
            Corner::BottomLeft => (x, flip(y)),
            Corner::BottomRight => (flip(x), flip(y)),
        }
    }
}

/// Palette config for one distinct tone span, as carried in the QR code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProofToneSpan {
    pub base: [u8; 3],
    pub luma_tolerance: u8,
    pub bits_per_pixel: u8,
    pub ordering: ToneOrdering,
    pub byte_length: usize,
}

impl From<&ToneSpanDescriptor> for ProofToneSpan {
    fn from(span: &ToneSpanDescriptor) -> Self {
        Self {
            base: span.base,
            luma_tolerance: span.luma_tolerance,
            bits_per_pixel: span.bits_per_pixel,
            ordering: span.ordering,
            byte_length: span.byte_length,
        }
    }
}

impl ProofToneSpan {
    fn groove_config(&self) -> TonedConfig {
        TonedConfig {
            base: self.base,
            luma_tolerance: self.luma_tolerance,
            bits_per_pixel: u32::from(self.bits_per_pixel),
            ordering: match self.ordering {
                ToneOrdering::BaseProximity => GrooveToneOrdering::BaseProximity,
                ToneOrdering::ChromaProximity => GrooveToneOrdering::ChromaProximity,
            },
        }
    }

    /// Same palette, regardless of byte length — swatches are deduplicated
    /// on this.
    fn palette_key(&self) -> ([u8; 3], u8, u8, ToneOrdering) {
        (
            self.base,
            self.luma_tolerance,
            self.bits_per_pixel,
            self.ordering,
        )
    }
}

/// Everything a scanner needs to regenerate the layout. This is what the QR
/// code carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProofConfig {
    pub layout_version: u8,
    pub record_profile: String,
    pub corner_side: u8,
    pub ring_width: u8,
    pub structural_count: u8,
    pub payload_encoding: String,
    /// Palette swatches painted per tone span. When a palette has more
    /// colours than this, indices are sampled by [`sampled_palette_indices`].
    pub samples_per_span: u16,
    /// Distinct tone span palettes in the order their swatches are painted.
    pub tone_spans: Vec<ProofToneSpan>,
}

/// The palette indices painted for a palette of `palette_len` colours when
/// `samples` swatches are available: the whole palette if it fits, otherwise
/// `samples` indices at an even stride from index 0 (closest to the base
/// tone) to the last (farthest).
pub fn sampled_palette_indices(palette_len: usize, samples: usize) -> Vec<usize> {
    if palette_len <= samples {
        return (0..palette_len).collect();
    }
    if samples <= 1 {
        return vec![0];
    }
    (0..samples)
        .map(|k| k * (palette_len - 1) / (samples - 1))
        .collect()
}

impl ProofConfig {
    fn from_descriptor(
        descriptor: &RecordDescriptor,
        ring_width: usize,
        samples_per_span: usize,
    ) -> Self {
        let mut tone_spans: Vec<ProofToneSpan> = Vec::new();
        for span in &descriptor.tone_spans {
            let candidate = ProofToneSpan::from(span);
            if !tone_spans
                .iter()
                .any(|s| s.palette_key() == candidate.palette_key())
            {
                tone_spans.push(candidate);
            }
        }
        Self {
            layout_version: PROOF_LAYOUT_VERSION,
            record_profile: descriptor.record_profile.clone(),
            corner_side: MINI_SIDE as u8,
            ring_width: ring_width as u8,
            structural_count: STRUCTURAL_SWATCHES.len() as u8,
            payload_encoding: descriptor.payload_encoding.clone(),
            samples_per_span: samples_per_span as u16,
            tone_spans,
        }
    }

    /// Compact binary form for the QR code:
    ///
    /// `BNPF | version | profile | corner_side | ring_width | structural_count
    /// | encoding_len | encoding | samples_per_span (u16 BE) | span_count | span*`
    ///
    /// where each span is `base[3] | luma_tolerance | bits_per_pixel |
    /// ordering | byte_length (LEB128)`.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(32);
        out.extend_from_slice(PROOF_MAGIC);
        out.push(self.layout_version);
        out.push(profile_code(&self.record_profile)?);
        out.push(self.corner_side);
        out.push(self.ring_width);
        out.push(self.structural_count);
        let encoding = self.payload_encoding.as_bytes();
        if encoding.len() > 255 {
            bail!("payload encoding name too long for proof config");
        }
        out.push(encoding.len() as u8);
        out.extend_from_slice(encoding);
        out.extend_from_slice(&self.samples_per_span.to_be_bytes());
        if self.tone_spans.len() > 255 {
            bail!("too many distinct tone spans for proof config");
        }
        out.push(self.tone_spans.len() as u8);
        for span in &self.tone_spans {
            out.extend_from_slice(&span.base);
            out.push(span.luma_tolerance);
            out.push(span.bits_per_pixel);
            out.push(match span.ordering {
                ToneOrdering::BaseProximity => 0,
                ToneOrdering::ChromaProximity => 1,
            });
            write_leb128(&mut out, span.byte_length as u64);
        }
        Ok(out)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let mut cursor = 0usize;
        let mut take = |n: usize| -> Result<&[u8]> {
            let end = cursor
                .checked_add(n)
                .context("proof config length overflow")?;
            if end > bytes.len() {
                bail!("proof config truncated");
            }
            let slice = &bytes[cursor..end];
            cursor = end;
            Ok(slice)
        };
        if take(4)? != PROOF_MAGIC {
            bail!("proof config magic mismatch");
        }
        let layout_version = take(1)?[0];
        if layout_version != PROOF_LAYOUT_VERSION {
            bail!("unsupported proof layout version {layout_version}");
        }
        let record_profile = profile_name(take(1)?[0])?.to_string();
        let corner_side = take(1)?[0];
        let ring_width = take(1)?[0];
        let structural_count = take(1)?[0];
        let encoding_len = take(1)?[0] as usize;
        let payload_encoding = String::from_utf8(take(encoding_len)?.to_vec())
            .context("proof config payload encoding is not UTF-8")?;
        let samples = take(2)?;
        let samples_per_span = u16::from_be_bytes([samples[0], samples[1]]);
        let span_count = take(1)?[0] as usize;
        let mut tone_spans = Vec::with_capacity(span_count);
        for _ in 0..span_count {
            let head = take(6)?;
            let base = [head[0], head[1], head[2]];
            let luma_tolerance = head[3];
            let bits_per_pixel = head[4];
            let ordering = match head[5] {
                0 => ToneOrdering::BaseProximity,
                1 => ToneOrdering::ChromaProximity,
                other => bail!("unknown tone ordering code {other}"),
            };
            let mut byte_length = 0u64;
            let mut shift = 0u32;
            loop {
                let b = take(1)?[0];
                byte_length |= u64::from(b & 0x7f) << shift;
                if b & 0x80 == 0 {
                    break;
                }
                shift += 7;
                if shift > 63 {
                    bail!("proof config byte length varint too long");
                }
            }
            tone_spans.push(ProofToneSpan {
                base,
                luma_tolerance,
                bits_per_pixel,
                ordering,
                byte_length: byte_length as usize,
            });
        }
        Ok(Self {
            layout_version,
            record_profile,
            corner_side,
            ring_width,
            structural_count,
            payload_encoding,
            samples_per_span,
            tone_spans,
        })
    }
}

fn profile_code(profile: &str) -> Result<u8> {
    Ok(match profile {
        "single45" => 0,
        "lp" => 1,
        "ten" => 2,
        other => bail!("unknown record profile {other} for proof config"),
    })
}

fn profile_name(code: u8) -> Result<&'static str> {
    Ok(match code {
        0 => "single45",
        1 => "lp",
        2 => "ten",
        other => bail!("unknown record profile code {other}"),
    })
}

fn write_leb128(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CellRole {
    Structural,
    /// Index into `ProofConfig::tone_spans`, and the palette index.
    Palette {
        span: usize,
        index: usize,
    },
    /// RGB-cube fallback for non-toned records.
    RgbCube {
        index: usize,
    },
}

/// One swatch cell: an annular sector of a groove ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwatchCell {
    pub ring: usize,
    pub sector: usize,
    pub role: CellRole,
    pub color: [u8; 3],
}

/// What a corner-local pixel of a mini record is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Element {
    Outside,
    Spindle,
    Label { crosshair: bool },
    Groove { ring: usize, sector: usize },
    Rim { notch: bool },
}

/// Number of groove rings.
pub fn ring_count() -> usize {
    ((RIM_INNER_RADIUS - GROOVE_INNER_RADIUS) as usize) / RING_WIDTH
}

/// Sectors in groove ring `ring` (0 = innermost).
pub fn sector_count(ring: usize) -> usize {
    let mid = GROOVE_INNER_RADIUS + (ring as f64 + 0.5) * RING_WIDTH as f64;
    ((2.0 * PI * mid) / RING_WIDTH as f64).floor() as usize
}

/// Classifies a corner-local pixel.
pub fn element_at(lx: usize, ly: usize) -> Element {
    let dx = lx as f64 + 0.5 - MINI_CENTER;
    let dy = ly as f64 + 0.5 - MINI_CENTER;
    let r = (dx * dx + dy * dy).sqrt();
    if r >= MINI_OUTER_RADIUS {
        return Element::Outside;
    }
    if r < SPINDLE_RADIUS {
        return Element::Spindle;
    }
    if r < GROOVE_INNER_RADIUS {
        return Element::Label {
            crosshair: dx.abs() < 0.5 || dy.abs() < 0.5,
        };
    }
    if r < RIM_INNER_RADIUS {
        let ring = ((r - GROOVE_INNER_RADIUS) / RING_WIDTH as f64) as usize;
        let ring = ring.min(ring_count() - 1);
        let theta = dy.atan2(dx).rem_euclid(2.0 * PI);
        let n = sector_count(ring);
        let sector = ((theta / (2.0 * PI)) * n as f64) as usize;
        return Element::Groove {
            ring,
            sector: sector.min(n - 1),
        };
    }
    // Rim: notch where the pixel lies within NOTCH_HALF_WIDTH of an axis.
    let notch = dx.abs() < NOTCH_HALF_WIDTH || dy.abs() < NOTCH_HALF_WIDTH;
    Element::Rim { notch }
}

/// The fully resolved layout: the config plus every swatch cell (shared by
/// all three swatch corners) and the QR module matrix.
#[derive(Debug, Clone)]
pub struct ProofLayout {
    pub config: ProofConfig,
    pub cells: Vec<SwatchCell>,
    pub qr_modules: usize,
    pub qr_module_px: usize,
    qr: Vec<bool>,
}

impl ProofLayout {
    /// Total swatch slots across all groove rings.
    pub fn cell_capacity() -> usize {
        (0..ring_count()).map(sector_count).sum()
    }

    pub fn for_descriptor(descriptor: &RecordDescriptor) -> Result<Self> {
        let spans = ProofConfig::from_descriptor(descriptor, RING_WIDTH, 0).tone_spans;
        let palettes = spans
            .iter()
            .enumerate()
            .map(|(i, span)| {
                TonedPalette::shared(span.groove_config())
                    .with_context(|| format!("failed to rebuild palette for tone span {i}"))
            })
            .collect::<Result<Vec<_>>>()?;
        let capacity = Self::cell_capacity() - STRUCTURAL_SWATCHES.len();

        // Enumerate every palette colour if they all fit; otherwise sample
        // each palette evenly.
        let full_swatches: usize = if palettes.is_empty() {
            rgb_cube_swatches().len()
        } else {
            palettes.iter().map(|p| p.len()).sum()
        };
        let samples_per_span = if full_swatches <= capacity {
            palettes.iter().map(|p| p.len()).max().unwrap_or(0)
        } else {
            let per_span = capacity / palettes.len().max(1);
            if per_span < 2 {
                bail!(
                    "{} distinct tone spans do not fit a mini record's {capacity} swatch cells",
                    palettes.len()
                );
            }
            per_span
        };
        let config = ProofConfig::from_descriptor(descriptor, RING_WIDTH, samples_per_span);
        let swatches = swatch_colors(&palettes, samples_per_span);

        let mut slots =
            (0..ring_count()).flat_map(|ring| (0..sector_count(ring)).map(move |s| (ring, s)));
        let mut cells = Vec::with_capacity(swatches.len());
        for (role, color) in swatches {
            let (ring, sector) = slots.next().context("swatch ring overflow")?;
            cells.push(SwatchCell {
                ring,
                sector,
                role,
                color,
            });
        }

        let qr_bytes = config.to_bytes()?;
        let qr = QrCode::with_error_correction_level(&qr_bytes, EcLevel::M)
            .context("failed to build proof QR code")?;
        let qr_modules = qr.width();
        // The QR sits on the white label inside the rim; its diagonal must
        // clear the rim's inner radius with a one-module quiet zone.
        let inscribed = (2.0 * (RIM_INNER_RADIUS - 1.0) / 2f64.sqrt()).floor() as usize;
        let qr_module_px = inscribed / (qr_modules + 2);
        if qr_module_px == 0 {
            bail!("proof config QR ({qr_modules} modules) does not fit the mini record label");
        }
        let qr = qr
            .to_colors()
            .into_iter()
            .map(|c| c == qrcode::Color::Dark)
            .collect();

        Ok(Self {
            config,
            cells,
            qr_modules,
            qr_module_px,
            qr,
        })
    }

    /// Cell index for a corner-local pixel, if it lies on a painted swatch.
    pub fn cell_at(&self, lx: usize, ly: usize) -> Option<usize> {
        match element_at(lx, ly) {
            Element::Groove { ring, sector } => self
                .cells
                .iter()
                .position(|c| c.ring == ring && c.sector == sector),
            _ => None,
        }
    }

    /// Image pixels of `cell` in `corner`.
    pub fn cell_pixels(&self, corner: Corner, cell: &SwatchCell) -> Vec<(usize, usize)> {
        let mut out = Vec::with_capacity(RING_WIDTH * RING_WIDTH);
        for ly in 0..MINI_SIDE {
            for lx in 0..MINI_SIDE {
                if element_at(lx, ly)
                    == (Element::Groove {
                        ring: cell.ring,
                        sector: cell.sector,
                    })
                {
                    out.push(corner.to_image(lx, ly));
                }
            }
        }
        out
    }

    /// Paints the layout into a 576×576 RGBA buffer. Only pixels that are
    /// fully transparent and outside the disc are ever written.
    pub fn paint(&self, rgba: &mut [u8], outer_radius: i32) -> Result<PaintStats> {
        if rgba.len() != RECORD_SIZE * RECORD_SIZE * 4 {
            bail!(
                "expected a {RECORD_SIZE}×{RECORD_SIZE} RGBA buffer, got {} bytes",
                rgba.len()
            );
        }
        let mut stats = PaintStats::default();
        let mut put = |x: usize, y: usize, color: [u8; 3], stats: &mut PaintStats| -> Result<()> {
            let center = (RECORD_SIZE as f64) / 2.0;
            let dx = x as f64 + 0.5 - center;
            let dy = y as f64 + 0.5 - center;
            if (dx * dx + dy * dy).sqrt() <= f64::from(outer_radius) + 1.0 {
                bail!("proof layout would touch the disc at ({x}, {y})");
            }
            let i = (y * RECORD_SIZE + x) * 4;
            if rgba[i + 3] != 0 {
                bail!("proof layout would overwrite an opaque pixel at ({x}, {y})");
            }
            rgba[i..i + 3].copy_from_slice(&color);
            rgba[i + 3] = 255;
            stats.painted_pixels += 1;
            Ok(())
        };

        let cell_color = |ring: usize, sector: usize| -> Option<[u8; 3]> {
            self.cells
                .iter()
                .find(|c| c.ring == ring && c.sector == sector)
                .map(|c| c.color)
        };

        let qr_side = self.qr_modules * self.qr_module_px;
        let qr_origin = MINI_CENTER - qr_side as f64 / 2.0;

        for corner in [Corner::TopLeft, Corner::TopRight, Corner::BottomLeft, Corner::BottomRight] {
            for ly in 0..MINI_SIDE {
                for lx in 0..MINI_SIDE {
                    let element = element_at(lx, ly);
                    let color = match (corner, element) {
                        (_, Element::Outside | Element::Spindle) => continue,
                        (_, Element::Rim { notch }) => {
                            if notch {
                                [255, 255, 255]
                            } else {
                                [0, 0, 0]
                            }
                        }
                        // Top-left: QR on a white label filling the disc.
                        (Corner::TopLeft, _) => {
                            let qx = lx as f64 + 0.5 - qr_origin;
                            let qy = ly as f64 + 0.5 - qr_origin;
                            let dark = qx >= 0.0
                                && qy >= 0.0
                                && (qx as usize) < qr_side
                                && (qy as usize) < qr_side
                                && self.qr[(qy as usize / self.qr_module_px) * self.qr_modules
                                    + qx as usize / self.qr_module_px];
                            if dark {
                                [0, 0, 0]
                            } else {
                                [255, 255, 255]
                            }
                        }
                        (_, Element::Label { crosshair }) => {
                            if crosshair {
                                [0, 0, 0]
                            } else {
                                [255, 255, 255]
                            }
                        }
                        (_, Element::Groove { ring, sector }) => {
                            // Unused slots stay white so they read as label.
                            cell_color(ring, sector).unwrap_or([255, 255, 255])
                        }
                    };
                    let (x, y) = corner.to_image(lx, ly);
                    put(x, y, color, &mut stats)?;
                }
            }
        }
        stats.swatch_cells = self.cells.len();
        stats.qr_bytes = self.config.to_bytes()?.len();
        Ok(stats)
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaintStats {
    pub painted_pixels: usize,
    pub swatch_cells: usize,
    pub qr_bytes: usize,
}

fn swatch_colors(
    palettes: &[std::sync::Arc<TonedPalette>],
    samples_per_span: usize,
) -> Vec<(CellRole, [u8; 3])> {
    let mut out: Vec<(CellRole, [u8; 3])> = STRUCTURAL_SWATCHES
        .iter()
        .map(|&c| (CellRole::Structural, c))
        .collect();
    if palettes.is_empty() {
        out.extend(
            rgb_cube_swatches()
                .into_iter()
                .enumerate()
                .map(|(index, c)| (CellRole::RgbCube { index }, c)),
        );
        return out;
    }
    for (span, palette) in palettes.iter().enumerate() {
        for index in sampled_palette_indices(palette.len(), samples_per_span) {
            out.push((CellRole::Palette { span, index }, palette.color(index)));
        }
    }
    out
}

/// Result of [`add_print_calibration`].
#[derive(Debug, Clone)]
pub struct ProofRecord {
    pub png: Vec<u8>,
    pub config: ProofConfig,
    pub stats: PaintStats,
}

/// Reads a record PNG, decodes its descriptor from the spiral, and returns a
/// new PNG with the disc untouched, the background still transparent, and
/// calibration targets in the four corners.
pub fn add_print_calibration(record_png: &[u8]) -> Result<ProofRecord> {
    let image = image::load_from_memory(record_png)
        .context("failed to decode record PNG")?
        .to_rgba8();
    let (width, height) = image.dimensions();
    if width as usize != RECORD_SIZE || height as usize != RECORD_SIZE {
        bail!("expected a {RECORD_SIZE}×{RECORD_SIZE} record PNG, got {width}×{height}");
    }
    let mut rgba = image.into_raw();

    let (profile, descriptor) = decode_record_descriptor_from_png(record_png, None)
        .context("failed to decode record descriptor")?;
    let geometry = describe_record_profile(&profile)?;

    let layout = ProofLayout::for_descriptor(&descriptor)?;
    let stats = layout.paint(&mut rgba, geometry.outer_radius)?;

    let png = encode_png(&rgba)?;
    Ok(ProofRecord {
        png,
        config: layout.config,
        stats,
    })
}

fn encode_png(rgba: &[u8]) -> Result<Vec<u8>> {
    use image::ImageEncoder;
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new_with_quality(
        &mut out,
        image::codecs::png::CompressionType::Best,
        image::codecs::png::FilterType::Adaptive,
    )
    .write_image(
        rgba,
        RECORD_SIZE as u32,
        RECORD_SIZE as u32,
        image::ExtendedColorType::Rgba8,
    )
    .context("failed to encode proof PNG")?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use record_cut::{
        encode_record_stream, PayloadDescriptorInput, PayloadEntryInput, RecordStreamInput,
        TrackInput,
    };
    use record_decode::decode_record_png_to_chunk_stream;
    use record_render::render_chunk_stream_to_png;

    const WESTSIDE_DURATION_SECONDS: f64 = 208.509396;

    fn golden_ecdc(id: &str) -> Vec<u8> {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../goldenfiles/records")
            .join(id)
            .join(format!("{id}.ecdc"));
        std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    fn chunk_stream(payload: Vec<u8>) -> Vec<u8> {
        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput::from_container("ECDC")],
            tracks: vec![TrackInput {
                title: "Test Track".into(),
                first_revolution_index: None,
                revolution_count: None,
            }],
            track_gaps: vec![],
        };
        let entries = vec![PayloadEntryInput {
            payload_descriptor_index: 0,
            bytes: payload,
        }];
        encode_record_stream(&input, &entries).unwrap()
    }

    /// Renders (id, profile) with an optional groove tone; returns (stream, png).
    fn render(id: &str, profile: &str, tone: Option<&str>) -> (Vec<u8>, Vec<u8>) {
        let stream = chunk_stream(golden_ecdc(id));
        let options = tone.map(|t| serde_json::json!({ "grooveToneColor": t }).to_string());
        let out = render_chunk_stream_to_png(
            &stream,
            profile,
            WESTSIDE_DURATION_SECONDS,
            options.as_deref(),
        )
        .unwrap();
        (stream, out.png_bytes)
    }

    const CASES: [(&str, &str, Option<&str>); 3] = [
        (
            "lori-asha-westside-single45-hq",
            "single45",
            Some("#FFC0CB"),
        ),
        ("lori-asha-westside-lp-hq", "lp", Some("#1F3A5F")),
        ("lori-asha-westside-single45-hq", "single45", None),
    ];

    #[test]
    fn config_round_trips_through_bytes() {
        let config = ProofConfig {
            layout_version: PROOF_LAYOUT_VERSION,
            record_profile: "lp".into(),
            corner_side: MINI_SIDE as u8,
            ring_width: 6,
            structural_count: 11,
            payload_encoding: "toned-v1".into(),
            samples_per_span: 77,
            tone_spans: vec![
                ProofToneSpan {
                    base: [200, 30, 40],
                    luma_tolerance: 3,
                    bits_per_pixel: 5,
                    ordering: ToneOrdering::ChromaProximity,
                    byte_length: 123_456,
                },
                ProofToneSpan {
                    base: [1, 2, 3],
                    luma_tolerance: 0,
                    bits_per_pixel: 1,
                    ordering: ToneOrdering::BaseProximity,
                    byte_length: 0,
                },
            ],
        };
        let bytes = config.to_bytes().unwrap();
        assert_eq!(ProofConfig::from_bytes(&bytes).unwrap(), config);
    }

    #[test]
    fn records_still_decode_after_calibration() {
        for (id, profile, tone) in CASES {
            let (stream, original) = render(id, profile, tone);
            let proof = add_print_calibration(&original).unwrap();
            assert!(proof.stats.painted_pixels > 0);
            assert_eq!(proof.config.record_profile, profile);
            if tone.is_some() {
                assert!(
                    !proof.config.tone_spans.is_empty(),
                    "{id} should carry palettes"
                );
                assert!(proof.config.tone_spans.iter().any(|s| s.bits_per_pixel > 0));
            } else {
                assert!(proof.config.tone_spans.is_empty());
            }

            let (profile_b, stream_b) = decode_record_png_to_chunk_stream(&proof.png).unwrap();
            assert_eq!(profile_b, profile);
            assert_eq!(stream_b.bytes, stream, "{id} {tone:?}");

            // Disc pixels are byte-identical; only transparent corners changed.
            let before = image::load_from_memory(&original).unwrap().to_rgba8();
            let after = image::load_from_memory(&proof.png).unwrap().to_rgba8();
            let mut changed = 0usize;
            for (b, a) in before.pixels().zip(after.pixels()) {
                if b.0[3] != 0 {
                    assert_eq!(b, a);
                } else if a.0[3] != 0 {
                    changed += 1;
                }
            }
            assert_eq!(changed, proof.stats.painted_pixels);

            // The QR payload decodes back to the config we painted.
            let bytes = proof.config.to_bytes().unwrap();
            assert_eq!(ProofConfig::from_bytes(&bytes).unwrap(), proof.config);
        }
    }

    #[test]
    fn palette_sampling_is_even_and_covers_ends() {
        assert_eq!(sampled_palette_indices(5, 10), vec![0, 1, 2, 3, 4]);
        let s = sampled_palette_indices(1 << 20, 77);
        assert_eq!(s.len(), 77);
        assert_eq!(s[0], 0);
        assert_eq!(*s.last().unwrap(), (1 << 20) - 1);
        assert!(s.windows(2).all(|w| w[1] > w[0]));
    }

    #[test]
    fn layout_is_deterministic_and_within_corner() {
        let (_, png) = render(CASES[0].0, CASES[0].1, CASES[0].2);
        let (_, descriptor) = decode_record_descriptor_from_png(&png, None).unwrap();
        let a = ProofLayout::for_descriptor(&descriptor).unwrap();
        let b = ProofLayout::for_descriptor(&descriptor).unwrap();
        assert_eq!(a.cells, b.cells);
        assert!(a.cells.len() <= ProofLayout::cell_capacity());
        // Every cell is reachable, has a sensible pixel footprint, and
        // classifies back to itself.
        for (i, cell) in a.cells.iter().enumerate() {
            let pixels = a.cell_pixels(Corner::TopRight, cell);
            assert!(pixels.len() >= 20 && pixels.len() <= 60, "cell {i}: {} px", pixels.len());
            let (x, y) = pixels[0];
            let (lx, ly) = (RECORD_SIZE - 1 - x, y);
            assert_eq!(a.cell_at(lx, ly), Some(i));
        }
        // Every painted palette swatch matches the palette the decoder rebuilds.
        for cell in &a.cells {
            if let CellRole::Palette { span, index } = cell.role {
                let palette =
                    TonedPalette::shared(a.config.tone_spans[span].groove_config()).unwrap();
                assert_eq!(palette.color(index), cell.color);
            }
        }
    }
}
