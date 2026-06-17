use anyhow::{bail, Context, Result};
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
pub const PAYLOAD_ENTRY_LP_MAGIC: &[u8; 4] = b"BPLP";
pub const PAYLOAD_ENTRY_LP_VERSION: u8 = 1;
pub const PAYLOAD_ENTRY_LP_HEADER_LENGTH: usize = 9;
pub const PAYLOAD_ENTRY_LP_LENGTH_FIELD_LENGTH: usize = 4;
pub const PAYLOAD_ENTRY_CONTAINER_LENGTH_PREFIXED: &str = "BPLP";
pub const CHUNK_STREAM_MAGIC: &[u8; 4] = b"BCS2";
pub const STREAM_HEADER_LENGTH: usize = 8;
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
pub const MAX_PAYLOAD_DESCRIPTORS: usize = u8::MAX as usize + 1;
pub const DEFAULT_PAYLOAD_DESCRIPTOR_INDEX: u8 = 0;
pub const PAYLOAD_DESCRIPTORS_METADATA_FIELD: &str = "payloadDescriptors";
pub const TRACK_LISTING_METADATA_FIELD: &str = "trackListing";
pub const CHUNK_SIGNATURE_DOMAIN: &[u8] = b"bitneedle.chunk-stream.chunk.v2";
pub const CHUNK_ENCRYPTION_ALGORITHM_CHACHA20POLY1305: &str = "chacha20poly1305";
pub const CHUNK_ENCRYPTION_NONCE_LENGTH: usize = 12;
pub const CHUNK_ENCRYPTION_TAG_LENGTH: usize = 16;
pub const CHUNK_ENCRYPTION_KEY_LENGTH: usize = 32;
pub const CHUNK_SIGNATURE_WITH_NONCE_DOMAIN: &[u8] = b"bitneedle.chunk-stream.chunk.v2.nonce";
pub const CHUNK_ENCRYPTION_DOMAIN: &[u8] = b"bitneedle.chunk-stream.chunk-encryption.v2";
pub const PAYLOAD_CONTAINER_ECDC: &str = "ECDC";
pub const DEFAULT_RAW_PAYLOAD_CHUNK_SIZE: usize = 64 * 1024;
pub const PLACEHOLDER_SIGNING_LABEL: &str = "facade-placeholder";

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

#[derive(Debug, Clone, PartialEq)]
pub struct ChunkStream {
    pub metadata: serde_json::Value,
    pub metadata_bytes: Vec<u8>,
    pub chunks: Vec<Chunk>,
}

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

pub fn known_record_profile_names() -> &'static [&'static str] {
    KNOWN_RECORD_PROFILES
}

pub fn normalize_record_profile_name(record_profile: &str) -> Result<String> {
    let normalized = record_profile.trim();
    let name = match normalized {
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

    fn scaled_radius_mm(
        outer_radius_px: i32,
        finished_diameter_mm: f64,
        feature_diameter_mm: f64,
    ) -> i32 {
        let scale = outer_radius_px as f64 / (finished_diameter_mm / 2.0);
        ((feature_diameter_mm / 2.0) * scale).round() as i32
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
        _ => bail!("Unknown record profile: {record_profile}"),
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
    let dink_radius = physical.dink_diameter_mm.map(|diameter_mm| {
        scaled_radius_mm(
            physical.outer_radius_px,
            physical.finished_diameter_mm,
            diameter_mm,
        )
    });
    let authentic_payload_inner_radius = scaled_radius_mm(
        physical.outer_radius_px,
        physical.finished_diameter_mm,
        physical.inner_recorded_diameter_mm,
    );
    let authentic_payload_outer_radius = scaled_radius_mm(
        physical.outer_radius_px,
        physical.finished_diameter_mm,
        physical.outer_recorded_diameter_mm,
    );
    let margin_radius = scaled_radius_mm(
        physical.outer_radius_px,
        physical.finished_diameter_mm,
        physical.margin_diameter_mm,
    );
    let payload_inner_radius = match physical.name {
        "single45" => label_radius + 18,
        "lp" => label_radius + 14,
        _ => authentic_payload_inner_radius,
    };
    let payload_outer_radius = match physical.name {
        "single45" | "lp" => (margin_radius - 6).max(1),
        _ => authentic_payload_outer_radius,
    };
    let label_clearance = (payload_inner_radius - label_radius).max(1);
    let outer_rim_thickness = (physical.outer_radius_px - margin_radius).max(1);
    let lead_in_band_thickness = (margin_radius - payload_outer_radius).max(1);

    Ok(RecordProfileDef {
        name: physical.name,
        spindle_hole_radius,
        dink_radius,
        label_radius,
        label_clearance,
        outer_radius: physical.outer_radius_px,
        outer_rim_thickness,
        lead_in_band_thickness,
    })
}

fn resolve_record_geometry(
    record_profile: &str,
    label_radius: Option<i32>,
    label_clearance: Option<i32>,
    outer_radius: Option<i32>,
) -> Result<RecordGeometry> {
    let profile = record_profile_def(record_profile)?;
    Ok(RecordGeometry {
        record_profile: profile.name.to_string(),
        spindle_hole_radius: profile.spindle_hole_radius,
        dink_radius: profile.dink_radius,
        label_radius: label_radius.unwrap_or(profile.label_radius),
        label_clearance: label_clearance.unwrap_or(profile.label_clearance),
        outer_radius: outer_radius.unwrap_or(profile.outer_radius),
        outer_rim_thickness: profile.outer_rim_thickness,
        lead_in_band_thickness: profile.lead_in_band_thickness,
    })
}

fn header_outer_radius(geometry: &RecordGeometry) -> i32 {
    (geometry.outer_radius - HEADER_SPIRAL_OUTER_EDGE_INSET).max(1)
}

pub fn payload_outer_radius_from_geometry(geometry: &RecordProfileGeometry) -> i32 {
    geometry.payload_outer_radius
}

pub fn payload_inner_radius_from_geometry(geometry: &RecordProfileGeometry) -> i32 {
    geometry.payload_inner_radius
}

fn payload_outer_radius(geometry: &RecordGeometry) -> i32 {
    (header_outer_radius(geometry) - geometry.lead_in_band_thickness).max(1)
}

fn payload_inner_radius(geometry: &RecordGeometry) -> i32 {
    geometry.label_radius + geometry.label_clearance
}

pub fn label_clearance_from_profile_geometry(geometry: &RecordProfileGeometry) -> i32 {
    geometry.payload_inner_radius - geometry.label_radius
}

pub fn label_sidecar_inner_radius_from_profile_geometry(geometry: &RecordProfileGeometry) -> i32 {
    geometry.spindle_hole_radius + 3
}

pub fn label_sidecar_outer_radius_from_profile_geometry(geometry: &RecordProfileGeometry) -> i32 {
    (geometry.label_radius - 3).max(label_sidecar_inner_radius_from_profile_geometry(geometry) + 1)
}

pub fn describe_record_profile(record_profile: &str) -> Result<RecordProfileGeometry> {
    let geometry = resolve_record_geometry(record_profile, None, None, None)?;
    Ok(RecordProfileGeometry {
        record_profile: geometry.record_profile.clone(),
        spindle_hole_radius: geometry.spindle_hole_radius,
        dink_radius: geometry.dink_radius,
        label_radius: geometry.label_radius,
        payload_inner_radius: payload_inner_radius(&geometry),
        payload_outer_radius: payload_outer_radius(&geometry),
        outer_radius: geometry.outer_radius,
        outer_rim_thickness: geometry.outer_rim_thickness,
        lead_in_band_thickness: geometry.lead_in_band_thickness,
    })
}

pub fn visible_spiral_turns(record_profile: &str, b_value: f64) -> Result<f64> {
    if !(b_value > 0.0) {
        bail!("A positive spiral pitch is required.");
    }

    let geometry = resolve_record_geometry(record_profile, None, None, None)?;
    let inner_radius = payload_inner_radius(&geometry) as f64;
    let radial_travel = (payload_outer_radius(&geometry) as f64 - inner_radius).max(0.0);

    Ok(radial_travel / (2.0 * PI * b_value))
}

pub fn spiral_b_value_for_visible_turns(record_profile: &str, visible_turns: f64) -> Result<f64> {
    if !(visible_turns.is_finite() && visible_turns > 0.0) {
        bail!("A positive finite visible turn count is required.");
    }

    let geometry = resolve_record_geometry(record_profile, None, None, None)?;
    let inner_radius = payload_inner_radius(&geometry) as f64;
    let radial_travel = (payload_outer_radius(&geometry) as f64 - inner_radius).max(0.0);

    Ok((radial_travel / (2.0 * PI * visible_turns)).max(MIN_B_VALUE))
}

pub fn resolve_pitch(b_value: f64, pitch: Option<f64>) -> Result<f64> {
    let resolved = pitch.unwrap_or(b_value);

    if resolved <= 0.0 {
        bail!("A positive spiral pitch is required.");
    }

    Ok(resolved)
}

pub fn js_round(value: f64) -> i32 {
    (value + 0.5).floor() as i32
}

pub fn payload_pixel_count_for_byte_length(byte_length: usize) -> usize {
    byte_length.div_ceil(3)
}

pub fn metadata_pixel_count_for_byte_length(byte_length: usize) -> usize {
    byte_length.saturating_mul(2)
}

pub fn metadata_byte_capacity_for_pixel_count(pixel_count: usize) -> usize {
    pixel_count / 2
}

pub fn header_spiral_pitch_for_profile(record_profile: &str) -> Result<f64> {
    let geometry = resolve_record_geometry(record_profile, None, None, None)?;
    let radial_travel =
        (header_outer_radius(&geometry) - payload_outer_radius(&geometry)).max(1) as f64;

    Ok(radial_travel / (2.0 * PI * HEADER_SPIRAL_TURNS.max(0.01)))
}

pub fn trailer_spiral_pitch_for_profile(record_profile: &str) -> Result<f64> {
    let geometry = resolve_record_geometry(record_profile, None, None, None)?;
    let radial_travel = (payload_inner_radius(&geometry) - geometry.label_radius).max(1) as f64;

    Ok(radial_travel / (2.0 * PI * TRAILER_SPIRAL_TURNS.max(0.01)))
}

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
    let record_radius = width.min(height) as f64 / 2.0;
    let resolved_pitch = resolve_pitch(b_value, pitch)?;
    let bounded_outer_radius = trace_outer_radius.min(record_radius - 1.0);
    let bounded_inner_radius = trace_inner_radius.max(0.0);
    let mut occupied = vec![0_u8; width * height];
    let mut ordered_pixel_indices = Vec::new();

    let mut swept_theta = 0.0_f64;
    let mut angle = start_angle;
    let mut radius = bounded_outer_radius;

    while radius >= bounded_inner_radius {
        let x = js_round(center_x + radius * angle.cos());
        let y = js_round(center_y - radius * angle.sin());

        if x >= 0 && x < width as i32 && y >= 0 && y < height as i32 {
            let pixel_index = y as usize * width + x as usize;

            if occupied[pixel_index] == 0 {
                occupied[pixel_index] = 1;
                ordered_pixel_indices.push(pixel_index);
            }
        }

        let theta_step = pixel_gap
            / (radius * radius + resolved_pitch * resolved_pitch)
                .sqrt()
                .max(1e-6);

        swept_theta += theta_step;
        angle = start_angle + if clockwise { -swept_theta } else { swept_theta };
        radius = bounded_outer_radius - resolved_pitch * swept_theta;
    }

    Ok((occupied, ordered_pixel_indices, center_x, center_y))
}

fn header_spiral_pitch_for_geometry(geometry: &RecordGeometry) -> f64 {
    let radial_travel =
        (header_outer_radius(geometry) - payload_outer_radius(geometry)).max(1) as f64;

    radial_travel / (2.0 * PI * HEADER_SPIRAL_TURNS.max(0.01))
}

fn trailer_spiral_pitch_for_geometry(geometry: &RecordGeometry) -> f64 {
    let radial_travel = (payload_inner_radius(geometry) - geometry.label_radius).max(1) as f64;

    radial_travel / (2.0 * PI * TRAILER_SPIRAL_TURNS.max(0.01))
}

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
    let _geometry =
        resolve_record_geometry(record_profile, label_radius, label_clearance, outer_radius)?;

    let (occupied, traced_pixel_indices, center_x, center_y) = trace_record_spiral(
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

    let mut ordered = Vec::with_capacity(traced_pixel_indices.len());

    for pixel_index in traced_pixel_indices {
        if occupied[pixel_index] == 0 {
            continue;
        }

        let x = pixel_index % width;
        let y = pixel_index / width;
        let dx = x as f64 - center_x;
        let dy = y as f64 - center_y;
        let distance = (dx * dx + dy * dy).sqrt();

        if distance > band_inner_radius && distance < band_outer_radius {
            ordered.push(pixel_index);
        }
    }

    Ok(ordered)
}

pub fn build_header_spiral_indices(
    width: usize,
    height: usize,
    record_profile: &str,
    label_radius: Option<i32>,
    label_clearance: Option<i32>,
    outer_radius: Option<i32>,
) -> Result<Vec<usize>> {
    let geometry =
        resolve_record_geometry(record_profile, label_radius, label_clearance, outer_radius)?;

    build_band_spiral_indices(
        width,
        height,
        record_profile,
        Some(geometry.label_radius),
        Some(geometry.label_clearance),
        Some(geometry.outer_radius),
        header_outer_radius(&geometry) as f64,
        payload_outer_radius(&geometry) as f64,
        header_spiral_pitch_for_geometry(&geometry),
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
    let geometry =
        resolve_record_geometry(record_profile, label_radius, label_clearance, outer_radius)?;

    build_band_spiral_indices(
        width,
        height,
        record_profile,
        Some(geometry.label_radius),
        Some(geometry.label_clearance),
        Some(geometry.outer_radius),
        payload_inner_radius(&geometry) as f64,
        geometry.label_radius as f64,
        trailer_spiral_pitch_for_geometry(&geometry),
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
    let geometry =
        resolve_record_geometry(record_profile, label_radius, label_clearance, outer_radius)?;
    let payload_outer = payload_outer_radius(&geometry);
    let payload_inner = payload_inner_radius(&geometry);

    let (occupied, traced_pixel_indices, center_x, center_y) = trace_record_spiral(
        width,
        height,
        b_value,
        None,
        DEFAULT_START_ANGLE,
        1.0,
        true,
        payload_outer as f64,
        0.0,
    )?;

    let mut kinds = vec![0_u8; width * height];
    let inner_cutoff = payload_inner as f64;
    let mut addressable_pixel_count = 0usize;
    let mut ordered_pixel_indices = Vec::with_capacity(traced_pixel_indices.len());

    for pixel_index in traced_pixel_indices {
        if occupied[pixel_index] == 0 {
            continue;
        }

        let x = pixel_index % width;
        let y = pixel_index / width;
        let dx = x as f64 - center_x;
        let dy = y as f64 - center_y;
        let distance = (dx * dx + dy * dy).sqrt();

        if distance <= inner_cutoff {
            continue;
        }

        if distance < payload_outer as f64 {
            kinds[pixel_index] = 1;
            addressable_pixel_count += 1;
            ordered_pixel_indices.push(pixel_index);
        } else {
            kinds[pixel_index] = 2;
        }
    }

    Ok(SpiralMask {
        b_value,
        record_profile: geometry.record_profile,
        label_radius: geometry.label_radius,
        label_clearance: geometry.label_clearance,
        outer_radius: geometry.outer_radius,
        kinds,
        addressable_pixel_count,
        ordered_pixel_indices,
    })
}

fn read_u16be(bytes: &[u8], offset: usize, label: &str) -> Result<u16> {
    if offset + 2 > bytes.len() {
        bail!("{label} is truncated");
    }

    Ok(u16::from_be_bytes(
        bytes[offset..offset + 2].try_into().expect("slice length"),
    ))
}

fn read_u32be(bytes: &[u8], offset: usize, label: &str) -> Result<u32> {
    if offset + 4 > bytes.len() {
        bail!("{label} is truncated");
    }

    Ok(u32::from_be_bytes(
        bytes[offset..offset + 4].try_into().expect("slice length"),
    ))
}

/// A tight text bounding box, expressed as fractions of the record image
/// width/height (scale independent). Produced by the JS label compositor and
/// stored in the record descriptor's arbitrary metadata so encode and decode
/// can reproduce the identical avoid-mask.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
pub struct TextAvoidBox {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Deterministic, platform-independent per-pixel hash in [0, u32::MAX].
/// Kept integer-only so it is byte-identical across the encode and decode
/// builds (no float transcendentals involved).
fn text_avoid_pixel_hash(seed: u32, x: u32, y: u32) -> u32 {
    let mut h = seed
        .wrapping_mul(0x9E37_79B1)
        .wrapping_add(x.wrapping_mul(0xC2B2_AE3D))
        .wrapping_add(y.wrapping_mul(0x27D4_EB2F));
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    h
}

/// Tuning for the label-text avoid mask. Travels in the record descriptor's
/// arbitrary metadata (alongside the boxes) so encode and decode rebuild the
/// byte-identical mask.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextAvoidMaskParams {
    /// Width of the blend ring outside the tight box, in image px.
    pub feather_px: f64,
    /// Seed for the per-pair coin flips.
    pub seed: u32,
    /// Fraction (0..=1) of interior pixel-pairs kept protected. 1.0 = solid
    /// no-dither box; lower values let sparse pair-swaps land inside the box so
    /// the text region does not read as a clean cut-out.
    pub interior_protect: f64,
    /// Protection density at the inner edge of the feather ring (0..=1),
    /// decaying linearly to 0 at the outer edge. Low values keep the ring
    /// mostly dithered.
    pub feather_protect: f64,
}

impl Default for TextAvoidMaskParams {
    fn default() -> Self {
        Self {
            feather_px: 8.0,
            seed: 0,
            interior_protect: 1.0,
            feather_protect: 0.5,
        }
    }
}

/// Mark label-text pixels as protected (left unpermuted by patternize) so the
/// dither does not run over the glyphs.
///
/// Two density zones, both decided per horizontally aligned pixel **pair**
/// (hash of `(seed, x >> 1, y)`): the sidecar pair-swap steg can only use two
/// adjacent unprotected pixels, so per-pixel randomness would collapse the
/// usable dither density quadratically — pair-aligned coins keep the perceived
/// density equal to the configured one.
///
/// * Interior: protected with probability `interior_protect` (1.0 = solid).
/// * Feather ring: protected with probability
///   `feather_protect * (1 - dist/feather)`, an uneven, seed-stable transition
///   so the dither boundary is not a hard rectangle line.
///
/// Pure function of its inputs, using only integer hashing and IEEE f64
/// compares (no `sqrt`/transcendentals), so it is byte-identical on the encode
/// and decode sides. `protected` is only ever set to `true`, so overlapping
/// boxes/feathers take the union and the result is independent of box order.
pub fn apply_text_avoid_boxes(
    protected: &mut [bool],
    width: usize,
    height: usize,
    boxes: &[TextAvoidBox],
    params: &TextAvoidMaskParams,
) {
    if width == 0 || height == 0 || protected.len() < width * height {
        return;
    }
    let feather = if params.feather_px.is_finite() && params.feather_px > 0.0 {
        params.feather_px
    } else {
        0.0
    };
    let interior_protect = params.interior_protect.clamp(0.0, 1.0);
    let feather_protect = params.feather_protect.clamp(0.0, 1.0);
    let seed = params.seed;
    let width_f = width as f64;
    let height_f = height as f64;
    for b in boxes {
        if !(b.w > 0.0) || !(b.h > 0.0) {
            continue;
        }
        let left = (b.x * width_f).floor();
        let top = (b.y * height_f).floor();
        let right = ((b.x + b.w) * width_f).ceil();
        let bottom = ((b.y + b.h) * height_f).ceil();
        let x0 = (left - feather).floor().max(0.0) as usize;
        let y0 = (top - feather).floor().max(0.0) as usize;
        let x1 = (((right + feather).ceil()).min(width_f)).max(0.0) as usize;
        let y1 = (((bottom + feather).ceil()).min(height_f)).max(0.0) as usize;
        for y in y0..y1 {
            let py = y as f64 + 0.5;
            let dy = if py < top {
                top - py
            } else if py > bottom {
                py - bottom
            } else {
                0.0
            };
            for x in x0..x1 {
                let idx = y * width + x;
                if protected[idx] {
                    continue;
                }
                let px = x as f64 + 0.5;
                let dx = if px < left {
                    left - px
                } else if px > right {
                    px - right
                } else {
                    0.0
                };
                // Pair-aligned coin: both pixels of the (even, odd) horizontal
                // pair share one draw so unprotected openings always come in
                // sidecar-usable pairs.
                let r = (text_avoid_pixel_hash(seed, (x as u32) >> 1, y as u32) as f64)
                    / (u32::MAX as f64);
                if dx <= 0.0 && dy <= 0.0 {
                    // Interior: solid at interior_protect = 1.0, otherwise leave
                    // sparse pair openings so the box blends with the dither.
                    if r < interior_protect {
                        protected[idx] = true;
                    }
                    continue;
                }
                if feather <= 0.0 || feather_protect <= 0.0 {
                    continue;
                }
                let dist_sq = dx * dx + dy * dy;
                if dist_sq >= feather * feather {
                    continue;
                }
                // Ring: protect with probability feather_protect * (1 - d/f).
                // Equivalent compare without sqrt: r/feather_protect < 1 - d/f
                // <=> d < f * (1 - r/feather_protect).
                let threshold = feather * (1.0 - r / feather_protect);
                if threshold > 0.0 && dist_sq < threshold * threshold {
                    protected[idx] = true;
                }
            }
        }
    }
}

fn push_u16be(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn push_u32be(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

pub fn is_payload_entry_length_prefixed_stream(bytes: &[u8]) -> bool {
    bytes.len() >= PAYLOAD_ENTRY_LP_HEADER_LENGTH && &bytes[..4] == PAYLOAD_ENTRY_LP_MAGIC
}


#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PayloadEntryRange {
    pub index: usize,
    pub offset: usize,
    pub byte_length: usize,
}

pub fn payload_entry_length_prefixed_ranges(bytes: &[u8]) -> Result<Vec<PayloadEntryRange>> {
    if bytes.len() < PAYLOAD_ENTRY_LP_HEADER_LENGTH {
        bail!("payload entry stream is too small");
    }

    if &bytes[..4] != PAYLOAD_ENTRY_LP_MAGIC {
        bail!("payload entry stream magic is invalid");
    }

    let version = bytes[4];

    if version != PAYLOAD_ENTRY_LP_VERSION {
        bail!("payload entry stream version is unsupported: {version}");
    }

    let entry_count = read_u32be(bytes, 5, "payload entry count")? as usize;

    if entry_count == 0 {
        bail!("payload entry stream must contain at least one entry");
    }

    if entry_count > MAX_CHUNKS {
        bail!("payload entry count exceeds chunk stream count limit");
    }

    let mut offset = PAYLOAD_ENTRY_LP_HEADER_LENGTH;
    let mut ranges = Vec::with_capacity(entry_count);

    for index in 0..entry_count {
        let entry_len = read_u32be(bytes, offset, "payload entry length")? as usize;
        offset = offset
            .checked_add(PAYLOAD_ENTRY_LP_LENGTH_FIELD_LENGTH)
            .context("payload entry offset overflow")?;

        if entry_len == 0 {
            bail!("payload entry {index} is empty");
        }

        let end = offset
            .checked_add(entry_len)
            .context("payload entry length overflow")?;

        if end > bytes.len() {
            bail!(
                "payload entry {index} exceeds stream length: entry_end={end}, stream_len={}",
                bytes.len()
            );
        }

        ranges.push(PayloadEntryRange {
            index,
            offset,
            byte_length: entry_len,
        });
        offset = end;
    }

    if offset != bytes.len() {
        bail!(
            "payload entry stream has trailing bytes: parsed_end={offset}, stream_len={}",
            bytes.len()
        );
    }

    Ok(ranges)
}

pub fn parse_payload_entry_length_prefixed_stream(bytes: &[u8]) -> Result<Vec<Vec<u8>>> {
    let ranges = payload_entry_length_prefixed_ranges(bytes)?;
    Ok(ranges
        .into_iter()
        .map(|range| bytes[range.offset..range.offset + range.byte_length].to_vec())
        .collect())
}

pub fn payload_entry_length_prefixed_payload_bytes(bytes: &[u8]) -> Result<Vec<u8>> {
    let entries = parse_payload_entry_length_prefixed_stream(bytes)?;
    Ok(concatenate_payload_entries(&entries))
}

pub fn concatenate_payload_entries(entries: &[Vec<u8>]) -> Vec<u8> {
    let total = entries.iter().map(Vec::len).sum::<usize>();
    let mut out = Vec::with_capacity(total);

    for entry in entries {
        out.extend_from_slice(entry);
    }

    out
}

pub fn payload_entries_from_length_prefixed_or_single(bytes: &[u8]) -> Result<Vec<Vec<u8>>> {
    if is_payload_entry_length_prefixed_stream(bytes) {
        return parse_payload_entry_length_prefixed_stream(bytes);
    }

    if bytes.is_empty() {
        bail!("payload is empty");
    }

    Ok(vec![bytes.to_vec()])
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

pub fn stream_header_end(bytes: &[u8]) -> Result<usize> {
    if bytes.len() < STREAM_HEADER_LENGTH {
        bail!("chunk stream is too small");
    }

    if &bytes[..4] != CHUNK_STREAM_MAGIC {
        bail!("chunk stream magic is invalid");
    }

    let metadata_len = read_u32be(bytes, 4, "chunk stream metadata length")? as usize;
    let end = STREAM_HEADER_LENGTH
        .checked_add(metadata_len)
        .context("chunk stream metadata length overflow")?;

    if end > bytes.len() {
        bail!("chunk stream metadata length exceeds stream length");
    }

    Ok(end)
}

pub fn stream_metadata(bytes: &[u8]) -> Result<serde_json::Value> {
    let end = stream_header_end(bytes)?;

    serde_json::from_slice(&bytes[STREAM_HEADER_LENGTH..end])
        .context("chunk stream metadata is not valid JSON")
}

pub fn stream_metadata_bytes(bytes: &[u8]) -> Result<&[u8]> {
    let end = stream_header_end(bytes)?;
    Ok(&bytes[STREAM_HEADER_LENGTH..end])
}

pub fn chunk_nonce_length_from_metadata(metadata: &serde_json::Value) -> Result<Option<usize>> {
    let Some(encryption) = metadata.get("encryption") else {
        return Ok(None);
    };

    if encryption.is_null() || encryption == false {
        return Ok(None);
    }

    let algorithm = encryption
        .get("algorithm")
        .or_else(|| encryption.get("alg"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    if !algorithm.eq_ignore_ascii_case(CHUNK_ENCRYPTION_ALGORITHM_CHACHA20POLY1305) {
        bail!("unsupported chunk encryption algorithm: {algorithm}");
    }

    let nonce_length = encryption
        .get("nonceLength")
        .or_else(|| encryption.get("chunkNonceLength"))
        .or_else(|| encryption.get("perChunkNonceLength"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(CHUNK_ENCRYPTION_NONCE_LENGTH as u64) as usize;

    if nonce_length != CHUNK_ENCRYPTION_NONCE_LENGTH {
        bail!("unsupported chunk encryption nonce length: {nonce_length}");
    }

    Ok(Some(nonce_length))
}

pub fn chunk_nonce_length_from_metadata_bytes(metadata_bytes: &[u8]) -> Result<Option<usize>> {
    let metadata: serde_json::Value = serde_json::from_slice(metadata_bytes)
        .context("chunk stream metadata is not valid JSON")?;

    chunk_nonce_length_from_metadata(&metadata)
}

pub fn payload_descriptor_count_from_metadata(metadata: &serde_json::Value) -> Result<usize> {
    let descriptors = metadata
        .get(PAYLOAD_DESCRIPTORS_METADATA_FIELD)
        .with_context(|| {
            format!("chunk stream metadata missing {PAYLOAD_DESCRIPTORS_METADATA_FIELD}")
        })?
        .as_array()
        .with_context(|| format!("{PAYLOAD_DESCRIPTORS_METADATA_FIELD} must be an array"))?;

    if descriptors.is_empty() {
        bail!("{PAYLOAD_DESCRIPTORS_METADATA_FIELD} must not be empty");
    }

    if descriptors.len() > MAX_PAYLOAD_DESCRIPTORS {
        bail!("{PAYLOAD_DESCRIPTORS_METADATA_FIELD} exceeds u8 index range");
    }

    validate_track_listing_metadata(metadata)?;

    Ok(descriptors.len())
}

pub fn validate_track_listing_metadata(metadata: &serde_json::Value) -> Result<()> {
    let tracks = metadata
        .get(TRACK_LISTING_METADATA_FIELD)
        .with_context(|| format!("chunk stream metadata missing {TRACK_LISTING_METADATA_FIELD}"))?
        .as_array()
        .with_context(|| format!("{TRACK_LISTING_METADATA_FIELD} must be an array"))?;

    if tracks.is_empty() {
        bail!("{TRACK_LISTING_METADATA_FIELD} must not be empty");
    }

    for (index, track) in tracks.iter().enumerate() {
        let Some(object) = track.as_object() else {
            bail!("{TRACK_LISTING_METADATA_FIELD}[{index}] must be an object");
        };
        let title = object
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim();
        if title.is_empty() {
            bail!("{TRACK_LISTING_METADATA_FIELD}[{index}].title must not be empty");
        }
    }

    Ok(())
}

pub fn payload_descriptor_count_from_metadata_bytes(metadata_bytes: &[u8]) -> Result<usize> {
    let metadata: serde_json::Value = serde_json::from_slice(metadata_bytes)
        .context("chunk stream metadata is not valid JSON")?;

    payload_descriptor_count_from_metadata(&metadata)
}

pub fn validate_payload_descriptor_index(
    descriptor_count: usize,
    payload_descriptor_index: u8,
) -> Result<()> {
    if payload_descriptor_index as usize >= descriptor_count {
        bail!(
            "payload descriptor index {} is out of range for {} descriptors",
            payload_descriptor_index,
            descriptor_count
        );
    }

    Ok(())
}


pub fn read_chunk_header(bytes: &[u8], chunk_start: usize) -> Result<ChunkHeader> {
    read_chunk_header_with_nonce_length(bytes, chunk_start, None)
}

pub fn read_chunk_header_with_nonce_length(
    bytes: &[u8],
    chunk_start: usize,
    nonce_length: Option<usize>,
) -> Result<ChunkHeader> {
    if chunk_start + CHUNK_FIXED_HEADER_LENGTH > bytes.len() {
        bail!("truncated chunk header");
    }

    if let Some(nonce_length) = nonce_length {
        if nonce_length != CHUNK_ENCRYPTION_NONCE_LENGTH {
            bail!("unsupported chunk encryption nonce length: {nonce_length}");
        }
    }

    let index = read_u16be(bytes, chunk_start, "chunk index")?;
    let chunk_count = read_u16be(bytes, chunk_start + 2, "chunk count")?;
    let payload_descriptor_index = bytes[chunk_start + 4];
    let payload_len = read_u32be(bytes, chunk_start + 5, "chunk payload length")? as usize;
    let crc32 = read_u32be(bytes, chunk_start + 9, "chunk CRC32")?;
    let signature_start = chunk_start + 13;
    let signature_end = signature_start + CHUNK_SIGNATURE_LENGTH;

    let mut signature = [0u8; CHUNK_SIGNATURE_LENGTH];
    signature.copy_from_slice(&bytes[signature_start..signature_end]);

    let nonce_start = nonce_length.map(|_| signature_end);
    let nonce_end = nonce_start.map(|start| start + CHUNK_ENCRYPTION_NONCE_LENGTH);
    let payload_start = nonce_end.unwrap_or(signature_end);

    if payload_start > bytes.len() {
        bail!("truncated chunk nonce");
    }

    let nonce = match (nonce_start, nonce_end) {
        (Some(start), Some(end)) => {
            if end > bytes.len() {
                bail!("truncated chunk nonce");
            }

            let mut nonce = [0u8; CHUNK_ENCRYPTION_NONCE_LENGTH];
            nonce.copy_from_slice(&bytes[start..end]);
            Some(nonce)
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
    payload_descriptor_index: u8,
    payload: &[u8],
    crc32: u32,
) -> Result<Vec<u8>> {
    let metadata_len = u32::try_from(metadata_bytes.len())
        .context("chunk stream metadata exceeds u32 length limit")?;
    let payload_len =
        u32::try_from(payload.len()).context("chunk payload exceeds u32 length limit")?;

    let mut out = Vec::with_capacity(
        CHUNK_SIGNATURE_DOMAIN.len() + 4 + metadata_bytes.len() + 2 + 2 + 1 + 4 + 4 + payload.len(),
    );

    out.extend_from_slice(CHUNK_SIGNATURE_DOMAIN);
    push_u32be(&mut out, metadata_len);
    out.extend_from_slice(metadata_bytes);
    push_u16be(&mut out, index);
    push_u16be(&mut out, chunk_count);
    out.push(payload_descriptor_index);
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

pub fn chunk_signature_preimage_with_nonce_and_descriptor_index(
    metadata_bytes: &[u8],
    index: u16,
    chunk_count: u16,
    payload_descriptor_index: u8,
    nonce: &[u8; CHUNK_ENCRYPTION_NONCE_LENGTH],
    payload: &[u8],
    crc32: u32,
) -> Result<Vec<u8>> {
    let metadata_len = u32::try_from(metadata_bytes.len())
        .context("chunk stream metadata exceeds u32 length limit")?;
    let payload_len =
        u32::try_from(payload.len()).context("chunk payload exceeds u32 length limit")?;

    let mut out = Vec::with_capacity(
        CHUNK_SIGNATURE_WITH_NONCE_DOMAIN.len()
            + 4
            + metadata_bytes.len()
            + 2
            + 2
            + 1
            + 4
            + CHUNK_ENCRYPTION_NONCE_LENGTH
            + 4
            + 4
            + payload.len(),
    );

    out.extend_from_slice(CHUNK_SIGNATURE_WITH_NONCE_DOMAIN);
    push_u32be(&mut out, metadata_len);
    out.extend_from_slice(metadata_bytes);
    push_u16be(&mut out, index);
    push_u16be(&mut out, chunk_count);
    out.push(payload_descriptor_index);
    push_u32be(&mut out, CHUNK_ENCRYPTION_NONCE_LENGTH as u32);
    out.extend_from_slice(nonce);
    push_u32be(&mut out, payload_len);
    push_u32be(&mut out, crc32);
    out.extend_from_slice(payload);

    Ok(out)
}

fn chunk_signature_preimage_for_chunk(
    metadata_bytes: &[u8],
    index: u16,
    chunk_count: u16,
    payload_descriptor_index: u8,
    nonce: Option<&[u8; CHUNK_ENCRYPTION_NONCE_LENGTH]>,
    payload: &[u8],
    crc32: u32,
) -> Result<Vec<u8>> {
    match nonce {
        Some(nonce) => chunk_signature_preimage_with_nonce_and_descriptor_index(
            metadata_bytes,
            index,
            chunk_count,
            payload_descriptor_index,
            nonce,
            payload,
            crc32,
        ),
        None => chunk_signature_preimage_with_descriptor_index(
            metadata_bytes,
            index,
            chunk_count,
            payload_descriptor_index,
            payload,
            crc32,
        ),
    }
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

pub fn chunk_encryption_aad_with_descriptor_index(
    metadata_bytes: &[u8],
    index: u16,
    chunk_count: u16,
    payload_descriptor_index: u8,
    nonce: &[u8; CHUNK_ENCRYPTION_NONCE_LENGTH],
) -> Result<Vec<u8>> {
    let metadata_len = u32::try_from(metadata_bytes.len())
        .context("chunk stream metadata exceeds u32 length limit")?;

    let mut out = Vec::with_capacity(
        CHUNK_ENCRYPTION_DOMAIN.len()
            + 4
            + metadata_bytes.len()
            + 2
            + 2
            + 1
            + 4
            + CHUNK_ENCRYPTION_NONCE_LENGTH,
    );

    out.extend_from_slice(CHUNK_ENCRYPTION_DOMAIN);
    push_u32be(&mut out, metadata_len);
    out.extend_from_slice(metadata_bytes);
    push_u16be(&mut out, index);
    push_u16be(&mut out, chunk_count);
    out.push(payload_descriptor_index);
    push_u32be(&mut out, CHUNK_ENCRYPTION_NONCE_LENGTH as u32);
    out.extend_from_slice(nonce);

    Ok(out)
}


pub fn decrypt_chunk_payload_chacha20poly1305(
    key: &[u8; CHUNK_ENCRYPTION_KEY_LENGTH],
    nonce: &[u8; CHUNK_ENCRYPTION_NONCE_LENGTH],
    aad: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));

    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            AeadPayload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| anyhow::anyhow!("failed to decrypt chunk payload"))
}

pub fn decrypt_chunk_stream_payloads_chacha20poly1305(
    document: &ChunkStream,
    key: &[u8; CHUNK_ENCRYPTION_KEY_LENGTH],
) -> Result<Vec<Vec<u8>>> {
    chunk_nonce_length_from_metadata(&document.metadata)?
        .context("chunk stream is not encrypted")?;

    let mut payloads = Vec::with_capacity(document.chunks.len());

    for chunk in &document.chunks {
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

        payloads.push(decrypt_chunk_payload_chacha20poly1305(
            key,
            nonce,
            &aad,
            &chunk.payload,
        )?);
    }

    Ok(payloads)
}

pub fn decrypt_chunk_stream_payload_bytes_chacha20poly1305(
    document: &ChunkStream,
    key: &[u8; CHUNK_ENCRYPTION_KEY_LENGTH],
) -> Result<Vec<u8>> {
    let payloads = decrypt_chunk_stream_payloads_chacha20poly1305(document, key)?;
    Ok(concatenate_payload_entries(&payloads))
}


pub fn ecdc_header_end(ecdc: &[u8]) -> Result<usize> {
    if ecdc.len() < 8 {
        bail!("ECDC payload is too small");
    }

    if &ecdc[..4] != b"ECDC" {
        bail!("ECDC magic is invalid");
    }

    let metadata_start = ecdc
        .iter()
        .position(|byte| *byte == b'{')
        .context("ECDC metadata JSON object was not found")?;

    let metadata_end = json_object_end(ecdc, metadata_start)?;
    let metadata = &ecdc[metadata_start..metadata_end];

    let _: serde_json::Value =
        serde_json::from_slice(metadata).context("ECDC metadata is not valid JSON")?;

    Ok(metadata_end)
}

fn json_object_end(bytes: &[u8], start: usize) -> Result<usize> {
    if bytes.get(start) != Some(&b'{') {
        bail!("JSON object does not start with {{");
    }

    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for index in start..bytes.len() {
        let byte = bytes[index];

        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }

        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth
                    .checked_sub(1)
                    .context("JSON object depth underflow")?;

                if depth == 0 {
                    return Ok(index + 1);
                }
            }
            _ => {}
        }
    }

    bail!("ECDC metadata JSON object is unterminated")
}

pub fn ecdc_frame_ranges(ecdc: &[u8], mut offset: usize) -> Result<Vec<Range<usize>>> {
    let mut ranges = Vec::new();

    while offset < ecdc.len() {
        if offset + 4 > ecdc.len() {
            bail!(
                "truncated ECDC frame length at byte {}: payload_len={}",
                offset,
                ecdc.len()
            );
        }

        let packet_len =
            u32::from_be_bytes(ecdc[offset..offset + 4].try_into().expect("slice length")) as usize;

        if packet_len < 4 {
            bail!(
                "invalid ECDC packet length at byte {}: {}",
                offset,
                packet_len
            );
        }

        let frame_end = offset
            .checked_add(packet_len)
            .context("ECDC packet length overflow")?;

        if frame_end > ecdc.len() {
            bail!(
                "ECDC packet at byte {} exceeds payload length: frame_end={}, payload_len={}",
                offset,
                frame_end,
                ecdc.len()
            );
        }

        ranges.push(offset..frame_end);
        offset = frame_end;
    }

    Ok(ranges)
}

pub fn parse_chunk_stream(bytes: &[u8]) -> Result<ChunkStream> {
    let header_end = stream_header_end(bytes)?;
    let metadata_bytes = bytes[STREAM_HEADER_LENGTH..header_end].to_vec();
    let metadata: serde_json::Value = serde_json::from_slice(&metadata_bytes)
        .context("chunk stream metadata is not valid JSON")?;
    let descriptor_count = payload_descriptor_count_from_metadata(&metadata)?;
    let nonce_length = chunk_nonce_length_from_metadata(&metadata)?;

    let mut offset = header_end;
    let mut chunks = Vec::new();
    let mut expected_chunk_count = None;

    while offset < bytes.len() {
        let header = read_chunk_header_with_nonce_length(bytes, offset, nonce_length)?;

        if header.chunk_count == 0 {
            bail!("chunk count must not be zero");
        }

        validate_payload_descriptor_index(descriptor_count, header.payload_descriptor_index)?;

        match expected_chunk_count {
            Some(count) if count != header.chunk_count => {
                bail!("chunk count changed inside stream");
            }
            Some(_) => {}
            None => expected_chunk_count = Some(header.chunk_count),
        }

        let expected_index =
            u16::try_from(chunks.len()).context("chunk index exceeds u16 range")?;

        if header.index != expected_index {
            bail!(
                "chunk index mismatch: expected {}, got {}",
                expected_index,
                header.index
            );
        }

        let payload = bytes[header.payload_start..header.payload_end].to_vec();
        let actual_crc32 = crc32_ieee(&payload);

        if actual_crc32 != header.crc32 {
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

    let expected_chunk_count = expected_chunk_count.context("chunk stream contains no chunks")?;

    if chunks.len() != expected_chunk_count as usize {
        bail!(
            "chunk count mismatch: expected {}, got {}",
            expected_chunk_count,
            chunks.len()
        );
    }

    Ok(ChunkStream {
        metadata,
        metadata_bytes,
        chunks,
    })
}

pub fn validate_chunk_stream(bytes: &[u8]) -> Result<()> {
    parse_chunk_stream(bytes).map(|_| ())
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

pub fn chunk_signature_ranges(bytes: &[u8]) -> Result<Vec<Range<usize>>> {
    Ok(chunk_all_ranges(bytes)?
        .into_iter()
        .map(|ranges| ranges.signature)
        .collect())
}

pub fn chunk_all_ranges(bytes: &[u8]) -> Result<Vec<ChunkRanges>> {
    let header_end = stream_header_end(bytes)?;
    let metadata: serde_json::Value =
        serde_json::from_slice(&bytes[STREAM_HEADER_LENGTH..header_end])
            .context("chunk stream metadata is not valid JSON")?;
    let descriptor_count = payload_descriptor_count_from_metadata(&metadata)?;
    let nonce_length = chunk_nonce_length_from_metadata(&metadata)?;

    let mut offset = header_end;
    let mut ranges = Vec::new();
    let mut expected_chunk_count = None;

    while offset < bytes.len() {
        let header = read_chunk_header_with_nonce_length(bytes, offset, nonce_length)?;

        if header.chunk_count == 0 {
            bail!("chunk count must not be zero");
        }

        validate_payload_descriptor_index(descriptor_count, header.payload_descriptor_index)?;

        match expected_chunk_count {
            Some(count) if count != header.chunk_count => {
                bail!("chunk count changed inside stream");
            }
            Some(_) => {}
            None => expected_chunk_count = Some(header.chunk_count),
        }

        let expected_index =
            u16::try_from(ranges.len()).context("chunk index exceeds u16 range")?;

        if header.index != expected_index {
            bail!(
                "chunk index mismatch: expected {}, got {}",
                expected_index,
                header.index
            );
        }

        ranges.push(ChunkRanges {
            chunk: offset..header.payload_end,
            payload: header.payload_start..header.payload_end,
            signature: header.signature_start..header.signature_end,
            nonce: match (header.nonce_start, header.nonce_end) {
                (Some(start), Some(end)) => Some(start..end),
                _ => None,
            },
        });

        offset = header.payload_end;
    }

    let expected_chunk_count = expected_chunk_count.context("chunk stream contains no chunks")?;

    if ranges.len() != expected_chunk_count as usize {
        bail!(
            "chunk count mismatch: expected {}, got {}",
            expected_chunk_count,
            ranges.len()
        );
    }

    Ok(ranges)
}

pub fn chunk_stream_payload_bytes(document: &ChunkStream) -> Vec<u8> {
    let total = document
        .chunks
        .iter()
        .map(|chunk| chunk.payload.len())
        .sum::<usize>();

    let mut out = Vec::with_capacity(total);

    for chunk in &document.chunks {
        out.extend_from_slice(&chunk.payload);
    }

    out
}

pub fn verify_chunk_signatures(
    document: &ChunkStream,
    mut verify_chunk: impl FnMut(&[u8], &[u8; CHUNK_SIGNATURE_LENGTH]) -> Result<()>,
) -> Result<()> {
    for chunk in &document.chunks {
        let preimage = chunk_signature_preimage_for_chunk(
            &document.metadata_bytes,
            chunk.index,
            chunk.chunk_count,
            chunk.payload_descriptor_index,
            chunk.nonce.as_ref(),
            &chunk.payload,
            chunk.crc32,
        )?;

        verify_chunk(&preimage, &chunk.signature)
            .with_context(|| format!("chunk signature failed at index {}", chunk.index))?;
    }

    Ok(())
}

