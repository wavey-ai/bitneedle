//! Public Bitneedle record wire-format, geometry, decoding, and verification primitives.
//!
//! This crate is the authoritative implementation of the public BRD1/BRS1 wire
//! contract. It intentionally contains no application workflow identifiers,
//! record-creation policy, placeholder signing, JSON stream metadata, legacy JSON support, or presentation masking.

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
pub const RECORD_STREAM_METADATA_VERSION: u8 = 1;

pub const METADATA_FLAG_ENCRYPTED: u8 = 0x01;
pub const METADATA_FLAG_ENTRY_DESCRIPTOR_INDEXES: u8 = 0x02;
pub const METADATA_FLAG_TRACK_ENTRY_MAPPINGS: u8 = 0x04;
pub const METADATA_KNOWN_FLAGS: u8 = METADATA_FLAG_ENCRYPTED
    | METADATA_FLAG_ENTRY_DESCRIPTOR_INDEXES
    | METADATA_FLAG_TRACK_ENTRY_MAPPINGS;

pub const DESCRIPTOR_FLAG_CODEC: u8 = 0x01;
pub const DESCRIPTOR_FLAG_SAMPLE_RATE: u8 = 0x02;
pub const DESCRIPTOR_FLAG_CHANNELS: u8 = 0x04;
pub const DESCRIPTOR_KNOWN_FLAGS: u8 =
    DESCRIPTOR_FLAG_CODEC | DESCRIPTOR_FLAG_SAMPLE_RATE | DESCRIPTOR_FLAG_CHANNELS;

pub const CONTAINER_RAW: u8 = 0;
pub const CONTAINER_ECDC: u8 = 1;
pub const CONTAINER_MOSS_NANO: u8 = 2;
pub const CONTAINER_EXTENSION: u8 = 255;

pub const CHUNK_INDEX_LENGTH: usize = 2;
pub const CHUNK_COUNT_LENGTH: usize = 2;
pub const CHUNK_PAYLOAD_DESCRIPTOR_INDEX_LENGTH: usize = 1;
pub const CHUNK_PAYLOAD_LENGTH_FIELD_LENGTH: usize = 4;
pub const CHUNK_CRC32_LENGTH: usize = 4;
pub const CHUNK_SIGNATURE_LENGTH: usize = 64;
pub const CHUNK_FIXED_HEADER_LENGTH: usize = CHUNK_INDEX_LENGTH
    + CHUNK_COUNT_LENGTH
    + CHUNK_PAYLOAD_DESCRIPTOR_INDEX_LENGTH
    + CHUNK_PAYLOAD_LENGTH_FIELD_LENGTH
    + CHUNK_CRC32_LENGTH
    + CHUNK_SIGNATURE_LENGTH;

pub const MAX_CHUNKS: usize = u16::MAX as usize;
pub const MAX_PAYLOAD_DESCRIPTORS: usize = u8::MAX as usize;
pub const DEFAULT_PAYLOAD_DESCRIPTOR_INDEX: u8 = 0;

pub const CHUNK_SIGNATURE_DOMAIN: &[u8] = b"bitneedle.record-stream.chunk.v1";
pub const CHUNK_SIGNATURE_WITH_NONCE_DOMAIN: &[u8] =
    b"bitneedle.record-stream.chunk.v1.nonce";
pub const CHUNK_ENCRYPTION_DOMAIN: &[u8] =
    b"bitneedle.record-stream.chunk-encryption.v1";
pub const CHUNK_ENCRYPTION_ALGORITHM_CHACHA20POLY1305: &str = "chacha20poly1305";
pub const CHUNK_ENCRYPTION_NONCE_LENGTH: usize = 12;
pub const CHUNK_ENCRYPTION_TAG_LENGTH: usize = 16;
pub const CHUNK_ENCRYPTION_KEY_LENGTH: usize = 32;

pub const PAYLOAD_CONTAINER_RAW: &str = "RAW";
pub const PAYLOAD_CONTAINER_ECDC: &str = "ECDC";
pub const PAYLOAD_CONTAINER_MOSS_NANO: &str = "MOSSNANO";
pub const DEFAULT_RAW_PAYLOAD_CHUNK_SIZE: usize = 64 * 1024;

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
    pub index: u16,
    pub chunk_count: u16,
    pub payload_descriptor_index: u8,
    pub payload: Vec<u8>,
    pub crc32: u32,
    pub signature: [u8; CHUNK_SIGNATURE_LENGTH],
    pub nonce: Option<[u8; CHUNK_ENCRYPTION_NONCE_LENGTH]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkRanges {
    pub chunk: Range<usize>,
    pub payload: Range<usize>,
    pub signature: Range<usize>,
    pub nonce: Option<Range<usize>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkHeader {
    pub index: u16,
    pub chunk_count: u16,
    pub payload_descriptor_index: u8,
    pub payload_len: usize,
    pub crc32: u32,
    pub signature: [u8; CHUNK_SIGNATURE_LENGTH],
    pub nonce: Option<[u8; CHUNK_ENCRYPTION_NONCE_LENGTH]>,
    pub chunk_start: usize,
    pub signature_start: usize,
    pub signature_end: usize,
    pub nonce_start: Option<usize>,
    pub nonce_end: Option<usize>,
    pub payload_start: usize,
    pub payload_end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PayloadDescriptor {
    pub container: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<u8>,
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
    pub payload_entry_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordStreamMetadata {
    pub version: u8,
    pub encrypted: bool,
    pub payload_descriptors: Vec<PayloadDescriptor>,
    pub payload_entries: Vec<PayloadEntryDescriptor>,
    pub tracks: Vec<TrackDescriptor>,
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
    pub codec: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
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
    pub payload_entry_index: usize,
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
    pub record_profile: String,
    pub label_radius: i32,
    pub label_clearance: i32,
    pub outer_radius: i32,
    pub kinds: Vec<u8>,
    pub addressable_pixel_count: usize,
    pub ordered_pixel_indices: Vec<usize>,
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

        let step = pixel_gap
            / (radius * radius + resolved_pitch * resolved_pitch)
                .sqrt()
                .max(1e-6);

        theta += step;
        angle = start_angle + if clockwise { -theta } else { theta };
        radius = outer - resolved_pitch * theta;
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
    let g = resolve_record_geometry(record_profile, label_radius, label_clearance, outer_radius)?;
    let outer = payload_outer_radius(&g);
    let inner = payload_inner_radius(&g);

    let (occupied, traced, cx, cy) = trace_record_spiral(
        width,
        height,
        b_value,
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

        let distance = (((i % width) as f64 - cx).powi(2)
            + ((i / width) as f64 - cy).powi(2))
        .sqrt();

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

fn push_u16be(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
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

    let metadata_len =
        read_u32be(bytes, 4, "record stream metadata length")? as usize;
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
        let value = std::str::from_utf8(bytes)
            .with_context(|| format!("{label} is not valid UTF-8"))?;

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
        CONTAINER_RAW => Ok(PAYLOAD_CONTAINER_RAW.to_string()),
        CONTAINER_ECDC => Ok(PAYLOAD_CONTAINER_ECDC.to_string()),
        CONTAINER_MOSS_NANO => Ok(PAYLOAD_CONTAINER_MOSS_NANO.to_string()),
        CONTAINER_EXTENSION => cursor.read_string("extension container name"),
        _ => bail!("unknown payload container code {code}"),
    }
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
    let has_entry_descriptor_indexes =
        flags & METADATA_FLAG_ENTRY_DESCRIPTOR_INDEXES != 0;
    let has_track_entry_mappings =
        flags & METADATA_FLAG_TRACK_ENTRY_MAPPINGS != 0;

    let descriptor_count =
        cursor.read_u8("payload descriptor count")? as usize;

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
        let descriptor_flags =
            cursor.read_u8("payload descriptor flags")?;

        if descriptor_flags & !DESCRIPTOR_KNOWN_FLAGS != 0 {
            bail!("payload descriptor {descriptor_index} contains unknown flags");
        }

        let codec = if descriptor_flags & DESCRIPTOR_FLAG_CODEC != 0 {
            Some(cursor.read_string("payload codec")?)
        } else {
            None
        };

        let sample_rate =
            if descriptor_flags & DESCRIPTOR_FLAG_SAMPLE_RATE != 0 {
                let value = cursor.read_u32be("payload sample rate")?;

                if value == 0 {
                    bail!("payload descriptor {descriptor_index} sample rate must be greater than zero");
                }

                Some(value)
            } else {
                None
            };

        let channels =
            if descriptor_flags & DESCRIPTOR_FLAG_CHANNELS != 0 {
                let value = cursor.read_u8("payload channels")?;

                if value == 0 {
                    bail!("payload descriptor {descriptor_index} channels must be greater than zero");
                }

                Some(value)
            } else {
                None
            };

        payload_descriptors.push(PayloadDescriptor {
            container,
            codec,
            sample_rate,
            channels,
        });
    }

    let entry_count =
        cursor.read_u16be("payload entry count")? as usize;

    if entry_count == 0 {
        bail!("payload entry count must not be zero");
    }

    let mut payload_entries = Vec::with_capacity(entry_count);

    for entry_index in 0..entry_count {
        let byte_length_u64 =
            cursor.read_varuint("payload entry byte length")?;
        let byte_length = usize::try_from(byte_length_u64)
            .context("payload entry byte length exceeds usize")?;

        if byte_length == 0 {
            bail!("payload entry {entry_index} byte length must be greater than zero");
        }

        let payload_descriptor_index =
            if has_entry_descriptor_indexes {
                cursor.read_u8("payload entry descriptor index")?
            } else {
                DEFAULT_PAYLOAD_DESCRIPTOR_INDEX
            };

        validate_payload_descriptor_index(
            payload_descriptors.len(),
            payload_descriptor_index,
        )?;

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
        let payload_entry_index =
            if has_track_entry_mappings {
                usize::try_from(
                    cursor.read_varuint("track payload entry index")?,
                )
                .context("track payload entry index exceeds usize")?
            } else {
                track_index
            };

        if payload_entry_index >= payload_entries.len() {
            bail!(
                "track {track_index} payload entry index {payload_entry_index} is out of range for {} entries",
                payload_entries.len()
            );
        }

        tracks.push(TrackDescriptor {
            title,
            payload_entry_index,
        });
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
    })
}

pub fn record_stream_metadata(bytes: &[u8]) -> Result<RecordStreamMetadata> {
    decode_record_stream_metadata(record_stream_metadata_bytes(bytes)?)
}

pub fn stream_metadata(bytes: &[u8]) -> Result<RecordStreamMetadata> {
    record_stream_metadata(bytes)
}

pub fn chunk_nonce_length_from_metadata(
    metadata: &RecordStreamMetadata,
) -> Result<Option<usize>> {
    Ok(metadata
        .encrypted
        .then_some(CHUNK_ENCRYPTION_NONCE_LENGTH))
}

pub fn chunk_nonce_length_from_metadata_bytes(
    bytes: &[u8],
) -> Result<Option<usize>> {
    chunk_nonce_length_from_metadata(&decode_record_stream_metadata(bytes)?)
}

pub fn payload_descriptor_count_from_metadata(
    metadata: &RecordStreamMetadata,
) -> Result<usize> {
    if metadata.payload_descriptors.is_empty() {
        bail!("payload descriptor list must not be empty");
    }

    if metadata.payload_descriptors.len() > MAX_PAYLOAD_DESCRIPTORS {
        bail!("payload descriptor count exceeds u8 range");
    }

    Ok(metadata.payload_descriptors.len())
}

pub fn payload_descriptor_count_from_metadata_bytes(
    bytes: &[u8],
) -> Result<usize> {
    payload_descriptor_count_from_metadata(
        &decode_record_stream_metadata(bytes)?,
    )
}

pub fn validate_payload_descriptor_index(
    count: usize,
    index: u8,
) -> Result<()> {
    if index as usize >= count {
        bail!(
            "payload descriptor index {index} is out of range for {count} descriptors"
        );
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

        validate_payload_descriptor_index(
            descriptor_count,
            entry.payload_descriptor_index,
        )?;

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
            bail!(
                "payload entry byte ranges cover {offset} bytes, expected {expected}"
            );
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

pub fn validate_track_listing_metadata(
    metadata: &RecordStreamMetadata,
) -> Result<()> {
    if metadata.tracks.is_empty() {
        bail!("track list must not be empty");
    }

    for (index, track) in metadata.tracks.iter().enumerate() {
        if track.title.trim().is_empty() {
            bail!("track {index} title must not be empty");
        }

        if track.payload_entry_index >= metadata.payload_entries.len() {
            bail!(
                "track {index} payload entry index {} is out of range for {} entries",
                track.payload_entry_index,
                metadata.payload_entries.len()
            );
        }
    }

    Ok(())
}

pub fn inspect_record_stream(
    document: &RecordStream,
) -> Result<RecordStreamInspection> {
    let resolved = resolve_payload_entries(
        &document.metadata.payload_entries,
        document.metadata.payload_descriptors.len(),
    )?;

    let payload_byte_length = resolved
        .iter()
        .try_fold(0usize, |total, entry| {
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
                payload_entry_index: track.payload_entry_index,
            })
            .collect(),
    })
}

pub fn read_chunk_header(
    bytes: &[u8],
    chunk_start: usize,
) -> Result<ChunkHeader> {
    read_chunk_header_with_nonce_length(bytes, chunk_start, None)
}

pub fn read_chunk_header_with_nonce_length(
    bytes: &[u8],
    chunk_start: usize,
    nonce_length: Option<usize>,
) -> Result<ChunkHeader> {
    let fixed_end = chunk_start
        .checked_add(CHUNK_FIXED_HEADER_LENGTH)
        .context("chunk header length overflow")?;

    if fixed_end > bytes.len() {
        bail!("truncated chunk header");
    }

    if nonce_length.is_some_and(|length| {
        length != CHUNK_ENCRYPTION_NONCE_LENGTH
    }) {
        bail!("unsupported chunk encryption nonce length");
    }

    let index = read_u16be(bytes, chunk_start, "chunk index")?;
    let chunk_count =
        read_u16be(bytes, chunk_start + 2, "chunk count")?;
    let payload_descriptor_index = bytes[chunk_start + 4];
    let payload_len =
        read_u32be(bytes, chunk_start + 5, "chunk payload length")?
            as usize;
    let crc32 = read_u32be(bytes, chunk_start + 9, "chunk CRC32")?;
    let signature_start = chunk_start + 13;
    let signature_end = signature_start + CHUNK_SIGNATURE_LENGTH;

    let mut signature = [0u8; CHUNK_SIGNATURE_LENGTH];
    signature.copy_from_slice(&bytes[signature_start..signature_end]);

    let nonce_start = nonce_length.map(|_| signature_end);
    let nonce_end =
        nonce_start.map(|start| start + CHUNK_ENCRYPTION_NONCE_LENGTH);
    let payload_start = nonce_end.unwrap_or(signature_end);

    let nonce = match (nonce_start, nonce_end) {
        (Some(start), Some(end)) => {
            let slice = bytes
                .get(start..end)
                .context("truncated chunk nonce")?;
            let mut value = [0u8; CHUNK_ENCRYPTION_NONCE_LENGTH];
            value.copy_from_slice(slice);
            Some(value)
        }
        _ => None,
    };

    let payload_end = payload_start
        .checked_add(payload_len)
        .context("chunk payload length overflow")?;

    if payload_end > bytes.len() {
        bail!("truncated chunk payload");
    }

    Ok(ChunkHeader {
        index,
        chunk_count,
        payload_descriptor_index,
        payload_len,
        crc32,
        signature,
        nonce,
        chunk_start,
        signature_start,
        signature_end,
        nonce_start,
        nonce_end,
        payload_start,
        payload_end,
    })
}

pub fn chunk_signature_preimage(
    metadata_bytes: &[u8],
    index: u16,
    chunk_count: u16,
    payload: &[u8],
    crc32: u32,
) -> Result<Vec<u8>> {
    chunk_signature_preimage_with_descriptor_index(
        metadata_bytes,
        index,
        chunk_count,
        DEFAULT_PAYLOAD_DESCRIPTOR_INDEX,
        payload,
        crc32,
    )
}

pub fn chunk_signature_preimage_with_descriptor_index(
    metadata_bytes: &[u8],
    index: u16,
    chunk_count: u16,
    descriptor_index: u8,
    payload: &[u8],
    crc32: u32,
) -> Result<Vec<u8>> {
    let metadata_len =
        u32::try_from(metadata_bytes.len()).context("metadata exceeds u32")?;
    let payload_len =
        u32::try_from(payload.len()).context("payload exceeds u32")?;

    let mut out = Vec::with_capacity(
        CHUNK_SIGNATURE_DOMAIN.len()
            + metadata_bytes.len()
            + payload.len()
            + 17,
    );

    out.extend_from_slice(CHUNK_SIGNATURE_DOMAIN);
    push_u32be(&mut out, metadata_len);
    out.extend_from_slice(metadata_bytes);
    push_u16be(&mut out, index);
    push_u16be(&mut out, chunk_count);
    out.push(descriptor_index);
    push_u32be(&mut out, payload_len);
    push_u32be(&mut out, crc32);
    out.extend_from_slice(payload);

    Ok(out)
}

pub fn chunk_signature_preimage_with_nonce_and_descriptor_index(
    metadata_bytes: &[u8],
    index: u16,
    chunk_count: u16,
    descriptor_index: u8,
    nonce: &[u8; CHUNK_ENCRYPTION_NONCE_LENGTH],
    payload: &[u8],
    crc32: u32,
) -> Result<Vec<u8>> {
    let metadata_len =
        u32::try_from(metadata_bytes.len()).context("metadata exceeds u32")?;
    let payload_len =
        u32::try_from(payload.len()).context("payload exceeds u32")?;

    let mut out = Vec::new();
    out.extend_from_slice(CHUNK_SIGNATURE_WITH_NONCE_DOMAIN);
    push_u32be(&mut out, metadata_len);
    out.extend_from_slice(metadata_bytes);
    push_u16be(&mut out, index);
    push_u16be(&mut out, chunk_count);
    out.push(descriptor_index);
    push_u32be(&mut out, CHUNK_ENCRYPTION_NONCE_LENGTH as u32);
    out.extend_from_slice(nonce);
    push_u32be(&mut out, payload_len);
    push_u32be(&mut out, crc32);
    out.extend_from_slice(payload);

    Ok(out)
}

pub fn chunk_signature_preimage_with_nonce(
    metadata_bytes: &[u8],
    index: u16,
    chunk_count: u16,
    nonce: &[u8; CHUNK_ENCRYPTION_NONCE_LENGTH],
    payload: &[u8],
    crc32: u32,
) -> Result<Vec<u8>> {
    chunk_signature_preimage_with_nonce_and_descriptor_index(
        metadata_bytes,
        index,
        chunk_count,
        DEFAULT_PAYLOAD_DESCRIPTOR_INDEX,
        nonce,
        payload,
        crc32,
    )
}

fn chunk_signature_preimage_for_chunk(
    metadata_bytes: &[u8],
    chunk: &Chunk,
) -> Result<Vec<u8>> {
    match chunk.nonce.as_ref() {
        Some(nonce) => {
            chunk_signature_preimage_with_nonce_and_descriptor_index(
                metadata_bytes,
                chunk.index,
                chunk.chunk_count,
                chunk.payload_descriptor_index,
                nonce,
                &chunk.payload,
                chunk.crc32,
            )
        }
        None => chunk_signature_preimage_with_descriptor_index(
            metadata_bytes,
            chunk.index,
            chunk.chunk_count,
            chunk.payload_descriptor_index,
            &chunk.payload,
            chunk.crc32,
        ),
    }
}

pub fn chunk_encryption_aad_with_descriptor_index(
    metadata_bytes: &[u8],
    index: u16,
    chunk_count: u16,
    descriptor_index: u8,
    nonce: &[u8; CHUNK_ENCRYPTION_NONCE_LENGTH],
) -> Result<Vec<u8>> {
    let metadata_len =
        u32::try_from(metadata_bytes.len()).context("metadata exceeds u32")?;

    let mut out = Vec::new();
    out.extend_from_slice(CHUNK_ENCRYPTION_DOMAIN);
    push_u32be(&mut out, metadata_len);
    out.extend_from_slice(metadata_bytes);
    push_u16be(&mut out, index);
    push_u16be(&mut out, chunk_count);
    out.push(descriptor_index);
    push_u32be(&mut out, CHUNK_ENCRYPTION_NONCE_LENGTH as u32);
    out.extend_from_slice(nonce);

    Ok(out)
}

pub fn chunk_encryption_aad(
    metadata_bytes: &[u8],
    index: u16,
    chunk_count: u16,
    nonce: &[u8; CHUNK_ENCRYPTION_NONCE_LENGTH],
) -> Result<Vec<u8>> {
    chunk_encryption_aad_with_descriptor_index(
        metadata_bytes,
        index,
        chunk_count,
        DEFAULT_PAYLOAD_DESCRIPTOR_INDEX,
        nonce,
    )
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

    document
        .chunks
        .iter()
        .map(|chunk| {
            let nonce = chunk
                .nonce
                .as_ref()
                .context("encrypted chunk is missing nonce")?;
            let aad = chunk_encryption_aad_with_descriptor_index(
                &document.metadata_bytes,
                chunk.index,
                chunk.chunk_count,
                chunk.payload_descriptor_index,
                nonce,
            )?;

            decrypt_chunk_payload_chacha20poly1305(
                key,
                nonce,
                &aad,
                &chunk.payload,
            )
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

fn validate_chunk_descriptors_against_entries(
    metadata: &RecordStreamMetadata,
    chunks: &[Chunk],
    plaintext_payload_length: Option<usize>,
) -> Result<()> {
    let resolved = validate_payload_entries_metadata(
        metadata,
        plaintext_payload_length,
    )?;

    if metadata.encrypted {
        return Ok(());
    }

    let mut chunk_offset = 0usize;

    for chunk in chunks {
        let chunk_end = chunk_offset
            .checked_add(chunk.payload.len())
            .context("chunk payload offset overflow")?;

        let entry = resolved
            .iter()
            .find(|entry| {
                let entry_end = entry.byte_offset + entry.byte_length;
                chunk_offset >= entry.byte_offset && chunk_end <= entry_end
            })
            .with_context(|| {
                format!(
                    "chunk {} crosses payload-entry boundaries or lies outside the declared entries",
                    chunk.index
                )
            })?;

        if chunk.payload_descriptor_index != entry.payload_descriptor_index {
            bail!(
                "chunk {} descriptor index {} does not match payload entry {} descriptor index {}",
                chunk.index,
                chunk.payload_descriptor_index,
                entry.index,
                entry.payload_descriptor_index
            );
        }

        chunk_offset = chunk_end;
    }

    Ok(())
}

pub fn parse_record_stream(bytes: &[u8]) -> Result<RecordStream> {
    let header_end = record_stream_header_end(bytes)?;
    let metadata_bytes =
        bytes[RECORD_STREAM_HEADER_LENGTH..header_end].to_vec();
    let metadata = decode_record_stream_metadata(&metadata_bytes)?;

    payload_descriptor_count_from_metadata(&metadata)?;
    validate_track_listing_metadata(&metadata)?;

    let nonce_length = chunk_nonce_length_from_metadata(&metadata)?;
    let mut offset = header_end;
    let mut chunks = Vec::new();
    let mut declared_count = None;

    while offset < bytes.len() {
        let header =
            read_chunk_header_with_nonce_length(bytes, offset, nonce_length)?;

        if header.chunk_count == 0 {
            bail!("chunk count must not be zero");
        }

        validate_payload_descriptor_index(
            metadata.payload_descriptors.len(),
            header.payload_descriptor_index,
        )?;

        if let Some(count) = declared_count {
            if count != header.chunk_count {
                bail!("chunk count changed inside stream");
            }
        } else {
            declared_count = Some(header.chunk_count);
        }

        let expected_index = u16::try_from(chunks.len())
            .context("chunk index exceeds u16 range")?;

        if header.index != expected_index {
            bail!(
                "chunk index mismatch: expected {expected_index}, got {}",
                header.index
            );
        }

        let payload =
            bytes[header.payload_start..header.payload_end].to_vec();

        if crc32_ieee(&payload) != header.crc32 {
            bail!("chunk CRC32 mismatch at chunk {}", header.index);
        }

        chunks.push(Chunk {
            index: header.index,
            chunk_count: header.chunk_count,
            payload_descriptor_index: header.payload_descriptor_index,
            payload,
            crc32: header.crc32,
            signature: header.signature,
            nonce: header.nonce,
        });

        offset = header.payload_end;
    }

    let declared_count =
        declared_count.context("record stream contains no chunks")?;

    if chunks.len() != declared_count as usize {
        bail!(
            "chunk count mismatch: expected {declared_count}, got {}",
            chunks.len()
        );
    }

    let plaintext_payload_length = if metadata.encrypted {
        None
    } else {
        Some(chunks.iter().map(|chunk| chunk.payload.len()).sum())
    };

    validate_chunk_descriptors_against_entries(
        &metadata,
        &chunks,
        plaintext_payload_length,
    )?;

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

pub fn chunk_all_ranges(bytes: &[u8]) -> Result<Vec<ChunkRanges>> {
    let header_end = record_stream_header_end(bytes)?;
    let metadata = record_stream_metadata(bytes)?;
    let nonce_length = chunk_nonce_length_from_metadata(&metadata)?;
    let mut offset = header_end;
    let mut ranges = Vec::new();
    let mut count = None;

    while offset < bytes.len() {
        let header =
            read_chunk_header_with_nonce_length(bytes, offset, nonce_length)?;

        validate_payload_descriptor_index(
            metadata.payload_descriptors.len(),
            header.payload_descriptor_index,
        )?;

        if let Some(expected) = count {
            if expected != header.chunk_count {
                bail!("chunk count changed inside stream");
            }
        } else {
            count = Some(header.chunk_count);
        }

        let expected_index = u16::try_from(ranges.len())
            .context("chunk index exceeds u16 range")?;

        if header.index != expected_index {
            bail!("chunk index mismatch");
        }

        ranges.push(ChunkRanges {
            chunk: offset..header.payload_end,
            payload: header.payload_start..header.payload_end,
            signature: header.signature_start..header.signature_end,
            nonce: header
                .nonce_start
                .zip(header.nonce_end)
                .map(|(start, end)| start..end),
        });

        offset = header.payload_end;
    }

    let expected = count.context("record stream contains no chunks")?;

    if ranges.len() != expected as usize {
        bail!("chunk count mismatch");
    }

    Ok(ranges)
}

pub fn chunk_ranges(bytes: &[u8]) -> Result<Vec<Range<usize>>> {
    Ok(chunk_all_ranges(bytes)?
        .into_iter()
        .map(|ranges| ranges.chunk)
        .collect())
}

pub fn chunk_payload_ranges(bytes: &[u8]) -> Result<Vec<Range<usize>>> {
    Ok(chunk_all_ranges(bytes)?
        .into_iter()
        .map(|ranges| ranges.payload)
        .collect())
}

pub fn chunk_signature_ranges(
    bytes: &[u8],
) -> Result<Vec<Range<usize>>> {
    Ok(chunk_all_ranges(bytes)?
        .into_iter()
        .map(|ranges| ranges.signature)
        .collect())
}

pub fn record_stream_payload_bytes(document: &RecordStream) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        document.chunks.iter().map(|chunk| chunk.payload.len()).sum(),
    );

    for chunk in &document.chunks {
        out.extend_from_slice(&chunk.payload);
    }

    out
}

pub fn chunk_stream_payload_bytes(document: &ChunkStream) -> Vec<u8> {
    record_stream_payload_bytes(document)
}

pub fn verify_chunk_signatures(
    document: &RecordStream,
    mut verify: impl FnMut(
        &[u8],
        &[u8; CHUNK_SIGNATURE_LENGTH],
    ) -> Result<()>,
) -> Result<()> {
    for chunk in &document.chunks {
        let preimage =
            chunk_signature_preimage_for_chunk(&document.metadata_bytes, chunk)?;

        verify(&preimage, &chunk.signature)
            .with_context(|| {
                format!("chunk signature failed at index {}", chunk.index)
            })?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        out.push(CONTAINER_RAW);
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

        assert_eq!(metadata.version, 1);
        assert!(!metadata.encrypted);
        assert_eq!(metadata.payload_descriptors.len(), 1);
        assert_eq!(metadata.payload_entries.len(), 2);
        assert_eq!(metadata.payload_entries[0].byte_length, 9);
        assert_eq!(metadata.payload_entries[1].byte_length, 16);
        assert_eq!(metadata.tracks[0].title, "One");
        assert_eq!(metadata.tracks[1].payload_entry_index, 1);
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
        let entry_length_offset = 7;
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
        let rgb = payload_pixel_count_for_encoding(
            bytes,
            PayloadPixelEncoding::Rgb24,
        )
        .unwrap();
        let toned = payload_pixel_count_for_encoding(
            bytes,
            PayloadPixelEncoding::Toned {
                bits_per_pixel: 20,
            },
        )
        .unwrap();

        assert!(toned > rgb);
    }
}
