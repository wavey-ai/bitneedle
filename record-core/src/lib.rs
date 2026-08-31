//! Public Bitneedle record wire-format, geometry, decoding, and verification primitives.
//!
//! This crate is the authoritative implementation of the public BRD1/BRS1 wire
//! contract. It intentionally contains no application workflow identifiers,
//! record-creation policy, placeholder signing, JSON stream metadata, legacy JSON support, or presentation masking.

pub mod chunk;
pub mod commitment;
pub mod ecdc;
pub mod gap;
pub mod tracks;
pub mod varuint;

use anyhow::{anyhow, bail, Context, Result};
use chacha20poly1305::aead::{Aead, KeyInit, Payload as AeadPayload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;
use std::ops::Range;

pub const DEFAULT_WIDTH: usize = 576;
pub const DEFAULT_HEIGHT: usize = 576;
pub const RECORD_SIZE: usize = 576;
pub const DEFAULT_START_ANGLE: f64 = PI / 2.0;
pub const MIN_B_VALUE: f64 = 1e-7;
pub const DEFAULT_MIN_PERCEPTIBLE_TURN_GAP: f64 = 2.0;
pub const DEFAULT_HARD_MIN_PERCEPTIBLE_TURN_GAP: f64 = 0.9;
pub const PAYLOAD_CODE_FORMAT_RGB: &str = "rgb";
pub const PAYLOAD_ENCODING_RGB: &str = "rgb";
pub const HEADER_SPIRAL_TURNS: f64 = 2.0;
pub const TRAILER_SPIRAL_TURNS: f64 = 4.0;
pub const HEADER_SPIRAL_OUTER_EDGE_INSET: i32 = 1;
pub const METADATA_GRAYSCALE_NIBBLE_BASE: u8 = 120;
pub const KNOWN_RECORD_PROFILES: &[&str] = &["single45", "lp"];

pub const RECORD_STREAM_MAGIC: &[u8; 4] = b"BRS1";
pub const RECORD_STREAM_HEADER_LENGTH: usize = 8;
// The optional typed PayloadDescriptor fields (codec, sample rate, channels,
// decoded-block output geometry) and the length-prefixed `codec_metadata` blob
// are signalled through previously-reserved per-descriptor flag bits, which are
// now promoted to DESCRIPTOR_KNOWN_FLAGS. Descriptors that set no new flags
// encode and decode identically to before, so the metadata version is unchanged.
pub const RECORD_STREAM_METADATA_VERSION: u8 = 2;

pub const METADATA_FLAG_ENCRYPTED: u8 = 0x01;
pub const METADATA_FLAG_ENTRY_DESCRIPTOR_INDEXES: u8 = 0x02;
pub const METADATA_FLAG_TRACK_ENTRY_MAPPINGS: u8 = 0x04;
/// Presence of the explicit track-gap section (count + per-gap
/// first_revolution_index/revolution_count/after_track_index), written after
/// the track section. Absent on older streams, which have no track gaps.
pub const METADATA_FLAG_TRACK_GAPS: u8 = 0x08;
pub const METADATA_KNOWN_FLAGS: u8 = METADATA_FLAG_ENCRYPTED
    | METADATA_FLAG_ENTRY_DESCRIPTOR_INDEXES
    | METADATA_FLAG_TRACK_ENTRY_MAPPINGS
    | METADATA_FLAG_TRACK_GAPS;

/// Per-descriptor presence flags for the optional typed fields. Each set bit is
/// followed, in this fixed order, by the corresponding field in the descriptor
/// body (see `decode_record_stream_metadata`).
pub const DESCRIPTOR_FLAG_CODEC: u8 = 0x01;
pub const DESCRIPTOR_FLAG_SAMPLE_RATE: u8 = 0x02;
pub const DESCRIPTOR_FLAG_CHANNELS: u8 = 0x04;
/// The three decoded-block output-geometry fields are signalled together by a
/// single bit: `block_samples`, `output_offset_samples`, `output_samples`.
pub const DESCRIPTOR_FLAG_OUTPUT_GEOMETRY: u8 = 0x08;
pub const DESCRIPTOR_FLAG_CODEC_METADATA: u8 = 0x10;

pub const DESCRIPTOR_KNOWN_FLAGS: u8 = DESCRIPTOR_FLAG_CODEC
    | DESCRIPTOR_FLAG_SAMPLE_RATE
    | DESCRIPTOR_FLAG_CHANNELS
    | DESCRIPTOR_FLAG_OUTPUT_GEOMETRY
    | DESCRIPTOR_FLAG_CODEC_METADATA;

/// Upper bound on the length-prefixed `codec_metadata` JSON blob.
pub const MAX_CODEC_METADATA_BYTES: usize = 64 * 1024;

pub const CONTAINER_ECDC: u8 = 1;
pub const CONTAINER_MOSS_NANO: u8 = 2;
pub const CONTAINER_EXTENSION: u8 = 255;

pub const CHUNK_CRC32_LENGTH: usize = 4;

pub const MAX_CHUNKS: usize = u16::MAX as usize;
pub const MAX_PAYLOAD_DESCRIPTORS: usize = u8::MAX as usize;
pub const DEFAULT_PAYLOAD_DESCRIPTOR_INDEX: u8 = 0;

pub const CHUNK_ENCRYPTION_DOMAIN: &[u8] = b"bitneedle.record-stream.chunk-encryption.v1";
pub const CHUNK_ENCRYPTION_ALGORITHM_CHACHA20POLY1305: &str = "chacha20poly1305";
pub const CHUNK_ENCRYPTION_NONCE_LENGTH: usize = 12;
pub const CHUNK_ENCRYPTION_TAG_LENGTH: usize = 16;
pub const CHUNK_ENCRYPTION_KEY_LENGTH: usize = 32;

pub const PAYLOAD_CONTAINER_ECDC: &str = "ECDC";
pub const PAYLOAD_CONTAINER_MOSS_NANO: &str = "MOSSNANO";
/// Intentional PCM silence in the playable record timeline.
///
/// Each GAP payload entry is a canonical, versioned `GAP1` payload (see the
/// [`gap`] module). The payload is authoritative for its own sample count, total
/// byte length, and deterministic filler seed; the descriptor supplies only the
/// sample rate and channel count.
///
/// GAP entries occupy timeline positions but are not musical tracks and must
/// not be covered by `TrackDescriptor` ranges.
pub const PAYLOAD_CONTAINER_GAP: &str = "GAP";
pub const PAYLOAD_CODEC_GAP: &str = "GAP";

const MAX_WIRE_STRING_BYTES: usize = u16::MAX as usize;

#[derive(Debug, Clone, Copy)]
struct RecordProfileDef {
    name: &'static str,
    spindle_hole_radius: i32,
    dink_radius: Option<i32>,
    label_radius: i32,
    label_clearance: i32,
    outer_radius: i32,
    outer_rim_thickness: i32,
    lead_in_band_thickness: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordProfileGeometry {
    pub record_profile: String,
    pub spindle_hole_radius: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dink_radius: Option<i32>,
    pub label_radius: i32,
    pub payload_inner_radius: i32,
    pub payload_outer_radius: i32,
    pub outer_radius: i32,
    pub outer_rim_thickness: i32,
    pub lead_in_band_thickness: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub payload: Vec<u8>,
    pub crc32: u32,
    pub nonce: Option<[u8; CHUNK_ENCRYPTION_NONCE_LENGTH]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PayloadDescriptor {
    pub container: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<u8>,
    /// Number of PCM samples per channel produced by decoding one complete
    /// codec block before any logical-output crop is applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_samples: Option<u32>,
    /// PCM sample offset per channel into the decoded block where the logical
    /// Bitneedle payload begins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_offset_samples: Option<u32>,
    /// Number of PCM samples per channel retained as the logical Bitneedle
    /// payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_samples: Option<u32>,
    /// UTF-8 JSON bytes containing codec/container-specific metadata that does
    /// not have a generic typed representation in `PayloadDescriptor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec_metadata: Option<Vec<u8>>,
}

impl PayloadDescriptor {
    /// A bare descriptor that carries only a container name (no codec, sample
    /// rate, channels, output geometry, or codec metadata).
    pub fn from_container(container: impl Into<String>) -> Self {
        Self {
            container: container.into(),
            codec: None,
            sample_rate: None,
            channels: None,
            block_samples: None,
            output_offset_samples: None,
            output_samples: None,
            codec_metadata: None,
        }
    }

    pub fn gap(sample_rate: u32, channels: u8) -> Result<Self> {
        let descriptor = Self {
            container: PAYLOAD_CONTAINER_GAP.to_owned(),
            codec: Some(PAYLOAD_CODEC_GAP.to_owned()),
            sample_rate: Some(sample_rate),
            channels: Some(channels),
            block_samples: None,
            output_offset_samples: None,
            output_samples: None,
            codec_metadata: None,
        };

        validate_payload_descriptor(&descriptor)?;
        Ok(descriptor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PayloadEntryDescriptor {
    pub byte_length: usize,
    pub payload_descriptor_index: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackDescriptor {
    pub title: String,
    pub first_revolution_index: usize,
    pub revolution_count: usize,
}

/// An explicit inter-track programme gap: real, independently decodable
/// audio (typically ambient near-silence) that is intentionally not part of
/// any track. `after_track_index` is the 0-based index into `tracks` this
/// gap immediately follows. See [`crate::tracks`] for the coverage rule this
/// is validated against: every playable revolution is covered by exactly one
/// track or track gap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackGapDescriptor {
    pub first_revolution_index: u32,
    pub revolution_count: u32,
    pub after_track_index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordStreamMetadata {
    pub version: u8,
    pub encrypted: bool,
    pub payload_descriptors: Vec<PayloadDescriptor>,
    pub payload_entries: Vec<PayloadEntryDescriptor>,
    pub tracks: Vec<TrackDescriptor>,
    #[serde(default)]
    pub track_gaps: Vec<TrackGapDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedPayloadEntry {
    pub index: usize,
    pub byte_offset: usize,
    pub byte_length: usize,
    pub payload_descriptor_index: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordStreamInspection {
    pub format: &'static str,
    pub version: u8,
    pub encrypted: bool,
    pub metadata_byte_length: usize,
    pub payload_byte_length: usize,
    pub payload_entry_count: usize,
    pub chunk_count: usize,
    pub payload_descriptors: Vec<PayloadDescriptorInspection>,
    pub payload_entries: Vec<PayloadEntryInspection>,
    pub tracks: Vec<TrackInspection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayloadDescriptorInspection {
    pub index: usize,
    pub container: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_samples: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_offset_samples: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_samples: Option<u32>,
    /// Byte length of the codec metadata blob, if present. The blob itself is
    /// not dumped here (it may contain long hashes); verbose inspectors can
    /// parse it separately.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec_metadata_byte_length: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayloadEntryInspection {
    pub index: usize,
    pub byte_offset: usize,
    pub byte_length: usize,
    pub payload_descriptor_index: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackInspection {
    pub number: usize,
    pub title: String,
    pub first_revolution_index: usize,
    pub revolution_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordStream {
    /// Parsed semantic metadata.
    pub metadata: RecordStreamMetadata,

    /// Exact signed metadata bytes from the wire.
    pub metadata_bytes: Vec<u8>,

    pub chunks: Vec<Chunk>,
}

pub type ChunkStream = RecordStream;

#[derive(Debug, Clone)]
struct RecordGeometry {
    record_profile: String,
    spindle_hole_radius: i32,
    dink_radius: Option<i32>,
    label_radius: i32,
    label_clearance: i32,
    outer_radius: i32,
    outer_rim_thickness: i32,
    lead_in_band_thickness: i32,
}

#[derive(Debug, Clone)]
pub struct SpiralMask {
    pub b_value: f64,
    pub spiral_family: SpiralFamily,
    pub record_profile: String,
    pub label_radius: i32,
    pub label_clearance: i32,
    pub outer_radius: i32,
    pub kinds: Vec<u8>,
    pub addressable_pixel_count: usize,
    pub ordered_pixel_indices: Vec<usize>,
}

pub const SPIRAL_FAMILY_ARCHIMEDEAN_CODE: u8 = 0;
pub const SPIRAL_FAMILY_VARI_PITCH_CODE: u8 = 1;

/// The strongest vari-pitch modulation the format admits. Above this the
/// tightest groove spacing falls below ~55% of the base pitch and adjacent
/// turns start to shear visually.
pub const VARI_PITCH_MAX_DEPTH: f64 = 0.45;

/// The groove geometry family a record is cut with.
///
/// `Archimedean` is the strict constant-pitch cut every v2 record uses:
/// `r = outer − b·θ`. `VariPitch` is the house v3 cut: the same base pitch
/// `b`, modulated the way a cutting lathe's vari-pitch head breathes — slow
/// bands of tighter and wider spacing drifting across the disc. The bands
/// come from two superimposed modulation waves whose periods span several
/// revolutions each, so every turn stays round; only the spacing between
/// turns moves. The periods are drawn from deliberately non-integer,
/// mutually incommensurate ranges so spacing extremes precess around the
/// disc instead of lining up into radial moiré rays.
///
/// `depth` is the groove character: the modulation amplitude as a fraction
/// of the base pitch (0 = strict Archimedean spacing, capped at
/// [`VARI_PITCH_MAX_DEPTH`]). `seed` makes the cut unique: the high 32 bits
/// choose the character of the banding (periods and phases), the low 32
/// bits nudge it microscopically (±2% period, small phase drift) — so a
/// pressing flow that keeps the high bits and re-rolls the low bits gives
/// every pressing of the same cut its own, very slightly different, groove.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "family", rename_all = "camelCase")]
pub enum SpiralFamily {
    #[default]
    Archimedean,
    #[serde(rename_all = "camelCase")]
    VariPitch { depth: f64, seed: u64 },
}

/// `depth` is validated finite at every encode and decode boundary, so the
/// NaN hole in `f64: PartialEq` cannot be observed through this type.
impl Eq for SpiralFamily {}

impl SpiralFamily {
    pub fn wire_code(&self) -> u8 {
        match self {
            SpiralFamily::Archimedean => SPIRAL_FAMILY_ARCHIMEDEAN_CODE,
            SpiralFamily::VariPitch { .. } => SPIRAL_FAMILY_VARI_PITCH_CODE,
        }
    }

    pub fn is_archimedean(&self) -> bool {
        matches!(self, SpiralFamily::Archimedean)
    }

    pub fn validate(&self) -> Result<()> {
        if let SpiralFamily::VariPitch { depth, .. } = self {
            if !(depth.is_finite() && *depth > 0.0 && *depth <= VARI_PITCH_MAX_DEPTH) {
                bail!(
                    "vari-pitch depth must be finite, positive, and at most {VARI_PITCH_MAX_DEPTH}"
                );
            }
        }
        Ok(())
    }
}

/// The resolved modulation a vari-pitch seed derives to. Periods are in
/// radians of sweep; phases in radians; weights sum to 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VariPitchParams {
    pub depth: f64,
    pub period_1: f64,
    pub period_2: f64,
    pub phase_1: f64,
    pub phase_2: f64,
    pub weight_1: f64,
    pub weight_2: f64,
}

const VARI_PITCH_WEIGHT_1: f64 = 0.62;
const VARI_PITCH_WEIGHT_2: f64 = 0.38;
const TAU: f64 = 2.0 * PI;

/// splitmix64: the derivation PRNG behind a vari-pitch seed. Frozen: this
/// exact sequence is part of the v3 format — a decoder regenerates the cut
/// from the seed alone, so the mapping can never change under family code
/// [`SPIRAL_FAMILY_VARI_PITCH_CODE`].
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn unit_from_draw(draw: u64) -> f64 {
    (draw >> 11) as f64 / (1u64 << 53) as f64
}

/// Derives the exact modulation a seed stands for. Deterministic and
/// frozen; see [`SpiralFamily::VariPitch`] for the high/low bit split.
pub fn vari_pitch_params(depth: f64, seed: u64) -> VariPitchParams {
    let mut character = seed >> 32;
    let phase_1 = TAU * unit_from_draw(splitmix64(&mut character));
    let phase_2 = TAU * unit_from_draw(splitmix64(&mut character));
    // Non-integer turn-count bands, mutually incommensurate: the short wave
    // lives between 4.7 and 6.1 revolutions, the long between 11.3 and 14.3.
    let period_1_turns = 4.7 + 1.4 * unit_from_draw(splitmix64(&mut character));
    let period_2_turns = 11.3 + 3.0 * unit_from_draw(splitmix64(&mut character));

    let mut micro = (seed & 0xFFFF_FFFF) | 0x5EED_0000_0000_0000;
    let period_1_jitter = 1.0 + 0.02 * (unit_from_draw(splitmix64(&mut micro)) - 0.5) * 2.0;
    let period_2_jitter = 1.0 + 0.02 * (unit_from_draw(splitmix64(&mut micro)) - 0.5) * 2.0;
    let phase_1_drift = 0.05 * (unit_from_draw(splitmix64(&mut micro)) - 0.5) * 2.0;
    let phase_2_drift = 0.05 * (unit_from_draw(splitmix64(&mut micro)) - 0.5) * 2.0;

    VariPitchParams {
        depth,
        period_1: TAU * period_1_turns * period_1_jitter,
        period_2: TAU * period_2_turns * period_2_jitter,
        phase_1: phase_1 + phase_1_drift,
        phase_2: phase_2 + phase_2_drift,
        weight_1: VARI_PITCH_WEIGHT_1,
        weight_2: VARI_PITCH_WEIGHT_2,
    }
}

impl VariPitchParams {
    /// The effective sweep Θ(θ): radius is `outer − b·Θ(θ)`. Built so
    /// Θ(0) = 0 (the cut starts exactly at the outer radius) and
    /// dΘ/dθ = 1 + depth·(w₁ sin(θ/P₁+φ₁) + w₂ sin(θ/P₂+φ₂)) ≥ 1 − depth.
    pub fn theta_effective(&self, theta: f64) -> f64 {
        theta
            + self.depth
                * (self.weight_1
                    * self.period_1
                    * (self.phase_1.cos() - (theta / self.period_1 + self.phase_1).cos())
                    + self.weight_2
                        * self.period_2
                        * (self.phase_2.cos() - (theta / self.period_2 + self.phase_2).cos()))
    }

    /// dΘ/dθ at `theta`: the local pitch is `b` times this.
    pub fn pitch_factor(&self, theta: f64) -> f64 {
        1.0 + self.depth
            * (self.weight_1 * (theta / self.period_1 + self.phase_1).sin()
                + self.weight_2 * (theta / self.period_2 + self.phase_2).sin())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadPixelEncoding {
    Rgb24,
    Toned { bits_per_pixel: u32 },
}

pub fn known_record_profile_names() -> &'static [&'static str] {
    KNOWN_RECORD_PROFILES
}

pub fn normalize_record_profile_name(record_profile: &str) -> Result<String> {
    let name = match record_profile.trim() {
        "single45" => "single45",
        "lp" => "lp",
        _ => bail!("Unknown record profile: {record_profile}"),
    };
    Ok(name.to_string())
}

fn record_profile_def(record_profile: &str) -> Result<RecordProfileDef> {
    #[derive(Clone, Copy)]
    struct PhysicalProfile {
        name: &'static str,
        finished_diameter_mm: f64,
        margin_diameter_mm: f64,
        outer_recorded_diameter_mm: f64,
        inner_recorded_diameter_mm: f64,
        label_diameter_mm: f64,
        spindle_hole_diameter_mm: f64,
        dink_diameter_mm: Option<f64>,
        outer_radius_px: i32,
    }

    fn scaled_radius_mm(outer: i32, finished_mm: f64, feature_mm: f64) -> i32 {
        let scale = outer as f64 / (finished_mm / 2.0);
        ((feature_mm / 2.0) * scale).round() as i32
    }

    let physical = match normalize_record_profile_name(record_profile)?.as_str() {
        "single45" => PhysicalProfile {
            name: "single45",
            finished_diameter_mm: 174.6,
            margin_diameter_mm: 172.2,
            outer_recorded_diameter_mm: 168.3,
            inner_recorded_diameter_mm: 107.95,
            label_diameter_mm: 92.1,
            spindle_hole_diameter_mm: 7.5,
            dink_diameter_mm: Some(38.1),
            outer_radius_px: 287,
        },
        "lp" => PhysicalProfile {
            name: "lp",
            finished_diameter_mm: 301.6,
            margin_diameter_mm: 297.6,
            outer_recorded_diameter_mm: 292.1,
            inner_recorded_diameter_mm: 120.65,
            label_diameter_mm: 100.0,
            spindle_hole_diameter_mm: 7.24,
            dink_diameter_mm: None,
            outer_radius_px: 287,
        },
        _ => unreachable!(),
    };

    let label_radius = scaled_radius_mm(
        physical.outer_radius_px,
        physical.finished_diameter_mm,
        physical.label_diameter_mm,
    );
    let spindle_hole_radius = scaled_radius_mm(
        physical.outer_radius_px,
        physical.finished_diameter_mm,
        physical.spindle_hole_diameter_mm,
    )
    .max(1);
    let dink_radius = physical.dink_diameter_mm.map(|diameter| {
        scaled_radius_mm(
            physical.outer_radius_px,
            physical.finished_diameter_mm,
            diameter,
        )
    });
    let authentic_inner = scaled_radius_mm(
        physical.outer_radius_px,
        physical.finished_diameter_mm,
        physical.inner_recorded_diameter_mm,
    );
    let authentic_outer = scaled_radius_mm(
        physical.outer_radius_px,
        physical.finished_diameter_mm,
        physical.outer_recorded_diameter_mm,
    );
    let margin_radius = scaled_radius_mm(
        physical.outer_radius_px,
        physical.finished_diameter_mm,
        physical.margin_diameter_mm,
    );
    let payload_inner = match physical.name {
        "single45" => label_radius + 18,
        "lp" => label_radius + 14,
        _ => authentic_inner,
    };
    let payload_outer = match physical.name {
        "single45" | "lp" => (margin_radius - 6).max(1),
        _ => authentic_outer,
    };

    Ok(RecordProfileDef {
        name: physical.name,
        spindle_hole_radius,
        dink_radius,
        label_radius,
        label_clearance: (payload_inner - label_radius).max(1),
        outer_radius: physical.outer_radius_px,
        outer_rim_thickness: (physical.outer_radius_px - margin_radius).max(1),
        lead_in_band_thickness: (margin_radius - payload_outer).max(1),
    })
}

fn resolve_record_geometry(
    record_profile: &str,
    label_radius: Option<i32>,
    label_clearance: Option<i32>,
    outer_radius: Option<i32>,
) -> Result<RecordGeometry> {
    let p = record_profile_def(record_profile)?;
    Ok(RecordGeometry {
        record_profile: p.name.to_string(),
        spindle_hole_radius: p.spindle_hole_radius,
        dink_radius: p.dink_radius,
        label_radius: label_radius.unwrap_or(p.label_radius),
        label_clearance: label_clearance.unwrap_or(p.label_clearance),
        outer_radius: outer_radius.unwrap_or(p.outer_radius),
        outer_rim_thickness: p.outer_rim_thickness,
        lead_in_band_thickness: p.lead_in_band_thickness,
    })
}

fn header_outer_radius(g: &RecordGeometry) -> i32 {
    (g.outer_radius - HEADER_SPIRAL_OUTER_EDGE_INSET).max(1)
}

fn payload_outer_radius(g: &RecordGeometry) -> i32 {
    (header_outer_radius(g) - g.lead_in_band_thickness).max(1)
}

fn payload_inner_radius(g: &RecordGeometry) -> i32 {
    g.label_radius + g.label_clearance
}

pub fn payload_outer_radius_from_geometry(g: &RecordProfileGeometry) -> i32 {
    g.payload_outer_radius
}

pub fn payload_inner_radius_from_geometry(g: &RecordProfileGeometry) -> i32 {
    g.payload_inner_radius
}

pub fn label_clearance_from_profile_geometry(g: &RecordProfileGeometry) -> i32 {
    g.payload_inner_radius - g.label_radius
}

pub fn label_sidecar_inner_radius_from_profile_geometry(g: &RecordProfileGeometry) -> i32 {
    g.spindle_hole_radius + 3
}

pub fn label_sidecar_outer_radius_from_profile_geometry(g: &RecordProfileGeometry) -> i32 {
    (g.label_radius - 3).max(label_sidecar_inner_radius_from_profile_geometry(g) + 1)
}

pub fn describe_record_profile(record_profile: &str) -> Result<RecordProfileGeometry> {
    let g = resolve_record_geometry(record_profile, None, None, None)?;
    Ok(RecordProfileGeometry {
        record_profile: g.record_profile.clone(),
        spindle_hole_radius: g.spindle_hole_radius,
        dink_radius: g.dink_radius,
        label_radius: g.label_radius,
        payload_inner_radius: payload_inner_radius(&g),
        payload_outer_radius: payload_outer_radius(&g),
        outer_radius: g.outer_radius,
        outer_rim_thickness: g.outer_rim_thickness,
        lead_in_band_thickness: g.lead_in_band_thickness,
    })
}

pub fn visible_spiral_turns(record_profile: &str, b_value: f64) -> Result<f64> {
    if b_value <= 0.0 {
        bail!("A positive spiral pitch is required.");
    }

    let g = resolve_record_geometry(record_profile, None, None, None)?;

    Ok(
        (payload_outer_radius(&g) as f64 - payload_inner_radius(&g) as f64).max(0.0)
            / (2.0 * PI * b_value),
    )
}

pub fn spiral_b_value_for_visible_turns(record_profile: &str, turns: f64) -> Result<f64> {
    if !(turns.is_finite() && turns > 0.0) {
        bail!("A positive finite visible turn count is required.");
    }

    let g = resolve_record_geometry(record_profile, None, None, None)?;

    Ok(
        ((payload_outer_radius(&g) as f64 - payload_inner_radius(&g) as f64).max(0.0)
            / (2.0 * PI * turns))
            .max(MIN_B_VALUE),
    )
}

pub fn resolve_pitch(b_value: f64, pitch: Option<f64>) -> Result<f64> {
    let value = pitch.unwrap_or(b_value);

    if value <= 0.0 {
        bail!("A positive spiral pitch is required.");
    }

    Ok(value)
}

pub fn js_round(value: f64) -> i32 {
    (value + 0.5).floor() as i32
}

pub fn payload_pixel_count_for_encoding(
    byte_length: usize,
    encoding: PayloadPixelEncoding,
) -> Result<usize> {
    match encoding {
        PayloadPixelEncoding::Rgb24 => Ok(byte_length.div_ceil(3)),
        PayloadPixelEncoding::Toned { bits_per_pixel } => {
            if !(1..=24).contains(&bits_per_pixel) {
                bail!("toned bits per pixel must be between 1 and 24");
            }

            let bits = byte_length
                .checked_mul(8)
                .context("payload bit length overflow")?;

            Ok(bits.div_ceil(bits_per_pixel as usize))
        }
    }
}

pub fn rgb24_pixel_count_for_byte_length(byte_length: usize) -> usize {
    byte_length.div_ceil(3)
}

pub fn metadata_pixel_count_for_byte_length(byte_length: usize) -> usize {
    byte_length.saturating_mul(2)
}

pub fn metadata_byte_capacity_for_pixel_count(pixel_count: usize) -> usize {
    pixel_count / 2
}

fn header_spiral_pitch_for_geometry(g: &RecordGeometry) -> f64 {
    (header_outer_radius(g) - payload_outer_radius(g)).max(1) as f64
        / (2.0 * PI * HEADER_SPIRAL_TURNS.max(0.01))
}

fn trailer_spiral_pitch_for_geometry(g: &RecordGeometry) -> f64 {
    (payload_inner_radius(g) - g.label_radius).max(1) as f64
        / (2.0 * PI * TRAILER_SPIRAL_TURNS.max(0.01))
}

pub fn header_spiral_pitch_for_profile(record_profile: &str) -> Result<f64> {
    Ok(header_spiral_pitch_for_geometry(&resolve_record_geometry(
        record_profile,
        None,
        None,
        None,
    )?))
}

pub fn trailer_spiral_pitch_for_profile(record_profile: &str) -> Result<f64> {
    Ok(trailer_spiral_pitch_for_geometry(&resolve_record_geometry(
        record_profile,
        None,
        None,
        None,
    )?))
}

#[allow(clippy::too_many_arguments)]
pub fn trace_record_spiral(
    width: usize,
    height: usize,
    b_value: f64,
    pitch: Option<f64>,
    start_angle: f64,
    pixel_gap: f64,
    clockwise: bool,
    trace_outer_radius: f64,
    trace_inner_radius: f64,
) -> Result<(Vec<u8>, Vec<usize>, f64, f64)> {
    trace_record_spiral_with_family(
        width,
        height,
        b_value,
        &SpiralFamily::Archimedean,
        pitch,
        start_angle,
        pixel_gap,
        clockwise,
        trace_outer_radius,
        trace_inner_radius,
    )
}

/// Traces the groove for any [`SpiralFamily`].
///
/// The Archimedean arm performs arithmetic identical to the historical
/// trace — every v2 record's pixel order is preserved bit for bit. The
/// vari-pitch arm cuts `r = outer − b·Θ(θ)` with Θ derived from the seed
/// (see [`vari_pitch_params`]); its mean pitch is the same `b`, so
/// capacity fitting against `b` carries over.
#[allow(clippy::too_many_arguments)]
pub fn trace_record_spiral_with_family(
    width: usize,
    height: usize,
    b_value: f64,
    family: &SpiralFamily,
    pitch: Option<f64>,
    start_angle: f64,
    pixel_gap: f64,
    clockwise: bool,
    trace_outer_radius: f64,
    trace_inner_radius: f64,
) -> Result<(Vec<u8>, Vec<usize>, f64, f64)> {
    family.validate()?;

    let center_x = width as f64 / 2.0;
    let center_y = height as f64 / 2.0;
    let resolved_pitch = resolve_pitch(b_value, pitch)?;
    let outer = trace_outer_radius.min(width.min(height) as f64 / 2.0 - 1.0);
    let inner = trace_inner_radius.max(0.0);
    let mut occupied = vec![0u8; width * height];
    let mut ordered = Vec::new();
    let mut theta = 0.0;
    let mut angle = start_angle;
    let mut radius = outer;

    let vari = match family {
        SpiralFamily::Archimedean => None,
        SpiralFamily::VariPitch { depth, seed } => Some(vari_pitch_params(*depth, *seed)),
    };

    while radius >= inner {
        let x = js_round(center_x + radius * angle.cos());
        let y = js_round(center_y - radius * angle.sin());

        if x >= 0 && x < width as i32 && y >= 0 && y < height as i32 {
            let i = y as usize * width + x as usize;

            if occupied[i] == 0 {
                occupied[i] = 1;
                ordered.push(i);
            }
        }

        let local_pitch = match &vari {
            None => resolved_pitch,
            Some(params) => resolved_pitch * params.pitch_factor(theta),
        };
        let step = pixel_gap
            / (radius * radius + local_pitch * local_pitch)
                .sqrt()
                .max(1e-6);

        theta += step;
        angle = start_angle + if clockwise { -theta } else { theta };
        radius = match &vari {
            None => outer - resolved_pitch * theta,
            Some(params) => outer - resolved_pitch * params.theta_effective(theta),
        };
    }

    Ok((occupied, ordered, center_x, center_y))
}

#[allow(clippy::too_many_arguments)]
pub fn build_band_spiral_indices(
    width: usize,
    height: usize,
    record_profile: &str,
    label_radius: Option<i32>,
    label_clearance: Option<i32>,
    outer_radius: Option<i32>,
    band_outer_radius: f64,
    band_inner_radius: f64,
    band_pitch: f64,
) -> Result<Vec<usize>> {
    let _ = resolve_record_geometry(record_profile, label_radius, label_clearance, outer_radius)?;

    let (occupied, traced, cx, cy) = trace_record_spiral(
        width,
        height,
        band_pitch,
        None,
        DEFAULT_START_ANGLE,
        1.0,
        true,
        band_outer_radius,
        band_inner_radius,
    )?;

    Ok(traced
        .into_iter()
        .filter(|&i| {
            if occupied[i] == 0 {
                return false;
            }

            let x = i % width;
            let y = i / width;
            let distance = ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt();

            distance > band_inner_radius && distance < band_outer_radius
        })
        .collect())
}

pub fn build_header_spiral_indices(
    width: usize,
    height: usize,
    record_profile: &str,
    label_radius: Option<i32>,
    label_clearance: Option<i32>,
    outer_radius: Option<i32>,
) -> Result<Vec<usize>> {
    let g = resolve_record_geometry(record_profile, label_radius, label_clearance, outer_radius)?;

    build_band_spiral_indices(
        width,
        height,
        record_profile,
        Some(g.label_radius),
        Some(g.label_clearance),
        Some(g.outer_radius),
        header_outer_radius(&g) as f64,
        payload_outer_radius(&g) as f64,
        header_spiral_pitch_for_geometry(&g),
    )
}

pub fn build_trailer_spiral_indices(
    width: usize,
    height: usize,
    record_profile: &str,
    label_radius: Option<i32>,
    label_clearance: Option<i32>,
    outer_radius: Option<i32>,
) -> Result<Vec<usize>> {
    let g = resolve_record_geometry(record_profile, label_radius, label_clearance, outer_radius)?;

    build_band_spiral_indices(
        width,
        height,
        record_profile,
        Some(g.label_radius),
        Some(g.label_clearance),
        Some(g.outer_radius),
        payload_inner_radius(&g) as f64,
        g.label_radius as f64,
        trailer_spiral_pitch_for_geometry(&g),
    )
}

pub fn build_spiral_mask(
    width: usize,
    height: usize,
    b_value: f64,
    record_profile: &str,
    label_radius: Option<i32>,
    label_clearance: Option<i32>,
    outer_radius: Option<i32>,
) -> Result<SpiralMask> {
    build_spiral_mask_with_family(
        width,
        height,
        b_value,
        &SpiralFamily::Archimedean,
        record_profile,
        label_radius,
        label_clearance,
        outer_radius,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn build_spiral_mask_with_family(
    width: usize,
    height: usize,
    b_value: f64,
    family: &SpiralFamily,
    record_profile: &str,
    label_radius: Option<i32>,
    label_clearance: Option<i32>,
    outer_radius: Option<i32>,
) -> Result<SpiralMask> {
    let g = resolve_record_geometry(record_profile, label_radius, label_clearance, outer_radius)?;
    let outer = payload_outer_radius(&g);
    let inner = payload_inner_radius(&g);

    let (occupied, traced, cx, cy) = trace_record_spiral_with_family(
        width,
        height,
        b_value,
        family,
        None,
        DEFAULT_START_ANGLE,
        1.0,
        true,
        outer as f64,
        0.0,
    )?;

    let mut kinds = vec![0u8; width * height];
    let mut ordered = Vec::new();

    for i in traced {
        if occupied[i] == 0 {
            continue;
        }

        let distance =
            (((i % width) as f64 - cx).powi(2) + ((i / width) as f64 - cy).powi(2)).sqrt();

        if distance <= inner as f64 {
            continue;
        }

        if distance < outer as f64 {
            kinds[i] = 1;
            ordered.push(i);
        } else {
            kinds[i] = 2;
        }
    }

    Ok(SpiralMask {
        b_value,
        spiral_family: *family,
        record_profile: g.record_profile,
        label_radius: g.label_radius,
        label_clearance: g.label_clearance,
        outer_radius: g.outer_radius,
        addressable_pixel_count: ordered.len(),
        ordered_pixel_indices: ordered,
        kinds,
    })
}

pub fn read_u16be(bytes: &[u8], offset: usize, label: &str) -> Result<u16> {
    let end = offset.checked_add(2).context("u16 offset overflow")?;
    let slice = bytes
        .get(offset..end)
        .with_context(|| format!("{label} is truncated"))?;

    Ok(u16::from_be_bytes(
        slice.try_into().expect("length checked"),
    ))
}

pub fn read_u32be(bytes: &[u8], offset: usize, label: &str) -> Result<u32> {
    let end = offset.checked_add(4).context("u32 offset overflow")?;
    let slice = bytes
        .get(offset..end)
        .with_context(|| format!("{label} is truncated"))?;

    Ok(u32::from_be_bytes(
        slice.try_into().expect("length checked"),
    ))
}

fn push_u32be(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

pub fn concatenate_payload_entries(entries: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(entries.iter().map(Vec::len).sum());

    for entry in entries {
        out.extend_from_slice(entry);
    }

    out
}

pub fn validate_payload_entry_bytes(descriptor: &PayloadDescriptor, bytes: &[u8]) -> Result<()> {
    if descriptor
        .container
        .eq_ignore_ascii_case(PAYLOAD_CONTAINER_GAP)
    {
        gap::validate_gap_payload(bytes)?;
    }

    Ok(())
}

pub fn crc32_ieee(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;

    for &byte in bytes {
        crc ^= byte as u32;

        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }

    !crc
}

pub fn record_stream_header_end(bytes: &[u8]) -> Result<usize> {
    if bytes.len() < RECORD_STREAM_HEADER_LENGTH {
        bail!("record stream is too small");
    }

    if &bytes[..4] != RECORD_STREAM_MAGIC {
        bail!("record stream magic is invalid");
    }

    let metadata_len = read_u32be(bytes, 4, "record stream metadata length")? as usize;
    let end = RECORD_STREAM_HEADER_LENGTH
        .checked_add(metadata_len)
        .context("record stream metadata length overflow")?;

    if end > bytes.len() {
        bail!("record stream metadata length exceeds stream length");
    }

    Ok(end)
}

pub fn stream_header_end(bytes: &[u8]) -> Result<usize> {
    record_stream_header_end(bytes)
}

pub fn record_stream_metadata_bytes(bytes: &[u8]) -> Result<&[u8]> {
    let end = record_stream_header_end(bytes)?;
    Ok(&bytes[RECORD_STREAM_HEADER_LENGTH..end])
}

pub fn stream_metadata_bytes(bytes: &[u8]) -> Result<&[u8]> {
    record_stream_metadata_bytes(bytes)
}

#[derive(Debug, Clone, Copy)]
struct WireCursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> WireCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn remaining(self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }

    fn read_u8(&mut self, label: &str) -> Result<u8> {
        let value = *self
            .bytes
            .get(self.offset)
            .with_context(|| format!("{label} is truncated"))?;
        self.offset += 1;
        Ok(value)
    }

    fn read_u16be(&mut self, label: &str) -> Result<u16> {
        let value = read_u16be(self.bytes, self.offset, label)?;
        self.offset += 2;
        Ok(value)
    }

    fn read_u32be(&mut self, label: &str) -> Result<u32> {
        let value = read_u32be(self.bytes, self.offset, label)?;
        self.offset += 4;
        Ok(value)
    }

    fn read_bytes(&mut self, length: usize, label: &str) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(length)
            .with_context(|| format!("{label} length overflow"))?;
        let value = self
            .bytes
            .get(self.offset..end)
            .with_context(|| format!("{label} is truncated"))?;
        self.offset = end;
        Ok(value)
    }

    fn read_string(&mut self, label: &str) -> Result<String> {
        let length = self.read_u16be(&format!("{label} length"))? as usize;

        if length > MAX_WIRE_STRING_BYTES {
            bail!("{label} exceeds string length limit");
        }

        let bytes = self.read_bytes(length, label)?;
        let value =
            std::str::from_utf8(bytes).with_context(|| format!("{label} is not valid UTF-8"))?;

        if value.is_empty() {
            bail!("{label} must not be empty");
        }

        if value.chars().any(char::is_control) {
            bail!("{label} must not contain control characters");
        }

        Ok(value.to_owned())
    }

    fn read_varuint(&mut self, label: &str) -> Result<u64> {
        let start = self.offset;
        let mut value = 0u64;
        let mut shift = 0u32;

        for byte_index in 0..10 {
            let byte = self.read_u8(label)?;
            let payload = u64::from(byte & 0x7f);

            if shift == 63 && payload > 1 {
                bail!("{label} exceeds u64 range");
            }

            value |= payload
                .checked_shl(shift)
                .with_context(|| format!("{label} shift overflow"))?;

            if byte & 0x80 == 0 {
                let consumed = self.offset - start;

                if consumed > 1 {
                    let minimum = 1u64 << (7 * (consumed - 1));
                    if value < minimum {
                        bail!("{label} uses non-canonical overlong varuint encoding");
                    }
                }

                return Ok(value);
            }

            shift += 7;

            if byte_index == 9 {
                bail!("{label} exceeds ten-byte varuint limit");
            }
        }

        unreachable!()
    }
}

fn container_name(code: u8, cursor: &mut WireCursor<'_>) -> Result<String> {
    match code {
        CONTAINER_ECDC => Ok(PAYLOAD_CONTAINER_ECDC.to_string()),
        CONTAINER_MOSS_NANO => Ok(PAYLOAD_CONTAINER_MOSS_NANO.to_string()),
        CONTAINER_EXTENSION => cursor.read_string("extension container name"),
        _ => bail!("unknown payload container code {code}"),
    }
}

/// Validate `codec_metadata` bytes: they must be valid UTF-8, syntactically
/// valid JSON, and (when shaped as a JSON object) must not duplicate any of the
/// three typed output-geometry fields, which now have dedicated descriptor
/// fields. The value is not required to be a JSON object.
pub fn validate_codec_metadata(bytes: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(bytes).context("codec metadata is not valid UTF-8")?;
    let value: serde_json::Value =
        serde_json::from_str(text).context("codec metadata is not valid JSON")?;

    if let Some(object) = value.as_object() {
        for key in ["block_samples", "output_offset_samples", "output_samples"] {
            if object.contains_key(key) {
                bail!("codec metadata must not duplicate typed geometry field {key:?}");
            }
        }
    }

    Ok(())
}

/// Validate a single `PayloadDescriptor`:
///
/// * the three output-geometry fields are all present together or all absent;
/// * when present, `block_samples > 0`, `output_samples > 0`, and
///   `output_offset_samples + output_samples <= block_samples` (checked);
/// * `codec_metadata`, when present, satisfies [`validate_codec_metadata`].
pub fn validate_payload_descriptor(descriptor: &PayloadDescriptor) -> Result<()> {
    let geometry_present = [
        descriptor.block_samples,
        descriptor.output_offset_samples,
        descriptor.output_samples,
    ]
    .iter()
    .filter(|value| value.is_some())
    .count();

    match (
        geometry_present,
        descriptor.block_samples,
        descriptor.output_offset_samples,
        descriptor.output_samples,
    ) {
        (0, _, _, _) => {}
        (3, Some(block_samples), Some(output_offset_samples), Some(output_samples)) => {
            if block_samples == 0 {
                bail!("payload descriptor block_samples must be greater than zero");
            }
            if output_samples == 0 {
                bail!("payload descriptor output_samples must be greater than zero");
            }
            let end = output_offset_samples
                .checked_add(output_samples)
                .context("payload descriptor output crop range overflows")?;
            if end > block_samples {
                bail!(
                    "payload descriptor output crop range {output_offset_samples}..{end} \
                     exceeds block_samples {block_samples}"
                );
            }
        }
        _ => bail!(
            "payload descriptor output geometry requires block_samples, \
             output_offset_samples, and output_samples together"
        ),
    }

    if let Some(codec_metadata) = &descriptor.codec_metadata {
        validate_codec_metadata(codec_metadata).context("payload descriptor codec metadata")?;
    }

    if descriptor
        .container
        .eq_ignore_ascii_case(PAYLOAD_CONTAINER_GAP)
    {
        match descriptor.codec.as_deref() {
            Some(codec) if codec.eq_ignore_ascii_case(PAYLOAD_CODEC_GAP) => {}
            Some(_) => bail!("GAP payload descriptor codec must be GAP"),
            None => bail!("GAP payload descriptor requires the GAP codec"),
        }

        match descriptor.sample_rate {
            Some(sample_rate) if sample_rate > 0 => {}
            Some(_) => bail!("GAP payload descriptor sample rate must be greater than zero"),
            None => bail!("GAP payload descriptor requires a sample rate"),
        }

        match descriptor.channels {
            Some(channels) if channels > 0 => {}
            Some(_) => bail!("GAP payload descriptor channel count must be greater than zero"),
            None => bail!("GAP payload descriptor requires a channel count"),
        }

        if descriptor.block_samples.is_some()
            || descriptor.output_offset_samples.is_some()
            || descriptor.output_samples.is_some()
        {
            bail!("GAP payload descriptor must not specify output geometry");
        }

        if descriptor.codec_metadata.is_some() {
            bail!("GAP payload descriptor must not specify codec metadata");
        }
    }

    Ok(())
}

/// Verify that two payload descriptors are identical across every field. Used by
/// record construction to confirm that every revolution shares one descriptor
/// before storing it once. The error names the first differing field; callers
/// add the revolution index via `.with_context(...)`.
pub fn validate_shared_payload_descriptor(
    expected: &PayloadDescriptor,
    actual: &PayloadDescriptor,
) -> Result<()> {
    macro_rules! check {
        ($field:ident) => {
            if expected.$field != actual.$field {
                bail!(concat!(
                    "payload descriptor mismatch in field `",
                    stringify!($field),
                    "`"
                ));
            }
        };
    }

    check!(container);
    check!(codec);
    check!(sample_rate);
    check!(channels);
    check!(block_samples);
    check!(output_offset_samples);
    check!(output_samples);
    check!(codec_metadata);

    Ok(())
}

pub fn decode_record_stream_metadata(bytes: &[u8]) -> Result<RecordStreamMetadata> {
    let mut cursor = WireCursor::new(bytes);

    let version = cursor.read_u8("record stream metadata version")?;

    if version != RECORD_STREAM_METADATA_VERSION {
        bail!(
            "record stream metadata version mismatch: expected {}, got {}",
            RECORD_STREAM_METADATA_VERSION,
            version
        );
    }

    let flags = cursor.read_u8("record stream metadata flags")?;

    if flags & !METADATA_KNOWN_FLAGS != 0 {
        bail!("record stream metadata contains unknown flags");
    }

    let encrypted = flags & METADATA_FLAG_ENCRYPTED != 0;
    let has_entry_descriptor_indexes = flags & METADATA_FLAG_ENTRY_DESCRIPTOR_INDEXES != 0;
    let has_track_entry_mappings = flags & METADATA_FLAG_TRACK_ENTRY_MAPPINGS != 0;
    let has_track_gaps = flags & METADATA_FLAG_TRACK_GAPS != 0;

    let descriptor_count = cursor.read_u8("payload descriptor count")? as usize;

    if descriptor_count == 0 {
        bail!("payload descriptor count must not be zero");
    }

    if descriptor_count > MAX_PAYLOAD_DESCRIPTORS {
        bail!("payload descriptor count exceeds u8 range");
    }

    let mut payload_descriptors = Vec::with_capacity(descriptor_count);

    for descriptor_index in 0..descriptor_count {
        let container_code = cursor.read_u8("payload container code")?;
        let container = container_name(container_code, &mut cursor)?;
        let descriptor_flags = cursor.read_u8("payload descriptor flags")?;

        if descriptor_flags & !DESCRIPTOR_KNOWN_FLAGS != 0 {
            bail!("payload descriptor {descriptor_index} contains unknown flags");
        }

        let codec = if descriptor_flags & DESCRIPTOR_FLAG_CODEC != 0 {
            Some(cursor.read_string("payload descriptor codec")?)
        } else {
            None
        };

        let sample_rate = if descriptor_flags & DESCRIPTOR_FLAG_SAMPLE_RATE != 0 {
            Some(cursor.read_u32be("payload descriptor sample rate")?)
        } else {
            None
        };

        let channels = if descriptor_flags & DESCRIPTOR_FLAG_CHANNELS != 0 {
            Some(cursor.read_u8("payload descriptor channels")?)
        } else {
            None
        };

        let (block_samples, output_offset_samples, output_samples) =
            if descriptor_flags & DESCRIPTOR_FLAG_OUTPUT_GEOMETRY != 0 {
                (
                    Some(cursor.read_u32be("payload descriptor block samples")?),
                    Some(cursor.read_u32be("payload descriptor output offset samples")?),
                    Some(cursor.read_u32be("payload descriptor output samples")?),
                )
            } else {
                (None, None, None)
            };

        let codec_metadata = if descriptor_flags & DESCRIPTOR_FLAG_CODEC_METADATA != 0 {
            let length = cursor.read_u32be("payload descriptor codec metadata length")? as usize;
            if length > MAX_CODEC_METADATA_BYTES {
                bail!(
                    "payload descriptor {descriptor_index} codec metadata length {length} \
                     exceeds limit {MAX_CODEC_METADATA_BYTES}"
                );
            }
            Some(
                cursor
                    .read_bytes(length, "payload descriptor codec metadata")?
                    .to_vec(),
            )
        } else {
            None
        };

        let descriptor = PayloadDescriptor {
            container,
            codec,
            sample_rate,
            channels,
            block_samples,
            output_offset_samples,
            output_samples,
            codec_metadata,
        };
        validate_payload_descriptor(&descriptor)
            .with_context(|| format!("payload descriptor {descriptor_index} is invalid"))?;

        payload_descriptors.push(descriptor);
    }

    let entry_count = cursor.read_u16be("payload entry count")? as usize;

    if entry_count == 0 {
        bail!("payload entry count must not be zero");
    }

    let mut payload_entries = Vec::with_capacity(entry_count);

    for entry_index in 0..entry_count {
        let byte_length_u64 = cursor.read_varuint("payload entry byte length")?;
        let byte_length =
            usize::try_from(byte_length_u64).context("payload entry byte length exceeds usize")?;

        if byte_length == 0 {
            bail!("payload entry {entry_index} byte length must be greater than zero");
        }

        let payload_descriptor_index = if has_entry_descriptor_indexes {
            cursor.read_u8("payload entry descriptor index")?
        } else {
            DEFAULT_PAYLOAD_DESCRIPTOR_INDEX
        };

        validate_payload_descriptor_index(payload_descriptors.len(), payload_descriptor_index)?;

        payload_entries.push(PayloadEntryDescriptor {
            byte_length,
            payload_descriptor_index,
        });
    }

    let track_count = cursor.read_u16be("track count")? as usize;

    if track_count == 0 {
        bail!("track count must not be zero");
    }

    let mut tracks = Vec::with_capacity(track_count);

    for track_index in 0..track_count {
        let title = cursor.read_string("track title")?;
        let (first_revolution_index, revolution_count) = if has_track_entry_mappings {
            let first = usize::try_from(cursor.read_varuint("track first revolution index")?)
                .context("track first revolution index exceeds usize")?;
            let count = usize::try_from(cursor.read_varuint("track revolution count")?)
                .context("track revolution count exceeds usize")?;
            (first, count)
        } else {
            (track_index, 1)
        };

        if revolution_count == 0 {
            bail!("track {track_index} revolution count must be greater than zero");
        }

        let end = first_revolution_index
            .checked_add(revolution_count)
            .context("track revolution range overflows")?;

        if end > payload_entries.len() {
            bail!(
                "track {track_index} revolution range [{first_revolution_index}, {end}) is out of range for {} entries",
                payload_entries.len()
            );
        }

        tracks.push(TrackDescriptor {
            title,
            first_revolution_index,
            revolution_count,
        });
    }

    let mut track_gaps: Vec<TrackGapDescriptor> = Vec::new();

    if has_track_gaps {
        let track_gap_count = cursor.read_u16be("track gap count")? as usize;
        track_gaps.reserve(track_gap_count);

        for gap_index in 0..track_gap_count {
            let first_revolution_index =
                u32::try_from(cursor.read_varuint("track gap first revolution index")?)
                    .context("track gap first revolution index exceeds u32")?;
            let revolution_count =
                u32::try_from(cursor.read_varuint("track gap revolution count")?)
                    .context("track gap revolution count exceeds u32")?;
            let after_track_index =
                u32::try_from(cursor.read_varuint("track gap after track index")?)
                    .context("track gap after track index exceeds u32")?;

            if revolution_count == 0 {
                bail!("track gap {gap_index} revolution count must be greater than zero");
            }

            if after_track_index as usize >= tracks.len() {
                bail!(
                    "track gap {gap_index} after_track_index {after_track_index} is out of range for {} tracks",
                    tracks.len()
                );
            }

            let end = first_revolution_index
                .checked_add(revolution_count)
                .context("track gap revolution range overflows")?;

            if end as usize > payload_entries.len() {
                bail!(
                    "track gap {gap_index} revolution range [{first_revolution_index}, {end}) is out of range for {} entries",
                    payload_entries.len()
                );
            }

            if let Some(previous) = track_gaps.last() {
                if first_revolution_index <= previous.first_revolution_index {
                    bail!(
                        "track gap {gap_index} first_revolution_index {first_revolution_index} is not strictly ascending after the previous track gap's {}",
                        previous.first_revolution_index
                    );
                }
            }

            track_gaps.push(TrackGapDescriptor {
                first_revolution_index,
                revolution_count,
                after_track_index,
            });
        }
    }

    if cursor.remaining() != 0 {
        bail!(
            "record stream metadata contains {} trailing bytes",
            cursor.remaining()
        );
    }

    Ok(RecordStreamMetadata {
        version,
        encrypted,
        payload_descriptors,
        payload_entries,
        tracks,
        track_gaps,
    })
}

pub fn record_stream_metadata(bytes: &[u8]) -> Result<RecordStreamMetadata> {
    decode_record_stream_metadata(record_stream_metadata_bytes(bytes)?)
}

pub fn stream_metadata(bytes: &[u8]) -> Result<RecordStreamMetadata> {
    record_stream_metadata(bytes)
}

pub fn chunk_nonce_length_from_metadata(metadata: &RecordStreamMetadata) -> Result<Option<usize>> {
    Ok(metadata.encrypted.then_some(CHUNK_ENCRYPTION_NONCE_LENGTH))
}

pub fn chunk_nonce_length_from_metadata_bytes(bytes: &[u8]) -> Result<Option<usize>> {
    chunk_nonce_length_from_metadata(&decode_record_stream_metadata(bytes)?)
}

pub fn payload_descriptor_count_from_metadata(metadata: &RecordStreamMetadata) -> Result<usize> {
    if metadata.payload_descriptors.is_empty() {
        bail!("payload descriptor list must not be empty");
    }

    if metadata.payload_descriptors.len() > MAX_PAYLOAD_DESCRIPTORS {
        bail!("payload descriptor count exceeds u8 range");
    }

    Ok(metadata.payload_descriptors.len())
}

pub fn payload_descriptor_count_from_metadata_bytes(bytes: &[u8]) -> Result<usize> {
    payload_descriptor_count_from_metadata(&decode_record_stream_metadata(bytes)?)
}

pub fn validate_payload_descriptor_index(count: usize, index: u8) -> Result<()> {
    if index as usize >= count {
        bail!("payload descriptor index {index} is out of range for {count} descriptors");
    }

    Ok(())
}

pub fn resolve_payload_entries(
    entries: &[PayloadEntryDescriptor],
    descriptor_count: usize,
) -> Result<Vec<ResolvedPayloadEntry>> {
    resolve_payload_entries_with_total(entries, descriptor_count, None)
}

fn resolve_payload_entries_with_total(
    entries: &[PayloadEntryDescriptor],
    descriptor_count: usize,
    expected: Option<usize>,
) -> Result<Vec<ResolvedPayloadEntry>> {
    if entries.is_empty() {
        bail!("payload entry list must not be empty");
    }

    let mut offset = 0usize;
    let mut resolved = Vec::with_capacity(entries.len());

    for (index, entry) in entries.iter().enumerate() {
        if entry.byte_length == 0 {
            bail!("payload entry {index} byte length must be greater than zero");
        }

        validate_payload_descriptor_index(descriptor_count, entry.payload_descriptor_index)?;

        resolved.push(ResolvedPayloadEntry {
            index,
            byte_offset: offset,
            byte_length: entry.byte_length,
            payload_descriptor_index: entry.payload_descriptor_index,
        });

        offset = offset
            .checked_add(entry.byte_length)
            .context("payload entry byte range overflow")?;
    }

    if let Some(expected) = expected {
        if offset != expected {
            bail!("payload entry byte ranges cover {offset} bytes, expected {expected}");
        }
    }

    Ok(resolved)
}

pub fn validate_payload_entries_metadata(
    metadata: &RecordStreamMetadata,
    expected_payload_byte_length: Option<usize>,
) -> Result<Vec<ResolvedPayloadEntry>> {
    resolve_payload_entries_with_total(
        &metadata.payload_entries,
        metadata.payload_descriptors.len(),
        expected_payload_byte_length,
    )
}

pub fn validate_track_listing_metadata(metadata: &RecordStreamMetadata) -> Result<()> {
    if metadata.tracks.is_empty() {
        bail!("track list must not be empty");
    }

    for (index, track) in metadata.tracks.iter().enumerate() {
        if track.title.trim().is_empty() {
            bail!("track {index} title must not be empty");
        }

        let end = track
            .first_revolution_index
            .checked_add(track.revolution_count)
            .context("track revolution range overflows")?;

        if end > metadata.payload_entries.len() {
            bail!(
                "track {index} revolution range is out of range for {} entries",
                metadata.payload_entries.len()
            );
        }
    }

    let is_playable_revolution: Vec<bool> = metadata
        .payload_entries
        .iter()
        .map(|entry| {
            metadata
                .payload_descriptors
                .get(entry.payload_descriptor_index as usize)
                .map(|descriptor| {
                    descriptor
                        .container
                        .eq_ignore_ascii_case(PAYLOAD_CONTAINER_ECDC)
                        || descriptor
                            .container
                            .eq_ignore_ascii_case(PAYLOAD_CONTAINER_MOSS_NANO)
                })
                .unwrap_or(false)
        })
        .collect();

    if !is_playable_revolution.iter().any(|&playable| playable) {
        return Ok(());
    }

    let ranges: Vec<crate::tracks::TrackRange> = metadata
        .tracks
        .iter()
        .map(|track| crate::tracks::TrackRange {
            title: track.title.clone(),
            first_revolution_index: track.first_revolution_index as u64,
            revolution_count: track.revolution_count as u64,
        })
        .collect();

    let gap_ranges: Vec<crate::tracks::TrackGapRange> = metadata
        .track_gaps
        .iter()
        .map(|gap| crate::tracks::TrackGapRange {
            first_revolution_index: gap.first_revolution_index as u64,
            revolution_count: gap.revolution_count as u64,
            after_track_index: gap.after_track_index as usize,
        })
        .collect();

    crate::tracks::validate_track_ranges(&ranges, &gap_ranges, &is_playable_revolution)
}

pub fn inspect_record_stream(document: &RecordStream) -> Result<RecordStreamInspection> {
    let resolved = resolve_payload_entries(
        &document.metadata.payload_entries,
        document.metadata.payload_descriptors.len(),
    )?;

    let payload_byte_length = resolved.iter().try_fold(0usize, |total, entry| {
        total
            .checked_add(entry.byte_length)
            .context("payload byte length overflow")
    })?;

    Ok(RecordStreamInspection {
        format: "bitneedle-record-stream",
        version: 1,
        encrypted: document.metadata.encrypted,
        metadata_byte_length: document.metadata_bytes.len(),
        payload_byte_length,
        payload_entry_count: resolved.len(),
        chunk_count: document.chunks.len(),
        payload_descriptors: document
            .metadata
            .payload_descriptors
            .iter()
            .enumerate()
            .map(|(index, descriptor)| PayloadDescriptorInspection {
                index,
                container: descriptor.container.clone(),
                codec: descriptor.codec.clone(),
                sample_rate: descriptor.sample_rate,
                channels: descriptor.channels,
                block_samples: descriptor.block_samples,
                output_offset_samples: descriptor.output_offset_samples,
                output_samples: descriptor.output_samples,
                codec_metadata_byte_length: descriptor.codec_metadata.as_ref().map(Vec::len),
            })
            .collect(),
        payload_entries: resolved
            .into_iter()
            .map(|entry| PayloadEntryInspection {
                index: entry.index,
                byte_offset: entry.byte_offset,
                byte_length: entry.byte_length,
                payload_descriptor_index: entry.payload_descriptor_index,
            })
            .collect(),
        tracks: document
            .metadata
            .tracks
            .iter()
            .enumerate()
            .map(|(index, track)| TrackInspection {
                number: index + 1,
                title: track.title.clone(),
                first_revolution_index: track.first_revolution_index,
                revolution_count: track.revolution_count,
            })
            .collect(),
    })
}

/// AAD for chunk encryption (final compact format): just a domain separator
/// over the exact BRS1 metadata bytes. There is no per-chunk index, count,
/// or descriptor index to bind, since none of those are stored on the wire.
pub fn chunk_encryption_aad(metadata_bytes: &[u8]) -> Result<Vec<u8>> {
    let metadata_len = u32::try_from(metadata_bytes.len()).context("metadata exceeds u32")?;

    let mut out = Vec::with_capacity(CHUNK_ENCRYPTION_DOMAIN.len() + 4 + metadata_bytes.len());
    out.extend_from_slice(CHUNK_ENCRYPTION_DOMAIN);
    push_u32be(&mut out, metadata_len);
    out.extend_from_slice(metadata_bytes);

    Ok(out)
}

pub fn decrypt_chunk_payload_chacha20poly1305(
    key: &[u8; CHUNK_ENCRYPTION_KEY_LENGTH],
    nonce: &[u8; CHUNK_ENCRYPTION_NONCE_LENGTH],
    aad: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>> {
    ChaCha20Poly1305::new(Key::from_slice(key))
        .decrypt(
            Nonce::from_slice(nonce),
            AeadPayload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| anyhow!("failed to decrypt chunk payload"))
}

pub fn decrypt_record_stream_payloads_chacha20poly1305(
    document: &RecordStream,
    key: &[u8; CHUNK_ENCRYPTION_KEY_LENGTH],
) -> Result<Vec<Vec<u8>>> {
    if !document.metadata.encrypted {
        bail!("record stream is not encrypted");
    }

    let aad = chunk_encryption_aad(&document.metadata_bytes)?;

    document
        .chunks
        .iter()
        .map(|chunk| {
            let nonce = chunk
                .nonce
                .as_ref()
                .context("encrypted chunk is missing nonce")?;

            decrypt_chunk_payload_chacha20poly1305(key, nonce, &aad, &chunk.payload)
        })
        .collect()
}

pub fn decrypt_chunk_stream_payloads_chacha20poly1305(
    document: &ChunkStream,
    key: &[u8; CHUNK_ENCRYPTION_KEY_LENGTH],
) -> Result<Vec<Vec<u8>>> {
    decrypt_record_stream_payloads_chacha20poly1305(document, key)
}

pub fn decrypt_record_stream_payload_bytes_chacha20poly1305(
    document: &RecordStream,
    key: &[u8; CHUNK_ENCRYPTION_KEY_LENGTH],
) -> Result<Vec<u8>> {
    Ok(concatenate_payload_entries(
        &decrypt_record_stream_payloads_chacha20poly1305(document, key)?,
    ))
}

pub fn decrypt_chunk_stream_payload_bytes_chacha20poly1305(
    document: &ChunkStream,
    key: &[u8; CHUNK_ENCRYPTION_KEY_LENGTH],
) -> Result<Vec<u8>> {
    decrypt_record_stream_payload_bytes_chacha20poly1305(document, key)
}

/// Verify that no chunk's bytes cross a payload-entry boundary. Skipped for
/// encrypted streams, since ciphertext lengths (plaintext + AEAD tag) don't
/// line up with the declared plaintext entry lengths.
fn validate_chunk_descriptors_against_entries(
    metadata: &RecordStreamMetadata,
    chunks: &[Chunk],
    plaintext_payload_length: Option<usize>,
) -> Result<()> {
    let resolved = validate_payload_entries_metadata(metadata, plaintext_payload_length)?;

    if metadata.encrypted {
        return Ok(());
    }

    let mut chunk_offset = 0usize;

    for (position, chunk) in chunks.iter().enumerate() {
        let chunk_end = chunk_offset
            .checked_add(chunk.payload.len())
            .context("chunk payload offset overflow")?;

        resolved
            .iter()
            .find(|entry| {
                let entry_end = entry.byte_offset + entry.byte_length;
                chunk_offset >= entry.byte_offset && chunk_end <= entry_end
            })
            .with_context(|| {
                format!(
                    "chunk {position} crosses payload-entry boundaries or lies outside the declared entries"
                )
            })?;

        chunk_offset = chunk_end;
    }

    Ok(())
}

pub fn parse_record_stream(bytes: &[u8]) -> Result<RecordStream> {
    let header_end = record_stream_header_end(bytes)?;
    let metadata_bytes = bytes[RECORD_STREAM_HEADER_LENGTH..header_end].to_vec();
    let metadata = decode_record_stream_metadata(&metadata_bytes)?;

    payload_descriptor_count_from_metadata(&metadata)?;
    validate_track_listing_metadata(&metadata)?;

    let encrypted = metadata.encrypted;
    let chunk_ranges = chunk::parse_chunk_section(bytes, header_end, encrypted)?;

    if chunk_ranges.is_empty() {
        bail!("record stream contains no chunks");
    }

    let mut chunks = Vec::with_capacity(chunk_ranges.len());

    for range in &chunk_ranges {
        if !chunk::verify_chunk_crc32(bytes, range) {
            bail!("chunk CRC32 mismatch");
        }

        chunks.push(Chunk {
            payload: bytes[range.payload_start..range.payload_end].to_vec(),
            crc32: range.crc32,
            nonce: range.nonce,
        });
    }

    let plaintext_payload_length = if encrypted {
        None
    } else {
        Some(chunks.iter().map(|chunk| chunk.payload.len()).sum())
    };

    validate_chunk_descriptors_against_entries(&metadata, &chunks, plaintext_payload_length)?;

    if !metadata.encrypted {
        let payload_bytes = chunks
            .iter()
            .flat_map(|chunk| chunk.payload.iter().copied())
            .collect::<Vec<_>>();

        let resolved = validate_payload_entries_metadata(&metadata, Some(payload_bytes.len()))?;

        for entry in resolved {
            let descriptor = &metadata.payload_descriptors[entry.payload_descriptor_index as usize];
            let end = entry
                .byte_offset
                .checked_add(entry.byte_length)
                .context("payload entry range overflow")?;

            validate_payload_entry_bytes(descriptor, &payload_bytes[entry.byte_offset..end])
                .with_context(|| format!("payload entry {} is invalid", entry.index))?;
        }
    }

    Ok(RecordStream {
        metadata,
        metadata_bytes,
        chunks,
    })
}

pub fn parse_chunk_stream(bytes: &[u8]) -> Result<ChunkStream> {
    parse_record_stream(bytes)
}

pub fn validate_record_stream(bytes: &[u8]) -> Result<()> {
    parse_record_stream(bytes).map(|_| ())
}

pub fn validate_chunk_stream(bytes: &[u8]) -> Result<()> {
    validate_record_stream(bytes)
}

pub fn chunk_all_ranges(bytes: &[u8]) -> Result<Vec<chunk::ChunkRanges>> {
    let header_end = record_stream_header_end(bytes)?;
    let metadata = record_stream_metadata(bytes)?;

    let ranges = chunk::parse_chunk_section(bytes, header_end, metadata.encrypted)?;

    if ranges.is_empty() {
        bail!("record stream contains no chunks");
    }

    Ok(ranges)
}

pub fn chunk_ranges(bytes: &[u8]) -> Result<Vec<Range<usize>>> {
    Ok(chunk_all_ranges(bytes)?
        .into_iter()
        .map(|ranges| ranges.chunk_start..ranges.chunk_end)
        .collect())
}

pub fn chunk_payload_ranges(bytes: &[u8]) -> Result<Vec<Range<usize>>> {
    Ok(chunk_all_ranges(bytes)?
        .into_iter()
        .map(|ranges| ranges.payload_start..ranges.payload_end)
        .collect())
}

pub fn record_stream_payload_bytes(document: &RecordStream) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        document
            .chunks
            .iter()
            .map(|chunk| chunk.payload.len())
            .sum(),
    );

    for chunk in &document.chunks {
        out.extend_from_slice(&chunk.payload);
    }

    out
}

pub fn chunk_stream_payload_bytes(document: &ChunkStream) -> Vec<u8> {
    record_stream_payload_bytes(document)
}

/// Kind of an ordered programme region in the pre-decode programme map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ProgrammeRegionKind {
    /// A musical track, identified by its 1-based number and title.
    Track { number: usize, title: String },
    /// An inter-track GAP, recorded against the preceding musical track number
    /// (0 when the programme opens with a gap).
    Gap { after_track_number: usize },
}

/// One ordered region of the programme with exact PCM sample boundaries,
/// recoverable without neural/PCM decoding.
///
/// `radial_start_normalized`/`radial_end_normalized` are the actual radial
/// positions of the region's payload pixels along the rendered Archimedean
/// spiral: `0.0` is the outer edge of the programme playback groove and `1.0`
/// the inner edge. They are derived from the equal-area spiral that the renderer
/// fills (each carrier pixel is one unit of annulus area, traversed outer→inner),
/// so they are true radial progress — not the encoded-byte fraction.
///
/// `byte_fraction_start`/`byte_fraction_end` are the raw carrier-byte fractions,
/// retained only as a diagnostic; they must not be used as a radial coordinate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgrammeRegion {
    #[serde(flatten)]
    pub kind: ProgrammeRegionKind,
    pub start_sample: u64,
    pub end_sample: u64,
    pub sample_count: u64,
    pub radial_start_normalized: f64,
    pub radial_end_normalized: f64,
    pub byte_fraction_start: f64,
    pub byte_fraction_end: f64,
}

/// The complete pre-decode programme map: exact musical/GAP sample boundaries
/// and total duration in samples, derived from BRS1 metadata plus payload bytes
/// without decoding any codec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgrammeMap {
    pub sample_rate: u32,
    pub channels: u8,
    pub total_samples: u64,
    pub regions: Vec<ProgrammeRegion>,
}

/// Exact per-channel PCM sample count represented by one payload entry,
/// without neural/PCM decoding.
///
/// ECDC entries are programme-time revolution units. Their compressed bytes are
/// packed continuously into the raster groove and do not correspond to one
/// geometric spiral turn. The shared descriptor's `output_samples` is therefore
/// the authoritative logical duration of an ECDC entry. The headerless entry
/// body is still parsed here so malformed framing is rejected.
///
/// GAP entries carry their own exact sample count in the GAP1 header.
fn entry_sample_count(descriptor: &PayloadDescriptor, bytes: &[u8]) -> Result<u64> {
    if descriptor
        .container
        .eq_ignore_ascii_case(PAYLOAD_CONTAINER_GAP)
    {
        let samples = gap::decode_gap_header(bytes)?.sample_count;
        if samples == 0 {
            bail!("GAP payload declares zero samples");
        }
        return Ok(samples);
    }

    if descriptor
        .container
        .eq_ignore_ascii_case(PAYLOAD_CONTAINER_ECDC)
    {
        // This currently provides the canonical headerless-frame structural
        // parse. Its returned sample count is deliberately ignored: logical
        // programme duration comes from output_samples, not compressed frame
        // count or codec metadata field `fl`.
        ecdc::headerless_entry_sample_count(bytes, descriptor)
            .context("invalid headerless ECDC payload entry")?;

        let output_samples = descriptor
            .output_samples
            .filter(|samples| *samples > 0)
            .context("ECDC payload descriptor is missing positive output_samples")?;
        return Ok(u64::from(output_samples));
    }

    if descriptor
        .container
        .eq_ignore_ascii_case(PAYLOAD_CONTAINER_MOSS_NANO)
    {
        let output_samples = descriptor
            .output_samples
            .filter(|samples| *samples > 0)
            .context("programme payload descriptor is missing positive output_samples")?;
        return Ok(u64::from(output_samples));
    }

    bail!(
        "cannot determine sample count for container `{}`",
        descriptor.container
    )
}

/// Build the ordered pre-decode programme map from a parsed record stream.
///
/// Consecutive musical payload entries covered by the same track collapse into
/// one track region; each GAP entry becomes its own region. Sample boundaries
/// are exact and cumulative, so `total_samples` divided by the sample rate is
/// the authoritative programme duration.
/// Map a normalized carrier-pixel fraction (0 at the outer edge, 1 at the inner
/// edge of the payload annulus) to a normalized radial position along the
/// rendered Archimedean spiral. The renderer fills the annulus uniformly — one
/// carrier pixel per unit area, traversed outer→inner — so equal pixel fractions
/// correspond to equal swept area, and `r(f) = sqrt(r_out² − f·(r_out² − r_in²))`.
fn spiral_radial_normalized(pixel_fraction: f64, inner_radius: f64, outer_radius: f64) -> f64 {
    let span = outer_radius * outer_radius - inner_radius * inner_radius;
    if !(span > 0.0) || outer_radius <= inner_radius {
        return pixel_fraction.clamp(0.0, 1.0);
    }
    let f = pixel_fraction.clamp(0.0, 1.0);
    let radius = (outer_radius * outer_radius - f * span).max(0.0).sqrt();
    ((outer_radius - radius) / (outer_radius - inner_radius)).clamp(0.0, 1.0)
}

pub fn build_programme_map(
    document: &RecordStream,
    record_profile: Option<&str>,
) -> Result<ProgrammeMap> {
    let metadata = &document.metadata;
    let resolved = resolve_payload_entries(
        &metadata.payload_entries,
        metadata.payload_descriptors.len(),
    )?;
    let payload_bytes = record_stream_payload_bytes(document);

    // Map each payload-entry index to the 1-based programme track that covers
    // it, or None when it is a track gap or an auxiliary entry outside both.
    let mut track_of_entry = vec![None; metadata.payload_entries.len()];
    for (track_index, track) in metadata.tracks.iter().enumerate() {
        let end = track
            .first_revolution_index
            .checked_add(track.revolution_count)
            .context("track revolution range overflows")?;
        for entry_index in track.first_revolution_index..end {
            if entry_index < track_of_entry.len() {
                track_of_entry[entry_index] = Some(track_index + 1);
            }
        }
    }

    // Map each payload-entry index to the 1-based track number its explicit
    // track gap follows. A gap's bytes are ordinary payload (commonly ECDC
    // ambient near-silence); classification comes from this metadata, not
    // from sniffing the descriptor's container.
    let mut gap_after_track_of_entry = vec![None; metadata.payload_entries.len()];
    for (gap_index, gap) in metadata.track_gaps.iter().enumerate() {
        let after_track_index = gap.after_track_index as usize;
        if after_track_index >= metadata.tracks.len() {
            bail!(
                "track gap {gap_index} after_track_index {after_track_index} is out of range for {} tracks",
                metadata.tracks.len()
            );
        }
        let first_revolution_index = gap.first_revolution_index as usize;
        let end = first_revolution_index
            .checked_add(gap.revolution_count as usize)
            .context("track gap revolution range overflows")?;
        for entry_index in first_revolution_index..end {
            if entry_index >= track_of_entry.len() {
                continue;
            }
            if track_of_entry[entry_index].is_some() {
                bail!(
                    "payload entry {entry_index} is covered by both a track and track gap {gap_index}"
                );
            }
            if gap_after_track_of_entry[entry_index].is_some() {
                bail!("payload entry {entry_index} is covered by more than one track gap");
            }
            gap_after_track_of_entry[entry_index] = Some(after_track_index + 1);
        }
    }

    let sample_rate = metadata
        .payload_descriptors
        .iter()
        .find_map(|d| d.sample_rate)
        .context("record stream has no sample rate")?;
    let channels = metadata
        .payload_descriptors
        .iter()
        .find_map(|d| d.channels)
        .context("record stream has no channel count")?;

    // Byte fraction == carrier-pixel fraction (3 bytes/pixel). The true radial
    // position comes from the equal-area spiral the renderer fills; without a
    // known profile we degrade to the linear pixel fraction.
    let total_payload_bytes = payload_bytes.len().max(1) as f64;
    let byte_fraction_of =
        |byte_offset: usize| (byte_offset as f64 / total_payload_bytes).clamp(0.0, 1.0);
    let radii = match record_profile {
        Some(profile) => {
            let geometry = describe_record_profile(profile)?;
            Some((
                f64::from(geometry.payload_inner_radius),
                f64::from(geometry.payload_outer_radius),
            ))
        }
        None => None,
    };
    let radial_of = |byte_offset: usize| {
        let f = byte_fraction_of(byte_offset);
        match radii {
            Some((inner, outer)) => spiral_radial_normalized(f, inner, outer),
            None => f,
        }
    };

    let mut regions: Vec<ProgrammeRegion> = Vec::new();
    let mut cursor_sample = 0u64;

    for entry in &resolved {
        let descriptor = &metadata.payload_descriptors[entry.payload_descriptor_index as usize];
        let end = entry
            .byte_offset
            .checked_add(entry.byte_length)
            .context("payload entry range overflow")?;
        let entry_bytes = payload_bytes
            .get(entry.byte_offset..end)
            .context("payload entry byte range is out of bounds")?;
        let samples = entry_sample_count(descriptor, entry_bytes)
            .with_context(|| format!("invalid payload entry {}", entry.index))?;
        if samples == 0 {
            bail!(
                "payload entry {} resolved to zero logical samples",
                entry.index
            );
        }
        let start_sample = cursor_sample;
        let end_sample = cursor_sample
            .checked_add(samples)
            .context("programme sample position overflow")?;
        cursor_sample = end_sample;
        let byte_fraction_start = byte_fraction_of(entry.byte_offset);
        let byte_fraction_end = byte_fraction_of(end);
        let radial_start_normalized = radial_of(entry.byte_offset);
        let radial_end_normalized = radial_of(end);

        match track_of_entry[entry.index] {
            Some(track_number) => {
                // Extend the current track region when it is the same track.
                if let Some(ProgrammeRegion {
                    kind: ProgrammeRegionKind::Track { number, .. },
                    end_sample: region_end,
                    sample_count,
                    radial_end_normalized: region_radial_end,
                    byte_fraction_end: region_byte_end,
                    ..
                }) = regions.last_mut()
                {
                    if *number == track_number {
                        *region_end = end_sample;
                        *sample_count = sample_count
                            .checked_add(samples)
                            .context("track region sample count overflow")?;
                        *region_radial_end = radial_end_normalized;
                        *region_byte_end = byte_fraction_end;
                        continue;
                    }
                }
                let title = metadata.tracks[track_number - 1].title.clone();
                regions.push(ProgrammeRegion {
                    kind: ProgrammeRegionKind::Track {
                        number: track_number,
                        title,
                    },
                    start_sample,
                    end_sample,
                    sample_count: samples,
                    radial_start_normalized,
                    radial_end_normalized,
                    byte_fraction_start,
                    byte_fraction_end,
                });
            }
            None => {
                let after_track_number =
                    gap_after_track_of_entry[entry.index].with_context(|| {
                        format!(
                            "payload entry {} is not covered by any track or track gap",
                            entry.index
                        )
                    })?;
                regions.push(ProgrammeRegion {
                    kind: ProgrammeRegionKind::Gap { after_track_number },
                    start_sample,
                    end_sample,
                    sample_count: samples,
                    radial_start_normalized,
                    radial_end_normalized,
                    byte_fraction_start,
                    byte_fraction_end,
                });
            }
        }
    }

    Ok(ProgrammeMap {
        sample_rate,
        channels,
        total_samples: cursor_sample,
        regions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_u16be(out: &mut Vec<u8>, value: u16) {
        out.extend_from_slice(&value.to_be_bytes());
    }

    fn push_varuint(out: &mut Vec<u8>, mut value: u64) {
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

    fn test_metadata() -> Vec<u8> {
        let mut out = Vec::new();
        out.push(RECORD_STREAM_METADATA_VERSION);
        out.push(0);
        out.push(1);
        out.push(CONTAINER_EXTENSION);
        push_u16be(&mut out, 4);
        out.extend_from_slice(b"TEST");
        out.push(0);
        push_u16be(&mut out, 2);
        push_varuint(&mut out, 9);
        push_varuint(&mut out, 16);
        push_u16be(&mut out, 2);
        push_u16be(&mut out, 3);
        out.extend_from_slice(b"One");
        push_u16be(&mut out, 3);
        out.extend_from_slice(b"Two");
        out
    }

    #[test]
    fn compact_binary_metadata_decodes() {
        let metadata = decode_record_stream_metadata(&test_metadata()).unwrap();

        assert_eq!(metadata.version, RECORD_STREAM_METADATA_VERSION);
        assert!(!metadata.encrypted);
        assert_eq!(metadata.payload_descriptors.len(), 1);
        assert_eq!(metadata.payload_descriptors[0].container, "TEST");
        assert_eq!(metadata.payload_entries.len(), 2);
        assert_eq!(metadata.payload_entries[0].byte_length, 9);
        assert_eq!(metadata.payload_entries[1].byte_length, 16);
        assert_eq!(metadata.tracks[0].title, "One");
        assert_eq!(metadata.tracks[1].first_revolution_index, 1);
        assert_eq!(metadata.tracks[1].revolution_count, 1);
    }

    /// Three entries, two single-entry tracks at positions 0 and 2 (explicit
    /// mappings, since the default position-equals-index mapping can't skip
    /// entry 1), and one track gap at entry 1 after track 0.
    fn test_metadata_with_track_gap() -> Vec<u8> {
        let mut out = Vec::new();
        out.push(RECORD_STREAM_METADATA_VERSION);
        out.push(METADATA_FLAG_TRACK_ENTRY_MAPPINGS | METADATA_FLAG_TRACK_GAPS);
        out.push(1);
        out.push(CONTAINER_EXTENSION);
        push_u16be(&mut out, 4);
        out.extend_from_slice(b"TEST");
        out.push(0);
        push_u16be(&mut out, 3);
        push_varuint(&mut out, 9);
        push_varuint(&mut out, 9);
        push_varuint(&mut out, 9);
        push_u16be(&mut out, 2);
        push_u16be(&mut out, 1);
        out.extend_from_slice(b"A");
        push_varuint(&mut out, 0);
        push_varuint(&mut out, 1);
        push_u16be(&mut out, 1);
        out.extend_from_slice(b"B");
        push_varuint(&mut out, 2);
        push_varuint(&mut out, 1);
        push_u16be(&mut out, 1); // track gap count
        push_varuint(&mut out, 1); // first_revolution_index
        push_varuint(&mut out, 1); // revolution_count
        push_varuint(&mut out, 0); // after_track_index
        out
    }

    #[test]
    fn track_gap_section_decodes_canonically() {
        let metadata = decode_record_stream_metadata(&test_metadata_with_track_gap()).unwrap();

        assert_eq!(metadata.track_gaps.len(), 1);
        assert_eq!(metadata.track_gaps[0].first_revolution_index, 1);
        assert_eq!(metadata.track_gaps[0].revolution_count, 1);
        assert_eq!(metadata.track_gaps[0].after_track_index, 0);
    }

    #[test]
    fn track_gap_section_absent_without_flag_yields_no_gaps() {
        // test_metadata() sets no flags at all, so there is no track-gap
        // section on the wire and none is read.
        let metadata = decode_record_stream_metadata(&test_metadata()).unwrap();
        assert!(metadata.track_gaps.is_empty());
    }

    #[test]
    fn track_gap_zero_revolution_count_is_rejected() {
        let mut bytes = test_metadata_with_track_gap();
        // Overwrite the gap's revolution_count varuint (the second of the
        // three trailing single-byte varuints) with zero.
        let len = bytes.len();
        bytes[len - 2] = 0;
        assert!(decode_record_stream_metadata(&bytes)
            .unwrap_err()
            .to_string()
            .contains("revolution count must be greater than zero"));
    }

    #[test]
    fn track_gap_invalid_after_track_index_is_rejected() {
        let mut bytes = test_metadata_with_track_gap();
        // Overwrite after_track_index (last byte) with an out-of-range track index.
        let len = bytes.len();
        bytes[len - 1] = 9;
        assert!(decode_record_stream_metadata(&bytes)
            .unwrap_err()
            .to_string()
            .contains("after_track_index"));
    }

    #[test]
    fn out_of_order_track_gaps_are_rejected() {
        let mut out = Vec::new();
        out.push(RECORD_STREAM_METADATA_VERSION);
        out.push(METADATA_FLAG_TRACK_ENTRY_MAPPINGS | METADATA_FLAG_TRACK_GAPS);
        out.push(1);
        out.push(CONTAINER_EXTENSION);
        push_u16be(&mut out, 4);
        out.extend_from_slice(b"TEST");
        out.push(0);
        push_u16be(&mut out, 4);
        push_varuint(&mut out, 9);
        push_varuint(&mut out, 9);
        push_varuint(&mut out, 9);
        push_varuint(&mut out, 9);
        push_u16be(&mut out, 1);
        push_u16be(&mut out, 1);
        out.extend_from_slice(b"A");
        push_varuint(&mut out, 0);
        push_varuint(&mut out, 1);
        push_u16be(&mut out, 2); // track gap count
                                 // Second gap's first_revolution_index (2) is not greater than the
                                 // first gap's (also 2): rejected as not strictly ascending.
        push_varuint(&mut out, 2);
        push_varuint(&mut out, 1);
        push_varuint(&mut out, 0);
        push_varuint(&mut out, 2);
        push_varuint(&mut out, 1);
        push_varuint(&mut out, 0);

        assert!(decode_record_stream_metadata(&out)
            .unwrap_err()
            .to_string()
            .contains("not strictly ascending"));
    }

    #[test]
    fn descriptor_reserved_flags_fail() {
        let mut metadata = test_metadata();
        let descriptor_flags_offset = 10;
        // 0x20 is above DESCRIPTOR_KNOWN_FLAGS (0x1F) and so remains reserved.
        metadata[descriptor_flags_offset] = 0x20;

        assert!(decode_record_stream_metadata(&metadata)
            .unwrap_err()
            .to_string()
            .contains("unknown flags"));
    }

    #[test]
    fn raw_container_code_is_not_current_format() {
        let mut metadata = test_metadata();
        metadata[3] = 0;

        assert!(decode_record_stream_metadata(&metadata)
            .unwrap_err()
            .to_string()
            .contains("unknown payload container code 0"));
    }

    #[test]
    fn compact_entries_resolve_offsets() {
        let entries = vec![
            PayloadEntryDescriptor {
                byte_length: 9,
                payload_descriptor_index: 0,
            },
            PayloadEntryDescriptor {
                byte_length: 16,
                payload_descriptor_index: 0,
            },
        ];

        let resolved = resolve_payload_entries(&entries, 1).unwrap();

        assert_eq!(resolved[0].byte_offset, 0);
        assert_eq!(resolved[1].byte_offset, 9);
    }

    #[test]
    fn rejects_overlong_varuint() {
        let mut metadata = test_metadata();
        let entry_length_offset = 13;
        metadata.splice(entry_length_offset..entry_length_offset + 1, [0x89, 0x00]);

        assert!(decode_record_stream_metadata(&metadata)
            .unwrap_err()
            .to_string()
            .contains("overlong"));
    }

    #[test]
    fn crc32_matches_standard_check_value() {
        assert_eq!(crc32_ieee(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn toned_capacity_is_not_rgb_capacity() {
        let bytes = 216_786;
        let rgb = payload_pixel_count_for_encoding(bytes, PayloadPixelEncoding::Rgb24).unwrap();
        let toned = payload_pixel_count_for_encoding(
            bytes,
            PayloadPixelEncoding::Toned { bits_per_pixel: 20 },
        )
        .unwrap();

        assert!(toned > rgb);
    }

    fn ecdc_descriptor() -> PayloadDescriptor {
        ecdc::ecdc_payload_descriptor(
            48_000,
            2,
            &ecdc::EcdcCodecMetadata {
                model: "encodec_48khz".to_owned(),
                num_codebooks: 8,
                lm: true,
                fp_scale: 8192,
                min_range: 2,
                bitstream_version: 2,
                lm_frame_length: 203,
            },
        )
        .unwrap()
    }

    fn gap_descriptor() -> PayloadDescriptor {
        PayloadDescriptor::gap(48_000, 2).unwrap()
    }

    #[test]
    fn gap_descriptor_passes_validation() {
        let descriptor = gap_descriptor();

        assert_eq!(descriptor.container, PAYLOAD_CONTAINER_GAP);
        assert_eq!(descriptor.sample_rate, Some(48_000));
        assert_eq!(descriptor.channels, Some(2));
        assert_eq!(descriptor.codec.as_deref(), Some(PAYLOAD_CODEC_GAP));
        assert!(descriptor.block_samples.is_none());
        assert!(descriptor.output_offset_samples.is_none());
        assert!(descriptor.output_samples.is_none());
        assert!(descriptor.codec_metadata.is_none());
    }

    fn gap_entry_bytes() -> Vec<u8> {
        let sample_count = 24_000u64;
        let payload_byte_length = 256usize;
        let seed = 0x1234_5678u32;

        let mut out = Vec::with_capacity(payload_byte_length);
        out.extend_from_slice(gap::GAP_MAGIC);
        out.push(gap::GAP_VERSION);
        out.push(0); // flags
        out.extend_from_slice(&[0u8, 0u8]); // reserved
        out.extend_from_slice(&sample_count.to_be_bytes());
        out.extend_from_slice(&(payload_byte_length as u64).to_be_bytes());
        out.extend_from_slice(&seed.to_be_bytes());
        out.resize(payload_byte_length, 0);
        gap::fill_gap_quiet_filler(seed, &mut out[gap::GAP_HEADER_LENGTH..]);
        out
    }

    fn headerless_ecdc_test_entry(fill: u8, payload_len: usize) -> Vec<u8> {
        let payload_len = u32::try_from(payload_len).expect("test ECDC payload length exceeds u32");

        let mut entry = Vec::with_capacity(8 + payload_len as usize);
        entry.extend_from_slice(&payload_len.to_be_bytes());
        entry.extend_from_slice(&[0u8; 4]);
        entry.resize(8 + payload_len as usize, fill);
        entry
    }

    #[test]
    fn programme_map_computes_exact_boundaries_across_gap() {
        // Two programme-time ECDC revolutions, an explicit track gap (also a
        // normal ECDC revolution under the same shared descriptor — no GAP
        // container, no GAP1 payload), then two more track revolutions.
        let ecdc = ecdc_descriptor();
        let output_samples = u64::from(ecdc.output_samples.unwrap());
        let rev = headerless_ecdc_test_entry(0xAA, 10);
        let rev_len = rev.len();

        let mut payload = Vec::new();
        for _ in 0..5 {
            payload.extend_from_slice(&rev);
        }

        let metadata = RecordStreamMetadata {
            version: RECORD_STREAM_METADATA_VERSION,
            encrypted: false,
            payload_descriptors: vec![ecdc],
            payload_entries: vec![
                PayloadEntryDescriptor {
                    byte_length: rev_len,
                    payload_descriptor_index: 0,
                },
                PayloadEntryDescriptor {
                    byte_length: rev_len,
                    payload_descriptor_index: 0,
                },
                PayloadEntryDescriptor {
                    byte_length: rev_len,
                    payload_descriptor_index: 0,
                },
                PayloadEntryDescriptor {
                    byte_length: rev_len,
                    payload_descriptor_index: 0,
                },
                PayloadEntryDescriptor {
                    byte_length: rev_len,
                    payload_descriptor_index: 0,
                },
            ],
            tracks: vec![
                TrackDescriptor {
                    title: "Track A".to_owned(),
                    first_revolution_index: 0,
                    revolution_count: 2,
                },
                TrackDescriptor {
                    title: "Track B".to_owned(),
                    first_revolution_index: 3,
                    revolution_count: 2,
                },
            ],
            track_gaps: vec![TrackGapDescriptor {
                first_revolution_index: 2,
                revolution_count: 1,
                after_track_index: 0,
            }],
        };

        let document = RecordStream {
            metadata,
            metadata_bytes: Vec::new(),
            chunks: vec![Chunk {
                payload,
                crc32: 0,
                nonce: None,
            }],
        };

        let map = build_programme_map(&document, Some("single45")).unwrap();
        assert_eq!(map.total_samples, output_samples * 5);
        assert_eq!(map.regions.len(), 3);

        // Region 0: Track A spanning two revolutions.
        assert_eq!(
            map.regions[0].kind,
            ProgrammeRegionKind::Track {
                number: 1,
                title: "Track A".to_owned()
            }
        );
        assert_eq!(map.regions[0].start_sample, 0);
        assert_eq!(map.regions[0].end_sample, output_samples * 2);

        // Region 1: the explicit track gap after track 1, one revolution.
        assert_eq!(
            map.regions[1].kind,
            ProgrammeRegionKind::Gap {
                after_track_number: 1
            }
        );
        assert_eq!(map.regions[1].sample_count, output_samples);
        assert_eq!(map.regions[1].start_sample, output_samples * 2);
        assert_eq!(map.regions[1].end_sample, output_samples * 3);

        // Region 2: Track B.
        assert_eq!(
            map.regions[2].kind,
            ProgrammeRegionKind::Track {
                number: 2,
                title: "Track B".to_owned()
            }
        );
        assert_eq!(map.regions[2].end_sample, map.total_samples);

        // Radial anchors are real spiral positions: 0 at the outer edge, 1 at the
        // inner edge, strictly increasing across the programme, and bracketing the
        // gap. Byte fractions are retained only as a diagnostic.
        let total_bytes = (rev_len * 5) as f64;
        assert_eq!(map.regions[0].radial_start_normalized, 0.0);
        assert!(
            (map.regions[1].byte_fraction_start - (rev_len * 2) as f64 / total_bytes).abs() < 1e-12
        );
        assert!(map.regions[2].radial_end_normalized > 0.999);
        // Monotonic non-decreasing radial position across regions.
        for w in map.regions.windows(2) {
            assert!(w[1].radial_start_normalized >= w[0].radial_end_normalized - 1e-12);
        }
        // Equal-area spiral: radial position is distinct from the raw byte
        // fraction (outer turns hold more pixels, so near the outer edge the
        // radius advances more slowly than the byte fraction).
        assert!(map.regions[1].radial_start_normalized < map.regions[1].byte_fraction_start);
        assert!(map.regions[1].radial_start_normalized > 0.0);
    }

    #[test]
    fn gap_payload_round_trips_through_entry_validation() {
        let bytes = gap_entry_bytes();
        let descriptor = gap_descriptor();
        validate_payload_entry_bytes(&descriptor, &bytes).unwrap();
        assert_eq!(gap::gap_sample_count(&bytes).unwrap(), 24_000);
    }

    #[test]
    fn malformed_gap_entry_is_rejected() {
        let descriptor = gap_descriptor();
        // Legacy bare u64be sample counts are no longer valid GAP entries.
        assert!(validate_payload_entry_bytes(&descriptor, &24_000u64.to_be_bytes()).is_err());
        // Truncated / corrupted GAP1 bytes are rejected.
        let mut bytes = gap_entry_bytes();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        assert!(validate_payload_entry_bytes(&descriptor, &bytes).is_err());
    }

    #[test]
    fn musical_tracks_may_be_separated_by_an_explicit_track_gap() {
        // The track-gap entry is an ordinary ECDC payload entry, just like
        // the track entries: classification comes entirely from explicit
        // track_gaps metadata, never from inspecting the payload container.
        let metadata = RecordStreamMetadata {
            version: RECORD_STREAM_METADATA_VERSION,
            encrypted: false,
            payload_descriptors: vec![ecdc_descriptor()],
            payload_entries: vec![
                PayloadEntryDescriptor {
                    byte_length: 11,
                    payload_descriptor_index: 0,
                },
                PayloadEntryDescriptor {
                    byte_length: 17,
                    payload_descriptor_index: 0,
                },
                PayloadEntryDescriptor {
                    byte_length: 13,
                    payload_descriptor_index: 0,
                },
            ],
            tracks: vec![
                TrackDescriptor {
                    title: "Side A".to_owned(),
                    first_revolution_index: 0,
                    revolution_count: 1,
                },
                TrackDescriptor {
                    title: "Side B".to_owned(),
                    first_revolution_index: 2,
                    revolution_count: 1,
                },
            ],
            track_gaps: vec![TrackGapDescriptor {
                first_revolution_index: 1,
                revolution_count: 1,
                after_track_index: 0,
            }],
        };

        validate_track_listing_metadata(&metadata).unwrap();
    }

    #[test]
    fn an_untracked_entry_without_an_explicit_gap_is_rejected() {
        // Same shape as the previous test, but with no track_gaps entry:
        // there is no implicit "uncovered means gap" fallback any more.
        let metadata = RecordStreamMetadata {
            version: RECORD_STREAM_METADATA_VERSION,
            encrypted: false,
            payload_descriptors: vec![ecdc_descriptor()],
            payload_entries: vec![
                PayloadEntryDescriptor {
                    byte_length: 11,
                    payload_descriptor_index: 0,
                },
                PayloadEntryDescriptor {
                    byte_length: 17,
                    payload_descriptor_index: 0,
                },
                PayloadEntryDescriptor {
                    byte_length: 13,
                    payload_descriptor_index: 0,
                },
            ],
            tracks: vec![
                TrackDescriptor {
                    title: "Side A".to_owned(),
                    first_revolution_index: 0,
                    revolution_count: 1,
                },
                TrackDescriptor {
                    title: "Side B".to_owned(),
                    first_revolution_index: 2,
                    revolution_count: 1,
                },
            ],
            track_gaps: vec![],
        };

        let err = validate_track_listing_metadata(&metadata).unwrap_err();
        assert!(err
            .to_string()
            .contains("not covered by any track or track gap"));
    }

    #[test]
    fn track_must_not_cover_an_explicit_track_gap_entry() {
        // All three entries are ordinary ECDC payload entries under the same
        // descriptor. What is illegal is a track range and a track-gap range
        // both claiming the same entry — never a function of container.
        let metadata = RecordStreamMetadata {
            version: RECORD_STREAM_METADATA_VERSION,
            encrypted: false,
            payload_descriptors: vec![ecdc_descriptor()],
            payload_entries: vec![
                PayloadEntryDescriptor {
                    byte_length: 11,
                    payload_descriptor_index: 0,
                },
                PayloadEntryDescriptor {
                    byte_length: 17,
                    payload_descriptor_index: 0,
                },
                PayloadEntryDescriptor {
                    byte_length: 13,
                    payload_descriptor_index: 0,
                },
            ],
            tracks: vec![TrackDescriptor {
                title: "Wrong".to_owned(),
                first_revolution_index: 0,
                revolution_count: 3,
            }],
            track_gaps: vec![TrackGapDescriptor {
                first_revolution_index: 1,
                revolution_count: 1,
                after_track_index: 0,
            }],
        };

        assert!(validate_track_listing_metadata(&metadata).is_err());
    }

    fn encode_descriptor(out: &mut Vec<u8>, descriptor: &PayloadDescriptor) {
        let (code, extension) = match descriptor.container.as_str() {
            PAYLOAD_CONTAINER_ECDC => (CONTAINER_ECDC, None),
            PAYLOAD_CONTAINER_MOSS_NANO => (CONTAINER_MOSS_NANO, None),
            other => (CONTAINER_EXTENSION, Some(other)),
        };
        out.push(code);
        if let Some(name) = extension {
            push_u16be(out, name.len() as u16);
            out.extend_from_slice(name.as_bytes());
        }

        let mut flags = 0u8;
        if descriptor.codec.is_some() {
            flags |= DESCRIPTOR_FLAG_CODEC;
        }
        if descriptor.sample_rate.is_some() {
            flags |= DESCRIPTOR_FLAG_SAMPLE_RATE;
        }
        if descriptor.channels.is_some() {
            flags |= DESCRIPTOR_FLAG_CHANNELS;
        }
        if descriptor.block_samples.is_some() {
            flags |= DESCRIPTOR_FLAG_OUTPUT_GEOMETRY;
        }
        if descriptor.codec_metadata.is_some() {
            flags |= DESCRIPTOR_FLAG_CODEC_METADATA;
        }
        out.push(flags);

        if let Some(codec) = &descriptor.codec {
            push_u16be(out, codec.len() as u16);
            out.extend_from_slice(codec.as_bytes());
        }
        if let Some(sample_rate) = descriptor.sample_rate {
            out.extend_from_slice(&sample_rate.to_be_bytes());
        }
        if let Some(channels) = descriptor.channels {
            out.push(channels);
        }
        if let Some(block_samples) = descriptor.block_samples {
            out.extend_from_slice(&block_samples.to_be_bytes());
            out.extend_from_slice(&descriptor.output_offset_samples.unwrap().to_be_bytes());
            out.extend_from_slice(&descriptor.output_samples.unwrap().to_be_bytes());
        }
        if let Some(codec_metadata) = &descriptor.codec_metadata {
            out.extend_from_slice(&(codec_metadata.len() as u32).to_be_bytes());
            out.extend_from_slice(codec_metadata);
        }
    }

    fn metadata_with_descriptors(descriptors: &[PayloadDescriptor]) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(RECORD_STREAM_METADATA_VERSION);
        out.push(METADATA_FLAG_ENTRY_DESCRIPTOR_INDEXES);
        out.push(descriptors.len() as u8);
        for descriptor in descriptors {
            encode_descriptor(&mut out, descriptor);
        }
        // Two payload entries, one per descriptor index where possible.
        push_u16be(&mut out, 2);
        for index in 0..2u8 {
            push_varuint(&mut out, 8);
            out.push(index.min(descriptors.len() as u8 - 1));
        }
        push_u16be(&mut out, 1);
        push_u16be(&mut out, 3);
        out.extend_from_slice(b"One");
        out
    }

    #[test]
    fn ecdc_profile_descriptor_values() {
        let descriptor = ecdc_descriptor();
        assert_eq!(descriptor.block_samples, Some(64_960));
        assert_eq!(descriptor.output_offset_samples, Some(480));
        assert_eq!(descriptor.output_samples, Some(64_000));
        assert_eq!(ecdc::ECDC_BLOCK_SAMPLES, 64_960);
        assert_eq!(ecdc::ECDC_OUTPUT_OFFSET_SAMPLES, 480);
        assert_eq!(ecdc::ECDC_OUTPUT_SAMPLES, 64_000);
        // Trailing discarded samples are derived, not stored.
        assert_eq!(
            descriptor.block_samples.unwrap()
                - descriptor.output_offset_samples.unwrap()
                - descriptor.output_samples.unwrap(),
            480
        );
    }

    #[test]
    fn descriptor_with_all_fields_round_trips() {
        let descriptor = ecdc_descriptor();
        let bytes = metadata_with_descriptors(&[descriptor.clone()]);
        let metadata = decode_record_stream_metadata(&bytes).unwrap();
        assert_eq!(metadata.payload_descriptors[0], descriptor);
        assert_eq!(metadata.payload_entries[0].payload_descriptor_index, 0);
    }

    #[test]
    fn descriptor_without_optional_geometry_round_trips() {
        let descriptor = PayloadDescriptor::from_container(PAYLOAD_CONTAINER_ECDC);
        let bytes = metadata_with_descriptors(&[descriptor.clone()]);
        let metadata = decode_record_stream_metadata(&bytes).unwrap();
        assert_eq!(metadata.payload_descriptors[0], descriptor);
        assert!(metadata.payload_descriptors[0].block_samples.is_none());
        assert!(metadata.payload_descriptors[0].codec_metadata.is_none());
    }

    #[test]
    fn multiple_entries_reference_shared_descriptor() {
        let bytes = metadata_with_descriptors(&[ecdc_descriptor()]);
        let metadata = decode_record_stream_metadata(&bytes).unwrap();
        assert_eq!(metadata.payload_descriptors.len(), 1);
        assert_eq!(metadata.payload_entries.len(), 2);
        for entry in &metadata.payload_entries {
            assert_eq!(entry.payload_descriptor_index, 0);
        }
    }

    #[test]
    fn truncated_codec_metadata_is_rejected() {
        let mut bytes = metadata_with_descriptors(&[ecdc_descriptor()]);
        bytes.truncate(bytes.len() - 4);
        assert!(decode_record_stream_metadata(&bytes).is_err());
    }

    #[test]
    fn oversized_codec_metadata_length_is_rejected() {
        let descriptor = ecdc_descriptor();
        let mut out = Vec::new();
        out.push(RECORD_STREAM_METADATA_VERSION);
        out.push(0);
        out.push(1);
        // ECDC descriptor with only the codec-metadata flag, but a bogus huge length.
        out.push(CONTAINER_ECDC);
        out.push(DESCRIPTOR_FLAG_CODEC_METADATA);
        out.extend_from_slice(&((MAX_CODEC_METADATA_BYTES as u32) + 1).to_be_bytes());
        out.extend_from_slice(descriptor.codec_metadata.as_ref().unwrap());
        let err = decode_record_stream_metadata(&out).unwrap_err().to_string();
        assert!(err.contains("exceeds limit"));
    }

    #[test]
    fn valid_geometry_passes_validation() {
        validate_payload_descriptor(&ecdc_descriptor()).unwrap();
    }

    #[test]
    fn zero_block_length_is_rejected() {
        let mut descriptor = ecdc_descriptor();
        descriptor.block_samples = Some(0);
        assert!(validate_payload_descriptor(&descriptor).is_err());
    }

    #[test]
    fn zero_output_length_is_rejected() {
        let mut descriptor = ecdc_descriptor();
        descriptor.output_samples = Some(0);
        assert!(validate_payload_descriptor(&descriptor).is_err());
    }

    #[test]
    fn offset_plus_output_exceeding_block_is_rejected() {
        let mut descriptor = ecdc_descriptor();
        descriptor.output_offset_samples = Some(1_000);
        descriptor.output_samples = Some(64_000);
        // 1000 + 64000 = 65000 > 64960
        assert!(validate_payload_descriptor(&descriptor).is_err());
    }

    #[test]
    fn offset_plus_output_overflow_is_rejected() {
        let mut descriptor = ecdc_descriptor();
        descriptor.block_samples = Some(u32::MAX);
        descriptor.output_offset_samples = Some(u32::MAX);
        descriptor.output_samples = Some(u32::MAX);
        assert!(validate_payload_descriptor(&descriptor).is_err());
    }

    #[test]
    fn partial_geometry_presence_is_rejected() {
        let mut descriptor = ecdc_descriptor();
        descriptor.output_samples = None;
        assert!(validate_payload_descriptor(&descriptor).is_err());
    }

    #[test]
    fn absent_geometry_is_accepted() {
        validate_payload_descriptor(&PayloadDescriptor::from_container("TEST")).unwrap();
    }

    #[test]
    fn codec_metadata_valid_json_accepted() {
        validate_codec_metadata(br#"{"m":"encodec_48khz","nc":8}"#).unwrap();
    }

    #[test]
    fn codec_metadata_malformed_json_rejected() {
        assert!(validate_codec_metadata(br#"{"m":"#).is_err());
        // Empty bytes are not valid JSON, so empty codec metadata is rejected.
        assert!(validate_codec_metadata(b"").is_err());
    }

    #[test]
    fn codec_metadata_non_utf8_rejected() {
        assert!(validate_codec_metadata(&[0xff, 0xfe, 0x00]).is_err());
    }

    #[test]
    fn codec_metadata_duplicate_geometry_key_rejected() {
        assert!(validate_codec_metadata(br#"{"block_samples":64960}"#).is_err());
        assert!(validate_codec_metadata(br#"{"output_samples":64000}"#).is_err());
    }

    #[test]
    fn shared_descriptor_equality_detects_mismatch() {
        let a = ecdc_descriptor();
        validate_shared_payload_descriptor(&a, &a.clone()).unwrap();

        let mut b = a.clone();
        b.channels = Some(1);
        let err = validate_shared_payload_descriptor(&a, &b).unwrap_err();
        assert!(err.to_string().contains("channels"));

        let mut c = a.clone();
        c.codec_metadata = Some(b"{}".to_vec());
        let err = validate_shared_payload_descriptor(&a, &c).unwrap_err();
        assert!(err.to_string().contains("codec_metadata"));
    }

    #[test]
    fn ecdc_codec_metadata_is_deterministic_and_has_no_geometry() {
        let meta = ecdc::EcdcCodecMetadata {
            model: "encodec_48khz".to_owned(),
            num_codebooks: 8,
            lm: true,
            fp_scale: 8192,
            min_range: 2,
            bitstream_version: 2,
            lm_frame_length: 203,
        };
        let a = ecdc::ecdc_codec_metadata_json(&meta).unwrap();
        let b = ecdc::ecdc_codec_metadata_json(&meta).unwrap();
        assert_eq!(a, b);
        let text = std::str::from_utf8(&a).unwrap();
        assert!(!text.contains("block_samples"));
        assert!(!text.contains("output_offset_samples"));
        assert!(!text.contains("output_samples"));
        // Compact JSON: no spaces between tokens.
        assert!(!text.contains(": "));
        validate_codec_metadata(&a).unwrap();
    }

    #[test]
    fn vari_pitch_params_are_deterministic_and_bounded() {
        let params = vari_pitch_params(0.3, 0x0123_4567_89AB_CDEF);
        assert_eq!(params, vari_pitch_params(0.3, 0x0123_4567_89AB_CDEF));

        // The cut starts exactly at the outer radius…
        assert_eq!(params.theta_effective(0.0), 0.0);

        // …its periods sit inside the moiré-safe non-integer turn bands
        // (±2% micro jitter included)…
        let turns_1 = params.period_1 / (2.0 * PI);
        let turns_2 = params.period_2 / (2.0 * PI);
        assert!((4.5..6.3).contains(&turns_1), "period 1: {turns_1} turns");
        assert!((11.0..14.7).contains(&turns_2), "period 2: {turns_2} turns");

        // …and the local pitch never leaves b·[1 − depth, 1 + depth], so
        // the groove always advances inward.
        let mut theta = 0.0_f64;
        while theta < 2_000.0 {
            let factor = params.pitch_factor(theta);
            assert!(
                (0.7 - 1e-9..=1.3 + 1e-9).contains(&factor),
                "pitch factor {factor} at theta {theta}"
            );
            theta += 0.01;
        }
    }

    #[test]
    fn vari_pitch_micro_bits_only_nudge_the_cut() {
        let base = vari_pitch_params(0.3, 0xAAAA_BBBB_0000_0000);
        let nudged = vari_pitch_params(0.3, 0xAAAA_BBBB_FFFF_FFFF);

        // Same high bits: the character holds — periods within the ±2%
        // micro jitter of each other, phases within the small drift.
        assert!((base.period_1 / nudged.period_1 - 1.0).abs() < 0.05);
        assert!((base.period_2 / nudged.period_2 - 1.0).abs() < 0.05);
        assert!((base.phase_1 - nudged.phase_1).abs() < 0.15);
        assert!((base.phase_2 - nudged.phase_2).abs() < 0.15);

        // But the cut is not identical: every pressing is its own record.
        assert_ne!(base, nudged);
    }

    #[test]
    fn vari_pitch_mask_differs_from_archimedean_but_matches_itself() {
        let family = SpiralFamily::VariPitch {
            depth: 0.3,
            seed: 42,
        };
        let vari =
            build_spiral_mask_with_family(576, 576, 0.6, &family, "single45", None, None, None)
                .unwrap();
        let vari_again =
            build_spiral_mask_with_family(576, 576, 0.6, &family, "single45", None, None, None)
                .unwrap();
        let archimedean = build_spiral_mask(576, 576, 0.6, "single45", None, None, None).unwrap();

        assert_eq!(
            vari.ordered_pixel_indices, vari_again.ordered_pixel_indices,
            "the same seed must retrace the identical groove"
        );
        assert_ne!(
            vari.ordered_pixel_indices, archimedean.ordered_pixel_indices,
            "vari-pitch must actually change the cut"
        );

        // Mean pitch is the same b, so capacity stays comparable: within a
        // few percent of the Archimedean mask at the same b.
        let ratio =
            vari.addressable_pixel_count as f64 / archimedean.addressable_pixel_count as f64;
        assert!(
            (0.9..1.1).contains(&ratio),
            "vari-pitch capacity drifted: {ratio}"
        );
    }

    #[test]
    fn vari_pitch_rejects_nan_and_out_of_range_depth() {
        for depth in [f64::NAN, 0.0, -0.1, 0.46] {
            assert!(
                SpiralFamily::VariPitch { depth, seed: 1 }.validate().is_err(),
                "depth {depth} must be refused"
            );
        }
        assert!(SpiralFamily::VariPitch {
            depth: 0.45,
            seed: 1
        }
        .validate()
        .is_ok());
    }
}
