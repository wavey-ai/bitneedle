// Copyright © Wavey, Inc.
// Licensed under the Wavey Artist Source Licence.
// Patent pending. All patent rights are reserved except as expressly granted by the licence.
// Commercial licensing: licence@yl.vin

#![doc = include_str!("../README.md")]

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose, Engine as _};
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder};
use record_core::{
    describe_record_profile, normalize_record_profile_name, vari_pitch_params,
    RecordProfileGeometry, SpiralFamily, VariPitchParams,
};
use record_groove::{
    adaptive_gap_tone_lightness, lighten_base_oklch, oklch_lightness, square_side_for_pixel_count,
    ToneOrdering as CarrierToneOrdering, ToneSpan, TonedConfig, TonedPalette, TonedRender,
};
pub mod metadata_groove;

use record_cut::descriptor::{paint_metadata_bytes_as_grayscale, RecordDescriptorInput};
use record_descriptor::{
    RecordDescriptor, SignedReleaseReference, ToneOrdering, ToneSpanDescriptor,
    SIGNED_RELEASE_REFERENCE_HASH_LENGTH, SIGNED_RELEASE_REFERENCE_VERSION,
};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

use metadata_groove::{metadata_fade_pixel_count, paint_unused_metadata_groove};

pub const RECORD_WIDTH: usize = 576;
pub const RECORD_HEIGHT: usize = 576;
pub const RECORD_SIZE: usize = 576;
pub const PAYLOAD_CODE_FORMAT_RGB: &str = "rgb";

const DEFAULT_START_ANGLE: f64 = PI / 2.0;
const MIN_B_VALUE: f64 = 1e-7;
const DEFAULT_MIN_PERCEPTIBLE_TURN_GAP: f64 = 2.0;
const DEFAULT_HARD_MIN_PERCEPTIBLE_TURN_GAP: f64 = 0.9;
const DEFAULT_MAX_PERCEPTIBLE_OUTER_SECTOR_COVERAGE_RATIO: f64 = 0.98;
const EMPTY_GROOVE_VISIBLE_TURNS: f64 = 64.0;

const HEADER_SPIRAL_TURNS: f64 = 2.0;
const TRAILER_SPIRAL_TURNS: f64 = 4.0;
const HEADER_SPIRAL_OUTER_EDGE_INSET: i32 = 1;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderOptions {
    pub spiral_fit_mode: Option<String>,
    pub fit_track_pixel_count: Option<usize>,
    pub min_perceptible_turn_gap: Option<f64>,
    pub track_listing: Option<serde_json::Value>,
    pub dummy_spiral_regions: Option<serde_json::Value>,
    pub header_title: Option<String>,
    pub header_artist: Option<String>,
    pub header_generation_version: Option<String>,
    pub header_release_id: Option<String>,
    pub header_catalog_number: Option<String>,
    pub header_label: Option<String>,
    pub header_copyright_year: Option<u16>,
    pub header_copyright_holder: Option<String>,
    pub header_artwork_credit: Option<String>,
    pub header_license: Option<String>,
    pub header_canonical_url: Option<String>,
    pub header_created_at: Option<u64>,
    pub header_arbitrary_metadata: Option<String>,
    pub header_signature_key_id: Option<String>,
    pub header_signature: Option<String>,
    pub header_release_commitment_sha256: Option<String>,
    pub header_registration_receipts: Option<String>,
    pub cache_encryption_secret_base64url: Option<String>,
    /// CSS hex base colour for a toned groove. When set, the payload is
    /// encoded with a chroma-ordered toned palette (auto-tuned per colour)
    /// instead of raw RGB, the descriptor payload encoding becomes
    /// "toned-v1", and the resolved span tuples are reported back in
    /// `RenderPayload::rgb_tone` for persisting into BRS1 metadata.
    pub groove_tone_color: Option<String>,
    /// How much lighter explicit TrackGap regions render relative to the
    /// normal groove tone: a perceptual (OKLCH lightness) amount from the
    /// track tone toward white, `0.0` (same as track) to `1.0` (white
    /// limit, before gamut mapping). Defaults to `0.2`. Has no effect
    /// unless `groove_tone_color` is also set.
    pub gap_tone_lightness: Option<f64>,
    #[serde(default)]
    pub guide_outlines: bool,
    /// Use a cheap, low-iteration spiral b-value fit instead of the exact
    /// fit search. Intended for progressive/streaming previews (e.g. the
    /// first small ECDC chunk of an in-progress encode), where a slightly
    /// imprecise groove fit is an acceptable trade for staying well inside
    /// the Worker CPU budget — the exact-fit search's iteration cost is
    /// dominated by full-canvas geometry evaluation per iteration and is
    /// unbounded in *wall time* even though it is bounded in iteration
    /// count. Final/published renders must not set this.
    #[serde(default)]
    pub fast_fit: bool,
    /// How much of the payload band this cut lays its programme across,
    /// measured outward-in from the rim, `0 < fraction <= 1`. Defaults to
    /// `record_core::DEFAULT_GROOVE_SPAN_FRACTION`. The pitch is fitted so
    /// the nominal fills exactly this much of the band; whatever is left
    /// inside it stays deadwax, the way a lathe leaves a side it did not
    /// fill. `1.0` is the historical cut that always ran to the label.
    pub groove_span_fraction: Option<f64>,
    /// Groove geometry family: `"archimedean"` (the default) or
    /// `"variPitch"`. Vari-pitch is the house v3 cut — the lathe's
    /// vari-pitch head, spacing breathing in slow bands across the disc.
    pub spiral_family: Option<String>,
    /// Vari-pitch groove character: modulation depth as a fraction of the
    /// base pitch, `0 < depth <= 0.45`. Defaults to `0.28` when the family
    /// is vari-pitch. Ignored for Archimedean.
    pub groove_character: Option<f64>,
    /// Vari-pitch seed. Required when `spiralFamily` is `"variPitch"`: the
    /// caller mints and persists it (the renderer never invents
    /// randomness), and the descriptor carries it so a decoder retraces the
    /// identical groove.
    pub spiral_seed: Option<u64>,
    /// Vari-pitch groove definition, 0–1: how squared-up the spacing
    /// modulation is. Zero glides; toward one the turns cluster into tight
    /// groups separated by wide land. Ignored for Archimedean.
    pub groove_definition: Option<f64>,
    /// Vari-pitch sheen, 0–1: one keeps the raster's interference light
    /// (no dither), zero is the fully dithered matte field. Defaults to
    /// sheen 0.8 — mostly shine, lightly grained. Ignored for Archimedean.
    pub groove_sheen: Option<f64>,
    /// Where the fire burns: "even" (default), "inner", or "outer".
    /// Ignored for Archimedean.
    pub fire_placement: Option<String>,
    /// The fire's own depth, poured into the placement's aura band on top
    /// of the base drift. `grooveCharacter + fireDepth` may not exceed
    /// 0.45. Ignored for Archimedean.
    pub fire_depth: Option<f64>,
    /// Low-level model tuning; absent fields keep the house defaults.
    pub wave_one_cycles: Option<f64>,
    pub wave_two_cycles: Option<f64>,
    pub wave_balance: Option<f64>,
    pub dither_frequency: Option<f64>,
    pub aura_width: Option<f64>,
    pub fire_cycles: Option<f64>,
}

const DEFAULT_GROOVE_SHEEN: f64 = 0.8;

const DEFAULT_GROOVE_CHARACTER: f64 = 0.28;

fn resolve_spiral_family(render_options: &RenderOptions) -> Result<SpiralFamily> {
    let family = match render_options.spiral_family.as_deref() {
        None | Some("archimedean") => {
            if render_options.spiral_seed.is_some() {
                bail!("spiralSeed is only meaningful for the variPitch spiral family");
            }
            if render_options.groove_character.is_some() {
                bail!("grooveCharacter is only meaningful for the variPitch spiral family");
            }
            if render_options.groove_definition.is_some() {
                bail!("grooveDefinition is only meaningful for the variPitch spiral family");
            }
            if render_options.groove_sheen.is_some() {
                bail!("grooveSheen is only meaningful for the variPitch spiral family");
            }
            if render_options.fire_placement.is_some() {
                bail!("firePlacement is only meaningful for the variPitch spiral family");
            }
            SpiralFamily::Archimedean
        }
        Some("variPitch") | Some("vari-pitch") => {
            let seed = render_options
                .spiral_seed
                .context("variPitch requires spiralSeed — mint one and persist it with the cut")?;
            let depth = render_options
                .groove_character
                .unwrap_or(DEFAULT_GROOVE_CHARACTER);
            let placement = match render_options.fire_placement.as_deref() {
                None | Some("even") => record_core::VariPitchPlacement::Even,
                Some("inner") => record_core::VariPitchPlacement::Inner,
                Some("outer") => record_core::VariPitchPlacement::Outer,
                Some(other) => bail!("unknown fire placement {other:?}"),
            };
            let defaults = record_core::VariPitchTuning::default();
            SpiralFamily::VariPitch {
                depth,
                seed,
                definition: render_options.groove_definition.unwrap_or(0.0),
                sheen: render_options.groove_sheen.unwrap_or(DEFAULT_GROOVE_SHEEN),
                placement,
                fire: render_options.fire_depth.unwrap_or(0.0),
                tuning: record_core::VariPitchTuning {
                    wave_one_cycles: render_options
                        .wave_one_cycles
                        .unwrap_or(defaults.wave_one_cycles),
                    wave_two_cycles: render_options
                        .wave_two_cycles
                        .unwrap_or(defaults.wave_two_cycles),
                    wave_balance: render_options.wave_balance.unwrap_or(defaults.wave_balance),
                    dither_frequency: render_options
                        .dither_frequency
                        .unwrap_or(defaults.dither_frequency),
                    aura_width: render_options.aura_width.unwrap_or(defaults.aura_width),
                    fire_cycles: render_options.fire_cycles.unwrap_or(defaults.fire_cycles),
                },
            }
        }
        Some(other) => bail!("unknown spiral family {other:?}"),
    };
    family.validate()?;
    Ok(family)
}

/// The tightest local pitch a family cuts, as a fraction of the base pitch
/// — the factor the perceptible-turn-gap validation must judge against.
fn min_pitch_factor(family: &SpiralFamily) -> f64 {
    match family {
        SpiralFamily::Archimedean => 1.0,
        SpiralFamily::VariPitch { depth, fire, .. } => 1.0 - (depth + fire),
    }
}

pub const DEFAULT_GAP_TONE_LIGHTNESS: f64 = 0.35;

fn normalize_gap_tone_lightness(value: Option<f64>) -> Result<f64> {
    let amount = value.unwrap_or(DEFAULT_GAP_TONE_LIGHTNESS);
    if !amount.is_finite() || !(0.0..=1.0).contains(&amount) {
        bail!("gapToneLightness must be finite and within [0.0, 1.0], got {amount}");
    }
    Ok(amount)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationPayload {
    pub ok: bool,
    pub code: Option<String>,
    pub message: Option<String>,
    pub track_pixel_count: usize,
    pub turn_gap_pixels: Option<f64>,
    pub fill_ratio: f64,
    pub overflow_track_pixels: usize,
    pub imperceptible_track_pixels: usize,
    pub outer_sector_coverage_pixels: Option<usize>,
    pub outer_sector_area_pixels: Option<usize>,
    pub outer_sector_coverage_ratio: Option<f64>,
    pub max_track_pixel_count_absolute: usize,
    pub max_track_pixel_count_perceptible: usize,
    pub absolute_duration_overrun_seconds: f64,
    pub perceptible_duration_overrun_seconds: f64,
    pub suggested_padding_track_pixels: usize,
    pub suggested_padding_seconds: f64,
    pub min_perceptible_turn_gap: f64,
    pub record_profile: String,
    pub spindle_hole_radius: i32,
    pub label_radius: i32,
    pub label_clearance: i32,
    pub outer_radius: i32,
    pub annulus_pixel_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderPayload {
    pub status: String,
    pub record_profile: String,
    pub duration_seconds: f64,
    pub spiral_fit_mode: Option<String>,
    pub exact: bool,
    pub b_value: f64,
    /// The span the pitch was actually fitted against, after any widening
    /// forced by the density floor.
    pub groove_span_fraction: f64,
    /// The radius the nominal was laid out to stop on. A payload that comes
    /// in over its nominal runs past this, inward, toward
    /// `payloadInnerRadius`.
    pub cut_inner_radius: i32,
    pub source_width: usize,
    pub source_height: usize,
    pub source_pixel_count: usize,
    pub filtered_pixel_count: usize,
    pub fit_track_pixel_count: usize,
    pub pixels_added: usize,
    pub pixels_remaining: isize,
    pub unused_spiral_pixels: usize,
    pub overflow_track_pixels: usize,
    /// Resolved groove tone span tuples
    /// (`[byteOffset, byteLength, baseRgbHex, lumaTolerance, bitsPerPixel, ordering]`),
    /// byte offsets relative to the first byte after the raw BRS1 prefix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rgb_tone: Option<serde_json::Value>,
    pub validation: ValidationPayload,
}

#[derive(Debug, Clone)]
pub struct RenderOutput {
    pub png_bytes: Vec<u8>,
    pub payload: RenderPayload,
    pub descriptor: RecordDescriptor,
    pub stream_bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
struct DummySpiralPixelRegion {
    carrier_pixel_start: usize,
    pixel_count: usize,
}

#[derive(Debug, Clone)]
struct RgbColorBlock {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
    byte_length: usize,
}

#[derive(Debug, Clone)]
struct TrackPixels {
    track_data: Vec<u8>,
    pixel_count: usize,
}

#[derive(Debug, Clone)]
struct SpiralMask {
    b_value: f64,
    record_profile: String,
    addressable_pixel_count: usize,
    ordered_pixel_indices: Vec<usize>,
}

#[derive(Debug, Clone)]
struct TransparentRender {
    width: usize,
    height: usize,
    data: Vec<u8>,
    b_value: f64,
    record_profile: String,
    pixels_added: usize,
    pixels_remaining: isize,
    unused_spiral_pixels: usize,
    overflow_track_pixels: usize,
    descriptor: RecordDescriptor,
}

#[derive(Debug, Clone)]
struct FitCandidate {
    b_value: f64,
    addressable_pixel_count: usize,
    pixels_remaining: isize,
}

#[derive(Debug, Clone)]
struct FitResult {
    b_value: f64,
    exact: bool,
}

#[derive(Debug, Clone)]
struct SpiralTrackCapacity {
    record_profile: String,
    spindle_hole_radius: i32,
    label_radius: i32,
    label_clearance: i32,
    outer_radius: i32,
    payload_inner_radius: i32,
    payload_outer_radius: i32,
    annulus_pixel_count: usize,
    max_track_pixel_count_absolute: usize,
}

#[derive(Debug, Clone)]
struct TransparentRenderResult {
    exact: bool,
    b_value: f64,
    groove_span_fraction: f64,
    cut_inner_radius: i32,
    source_width: usize,
    source_height: usize,
    source_pixel_count: usize,
    filtered_pixel_count: usize,
    fit_track_pixel_count: usize,
    rgb_tone: Option<serde_json::Value>,
    rendered: TransparentRender,
    validation: ValidationPayload,
}

pub fn render_chunk_stream_to_png(
    stream: &[u8],
    record_profile: &str,
    duration_seconds: f64,
    render_options_json: Option<&str>,
) -> Result<RenderOutput> {
    render_payload_codes_to_png(
        stream,
        PAYLOAD_CODE_FORMAT_RGB,
        record_profile,
        duration_seconds,
        render_options_json,
    )
}

pub fn render_payload_codes_to_png(
    codes: &[u8],
    code_format: &str,
    record_profile: &str,
    duration_seconds: f64,
    render_options_json: Option<&str>,
) -> Result<RenderOutput> {
    let render_options = parse_render_options(render_options_json)?;
    let result = render_payload_codes_to_transparent_spiral(
        codes,
        code_format,
        record_profile,
        duration_seconds,
        None,
        &render_options,
    )?;

    normalize_spiral_fit_mode(render_options.spiral_fit_mode.as_deref())?;
    let status = if !result.validation.ok {
        "error".to_string()
    } else if !result.exact {
        "needs_padding".to_string()
    } else {
        "ok".to_string()
    };

    if status == "error" {
        let validation_code = result
            .validation
            .code
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let validation_message = result
            .validation
            .message
            .clone()
            .unwrap_or_else(|| "validation failed".to_string());
        bail!(
            "record render failed: {status}; code={validation_code}; message={validation_message}; \
            track_pixels={}; fill_ratio={:.6}; overflow_track_pixels={}; \
            imperceptible_track_pixels={}; suggested_padding_track_pixels={}; \
            absolute_duration_overrun_seconds={:.3}; perceptible_duration_overrun_seconds={:.3}; \
            min_perceptible_turn_gap={:.3}",
            result.validation.track_pixel_count,
            result.validation.fill_ratio,
            result.validation.overflow_track_pixels,
            result.validation.imperceptible_track_pixels,
            result.validation.suggested_padding_track_pixels,
            result.validation.absolute_duration_overrun_seconds,
            result.validation.perceptible_duration_overrun_seconds,
            result.validation.min_perceptible_turn_gap,
        );
    }

    let png_bytes = write_rgba_png(
        result.rendered.width,
        result.rendered.height,
        &result.rendered.data,
    )?;

    let normalized_profile = normalize_record_profile_name(record_profile)?;

    // Mandatory render-time self-check: a record PNG must decode back to the
    // exact BRS1 bytes it was built from. This is a format-integrity boundary,
    // not a debug aid — it catches toned capacity mismatches, palette drift,
    // and groove-extraction errors before any corrupted PNG leaves the encoder.
    // Skipped only for inputs that are not BRS1 record streams (e.g. raw RGB
    // code blocks), which have no chunk-stream decode contract.
    if codes.starts_with(record_core::RECORD_STREAM_MAGIC) {
        verify_rendered_record_roundtrip(&png_bytes, &normalized_profile, codes)
            .context("rendered PNG groove could not be decoded")?;
    }

    let payload = RenderPayload {
        status,
        record_profile: normalized_profile,
        duration_seconds,
        spiral_fit_mode: None,
        exact: result.exact,
        b_value: result.b_value,
        groove_span_fraction: result.groove_span_fraction,
        cut_inner_radius: result.cut_inner_radius,
        source_width: result.source_width,
        source_height: result.source_height,
        source_pixel_count: result.source_pixel_count,
        filtered_pixel_count: result.filtered_pixel_count,
        fit_track_pixel_count: result.fit_track_pixel_count,
        pixels_added: result.rendered.pixels_added,
        pixels_remaining: result.rendered.pixels_remaining,
        unused_spiral_pixels: result.rendered.unused_spiral_pixels,
        overflow_track_pixels: result.rendered.overflow_track_pixels,
        rgb_tone: result.rgb_tone.clone(),
        validation: result.validation.clone(),
    };

    Ok(RenderOutput {
        png_bytes,
        payload,
        descriptor: result.rendered.descriptor.clone(),
        stream_bytes: codes.to_vec(),
    })
}

/// Decodes a freshly rendered record PNG and verifies it reconstructs the exact
/// BRS1 bytes it was built from, then re-parses the recovered stream so a
/// malformed groove cannot leave the encoder boundary.
fn verify_rendered_record_roundtrip(
    png_bytes: &[u8],
    record_profile: &str,
    record_stream_bytes: &[u8],
) -> Result<()> {
    let decoded = record_decode::decode_record_png_to_chunk_stream_for_profile_with_length(
        png_bytes,
        record_profile,
        Some(record_stream_bytes.len()),
    )
    .context("decoding rendered PNG back to a record stream failed")?;

    if decoded.bytes != record_stream_bytes {
        bail!(
            "rendered PNG groove byte mismatch: expected {} bytes, recovered {} bytes, \
             first_difference={:?}",
            record_stream_bytes.len(),
            decoded.bytes.len(),
            first_difference(record_stream_bytes, &decoded.bytes),
        );
    }

    record_core::parse_chunk_stream(&decoded.bytes)
        .context("rendered PNG groove did not decode to a valid record stream")?;

    Ok(())
}

/// Index and bytes of the first position where `a` and `b` differ, or where one
/// ends short. `None` when the slices are byte-identical.
fn first_difference(a: &[u8], b: &[u8]) -> Option<(usize, Option<u8>, Option<u8>)> {
    let max = a.len().max(b.len());
    (0..max).find_map(|i| {
        let (x, y) = (a.get(i).copied(), b.get(i).copied());
        (x != y).then_some((i, x, y))
    })
}

pub fn render_empty_groove_record_to_png(
    record_profile: &str,
    groove_color: [u8; 3],
) -> Result<Vec<u8>> {
    let normalized_profile = normalize_record_profile_name(record_profile)?;
    let b_value = record_core::spiral_b_value_for_visible_turns(
        &normalized_profile,
        EMPTY_GROOVE_VISIBLE_TURNS,
    )?;
    let spiral_mask = build_spiral_mask(
        RECORD_WIDTH,
        RECORD_HEIGHT,
        b_value,
        &SpiralFamily::Archimedean,
        &normalized_profile,
    )?;
    let mut rgba = vec![0_u8; RECORD_WIDTH * RECORD_HEIGHT * 4];

    for pixel_index in spiral_mask.ordered_pixel_indices {
        let rgba_index = pixel_index * 4;
        rgba[rgba_index] = groove_color[0];
        rgba[rgba_index + 1] = groove_color[1];
        rgba[rgba_index + 2] = groove_color[2];
        rgba[rgba_index + 3] = 255;
    }

    write_rgba_png(RECORD_WIDTH, RECORD_HEIGHT, &rgba)
}

fn parse_render_options(raw: Option<&str>) -> Result<RenderOptions> {
    match raw {
        Some(value) if !value.trim().is_empty() => serde_json::from_str::<RenderOptions>(value)
            .with_context(|| format!("Could not parse render options: {value}")),
        _ => Ok(RenderOptions::default()),
    }
}

fn label_clearance_from_geometry(geometry: &RecordProfileGeometry) -> i32 {
    geometry.payload_inner_radius - geometry.label_radius
}

fn payload_pixel_count_for_byte_length(byte_length: usize) -> usize {
    byte_length.div_ceil(3)
}

fn smallest_even_square_side(pixel_count: usize) -> usize {
    let mut side = (pixel_count as f64).sqrt().ceil() as usize;
    side = side.max(2);

    if side % 2 == 1 {
        side += 1;
    }

    while side.saturating_mul(side) < pixel_count {
        side += 2;
    }

    while side > 2 {
        let previous = side - 2;
        if previous.saturating_mul(previous) < pixel_count {
            break;
        }
        side = previous;
    }

    side
}

fn bytes_to_rgb_block(bytes: &[u8]) -> RgbColorBlock {
    let byte_length = bytes.len();
    let real_pixel_count = payload_pixel_count_for_byte_length(byte_length);
    let size = smallest_even_square_side(real_pixel_count);
    let square_pixel_count = size * size;
    let mut rgba = vec![0_u8; square_pixel_count * 4];

    for pixel_index in 0..real_pixel_count {
        let rgb_offset = pixel_index * 3;
        let rgba_offset = pixel_index * 4;
        rgba[rgba_offset] = bytes.get(rgb_offset).copied().unwrap_or(0);
        rgba[rgba_offset + 1] = bytes.get(rgb_offset + 1).copied().unwrap_or(0);
        rgba[rgba_offset + 2] = bytes.get(rgb_offset + 2).copied().unwrap_or(0);
        rgba[rgba_offset + 3] = 255;
    }

    RgbColorBlock {
        width: size,
        height: size,
        rgba,
        byte_length,
    }
}

fn normalize_payload_code_format(format: &str) -> Result<&'static str> {
    let normalized = format.trim().to_ascii_lowercase().replace('_', "-");

    match normalized.as_str() {
        PAYLOAD_CODE_FORMAT_RGB => Ok(PAYLOAD_CODE_FORMAT_RGB),
        other => bail!("Unsupported payload code format: {other}"),
    }
}

fn payload_codes_to_rgb_color_block(codes: &[u8], format: &str) -> Result<RgbColorBlock> {
    match normalize_payload_code_format(format)? {
        PAYLOAD_CODE_FORMAT_RGB => Ok(bytes_to_rgb_block(codes)),
        _ => unreachable!(),
    }
}

fn payload_track_from_rgb_block(block: &RgbColorBlock) -> TrackPixels {
    let pixel_count = payload_pixel_count_for_byte_length(block.byte_length);

    TrackPixels {
        track_data: block.rgba[..pixel_count * 4].to_vec(),
        pixel_count,
    }
}

fn json_usize_field(value: &serde_json::Value, names: &[&str]) -> Option<usize> {
    names.iter().find_map(|name| {
        let raw = value.get(*name)?;
        raw.as_u64()
            .and_then(|number| usize::try_from(number).ok())
            .or_else(|| {
                raw.as_f64().and_then(|number| {
                    if number.is_finite() && number >= 0.0 {
                        Some(number.floor() as usize)
                    } else {
                        None
                    }
                })
            })
    })
}

fn dummy_spiral_pixel_regions(render_options: &RenderOptions) -> Vec<DummySpiralPixelRegion> {
    let Some(regions) = render_options
        .dummy_spiral_regions
        .as_ref()
        .and_then(|value| value.as_array())
    else {
        return Vec::new();
    };

    let mut parsed = regions
        .iter()
        .filter_map(|region| {
            let carrier_pixel_start =
                json_usize_field(region, &["carrierPixelStart", "spiralPixelStart"])?;
            let pixel_count = json_usize_field(region, &["pixelCount", "spiralPixelCount"])?;
            if pixel_count == 0 {
                return None;
            }
            Some(DummySpiralPixelRegion {
                carrier_pixel_start,
                pixel_count,
            })
        })
        .collect::<Vec<_>>();

    parsed.sort_by_key(|region| region.carrier_pixel_start);
    parsed
}

fn dummy_spiral_pixel_regions_for_track(
    render_options: &RenderOptions,
    carrier_pixel_count: usize,
) -> Vec<DummySpiralPixelRegion> {
    dummy_spiral_pixel_regions(render_options)
        .into_iter()
        .filter(|region| region.carrier_pixel_start < carrier_pixel_count)
        .collect()
}

fn paint_dummy_spiral_pixel(data: &mut [u8], pixel_index: usize) {
    let rgba_index = pixel_index * 4;
    if rgba_index + 3 >= data.len() {
        return;
    }
    data[rgba_index] = 136;
    data[rgba_index + 1] = 136;
    data[rgba_index + 2] = 136;
    data[rgba_index + 3] = 255;
}

fn filter_track_pixels(rgba: &[u8], ignore_transparent: bool, ignore_black: bool) -> TrackPixels {
    let mut filtered = Vec::with_capacity(rgba.len());

    for chunk in rgba.chunks_exact(4) {
        let alpha = chunk[3];

        if ignore_transparent && alpha == 0 {
            continue;
        }

        if ignore_black && chunk[0] == 0 && chunk[1] == 0 && chunk[2] == 0 {
            continue;
        }

        filtered.extend_from_slice(chunk);
    }

    TrackPixels {
        pixel_count: filtered.len() / 4,
        track_data: filtered,
    }
}

fn js_round(value: f64) -> i32 {
    (value + 0.5).floor() as i32
}

fn payload_outer_radius(geometry: &RecordProfileGeometry) -> i32 {
    geometry.payload_outer_radius
}

fn payload_inner_radius(geometry: &RecordProfileGeometry) -> i32 {
    geometry.payload_inner_radius
}

/// The radius this cut's pitch is fitted against. Only the fit sees it:
/// the painting mask still spans the whole band, so a payload that overruns
/// its nominal runs on inward instead of being truncated.
fn cut_inner_radius(geometry: &RecordProfileGeometry, span_fraction: f64) -> Result<i32> {
    record_core::cut_inner_radius_from_geometry(geometry, span_fraction)
}

fn header_outer_radius(geometry: &RecordProfileGeometry) -> i32 {
    (geometry.outer_radius - HEADER_SPIRAL_OUTER_EDGE_INSET).max(1)
}

fn header_spiral_pitch_for_geometry(geometry: &RecordProfileGeometry) -> f64 {
    let radial_travel =
        (header_outer_radius(geometry) - payload_outer_radius(geometry)).max(1) as f64;

    radial_travel / (2.0 * PI * HEADER_SPIRAL_TURNS.max(0.01))
}

fn trailer_spiral_pitch_for_geometry(geometry: &RecordProfileGeometry) -> f64 {
    let radial_travel = (payload_inner_radius(geometry) - geometry.label_radius).max(1) as f64;

    radial_travel / (2.0 * PI * TRAILER_SPIRAL_TURNS.max(0.01))
}

fn resolve_pitch(b_value: f64, pitch: Option<f64>) -> Result<f64> {
    let resolved = pitch.unwrap_or(b_value);

    if resolved <= 0.0 {
        bail!("A positive spiral pitch is required.");
    }

    Ok(resolved)
}

fn trace_record_spiral(
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
    let record_radius = width.min(height) as f64 / 2.0;
    let resolved_pitch = resolve_pitch(b_value, pitch)?;
    let bounded_outer_radius = trace_outer_radius.min(record_radius - 1.0);
    let bounded_inner_radius = trace_inner_radius.max(0.0);
    let mut occupied = vec![0_u8; width * height];
    let mut ordered_pixel_indices = Vec::new();

    let vari: Option<VariPitchParams> = vari_pitch_params(
        family,
        ((bounded_outer_radius - bounded_inner_radius) / resolved_pitch).max(0.0),
    );

    let mut swept_theta = 0.0_f64;
    let mut theta_effective = 0.0_f64;
    let mut angle = start_angle;
    let mut radius = bounded_outer_radius;

    while radius >= bounded_inner_radius {
        let draw_radius = match &vari {
            None => radius,
            Some(params) => radius + params.dither(swept_theta),
        };
        let x = js_round(center_x + draw_radius * angle.cos());
        let y = js_round(center_y - draw_radius * angle.sin());

        if x >= 0 && x < width as i32 && y >= 0 && y < height as i32 {
            let pixel_index = y as usize * width + x as usize;

            if occupied[pixel_index] == 0 {
                occupied[pixel_index] = 1;
                ordered_pixel_indices.push(pixel_index);
            }
        }

        let (local_pitch, factor) = match &vari {
            None => (resolved_pitch, 1.0),
            Some(params) => {
                let factor = params.pitch_factor(swept_theta);
                (resolved_pitch * factor, factor)
            }
        };
        let theta_step = pixel_gap
            / (radius * radius + local_pitch * local_pitch)
                .sqrt()
                .max(1e-6);

        swept_theta += theta_step;
        theta_effective += factor * theta_step;
        angle = start_angle + if clockwise { -swept_theta } else { swept_theta };
        radius = match &vari {
            None => bounded_outer_radius - resolved_pitch * swept_theta,
            Some(_) => bounded_outer_radius - resolved_pitch * theta_effective,
        };
    }

    Ok((occupied, ordered_pixel_indices, center_x, center_y))
}

fn build_band_spiral_indices(
    width: usize,
    height: usize,
    band_outer_radius: f64,
    band_inner_radius: f64,
    band_pitch: f64,
) -> Result<Vec<usize>> {
    let (occupied, traced_pixel_indices, center_x, center_y) = trace_record_spiral(
        width,
        height,
        band_pitch,
        // Deadwax bands are always Archimedean: the descriptor rides here,
        // and a decoder must be able to read it before it knows the
        // payload's family.
        &SpiralFamily::Archimedean,
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

fn build_header_spiral_indices(
    width: usize,
    height: usize,
    record_profile: &str,
) -> Result<Vec<usize>> {
    let geometry = describe_record_profile(record_profile)?;

    build_band_spiral_indices(
        width,
        height,
        header_outer_radius(&geometry) as f64,
        payload_outer_radius(&geometry) as f64,
        header_spiral_pitch_for_geometry(&geometry),
    )
}

fn build_trailer_spiral_indices(
    width: usize,
    height: usize,
    record_profile: &str,
) -> Result<Vec<usize>> {
    let geometry = describe_record_profile(record_profile)?;

    build_band_spiral_indices(
        width,
        height,
        payload_inner_radius(&geometry) as f64,
        geometry.label_radius as f64,
        trailer_spiral_pitch_for_geometry(&geometry),
    )
}

fn build_spiral_mask(
    width: usize,
    height: usize,
    b_value: f64,
    family: &SpiralFamily,
    record_profile: &str,
) -> Result<SpiralMask> {
    let geometry = describe_record_profile(record_profile)?;
    let payload_outer = payload_outer_radius(&geometry);
    let payload_inner = payload_inner_radius(&geometry);
    let (occupied, traced_pixel_indices, center_x, center_y) = trace_record_spiral(
        width,
        height,
        b_value,
        family,
        None,
        DEFAULT_START_ANGLE,
        1.0,
        true,
        payload_outer as f64,
        0.0,
    )?;

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
            addressable_pixel_count += 1;
            ordered_pixel_indices.push(pixel_index);
        }
    }

    Ok(SpiralMask {
        b_value,
        record_profile: geometry.record_profile.clone(),
        addressable_pixel_count,
        ordered_pixel_indices,
    })
}

// Exact-fit probes need only the number of addressable pixels. Building an
// ordered `Vec<usize>` for every binary-search and sweep candidate needlessly
// drives the WASM allocator to a large high-water mark. Count the same unique
// traced pixels in place and reserve the ordered representation for the one
// winning spiral that is actually rendered.
fn count_spiral_mask_pixels(
    width: usize,
    height: usize,
    b_value: f64,
    family: &SpiralFamily,
    record_profile: &str,
    span_fraction: f64,
) -> Result<usize> {
    let geometry = describe_record_profile(record_profile)?;
    let payload_outer = payload_outer_radius(&geometry);
    let payload_inner = cut_inner_radius(&geometry, span_fraction)?;
    let center_x = width as f64 / 2.0;
    let center_y = height as f64 / 2.0;
    let record_radius = width.min(height) as f64 / 2.0;
    let resolved_pitch = resolve_pitch(b_value, None)?;
    let bounded_outer_radius = (payload_outer as f64).min(record_radius - 1.0);
    // The mask trace runs to the centre, so the sweep the banding is scaled
    // to is the bounded outer span — identical to the figure the core trace
    // derives for the same bounds.
    let vari: Option<VariPitchParams> =
        vari_pitch_params(family, (bounded_outer_radius / resolved_pitch).max(0.0));
    let mut occupied = vec![0_u8; width * height];
    let mut addressable_pixel_count = 0usize;
    let mut swept_theta = 0.0_f64;
    let mut theta_effective = 0.0_f64;
    let mut angle = DEFAULT_START_ANGLE;
    let mut radius = bounded_outer_radius;

    while radius >= 0.0 {
        let draw_radius = match &vari {
            None => radius,
            Some(params) => radius + params.dither(swept_theta),
        };
        let x = js_round(center_x + draw_radius * angle.cos());
        let y = js_round(center_y - draw_radius * angle.sin());

        if x >= 0 && x < width as i32 && y >= 0 && y < height as i32 {
            let pixel_index = y as usize * width + x as usize;
            if occupied[pixel_index] == 0 {
                occupied[pixel_index] = 1;
                let dx = x as f64 - center_x;
                let dy = y as f64 - center_y;
                let distance = (dx * dx + dy * dy).sqrt();
                if distance > payload_inner as f64 && distance < payload_outer as f64 {
                    addressable_pixel_count += 1;
                }
            }
        }

        let (local_pitch, factor) = match &vari {
            None => (resolved_pitch, 1.0),
            Some(params) => {
                let factor = params.pitch_factor(swept_theta);
                (resolved_pitch * factor, factor)
            }
        };
        let theta_step = 1.0
            / (radius * radius + local_pitch * local_pitch)
                .sqrt()
                .max(1e-6);
        swept_theta += theta_step;
        theta_effective += factor * theta_step;
        angle = DEFAULT_START_ANGLE - swept_theta;
        radius = match &vari {
            None => bounded_outer_radius - resolved_pitch * swept_theta,
            Some(_) => bounded_outer_radius - resolved_pitch * theta_effective,
        };
    }

    Ok(addressable_pixel_count)
}

fn count_addressable_capacity(width: usize, height: usize, record_profile: &str) -> Result<usize> {
    let geometry = describe_record_profile(record_profile)?;
    let payload_outer = payload_outer_radius(&geometry);
    let center_x = width as f64 / 2.0;
    let center_y = height as f64 / 2.0;
    let inner_cutoff = payload_inner_radius(&geometry) as f64;
    let mut count = 0usize;

    for y in 0..height {
        for x in 0..width {
            let dx = x as f64 - center_x;
            let dy = y as f64 - center_y;
            let distance = (dx * dx + dy * dy).sqrt();

            if distance > inner_cutoff && distance < payload_outer as f64 {
                count += 1;
            }
        }
    }

    Ok(count)
}

fn estimate_spiral_track_capacity(
    width: usize,
    height: usize,
    record_profile: &str,
) -> Result<SpiralTrackCapacity> {
    let geometry = describe_record_profile(record_profile)?;
    let annulus_pixel_count = count_addressable_capacity(width, height, &geometry.record_profile)?;

    Ok(SpiralTrackCapacity {
        record_profile: geometry.record_profile.clone(),
        spindle_hole_radius: geometry.spindle_hole_radius,
        label_radius: geometry.label_radius,
        label_clearance: label_clearance_from_geometry(&geometry),
        outer_radius: geometry.outer_radius,
        payload_inner_radius: geometry.payload_inner_radius,
        payload_outer_radius: geometry.payload_outer_radius,
        annulus_pixel_count,
        max_track_pixel_count_absolute: annulus_pixel_count,
    })
}

fn find_b_for_partial_arc(
    track_pixel_count: usize,
    record_profile: &str,
    span_fraction: f64,
) -> Result<f64> {
    let geometry = describe_record_profile(record_profile)?;
    let inner_radius = cut_inner_radius(&geometry, span_fraction)? as f64;
    let outer = payload_outer_radius(&geometry) as f64;
    let annulus_area = (outer * outer - inner_radius * inner_radius).max(1.0);

    Ok(MIN_B_VALUE.max(annulus_area / (2.0 * track_pixel_count.max(1) as f64)))
}

fn evaluate_spiral_fit(
    width: usize,
    height: usize,
    track_pixel_count: usize,
    family: &SpiralFamily,
    record_profile: &str,
    span_fraction: f64,
    b_value: f64,
) -> Result<FitCandidate> {
    let addressable_pixel_count = count_spiral_mask_pixels(
        width,
        height,
        b_value,
        family,
        record_profile,
        span_fraction,
    )?;

    Ok(FitCandidate {
        b_value,
        addressable_pixel_count,
        pixels_remaining: track_pixel_count as isize - addressable_pixel_count as isize,
    })
}

#[allow(clippy::too_many_arguments)]
fn find_optimal_b(
    width: usize,
    height: usize,
    track_pixel_count: usize,
    family: &SpiralFamily,
    record_profile: &str,
    span_fraction: f64,
    initial_b: Option<f64>,
    growth_factor: f64,
    max_expansions: usize,
    max_binary_iterations: usize,
) -> Result<FitCandidate> {
    let start_b = initial_b.unwrap_or(find_b_for_partial_arc(
        track_pixel_count,
        record_profile,
        span_fraction,
    )?);
    let addressable_capacity = count_addressable_capacity(width, height, record_profile)?;
    let start = evaluate_spiral_fit(
        width,
        height,
        track_pixel_count,
        family,
        record_profile,
        span_fraction,
        start_b,
    )?;
    let mut best = start.clone();

    let mut update_best = |candidate: &FitCandidate| {
        let candidate_distance = candidate.pixels_remaining.abs();
        let best_distance = best.pixels_remaining.abs();

        if candidate_distance < best_distance
            || (candidate_distance == best_distance
                && candidate.pixels_remaining <= 0
                && best.pixels_remaining > 0)
        {
            best = candidate.clone();
        }
    };

    let mut low = start.clone();
    let mut high = start;
    let mut low_b = start_b;
    let mut high_b = start_b;
    let capacity_threshold = (addressable_capacity as f64 * 0.995).floor() as usize;
    let saturation_threshold = 8usize.max((addressable_capacity as f64 * 0.001).floor() as usize);

    if low.pixels_remaining == 0 {
        return Ok(low);
    }

    for _ in 0..max_expansions {
        low_b = (low_b / growth_factor).max(MIN_B_VALUE);
        high_b *= growth_factor;
        low = evaluate_spiral_fit(
            width,
            height,
            track_pixel_count,
            family,
            record_profile,
            span_fraction,
            low_b,
        )?;
        high = evaluate_spiral_fit(
            width,
            height,
            track_pixel_count,
            family,
            record_profile,
            span_fraction,
            high_b,
        )?;
        update_best(&low);
        update_best(&high);

        if track_pixel_count >= addressable_capacity
            && (low.addressable_pixel_count >= capacity_threshold
                || high
                    .addressable_pixel_count
                    .abs_diff(low.addressable_pixel_count)
                    <= saturation_threshold)
        {
            return Ok(best);
        }

        if (low.pixels_remaining <= 0 && high.pixels_remaining >= 0)
            || (low.pixels_remaining >= 0 && high.pixels_remaining <= 0)
        {
            break;
        }
    }

    if !((low.pixels_remaining <= 0 && high.pixels_remaining >= 0)
        || (low.pixels_remaining >= 0 && high.pixels_remaining <= 0))
    {
        return Ok(best);
    }

    let (mut left_b, mut right_b, mut left, mut right) = if low_b > high_b {
        (high_b, low_b, high, low)
    } else {
        (low_b, high_b, low, high)
    };

    for _ in 0..max_binary_iterations {
        let mid_b = (left_b + right_b) / 2.0;
        let mid = evaluate_spiral_fit(
            width,
            height,
            track_pixel_count,
            family,
            record_profile,
            span_fraction,
            mid_b,
        )?;
        update_best(&mid);

        if mid.pixels_remaining == 0 {
            return Ok(mid);
        }

        let same_sign_as_left = (mid.pixels_remaining < 0 && left.pixels_remaining < 0)
            || (mid.pixels_remaining > 0 && left.pixels_remaining > 0);

        if same_sign_as_left {
            left_b = mid_b;
            left = mid;
        } else {
            right_b = mid_b;
            right = mid;
        }
    }

    let _ = right;

    Ok(best)
}

#[allow(clippy::too_many_arguments)]
fn find_exact_fit_b(
    width: usize,
    height: usize,
    track_pixel_count: usize,
    family: &SpiralFamily,
    record_profile: &str,
    span_fraction: f64,
    initial_b: Option<f64>,
    growth_factor: f64,
    max_expansions: usize,
    max_binary_iterations: usize,
    final_sweep_steps: usize,
) -> Result<FitResult> {
    let seed_b = initial_b.unwrap_or(
        find_optimal_b(
            width,
            height,
            track_pixel_count,
            family,
            record_profile,
            span_fraction,
            None,
            growth_factor,
            max_expansions,
            max_binary_iterations.min(48),
        )?
        .b_value,
    );

    let mut best = evaluate_spiral_fit(
        width,
        height,
        track_pixel_count,
        family,
        record_profile,
        span_fraction,
        seed_b,
    )?;
    let mut low_b = seed_b;
    let mut high_b = seed_b;
    let mut low = best.clone();
    let mut high = best.clone();

    let mut update_best = |candidate: &FitCandidate| {
        let replace = best.pixels_remaining > 0 && candidate.pixels_remaining <= 0
            || (candidate.pixels_remaining <= 0
                && best.pixels_remaining <= 0
                && candidate.pixels_remaining > best.pixels_remaining)
            || (candidate.pixels_remaining > 0
                && best.pixels_remaining > 0
                && candidate.pixels_remaining < best.pixels_remaining);

        if replace {
            best = candidate.clone();
        }
    };

    if low.pixels_remaining == 0 {
        return Ok(FitResult {
            b_value: low.b_value,
            exact: true,
        });
    }

    for _ in 0..max_expansions {
        low_b = (low_b / growth_factor).max(MIN_B_VALUE);
        high_b *= growth_factor;
        low = evaluate_spiral_fit(
            width,
            height,
            track_pixel_count,
            family,
            record_profile,
            span_fraction,
            low_b,
        )?;
        high = evaluate_spiral_fit(
            width,
            height,
            track_pixel_count,
            family,
            record_profile,
            span_fraction,
            high_b,
        )?;
        update_best(&low);
        update_best(&high);

        if low.pixels_remaining == 0 {
            return Ok(FitResult {
                b_value: low.b_value,
                exact: true,
            });
        }

        if high.pixels_remaining == 0 {
            return Ok(FitResult {
                b_value: high.b_value,
                exact: true,
            });
        }

        if (low.pixels_remaining <= 0 && high.pixels_remaining >= 0)
            || (low.pixels_remaining >= 0 && high.pixels_remaining <= 0)
        {
            break;
        }
    }

    if !((low.pixels_remaining <= 0 && high.pixels_remaining >= 0)
        || (low.pixels_remaining >= 0 && high.pixels_remaining <= 0))
    {
        return Ok(FitResult {
            b_value: best.b_value,
            exact: best.pixels_remaining == 0,
        });
    }

    let (mut left_b, mut right_b, mut left, mut right) = if low_b > high_b {
        (high_b, low_b, high, low)
    } else {
        (low_b, high_b, low, high)
    };

    for _ in 0..max_binary_iterations {
        let mid_b = (left_b + right_b) / 2.0;
        let mid = evaluate_spiral_fit(
            width,
            height,
            track_pixel_count,
            family,
            record_profile,
            span_fraction,
            mid_b,
        )?;
        update_best(&mid);

        if mid.pixels_remaining == 0 {
            return Ok(FitResult {
                b_value: mid.b_value,
                exact: true,
            });
        }

        let same_sign_as_left = (mid.pixels_remaining < 0 && left.pixels_remaining < 0)
            || (mid.pixels_remaining > 0 && left.pixels_remaining > 0);

        if same_sign_as_left {
            left_b = mid_b;
            left = mid;
        } else {
            right_b = mid_b;
            right = mid;
        }
    }

    let sweep_step = (right_b - left_b) / final_sweep_steps as f64;

    if sweep_step > 0.0 {
        for i in 0..=final_sweep_steps {
            let b_value = left_b + sweep_step * i as f64;
            let rendered = evaluate_spiral_fit(
                width,
                height,
                track_pixel_count,
                family,
                record_profile,
                span_fraction,
                b_value,
            )?;
            update_best(&rendered);

            if rendered.pixels_remaining == 0 {
                return Ok(FitResult {
                    b_value: rendered.b_value,
                    exact: true,
                });
            }
        }
    }

    let _ = right;

    Ok(FitResult {
        b_value: best.b_value,
        exact: best.pixels_remaining == 0,
    })
}

/// How much wider the cut reaches each time the density floor turns it
/// back. The search starts from a closed-form estimate of where the floor
/// bites, so this only has to walk off the few percent the estimate loses to
/// diagonal steps — small steps, and few of them.
const CUT_SPAN_WIDEN_FACTOR: f64 = 1.08;

/// The narrowest span whose turns could clear [`MIN_TURN_SEPARATION_PX`],
/// in closed form.
///
/// A spiral of pitch `p` running from `R_out` in to `R_end` has an arc
/// length of about `PI * (R_out^2 - R_end^2) / p`, so the radius at which a
/// given pixel count runs out at the floor pitch falls straight out of it.
/// The traced groove loses a few percent to diagonal steps and duplicate
/// pixels, so this is a floor to start the search from, not the answer.
/// Seeding with it keeps `solve_cut` monotone in the requested span: without
/// it a geometric ladder can overshoot a narrower request past a wider one
/// that would have fitted.
fn span_fraction_floor_estimate(track_pixel_count: usize, geometry: &RecordProfileGeometry) -> f64 {
    let outer = payload_outer_radius(geometry) as f64;
    let inner = payload_inner_radius(geometry) as f64;
    let band = (outer - inner).max(1.0);
    let swept = outer * outer - track_pixel_count as f64 * record_core::MIN_TURN_SEPARATION_PX / PI;

    if swept <= inner * inner {
        return 1.0;
    }

    ((outer - swept.sqrt()) / band).clamp(0.0, 1.0)
}

#[derive(Debug, Clone)]
struct CutFit {
    span_fraction: f64,
    fit: FitResult,
}

/// Lay out the cut: the pitch that puts `track_pixel_count` across
/// `requested_span_fraction` of the band, widened only if that pitch would
/// pack the turns tighter than `MIN_TURN_SEPARATION_PX`.
///
/// `track_pixel_count` is the nominal — the caller's declared final size,
/// not what this render happens to be holding. A progressive load passes the
/// same nominal for every chunk and so resolves the identical pitch each
/// time, which is what lets a partial render be a prefix of the finished cut
/// rather than a smaller record of its own.
///
/// If the floor cannot be cleared even across the whole band, the widest
/// attempt is returned and `validate_spiral_renderable` is left to say so:
/// the floor is a preference about where to cut, not a second opinion on
/// whether a record is renderable.
#[allow(clippy::too_many_arguments)]
fn solve_cut(
    width: usize,
    height: usize,
    track_pixel_count: usize,
    family: &SpiralFamily,
    record_profile: &str,
    requested_span_fraction: f64,
    fast_fit: bool,
) -> Result<CutFit> {
    let geometry = describe_record_profile(record_profile)?;
    let mut span_fraction = record_core::validate_groove_span_fraction(requested_span_fraction)?
        .max(span_fraction_floor_estimate(track_pixel_count, &geometry))
        .min(1.0);

    loop {
        let fit = if fast_fit {
            find_exact_fit_b(
                width,
                height,
                track_pixel_count,
                family,
                record_profile,
                span_fraction,
                None,
                1.35,
                6,
                10,
                0,
            )?
        } else {
            find_exact_fit_with_coverage(
                width,
                height,
                track_pixel_count,
                family,
                record_profile,
                span_fraction,
                None,
            )?
        };

        let cleared =
            record_core::turn_separation_px(fit.b_value) >= record_core::MIN_TURN_SEPARATION_PX;

        if cleared || span_fraction >= 1.0 {
            return Ok(CutFit { span_fraction, fit });
        }

        span_fraction = (span_fraction * CUT_SPAN_WIDEN_FACTOR).min(1.0);
    }
}

fn resolve_groove_span_fraction(render_options: &RenderOptions) -> Result<f64> {
    record_core::validate_groove_span_fraction(
        render_options
            .groove_span_fraction
            .unwrap_or(record_core::DEFAULT_GROOVE_SPAN_FRACTION),
    )
}

fn find_exact_fit_with_coverage(
    width: usize,
    height: usize,
    track_pixel_count: usize,
    family: &SpiralFamily,
    record_profile: &str,
    span_fraction: f64,
    initial_b: Option<f64>,
) -> Result<FitResult> {
    find_exact_fit_b(
        width,
        height,
        track_pixel_count,
        family,
        record_profile,
        span_fraction,
        initial_b,
        1.08,
        48,
        64,
        1024,
    )
}

fn paint_descriptor_spiral(
    data: &mut [u8],
    width: usize,
    height: usize,
    record_profile: &str,
    main_b_value: f64,
    descriptor: &RecordDescriptorInput,
) -> Result<RecordDescriptor> {
    let header_indices = build_header_spiral_indices(width, height, record_profile)?;
    let trailer_indices = build_trailer_spiral_indices(width, height, record_profile)?;
    let mut metadata_indices = header_indices.clone();
    let trailer_start = metadata_indices.len();

    metadata_indices.extend_from_slice(&trailer_indices);

    let byte_capacity =
        record_descriptor::metadata_byte_capacity_for_pixel_count(metadata_indices.len());

    let descriptor_bytes = record_cut::descriptor::encode_record_descriptor_stream(
        main_b_value,
        descriptor,
        byte_capacity,
    )?;

    let written_pixels =
        paint_metadata_bytes_as_grayscale(data, &metadata_indices, &descriptor_bytes);

    let header_fade_pixels = metadata_fade_pixel_count(header_indices.len(), HEADER_SPIRAL_TURNS);
    let trailer_fade_pixels =
        metadata_fade_pixel_count(trailer_indices.len(), TRAILER_SPIRAL_TURNS);

    let boundary_fade_pixels = if written_pixels < trailer_start {
        header_fade_pixels
    } else {
        trailer_fade_pixels
    };

    paint_unused_metadata_groove(
        data,
        &metadata_indices,
        written_pixels,
        17,
        boundary_fade_pixels,
    );

    if written_pixels < trailer_start {
        paint_unused_metadata_groove(data, &metadata_indices, trailer_start, 31, 0);
    }

    record_descriptor::decode_record_descriptor_bytes(&descriptor_bytes)
}

fn paint_guide_ring(
    data: &mut [u8],
    width: usize,
    height: usize,
    radius: f64,
    thickness: f64,
    gray: u8,
) {
    let center_x = width as f64 / 2.0;
    let center_y = height as f64 / 2.0;
    let inner = (radius - thickness / 2.0).max(0.0);
    let outer = radius + thickness / 2.0;

    for y in 0..height {
        for x in 0..width {
            let dx = x as f64 - center_x;
            let dy = y as f64 - center_y;
            let distance = (dx * dx + dy * dy).sqrt();

            if distance < inner || distance > outer {
                continue;
            }

            let rgba_index = (y * width + x) * 4;
            data[rgba_index] = gray;
            data[rgba_index + 1] = gray;
            data[rgba_index + 2] = gray;
            data[rgba_index + 3] = 255;
        }
    }
}

fn paint_record_guides(
    data: &mut [u8],
    width: usize,
    height: usize,
    record_profile: &str,
) -> Result<()> {
    let geometry = describe_record_profile(record_profile)?;

    paint_guide_ring(
        data,
        width,
        height,
        (geometry.outer_radius - 1).max(1) as f64,
        1.25,
        152,
    );

    paint_guide_ring(
        data,
        width,
        height,
        (geometry.label_radius - 1).max(1) as f64,
        1.0,
        152,
    );

    paint_guide_ring(
        data,
        width,
        height,
        geometry.spindle_hole_radius.max(1) as f64,
        1.0,
        152,
    );

    Ok(())
}

/// "Scanline" names how the source track buffer is consumed — linearly, as
/// the square RGB block reads out. The write side is groove order: pixels
/// land by walking `spiral_mask.ordered_pixel_indices`, the traced spiral.
#[allow(clippy::too_many_arguments)]
fn render_track_scanline_onto_transparent_spiral(
    width: usize,
    height: usize,
    b_value: f64,
    family: &SpiralFamily,
    track_data: &[u8],
    track_pixel_count: usize,
    record_profile: &str,
    descriptor_input: &RecordDescriptorInput,
    guide_outlines: bool,
    dummy_spiral_regions: &[DummySpiralPixelRegion],
) -> Result<TransparentRender> {
    let spiral_mask = build_spiral_mask(width, height, b_value, family, record_profile)?;
    let mut data = vec![0_u8; width * height * 4];

    if guide_outlines {
        paint_record_guides(&mut data, width, height, record_profile)?;
    }

    let mut track_offset = 0usize;
    let mut carrier_pixels_written = 0usize;
    let mut dummy_region_index = 0usize;
    let mut dummy_region_pixels_written = 0usize;
    let mut pixels_added = 0usize;

    for &pixel_index in spiral_mask.ordered_pixel_indices.iter() {
        while let Some(region) = dummy_spiral_regions.get(dummy_region_index) {
            if carrier_pixels_written <= region.carrier_pixel_start {
                break;
            }
            dummy_region_index += 1;
            dummy_region_pixels_written = 0;
        }

        if let Some(region) = dummy_spiral_regions.get(dummy_region_index) {
            if carrier_pixels_written >= region.carrier_pixel_start
                && dummy_region_pixels_written < region.pixel_count
            {
                paint_dummy_spiral_pixel(&mut data, pixel_index);
                dummy_region_pixels_written += 1;
                pixels_added += 1;
                if dummy_region_pixels_written >= region.pixel_count {
                    dummy_region_index += 1;
                    dummy_region_pixels_written = 0;
                }
                continue;
            }
        }

        if track_offset + 3 >= track_data.len() {
            break;
        }

        let rgba_index = pixel_index * 4;

        data[rgba_index..rgba_index + 4]
            .copy_from_slice(&track_data[track_offset..track_offset + 4]);

        track_offset += 4;
        carrier_pixels_written += 1;
        pixels_added += 1;
    }

    let descriptor = paint_descriptor_spiral(
        &mut data,
        width,
        height,
        record_profile,
        b_value,
        descriptor_input,
    )?;

    let pixels_remaining =
        track_pixel_count as isize - spiral_mask.addressable_pixel_count as isize;

    Ok(TransparentRender {
        width,
        height,
        data,
        b_value: spiral_mask.b_value,
        record_profile: spiral_mask.record_profile,
        pixels_added,
        pixels_remaining,
        unused_spiral_pixels: spiral_mask
            .addressable_pixel_count
            .saturating_sub(track_pixel_count),
        overflow_track_pixels: track_pixel_count
            .saturating_sub(spiral_mask.addressable_pixel_count),
        descriptor,
    })
}

fn measure_outer_sector_coverage_stats(
    rendered: &TransparentRender,
    inner_radius: f64,
    outer_radius: f64,
    sector_start: f64,
    sector_end: f64,
    record_color: [u8; 3],
) -> (usize, usize, f64) {
    let cx = rendered.width as f64 / 2.0;
    let cy = rendered.height as f64 / 2.0;
    let mut covered_pixels = 0usize;
    let mut area_pixels = 0usize;

    for y in 0..rendered.height {
        for x in 0..rendered.width {
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            let distance = (dx * dx + dy * dy).sqrt();

            if distance <= inner_radius || distance > outer_radius {
                continue;
            }

            let angle = dy.atan2(dx);

            if angle < sector_start || angle > sector_end {
                continue;
            }

            area_pixels += 1;

            let index = (y * rendered.width + x) * 4;
            let alpha = rendered.data[index + 3];
            let is_record_color = alpha == 255
                && rendered.data[index] == record_color[0]
                && rendered.data[index + 1] == record_color[1]
                && rendered.data[index + 2] == record_color[2];

            if alpha != 0 && !is_record_color {
                covered_pixels += 1;
            }
        }
    }

    let coverage_ratio = if area_pixels > 0 {
        covered_pixels as f64 / area_pixels as f64
    } else {
        0.0
    };

    (covered_pixels, area_pixels, coverage_ratio)
}

fn estimate_duration_for_pixel_delta(
    duration_seconds: f64,
    track_pixel_count: usize,
    pixel_delta: usize,
) -> f64 {
    if duration_seconds <= 0.0 || track_pixel_count == 0 || pixel_delta == 0 {
        return 0.0;
    }

    (duration_seconds * pixel_delta as f64) / track_pixel_count as f64
}

fn validate_spiral_renderable(
    width: usize,
    height: usize,
    track_pixel_count: usize,
    duration_seconds: f64,
    rendered: &TransparentRender,
    min_pitch_factor: f64,
    min_perceptible_turn_gap: f64,
) -> Result<ValidationPayload> {
    let capacity = estimate_spiral_track_capacity(width, height, &rendered.record_profile)?;

    let overflow_track_pixels = track_pixel_count
        .saturating_sub(capacity.max_track_pixel_count_absolute)
        .max(rendered.overflow_track_pixels)
        .max(rendered.pixels_remaining.max(0) as usize);
    // Judged at the tightest point of the cut: for vari-pitch the local
    // pitch dips to (1 − depth) of the base, and that is where turns crowd.
    let turn_gap_pixels = 2.0 * PI * rendered.b_value * min_pitch_factor;
    let fill_ratio = if capacity.annulus_pixel_count > 0 {
        track_pixel_count as f64 / capacity.annulus_pixel_count as f64
    } else {
        0.0
    };

    let (outer_sector_coverage_pixels, outer_sector_area_pixels, outer_sector_coverage_ratio) =
        measure_outer_sector_coverage_stats(
            rendered,
            capacity.payload_inner_radius as f64,
            capacity.payload_outer_radius as f64,
            PI / 3.0,
            (2.0 * PI) / 3.0,
            [0, 0, 0],
        );

    let mut max_track_pixel_count_perceptible = capacity.max_track_pixel_count_absolute;
    let mut imperceptible_track_pixels = 0usize;
    let is_perceptibly_solid = outer_sector_coverage_ratio
        >= DEFAULT_MAX_PERCEPTIBLE_OUTER_SECTOR_COVERAGE_RATIO
        && turn_gap_pixels < DEFAULT_HARD_MIN_PERCEPTIBLE_TURN_GAP;

    if is_perceptibly_solid && outer_sector_coverage_ratio > 0.0 {
        max_track_pixel_count_perceptible = (((track_pixel_count as f64)
            * (DEFAULT_MAX_PERCEPTIBLE_OUTER_SECTOR_COVERAGE_RATIO / outer_sector_coverage_ratio))
            .floor()
            .max(0.0)) as usize;
        imperceptible_track_pixels =
            track_pixel_count.saturating_sub(max_track_pixel_count_perceptible);
    }

    let suggested_padding_track_pixels = rendered.pixels_remaining.saturating_neg() as usize;
    let suggested_padding_seconds = estimate_duration_for_pixel_delta(
        duration_seconds,
        track_pixel_count,
        suggested_padding_track_pixels,
    );
    let absolute_duration_overrun_seconds = estimate_duration_for_pixel_delta(
        duration_seconds,
        track_pixel_count,
        overflow_track_pixels,
    );
    let perceptible_duration_overrun_seconds = estimate_duration_for_pixel_delta(
        duration_seconds,
        track_pixel_count,
        imperceptible_track_pixels,
    );

    let (code, message) = if overflow_track_pixels > 0 {
        (
            Some("spiral_overflow".to_string()),
            Some(
                "Chunk stream exceeds the available spiral capacity for this record profile."
                    .to_string(),
            ),
        )
    } else if imperceptible_track_pixels > 0 {
        (
            Some("imperceptible_spiral".to_string()),
            Some(
                "The spiral would render too densely to read as a perceptible groove pattern."
                    .to_string(),
            ),
        )
    } else {
        (None, None)
    };

    Ok(ValidationPayload {
        ok: code.is_none(),
        code,
        message,
        track_pixel_count,
        turn_gap_pixels: Some(turn_gap_pixels),
        fill_ratio,
        overflow_track_pixels,
        imperceptible_track_pixels,
        outer_sector_coverage_pixels: Some(outer_sector_coverage_pixels),
        outer_sector_area_pixels: Some(outer_sector_area_pixels),
        outer_sector_coverage_ratio: Some(outer_sector_coverage_ratio),
        max_track_pixel_count_absolute: capacity.max_track_pixel_count_absolute,
        max_track_pixel_count_perceptible,
        absolute_duration_overrun_seconds,
        perceptible_duration_overrun_seconds,
        suggested_padding_track_pixels,
        suggested_padding_seconds,
        min_perceptible_turn_gap,
        record_profile: capacity.record_profile,
        spindle_hole_radius: capacity.spindle_hole_radius,
        label_radius: capacity.label_radius,
        label_clearance: capacity.label_clearance,
        outer_radius: capacity.outer_radius,
        annulus_pixel_count: capacity.annulus_pixel_count,
    })
}

fn signed_release_reference_from_render_options(
    render_options: &RenderOptions,
) -> Result<Option<SignedReleaseReference>> {
    let key_id = render_options.header_signature_key_id.as_deref();
    let commitment = render_options.header_release_commitment_sha256.as_deref();
    let signature = render_options.header_signature.as_deref();

    let (key_id, commitment, signature) = match (key_id, commitment, signature) {
        (Some(key_id), Some(commitment), Some(signature)) => (key_id, commitment, signature),
        (None, None, None) => return Ok(None),
        _ => bail!(
            "headerSignatureKeyId, headerReleaseCommitmentSha256, and headerSignature must all be provided together"
        ),
    };

    let commitment_bytes = decode_base64_field(commitment, "headerReleaseCommitmentSha256")?;
    let release_commitment_sha256: [u8; SIGNED_RELEASE_REFERENCE_HASH_LENGTH] = commitment_bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("headerReleaseCommitmentSha256 must be 32 bytes"))?;
    let signature = decode_base64_field(signature, "headerSignature")?;

    let reference = SignedReleaseReference {
        version: SIGNED_RELEASE_REFERENCE_VERSION,
        release_commitment_sha256,
        key_id: key_id.as_bytes().to_vec(),
        signature,
    };
    reference.validate()?;

    Ok(Some(reference))
}

fn decode_base64_field(value: &str, label: &str) -> Result<Vec<u8>> {
    let trimmed = value.trim();
    general_purpose::URL_SAFE_NO_PAD
        .decode(trimmed)
        .or_else(|_| general_purpose::URL_SAFE.decode(trimmed))
        .or_else(|_| general_purpose::STANDARD.decode(trimmed))
        .with_context(|| format!("{label} is not valid base64"))
}

fn normalize_spiral_fit_mode(mode: Option<&str>) -> Result<Option<String>> {
    let Some(mode) = mode else {
        return Ok(None);
    };

    if mode.trim().is_empty() {
        return Ok(None);
    }

    bail!("spiralFitMode is not supported; omit it to use exact b-value fit");
}

fn resolve_render_min_perceptible_turn_gap(render_options: &RenderOptions) -> Result<Option<f64>> {
    let Some(value) = render_options.min_perceptible_turn_gap else {
        return Ok(None);
    };

    if !(value.is_finite() && value > 0.0) {
        bail!("minPerceptibleTurnGap must be a positive finite value");
    }

    Ok(Some(value))
}

fn render_payload_codes_to_transparent_spiral(
    codes: &[u8],
    code_format: &str,
    record_profile: &str,
    duration_seconds: f64,
    fit_track_pixel_count: Option<usize>,
    render_options: &RenderOptions,
) -> Result<TransparentRenderResult> {
    let normalized_profile = normalize_record_profile_name(record_profile)?;
    let groove_tone = groove_toned_track(codes, render_options)?;
    let (track, source_dimensions, rgb_tone, tone_spans) = match groove_tone {
        Some((track, rgb_tone, tone_spans)) => {
            let side = square_side_for_pixel_count(track.pixel_count);
            let dimensions = (side, side, track.pixel_count);
            (track, dimensions, Some(rgb_tone), tone_spans)
        }
        None => {
            let source = payload_codes_to_rgb_color_block(codes, code_format)?;
            let source_track = payload_track_from_rgb_block(&source);
            let track = filter_track_pixels(&source_track.track_data, true, false);
            let dimensions = (source.width, source.height, source_track.pixel_count);
            (track, dimensions, None, Vec::new())
        }
    };
    let dummy_spiral_regions =
        dummy_spiral_pixel_regions_for_track(render_options, track.pixel_count);
    let dummy_spiral_pixel_count = dummy_spiral_regions
        .iter()
        .map(|region| region.pixel_count)
        .sum::<usize>();
    let required_track_pixel_count = track.pixel_count.saturating_add(dummy_spiral_pixel_count);
    // The nominal the cut is laid out against: the caller's declared final
    // size, not what this render is holding. A progressive load passes the
    // same nominal for every chunk, so every partial render resolves the
    // same pitch and paints a prefix of one groove — nothing already cut
    // ever moves. It is floored at what we actually have, so a payload that
    // comes in over its nominal is laid out for its real size rather than
    // being cut off.
    let nominal_track_pixel_count = render_options
        .fit_track_pixel_count
        .filter(|value| *value > 0);
    normalize_spiral_fit_mode(render_options.spiral_fit_mode.as_deref())?;
    let resolved_fit_track_pixel_count = if let Some(explicit) = fit_track_pixel_count
        .filter(|value| *value > 0)
        .or(nominal_track_pixel_count)
    {
        explicit.max(required_track_pixel_count)
    } else {
        required_track_pixel_count
    };

    let spiral_family = resolve_spiral_family(render_options)?;
    let cut = solve_cut(
        RECORD_WIDTH,
        RECORD_HEIGHT,
        resolved_fit_track_pixel_count,
        &spiral_family,
        &normalized_profile,
        resolve_groove_span_fraction(render_options)?,
        render_options.fast_fit,
    )?;
    let CutFit {
        span_fraction: groove_span_fraction,
        fit,
    } = cut;
    let cut_inner_radius = cut_inner_radius(
        &describe_record_profile(&normalized_profile)?,
        groove_span_fraction,
    )?;

    let cache_encryption = render_options
        .cache_encryption_secret_base64url
        .as_deref()
        .map(record_descriptor::CacheEncryptionDescriptor::from_secret_base64url)
        .transpose()?;

    let descriptor_input = RecordDescriptorInput {
        record_profile: normalized_profile.clone(),
        stream_byte_length: codes.len(),
        payload_encoding: Some(if rgb_tone.is_some() {
            PAYLOAD_ENCODING_TONED_V1.to_string()
        } else {
            normalize_payload_code_format(code_format)?.to_string()
        }),
        title: render_options.header_title.clone(),
        artist: render_options.header_artist.clone(),
        release_id: render_options.header_release_id.clone(),
        catalog_number: render_options.header_catalog_number.clone(),
        label: render_options.header_label.clone(),
        copyright_year: render_options.header_copyright_year,
        copyright_holder: render_options.header_copyright_holder.clone(),
        artwork_credit: render_options.header_artwork_credit.clone(),
        canonical_url: render_options.header_canonical_url.clone(),
        created_at: render_options.header_created_at,
        signed_release_reference: signed_release_reference_from_render_options(render_options)?,
        bsc_pointer: None,
        tone_spans,
        cache_encryption,
        chain_anchor: None,
        additional_signatures: Vec::new(),
        isrcs: Vec::new(),
        upc: None,
        deferred_attestation: None,
        spiral_family,
    };

    let rendered = render_track_scanline_onto_transparent_spiral(
        RECORD_WIDTH,
        RECORD_HEIGHT,
        fit.b_value,
        &spiral_family,
        &track.track_data,
        resolved_fit_track_pixel_count,
        &normalized_profile,
        &descriptor_input,
        render_options.guide_outlines,
        &dummy_spiral_regions,
    )?;

    let min_perceptible_turn_gap = resolve_render_min_perceptible_turn_gap(render_options)?
        .unwrap_or(DEFAULT_MIN_PERCEPTIBLE_TURN_GAP);

    let validation = validate_spiral_renderable(
        RECORD_WIDTH,
        RECORD_HEIGHT,
        resolved_fit_track_pixel_count,
        duration_seconds,
        &rendered,
        min_pitch_factor(&spiral_family),
        min_perceptible_turn_gap,
    )?;

    Ok(TransparentRenderResult {
        exact: fit.exact,
        b_value: fit.b_value,
        groove_span_fraction,
        cut_inner_radius,
        source_width: source_dimensions.0,
        source_height: source_dimensions.1,
        source_pixel_count: source_dimensions.2,
        filtered_pixel_count: track.pixel_count,
        fit_track_pixel_count: resolved_fit_track_pixel_count,
        rgb_tone,
        rendered,
        validation,
    })
}

pub const PAYLOAD_ENCODING_TONED_V1: &str = "toned-v1";
/// Size budget used when auto-tuning a groove tone colour; shared so other
/// layers (e.g. record-wasm metadata injection) resolve identical configs.
pub const GROOVE_TONE_MAX_SIZE_FACTOR: f64 = 1.2;

/// Resolve the byte offsets (within the full BRS1 `codes` stream) at which
/// the groove's tone must switch between the normal track tone and the
/// lighter TrackGap tone, alternating from an initial "normal" state.
///
/// Identity comes entirely from `metadata.track_gaps` — never from
/// uncovered entries, payload size, silence-like content, or container
/// type. Returns `None` when `codes` is not a parseable BRS1 stream, or has
/// no track gaps at all, in which case the caller keeps the single
/// normal-tone request it already had (byte-identical to prior behaviour).
fn track_gap_tone_switch_offsets(codes: &[u8]) -> Option<Vec<usize>> {
    if !codes.starts_with(record_core::RECORD_STREAM_MAGIC) {
        return None;
    }
    let stream = record_core::parse_record_stream(codes).ok()?;
    if stream.metadata.track_gaps.is_empty() {
        return None;
    }
    let chunk_ranges = record_core::chunk_all_ranges(codes).ok()?;
    let entry_count = stream.metadata.payload_entries.len();
    if chunk_ranges.len() != entry_count {
        // Each payload entry is expected to be exactly one transport chunk
        // (true for the ECDC-only authoring path this feature targets). If
        // that invariant doesn't hold, fall back rather than mis-tone.
        return None;
    }

    let mut entry_is_gap = vec![false; entry_count];
    for gap in &stream.metadata.track_gaps {
        let start = gap.first_revolution_index as usize;
        let end = start.checked_add(gap.revolution_count as usize)?;
        for slot in entry_is_gap
            .iter_mut()
            .take(end.min(entry_count))
            .skip(start)
        {
            *slot = true;
        }
    }

    let mut offsets = Vec::new();
    let mut current_is_gap = false; // BRS1 header/metadata, and track entries, start "normal".
    for (index, &is_gap) in entry_is_gap.iter().enumerate() {
        if is_gap != current_is_gap {
            offsets.push(chunk_ranges[index].chunk_start);
            current_is_gap = is_gap;
        }
    }

    Some(offsets)
}

/// Encode `bytes` as toned pixels using one resolved [`TonedConfig`] (luma
/// tolerance, bits per pixel, and ordering held fixed) across every request,
/// varying only `base`. Unlike [`record_groove::encode_toned_spans`],
/// which auto-tunes a fresh config per unique base, this guarantees a
/// TrackGap span never silently gets a different bit density or luma
/// tolerance than the track tone it's paired with.
fn encode_toned_spans_with_shared_config(
    bytes: &[u8],
    requests: &[(usize, [u8; 3])],
    base_config: TonedConfig,
) -> Result<TonedRender> {
    if requests.is_empty() {
        bail!("at least one tone request is required");
    }
    if requests[0].0 != 0 {
        bail!("first tone request must start at byte offset 0");
    }
    for pair in requests.windows(2) {
        if pair[1].0 <= pair[0].0 {
            bail!("tone request offsets must be strictly increasing");
        }
    }

    let mut rgba = Vec::new();
    let mut spans = Vec::with_capacity(requests.len());

    for (i, &(offset, base)) in requests.iter().enumerate() {
        let end = requests.get(i + 1).map_or(bytes.len(), |next| next.0);
        let slice = &bytes[offset..end];

        let config = TonedConfig {
            base,
            ..base_config
        };
        let palette = TonedPalette::shared(config).with_context(|| {
            format!(
                "requested TrackGap lightness cannot produce a valid toned-v1 palette at {} bits per pixel",
                config.bits_per_pixel
            )
        })?;

        let pixel_offset = rgba.len() / 4;
        let span_rgba = palette.bytes_to_rgba(slice);
        spans.push(ToneSpan {
            byte_offset: offset,
            byte_length: slice.len(),
            pixel_offset,
            pixel_count: span_rgba.len() / 4,
            config,
        });
        rgba.extend_from_slice(&span_rgba);
    }

    Ok(TonedRender { rgba, spans })
}

/// Builds the toned groove track when `grooveToneColor` is set: raw RGB
/// BRS1 prefix followed by toned spans, plus the resolved span tuples
/// (`[byteOffset, byteLength, baseRgbHex, lumaTolerance, bitsPerPixel, ordering]`).
///
/// TrackGap regions (resolved from explicit `metadata.track_gaps`, never
/// inferred) render at a perceptually lighter tone than track regions —
/// `gapToneLightness` (default `0.2`) of the remaining OKLCH lightness
/// distance from the track tone to white, hue preserved, chroma reduced
/// only if needed for the sRGB gamut. The BRS1/ECDC bytes themselves are
/// completely unaffected; only the BRD1 tone-span map and rendered pixels
/// change.
fn groove_toned_track(
    codes: &[u8],
    render_options: &RenderOptions,
) -> Result<Option<(TrackPixels, serde_json::Value, Vec<ToneSpanDescriptor>)>> {
    let Some(hex) = render_options
        .groove_tone_color
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };

    if codes.is_empty() {
        bail!("cannot tone an empty payload");
    }

    let base = TonedConfig::from_hex(hex, 0, 1).base;
    let predominant_gap_tone_lightness =
        normalize_gap_tone_lightness(render_options.gap_tone_lightness)?;

    // Resolve the normal tone's full config first (luma tolerance, bits per
    // pixel, ordering), then immediately derive the lighter TrackGap tone —
    // before any BRS1/chunk byte-range work — so both are available as soon
    // as tone configuration is established, per the canonical pipeline order.
    //
    // A flat lightening fraction shrinks in absolute terms as the track tone
    // itself gets lighter (less room left to white), so it can read as
    // visually identical to the track tone on light records even though it's
    // clearly lighter on mid/dark ones. The adaptive amount keeps
    // `predominant_gap_tone_lightness` for mid/dark bases and scales it up
    // for lighter ones to hold the same absolute contrast.
    let base_config = TonedConfig::balanced(base, GROOVE_TONE_MAX_SIZE_FACTOR)?;
    let base_lightness = oklch_lightness(base);
    let effective_gap_tone_lightness =
        adaptive_gap_tone_lightness(base_lightness, predominant_gap_tone_lightness)?;
    let gap_base = lighten_base_oklch(base, effective_gap_tone_lightness)?;

    let tone_requests: Vec<(usize, [u8; 3])> = match track_gap_tone_switch_offsets(codes) {
        Some(switch_offsets) => {
            let mut requests = vec![(0usize, base)];
            let mut current_is_gap = false;
            for offset in switch_offsets {
                current_is_gap = !current_is_gap;
                requests.push((offset, if current_is_gap { gap_base } else { base }));
            }
            requests
        }
        None => vec![(0usize, base)],
    };

    let toned = encode_toned_spans_with_shared_config(codes, &tone_requests, base_config)?;

    let tone_spans = toned
        .spans
        .iter()
        .map(|span| {
            Ok(ToneSpanDescriptor {
                byte_length: span.byte_length,
                base: span.config.base,
                luma_tolerance: span.config.luma_tolerance,
                bits_per_pixel: u8::try_from(span.config.bits_per_pixel)
                    .context("tone bits per pixel exceeds u8")?,
                ordering: match span.config.ordering {
                    CarrierToneOrdering::BaseProximity => ToneOrdering::BaseProximity,
                    CarrierToneOrdering::ChromaProximity => ToneOrdering::ChromaProximity,
                },
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let spans_json = serde_json::Value::Array(
        toned
            .spans
            .iter()
            .map(|span| {
                serde_json::json!([
                    span.byte_offset,
                    span.byte_length,
                    format!(
                        "{:02X}{:02X}{:02X}",
                        span.config.base[0], span.config.base[1], span.config.base[2]
                    ),
                    span.config.luma_tolerance,
                    span.config.bits_per_pixel,
                    match span.config.ordering {
                        CarrierToneOrdering::BaseProximity => 0,
                        CarrierToneOrdering::ChromaProximity => 1,
                    },
                ])
            })
            .collect(),
    );

    let pixel_count = toned.rgba.len() / 4;
    Ok(Some((
        TrackPixels {
            track_data: toned.rgba,
            pixel_count,
        },
        spans_json,
        tone_spans,
    )))
}

pub fn write_rgba_png(width: usize, height: usize, rgba: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let encoder =
        PngEncoder::new_with_quality(&mut out, CompressionType::Best, FilterType::Adaptive);

    encoder
        .write_image(rgba, width as u32, height as u32, ExtendedColorType::Rgba8)
        .context("failed to encode PNG")?;

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use record_core::parse_chunk_stream;
    use record_cut::{
        encode_record_stream, PayloadDescriptorInput, PayloadEntryInput, RecordStreamInput,
        TrackGapInput, TrackInput,
    };
    use serde_json::Value;
    use std::fs;
    use std::path::PathBuf;

    const WESTSIDE_DURATION_SECONDS: f64 = 208.509396;

    #[test]
    fn fit_counter_matches_materialized_spiral_mask() {
        for profile in ["single45", "lp"] {
            for b_value in [0.5, 1.0, 2.5, 5.0] {
                let materialized = build_spiral_mask(
                    RECORD_WIDTH,
                    RECORD_HEIGHT,
                    b_value,
                    &SpiralFamily::Archimedean,
                    profile,
                )
                .unwrap();
                // The painting mask always spans the whole band; only the
                // fit counter is bounded, so the two agree at span 1.0.
                let counted = count_spiral_mask_pixels(
                    RECORD_WIDTH,
                    RECORD_HEIGHT,
                    b_value,
                    &SpiralFamily::Archimedean,
                    profile,
                    1.0,
                )
                .unwrap();
                assert_eq!(
                    counted, materialized.addressable_pixel_count,
                    "profile={profile} b={b_value}",
                );
            }
        }
    }

    /// A raw RGB code block of `byte_length`, taken from a golden payload so
    /// the pixel content is realistic. Sliced off the BRS1 magic so the
    /// renderer treats it as a code block rather than a chunk stream, which
    /// is what lets the size be chosen freely.
    fn rgb_code_block(byte_length: usize) -> Vec<u8> {
        let payload = fixture_bytes("lori-asha-westside-lp-hq", "lori-asha-westside-lp-hq.ecdc");
        assert!(payload.len() > 64 + byte_length);
        payload[64..64 + byte_length].to_vec()
    }

    fn render_lp(codes: &[u8], options_json: &str) -> RenderPayload {
        render_payload_codes_to_png(
            codes,
            PAYLOAD_CODE_FORMAT_RGB,
            "lp",
            100.0,
            Some(options_json),
        )
        .expect("render should succeed")
        .payload
    }

    /// The cut a progressive load lays down must be the finished cut, not a
    /// smaller record that happens to be on the way to it. Pin the nominal
    /// and the pitch must not move as chunks arrive — that is the whole
    /// invariant behind rendering a record the way a lathe cuts one.
    #[test]
    fn a_pinned_nominal_holds_the_pitch_still_while_the_payload_grows() {
        let nominal_bytes = 90_000;
        let nominal_pixels = record_core::rgb24_pixel_count_for_byte_length(nominal_bytes);
        let options =
            format!(r#"{{"grooveSpanFraction":0.33,"fitTrackPixelCount":{nominal_pixels}}}"#);

        let finished = render_lp(&rgb_code_block(nominal_bytes), &options);

        for arrived in [9_000, 30_000, 60_000, nominal_bytes] {
            let partial = render_lp(&rgb_code_block(arrived), &options);

            assert_eq!(
                partial.b_value, finished.b_value,
                "pitch moved at {arrived} bytes of a {nominal_bytes}-byte nominal",
            );
            assert_eq!(
                partial.groove_span_fraction, finished.groove_span_fraction,
                "span moved at {arrived} bytes",
            );
            assert_eq!(
                partial.cut_inner_radius, finished.cut_inner_radius,
                "the radius the cut is laid out to end on moved at {arrived} bytes",
            );
            assert!(
                partial.pixels_added <= finished.pixels_added,
                "a partial cut painted more than the finished one",
            );
        }
    }

    /// Without a nominal the fit has only what has arrived to go on, so each
    /// partial render is a complete small record at its own pitch. This is
    /// the behaviour the nominal exists to replace; pin it so the two paths
    /// cannot quietly converge.
    #[test]
    fn without_a_nominal_the_pitch_tracks_whatever_has_arrived() {
        let options = r#"{"grooveSpanFraction":0.33}"#;
        let small = render_lp(&rgb_code_block(30_000), options);
        let large = render_lp(&rgb_code_block(90_000), options);

        assert!(
            small.b_value > large.b_value,
            "a shorter payload should cut a looser groove, got {} then {}",
            small.b_value,
            large.b_value,
        );
    }

    /// The span is honoured when the payload fits inside it, and widened —
    /// never packed tighter — when it does not.
    #[test]
    fn the_density_floor_widens_the_span_rather_than_packing_tighter() {
        for requested in [0.25_f64, 0.33, 0.5] {
            let options = format!(r#"{{"grooveSpanFraction":{requested}}}"#);

            let roomy = render_lp(&rgb_code_block(60_000), &options);
            assert!(
                (roomy.groove_span_fraction - requested).abs() < 1e-9,
                "a payload that fits should get the span it asked for, got {}",
                roomy.groove_span_fraction,
            );
            assert!(
                record_core::turn_separation_px(roomy.b_value)
                    >= record_core::MIN_TURN_SEPARATION_PX,
            );

            let crowded = render_lp(&rgb_code_block(240_000), &options);
            assert!(
                crowded.groove_span_fraction > requested,
                "a payload that does not fit should widen past {requested}, got {}",
                crowded.groove_span_fraction,
            );
            assert!(
                record_core::turn_separation_px(crowded.b_value)
                    >= record_core::MIN_TURN_SEPARATION_PX,
                "widening should have cleared the density floor",
            );
        }
    }

    /// A cut that does not fill its band stops short of the label and leaves
    /// the rest as deadwax, which is the whole point of the span.
    #[test]
    fn a_short_cut_stops_short_of_the_label() {
        let geometry = describe_record_profile("lp").unwrap();
        let short = render_lp(&rgb_code_block(60_000), r#"{"grooveSpanFraction":0.33}"#);
        let full = render_lp(&rgb_code_block(60_000), r#"{"grooveSpanFraction":1.0}"#);

        assert!(
            short.cut_inner_radius > geometry.payload_inner_radius,
            "a 0.33 cut should leave deadwax, but ended on {}",
            short.cut_inner_radius,
        );
        assert_eq!(
            full.cut_inner_radius, geometry.payload_inner_radius,
            "a 1.0 cut is the historical fit-to-fill and must still reach the label",
        );
        assert!(
            short.b_value < full.b_value,
            "packing the same payload into less band must tighten the pitch",
        );
    }

    const GOLDENS: &[Golden] = &[
        Golden {
            id: "lori-asha-westside-single45-hq",
            profile: "single45",
            payload_name: "lori-asha-westside-single45-hq.ecdc",
            record_png_name: "lori-asha-westside-single45-hq.record.png",
        },
        Golden {
            id: "lori-asha-westside-lp-hq",
            profile: "lp",
            payload_name: "lori-asha-westside-lp-hq.ecdc",
            record_png_name: "lori-asha-westside-lp-hq.record.png",
        },
    ];

    struct Golden {
        id: &'static str,
        profile: &'static str,
        payload_name: &'static str,
        record_png_name: &'static str,
    }

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("record-render should live one level below repository root")
            .to_path_buf()
    }

    fn fixture_dir(id: &str) -> PathBuf {
        repo_root().join("goldenfiles").join("records").join(id)
    }

    fn fixture_bytes(id: &str, name: &str) -> Vec<u8> {
        let path = fixture_dir(id).join(name);
        fs::read(&path).unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
    }

    #[test]
    fn renders_empty_groove_record_png_for_known_profiles() {
        for profile in record_core::known_record_profile_names() {
            let png =
                render_empty_groove_record_to_png(profile, [228, 86, 79]).unwrap_or_else(|error| {
                    panic!("failed to render empty groove for {profile}: {error:#}")
                });
            let image = image::load_from_memory(&png)
                .unwrap_or_else(|error| panic!("failed to decode {profile} PNG: {error}"))
                .to_rgba8();

            assert_eq!(image.width(), RECORD_WIDTH as u32);
            assert_eq!(image.height(), RECORD_HEIGHT as u32);

            let groove_pixels = image
                .pixels()
                .filter(|pixel| pixel.0 == [228, 86, 79, 255])
                .count();
            assert!(
                groove_pixels > 10_000,
                "{profile} empty groove should contain visible groove pixels"
            );
        }
    }

    fn encode_test_chunk_stream(metadata: Value, payloads: &[Vec<u8>]) -> Vec<u8> {
        let container = metadata
            .get("payloadDescriptors")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("container"))
            .and_then(Value::as_str)
            .unwrap_or("TEST")
            .to_string();

        let first_title = metadata
            .get("trackListing")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("Test Track")
            .to_string();

        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput::from_container(container)],
            tracks: payloads
                .iter()
                .enumerate()
                .map(|(index, _)| TrackInput {
                    title: if index == 0 {
                        first_title.clone()
                    } else {
                        format!("{first_title} {}", index + 1)
                    },
                    first_revolution_index: None,
                    revolution_count: None,
                })
                .collect(),
            track_gaps: vec![],
        };

        let entries = payloads
            .iter()
            .cloned()
            .map(|bytes| PayloadEntryInput {
                payload_descriptor_index: 0,
                bytes,
            })
            .collect::<Vec<_>>();

        encode_record_stream(&input, &entries).unwrap()
    }

    #[test]
    fn toned_groove_renders_and_decodes_byte_exact() {
        let mut state = 0x0123_4567_89ab_cdefu64;
        let payload: Vec<u8> = (0..60_000)
            .map(|_| {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                (state >> 56) as u8
            })
            .collect();

        let metadata = serde_json::json!({
            "payloadDescriptors": [{ "container": "TEST" }],
            "trackListing": [{
                "number": 1,
                "title": "Toned Test",
                "payloadEntryIndex": 0
            }],
        });
        let stream = encode_test_chunk_stream(metadata, &[payload]);

        let options = serde_json::json!({ "grooveToneColor": "#FFC0CB" }).to_string();
        let output =
            render_payload_codes_to_png(&stream, "rgb", "single45", 208.5, Some(&options)).unwrap();

        assert_eq!(
            output.descriptor.payload_encoding,
            PAYLOAD_ENCODING_TONED_V1
        );
        assert!(!output.descriptor.tone_spans.is_empty());
        assert_eq!(
            output
                .descriptor
                .tone_spans
                .iter()
                .map(|span| span.byte_length)
                .sum::<usize>(),
            stream.len()
        );

        let rgb_tone = output
            .payload
            .rgb_tone
            .as_ref()
            .expect("resolved tone spans");
        assert_eq!(rgb_tone[0][0], serde_json::json!(0));
        assert_eq!(rgb_tone[0][1], serde_json::json!(stream.len()));

        let decoded = record_decode::decode_record_png_to_chunk_stream_for_profile_with_length(
            &output.png_bytes,
            "single45",
            Some(stream.len()),
        )
        .unwrap();
        assert_eq!(decoded.bytes, stream, "toned groove did not round-trip");
    }

    #[test]
    fn two_track_compact_groove_renders_and_decodes_byte_exact() {
        let payload_one = vec![0xAAu8; 4_000];
        let payload_two = vec![0xBBu8; 5_500];

        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput::from_container("TEST")],
            tracks: vec![
                TrackInput {
                    title: "Side A".to_string(),
                    first_revolution_index: None,
                    revolution_count: None,
                },
                TrackInput {
                    title: "Side B".to_string(),
                    first_revolution_index: None,
                    revolution_count: None,
                },
            ],
            track_gaps: vec![],
        };
        let entries = vec![
            PayloadEntryInput {
                payload_descriptor_index: 0,
                bytes: payload_one.clone(),
            },
            PayloadEntryInput {
                payload_descriptor_index: 0,
                bytes: payload_two.clone(),
            },
        ];
        let stream = encode_record_stream(&input, &entries).unwrap();

        let output = render_payload_codes_to_png(&stream, "rgb", "single45", 208.5, None).unwrap();

        let decoded = record_decode::decode_record_png_to_chunk_stream_for_profile_with_length(
            &output.png_bytes,
            "single45",
            Some(stream.len()),
        )
        .unwrap();
        assert_eq!(
            decoded.bytes, stream,
            "multitrack groove did not round-trip"
        );

        let parsed = record_core::parse_chunk_stream(&decoded.bytes).unwrap();
        assert_eq!(parsed.metadata.tracks.len(), 2);
        assert_eq!(parsed.metadata.tracks[0].title, "Side A");
        assert_eq!(parsed.metadata.tracks[1].title, "Side B");

        let resolved =
            record_core::resolve_payload_entries(&parsed.metadata.payload_entries, 1).unwrap();
        assert_eq!(resolved.len(), 2);

        let payload_bytes = record_core::record_stream_payload_bytes(&parsed);
        assert_eq!(
            &payload_bytes
                [resolved[0].byte_offset..resolved[0].byte_offset + resolved[0].byte_length],
            payload_one.as_slice()
        );
        assert_eq!(
            &payload_bytes
                [resolved[1].byte_offset..resolved[1].byte_offset + resolved[1].byte_length],
            payload_two.as_slice()
        );
    }

    #[test]
    fn vari_pitch_groove_renders_and_decodes_byte_exact() {
        let payload = vec![0xC3u8; 6_000];
        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput::from_container("TEST")],
            tracks: vec![TrackInput {
                title: "Vari".to_string(),
                first_revolution_index: None,
                revolution_count: None,
            }],
            track_gaps: vec![],
        };
        let entries = vec![PayloadEntryInput {
            payload_descriptor_index: 0,
            bytes: payload,
        }];
        let stream = encode_record_stream(&input, &entries).unwrap();

        let options = r#"{
            "spiralFamily": "variPitch",
            "grooveCharacter": 0.3,
            "grooveDefinition": 0.7,
            "grooveSheen": 0.55,
            "firePlacement": "inner",
            "spiralSeed": 81985529216486895
        }"#;
        // render_payload_codes_to_png runs the mandatory render-time
        // self-check (decode back to exact BRS1 bytes), so a successful
        // render is already a round-trip proof; the assertions below
        // re-prove it from the outside and pin the v3 descriptor.
        let output =
            render_payload_codes_to_png(&stream, "rgb", "single45", 208.5, Some(options)).unwrap();

        let decoded = record_decode::decode_record_png_to_chunk_stream_for_profile_with_length(
            &output.png_bytes,
            "single45",
            Some(stream.len()),
        )
        .unwrap();
        assert_eq!(
            decoded.bytes, stream,
            "vari-pitch groove did not round-trip"
        );

        assert_eq!(output.descriptor.version, 3);
        assert_eq!(
            output.descriptor.spiral_family,
            SpiralFamily::VariPitch {
                depth: 0.3,
                seed: 81985529216486895,
                definition: 0.7,
                sheen: 0.55,
                placement: record_core::VariPitchPlacement::Inner,
                fire: 0.0,
                tuning: record_core::VariPitchTuning::default(),
            }
        );
    }

    #[test]
    fn vari_pitch_without_seed_is_refused() {
        let error = resolve_spiral_family(&RenderOptions {
            spiral_family: Some("variPitch".to_string()),
            ..RenderOptions::default()
        })
        .unwrap_err();
        assert!(
            error.to_string().contains("spiralSeed"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn archimedean_render_ignores_family_plumbing() {
        let payload = vec![0x5Au8; 3_000];
        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput::from_container("TEST")],
            tracks: vec![TrackInput {
                title: "Straight".to_string(),
                first_revolution_index: None,
                revolution_count: None,
            }],
            track_gaps: vec![],
        };
        let entries = vec![PayloadEntryInput {
            payload_descriptor_index: 0,
            bytes: payload,
        }];
        let stream = encode_record_stream(&input, &entries).unwrap();

        let implicit =
            render_payload_codes_to_png(&stream, "rgb", "single45", 208.5, None).unwrap();
        let explicit = render_payload_codes_to_png(
            &stream,
            "rgb",
            "single45",
            208.5,
            Some(r#"{"spiralFamily": "archimedean"}"#),
        )
        .unwrap();

        assert_eq!(
            implicit.png_bytes, explicit.png_bytes,
            "an explicit archimedean family must not change the render"
        );
        assert_eq!(implicit.descriptor.version, 2);
        assert_eq!(implicit.descriptor.spiral_family, SpiralFamily::Archimedean);
    }

    #[test]
    fn two_track_compact_groove_with_two_descriptors_renders_and_decodes_byte_exact() {
        let payload_one = vec![0xAAu8; 4_000];
        let payload_two = vec![0xCCu8; 3_200];

        let input = RecordStreamInput {
            payload_descriptors: vec![
                PayloadDescriptorInput::from_container("TEST"),
                PayloadDescriptorInput::from_container("TEST"),
            ],
            tracks: vec![
                TrackInput {
                    title: "Side A".to_string(),
                    first_revolution_index: None,
                    revolution_count: None,
                },
                TrackInput {
                    title: "Side B".to_string(),
                    first_revolution_index: None,
                    revolution_count: None,
                },
            ],
            track_gaps: vec![],
        };
        let entries = vec![
            PayloadEntryInput {
                payload_descriptor_index: 0,
                bytes: payload_one.clone(),
            },
            PayloadEntryInput {
                payload_descriptor_index: 1,
                bytes: payload_two.clone(),
            },
        ];
        let stream = encode_record_stream(&input, &entries).unwrap();

        let output = render_payload_codes_to_png(&stream, "rgb", "single45", 208.5, None).unwrap();

        let decoded = record_decode::decode_record_png_to_chunk_stream_for_profile_with_length(
            &output.png_bytes,
            "single45",
            Some(stream.len()),
        )
        .unwrap();
        assert_eq!(
            decoded.bytes, stream,
            "two-descriptor groove did not round-trip"
        );

        let parsed = record_core::parse_chunk_stream(&decoded.bytes).unwrap();
        assert_eq!(parsed.metadata.payload_descriptors.len(), 2);
        assert_eq!(
            parsed.metadata.payload_entries[0].payload_descriptor_index,
            0
        );
        assert_eq!(
            parsed.metadata.payload_entries[1].payload_descriptor_index,
            1
        );

        let resolved =
            record_core::resolve_payload_entries(&parsed.metadata.payload_entries, 2).unwrap();
        let payload_bytes = record_core::record_stream_payload_bytes(&parsed);
        assert_eq!(
            &payload_bytes
                [resolved[0].byte_offset..resolved[0].byte_offset + resolved[0].byte_length],
            payload_one.as_slice()
        );
        assert_eq!(
            &payload_bytes
                [resolved[1].byte_offset..resolved[1].byte_offset + resolved[1].byte_length],
            payload_two.as_slice()
        );
    }

    // A track-gap entry is rendered with the same toned base as the rest of
    // the stream — it is an ordinary payload entry (no GAP container, no
    // GAP1 payload). Its identity as a gap comes entirely from the explicit
    // track_gaps list, never from a special container or codec.
    #[test]
    fn track_gap_entry_uses_lighter_groove_tone_and_round_trips() {
        let music_one = vec![0xAAu8; 4_000];
        let gap = vec![0xCCu8; 1_500];
        let music_two = vec![0xBBu8; 5_500];

        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput::from_container("TEST")],
            tracks: vec![
                TrackInput {
                    title: "Side A".to_string(),
                    first_revolution_index: Some(0),
                    revolution_count: Some(1),
                },
                TrackInput {
                    title: "Side B".to_string(),
                    first_revolution_index: Some(2),
                    revolution_count: Some(1),
                },
            ],
            track_gaps: vec![TrackGapInput {
                first_revolution_index: 1,
                revolution_count: 1,
                after_track_index: 0,
            }],
        };
        let entries = vec![
            PayloadEntryInput {
                payload_descriptor_index: 0,
                bytes: music_one,
            },
            PayloadEntryInput {
                payload_descriptor_index: 0,
                bytes: gap,
            },
            PayloadEntryInput {
                payload_descriptor_index: 0,
                bytes: music_two,
            },
        ];
        let stream = encode_record_stream(&input, &entries).unwrap();

        let options = serde_json::json!({ "grooveToneColor": "#FFC0CB" }).to_string();
        let output =
            render_payload_codes_to_png(&stream, "rgb", "single45", 208.5, Some(&options)).unwrap();

        let decoded = record_decode::decode_record_png_to_chunk_stream_for_profile_with_length(
            &output.png_bytes,
            "single45",
            Some(stream.len()),
        )
        .unwrap();
        assert_eq!(
            decoded.bytes, stream,
            "normally toned track-gap groove did not round-trip"
        );

        let base = TonedConfig::from_hex("#FFC0CB", 0, 1).base;
        let base_lightness = oklch_lightness(base);
        let effective_amount =
            adaptive_gap_tone_lightness(base_lightness, DEFAULT_GAP_TONE_LIGHTNESS).unwrap();
        let expected_gap_base = lighten_base_oklch(base, effective_amount).unwrap();
        assert_eq!(
            output
                .descriptor
                .tone_spans
                .iter()
                .map(|span| span.byte_length)
                .sum::<usize>(),
            stream.len(),
        );
        // Three spans: normal (header + track A), lighter (the gap), normal
        // (track B) — the default gapToneLightness, adaptively boosted since
        // this light pink base is above the adaptive reference lightness.
        assert_eq!(output.descriptor.tone_spans.len(), 3);
        assert_eq!(output.descriptor.tone_spans[0].base, base);
        assert_eq!(output.descriptor.tone_spans[1].base, expected_gap_base);
        assert_ne!(
            expected_gap_base, base,
            "gap tone must differ from the track tone"
        );
        assert_eq!(output.descriptor.tone_spans[2].base, base);
        // bits_per_pixel, luma_tolerance, and ordering are shared, not re-tuned.
        for span in &output.descriptor.tone_spans {
            assert_eq!(
                span.bits_per_pixel,
                output.descriptor.tone_spans[0].bits_per_pixel
            );
            assert_eq!(
                span.luma_tolerance,
                output.descriptor.tone_spans[0].luma_tolerance
            );
        }

        let native_stream = record_core::parse_chunk_stream(&decoded.bytes).unwrap();
        record_core::validate_track_listing_metadata(&native_stream.metadata).unwrap();
    }

    #[test]
    fn gap_tone_lightness_zero_matches_track_tone() {
        let music_one = vec![0xAAu8; 4_000];
        let gap = vec![0xCCu8; 1_500];
        let music_two = vec![0xBBu8; 5_500];

        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput::from_container("TEST")],
            tracks: vec![
                TrackInput {
                    title: "Side A".to_string(),
                    first_revolution_index: Some(0),
                    revolution_count: Some(1),
                },
                TrackInput {
                    title: "Side B".to_string(),
                    first_revolution_index: Some(2),
                    revolution_count: Some(1),
                },
            ],
            track_gaps: vec![TrackGapInput {
                first_revolution_index: 1,
                revolution_count: 1,
                after_track_index: 0,
            }],
        };
        let entries = vec![
            PayloadEntryInput {
                payload_descriptor_index: 0,
                bytes: music_one,
            },
            PayloadEntryInput {
                payload_descriptor_index: 0,
                bytes: gap,
            },
            PayloadEntryInput {
                payload_descriptor_index: 0,
                bytes: music_two,
            },
        ];
        let stream = encode_record_stream(&input, &entries).unwrap();

        let options = serde_json::json!({
            "grooveToneColor": "#FFC0CB",
            "gapToneLightness": 0.0,
        })
        .to_string();
        let output =
            render_payload_codes_to_png(&stream, "rgb", "single45", 208.5, Some(&options)).unwrap();

        let base = TonedConfig::from_hex("#FFC0CB", 0, 1).base;
        assert!(
            output
                .descriptor
                .tone_spans
                .iter()
                .all(|span| span.base == base),
            "gapToneLightness 0.0 must match the track tone exactly",
        );
    }

    #[test]
    fn invalid_gap_tone_lightness_is_rejected() {
        let stream = encode_test_chunk_stream(
            serde_json::json!({
                "payloadDescriptors": [{ "container": "TEST" }],
                "trackListing": [{ "number": 1, "title": "A", "payloadEntryIndex": 0 }],
            }),
            &[vec![0xAAu8; 4_000]],
        );
        let options = serde_json::json!({
            "grooveToneColor": "#FFC0CB",
            "gapToneLightness": 1.5,
        })
        .to_string();
        let err = render_payload_codes_to_png(&stream, "rgb", "single45", 208.5, Some(&options))
            .unwrap_err();
        assert!(err.to_string().contains("gapToneLightness"), "{err}");
    }

    #[test]
    fn no_track_gaps_produces_no_lighter_spans() {
        let payload_one = vec![0xAAu8; 4_000];
        let payload_two = vec![0xBBu8; 5_500];
        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput::from_container("TEST")],
            tracks: vec![
                TrackInput {
                    title: "Side A".to_string(),
                    first_revolution_index: None,
                    revolution_count: None,
                },
                TrackInput {
                    title: "Side B".to_string(),
                    first_revolution_index: None,
                    revolution_count: None,
                },
            ],
            track_gaps: vec![],
        };
        let entries = vec![
            PayloadEntryInput {
                payload_descriptor_index: 0,
                bytes: payload_one,
            },
            PayloadEntryInput {
                payload_descriptor_index: 0,
                bytes: payload_two,
            },
        ];
        let stream = encode_record_stream(&input, &entries).unwrap();

        let options = serde_json::json!({ "grooveToneColor": "#FFC0CB" }).to_string();
        let output =
            render_payload_codes_to_png(&stream, "rgb", "single45", 208.5, Some(&options)).unwrap();

        let base = TonedConfig::from_hex("#FFC0CB", 0, 1).base;
        assert!(output
            .descriptor
            .tone_spans
            .iter()
            .all(|span| span.base == base));
    }

    #[test]
    fn gap_tone_lightness_is_boosted_for_lighter_records() {
        fn rendered_gap_base(hex: &str) -> [u8; 3] {
            let music_one = vec![0xAAu8; 4_000];
            let gap = vec![0xCCu8; 1_500];
            let music_two = vec![0xBBu8; 5_500];
            let input = RecordStreamInput {
                payload_descriptors: vec![PayloadDescriptorInput::from_container("TEST")],
                tracks: vec![
                    TrackInput {
                        title: "Side A".to_string(),
                        first_revolution_index: Some(0),
                        revolution_count: Some(1),
                    },
                    TrackInput {
                        title: "Side B".to_string(),
                        first_revolution_index: Some(2),
                        revolution_count: Some(1),
                    },
                ],
                track_gaps: vec![TrackGapInput {
                    first_revolution_index: 1,
                    revolution_count: 1,
                    after_track_index: 0,
                }],
            };
            let entries = vec![
                PayloadEntryInput {
                    payload_descriptor_index: 0,
                    bytes: music_one,
                },
                PayloadEntryInput {
                    payload_descriptor_index: 0,
                    bytes: gap,
                },
                PayloadEntryInput {
                    payload_descriptor_index: 0,
                    bytes: music_two,
                },
            ];
            let stream = encode_record_stream(&input, &entries).unwrap();
            let options = serde_json::json!({ "grooveToneColor": hex }).to_string();
            let output =
                render_payload_codes_to_png(&stream, "rgb", "single45", 208.5, Some(&options))
                    .unwrap();
            output.descriptor.tone_spans[1].base
        }

        // A dark base (well below the adaptive reference) keeps the
        // predominant amount unchanged.
        let dark_base = TonedConfig::from_hex("#202020", 0, 1).base;
        let dark_gap = rendered_gap_base("#202020");
        let dark_delta = oklch_lightness(dark_gap) - oklch_lightness(dark_base);
        assert!(dark_delta > 0.0);

        // A light base (well above the reference) has very little room left
        // to white. Adaptive scaling can't conjure room that doesn't exist,
        // but it must use *all* of what's left (amount saturates at the
        // white limit, 1.0) rather than applying the flat predominant
        // fraction, which would waste most of that remaining room.
        let light_base = TonedConfig::from_hex("#E8E4DC", 0, 1).base;
        let light_base_lightness = oklch_lightness(light_base);
        let light_gap = rendered_gap_base("#E8E4DC");
        let light_delta = oklch_lightness(light_gap) - light_base_lightness;
        assert!(light_delta > 0.0);

        let flat_amount_delta = (1.0 - light_base_lightness) * DEFAULT_GAP_TONE_LIGHTNESS;
        assert!(
            light_delta > flat_amount_delta * 1.5,
            "adaptive light_delta={light_delta} should clearly exceed the flat-amount \
             delta={flat_amount_delta} that a non-adaptive 0.35 would have produced",
        );
    }

    #[test]
    fn non_positional_track_mapping_round_trips_revolution_ranges() {
        let payload_one = vec![0x11u8; 2_000];
        let payload_two = vec![0x22u8; 2_500];
        let payload_three = vec![0x33u8; 1_800];

        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput::from_container("TEST")],
            tracks: vec![
                // Track 0 spans two revolutions (entries 0 and 1) instead of the
                // default one-revolution-per-array-position mapping.
                TrackInput {
                    title: "Side A".to_string(),
                    first_revolution_index: Some(0),
                    revolution_count: Some(2),
                },
                TrackInput {
                    title: "Side B".to_string(),
                    first_revolution_index: Some(2),
                    revolution_count: Some(1),
                },
            ],
            track_gaps: vec![],
        };
        let entries = vec![
            PayloadEntryInput {
                payload_descriptor_index: 0,
                bytes: payload_one.clone(),
            },
            PayloadEntryInput {
                payload_descriptor_index: 0,
                bytes: payload_two.clone(),
            },
            PayloadEntryInput {
                payload_descriptor_index: 0,
                bytes: payload_three.clone(),
            },
        ];
        let stream = encode_record_stream(&input, &entries).unwrap();

        let output = render_payload_codes_to_png(&stream, "rgb", "single45", 208.5, None).unwrap();

        let decoded = record_decode::decode_record_png_to_chunk_stream_for_profile_with_length(
            &output.png_bytes,
            "single45",
            Some(stream.len()),
        )
        .unwrap();
        assert_eq!(
            decoded.bytes, stream,
            "non-positional track mapping did not round-trip"
        );

        let parsed = record_core::parse_chunk_stream(&decoded.bytes).unwrap();
        assert_eq!(parsed.metadata.tracks.len(), 2);
        assert_eq!(parsed.metadata.tracks[0].first_revolution_index, 0);
        assert_eq!(parsed.metadata.tracks[0].revolution_count, 2);
        assert_eq!(parsed.metadata.tracks[1].first_revolution_index, 2);
        assert_eq!(parsed.metadata.tracks[1].revolution_count, 1);

        let resolved =
            record_core::resolve_payload_entries(&parsed.metadata.payload_entries, 1).unwrap();
        assert_eq!(resolved.len(), 3);

        let payload_bytes = record_core::record_stream_payload_bytes(&parsed);
        assert_eq!(
            &payload_bytes
                [resolved[0].byte_offset..resolved[0].byte_offset + resolved[0].byte_length],
            payload_one.as_slice()
        );
        assert_eq!(
            &payload_bytes
                [resolved[1].byte_offset..resolved[1].byte_offset + resolved[1].byte_length],
            payload_two.as_slice()
        );
        assert_eq!(
            &payload_bytes
                [resolved[2].byte_offset..resolved[2].byte_offset + resolved[2].byte_length],
            payload_three.as_slice()
        );
    }

    fn chunk_stream_for_payload(id: &str, payload_name: &str) -> Vec<u8> {
        let payload = fixture_bytes(id, payload_name);
        let metadata = serde_json::json!({
            "payloadDescriptors": [{ "container": "ECDC" }],
            "payloadContainer": "ECDC",
            "trackListing": [{ "number": 1, "title": "Test Track", "payloadEntryIndex": 0 }]
        });

        encode_test_chunk_stream(metadata, &[payload])
    }

    fn chunk_stream_for_browser_facade_payload(payload: &[u8]) -> Vec<u8> {
        let metadata = serde_json::json!({
            "payloadDescriptors": [{ "container": "ECDC" }],
            "payloadContainer": "ECDC",
            "signing": "facade-placeholder",
            "trackListing": [{ "number": 1, "title": "Test Track", "payloadEntryIndex": 0 }]
        });

        encode_test_chunk_stream(metadata, &[payload.to_vec()])
    }

    fn update_manifest_payload_bytes(path: PathBuf, byte_len: usize) {
        let raw = fs::read_to_string(&path).unwrap();
        let mut manifest: Value = serde_json::from_str(&raw).unwrap();

        manifest["payload"]["bytes"] = Value::from(byte_len as u64);

        fs::write(&path, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();
    }

    fn deterministic_noise_byte(pixel_index: usize, channel: u32) -> u8 {
        let mut value = (pixel_index as u32)
            .wrapping_mul(747_796_405)
            .wrapping_add(2_891_336_453)
            .wrapping_add(channel.wrapping_mul(277_803_737));
        value ^= value >> 16;
        value = value.wrapping_mul(2_246_822_519);
        value ^= value >> 13;
        (value >> 24) as u8
    }

    fn composite_deterministic_noise_under_transparent_pixels(rgba: &mut [u8]) {
        for pixel_index in 0..rgba.len() / 4 {
            let offset = pixel_index * 4;

            if rgba[offset + 3] != 0 {
                continue;
            }

            rgba[offset] = deterministic_noise_byte(pixel_index, 0);
            rgba[offset + 1] = deterministic_noise_byte(pixel_index, 1);
            rgba[offset + 2] = deterministic_noise_byte(pixel_index, 2);
            rgba[offset + 3] = 255;
        }
    }

    fn make_picture_noise_png(record_png: &[u8]) -> Vec<u8> {
        let image = image::load_from_memory(record_png).unwrap().to_rgba8();
        let (width, height) = image.dimensions();
        let mut rgba = image.into_raw();

        composite_deterministic_noise_under_transparent_pixels(&mut rgba);

        write_rgba_png(width as usize, height as usize, &rgba).unwrap()
    }

    fn assert_render_decodes_to_payload(id: &str, profile: &str, payload_name: &str) {
        let expected_payload = fixture_bytes(id, payload_name);
        let stream = chunk_stream_for_payload(id, payload_name);

        assert_eq!(&stream[..4], record_core::RECORD_STREAM_MAGIC);

        let rendered =
            render_chunk_stream_to_png(&stream, profile, WESTSIDE_DURATION_SECONDS, None).unwrap();

        let (decoded_profile, decoded) =
            record_decode::decode_record_png_to_chunk_stream(&rendered.png_bytes).unwrap();

        assert_eq!(decoded_profile, profile);
        assert_eq!(decoded.bytes, stream);
        assert_eq!(rendered.payload.record_profile, profile);
        assert_eq!(rendered.descriptor.record_profile, profile);
        assert_eq!(rendered.descriptor.stream_byte_length, stream.len());

        let decoded_stream = parse_chunk_stream(&decoded.bytes).unwrap();
        let decoded_payload = record_core::chunk_stream_payload_bytes(&decoded_stream);

        assert_eq!(
            decoded_payload, expected_payload,
            "{id} rendered PNG should decode to a chunk stream whose payload reconstructs {payload_name}"
        );
    }

    #[test]
    #[ignore]
    fn scan_mobygratis_corpus_for_single45_overflow() {
        let corpus_root = std::env::var("BITNEEDLE_MOBYGRATIS_ECDC_ROOT")
            .or_else(|_| std::env::var("MOBYGRATIS_ECDC_ROOT"))
            .map(PathBuf::from)
            .unwrap_or_else(|_| repo_root().join("mobygratis/ecdc-300lm-1333ms/ecdc"));

        assert!(
            corpus_root.is_dir(),
            "Mobygratis ECDC corpus root does not exist: {}",
            corpus_root.display()
        );

        let mut entries = fs::read_dir(&corpus_root)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", corpus_root.display()))
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
            .collect::<Vec<_>>();
        entries.sort();

        let single45_capacity =
            estimate_spiral_track_capacity(RECORD_WIDTH, RECORD_HEIGHT, "single45")
                .expect("single45 capacity should be measurable")
                .max_track_pixel_count_absolute;
        let exact_candidate_threshold = ((single45_capacity as f64) * 0.94).floor() as usize;
        let mut preflight_candidates = 0usize;
        let mut single45_failures = Vec::new();
        let mut lp_failures = Vec::new();

        for json_path in entries {
            let metadata: Value =
                serde_json::from_str(&fs::read_to_string(&json_path).unwrap_or_else(|error| {
                    panic!("failed to read {}: {error}", json_path.display())
                }))
                .unwrap_or_else(|error| panic!("failed to parse {}: {error}", json_path.display()));
            let ecdc_path = json_path.with_extension("ecdc");
            let ecdc = fs::read(&ecdc_path)
                .unwrap_or_else(|error| panic!("failed to read {}: {error}", ecdc_path.display()));
            let stream = chunk_stream_for_browser_facade_payload(&ecdc);
            let duration_seconds = metadata
                .get("duration_s")
                .and_then(Value::as_f64)
                .unwrap_or_default();
            let chunk_ms = metadata
                .get("chunk_ms")
                .and_then(Value::as_f64)
                .unwrap_or_default();
            let bundle = metadata
                .get("bundle")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let id = json_path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("record")
                .to_string();

            if (chunk_ms - 1333.333333).abs() > 1.0 {
                continue;
            }
            let track_pixel_count = payload_pixel_count_for_byte_length(stream.len());
            if track_pixel_count < exact_candidate_threshold {
                continue;
            }
            preflight_candidates += 1;

            let single45_error =
                match render_chunk_stream_to_png(&stream, "single45", duration_seconds, None) {
                    Ok(_) => continue,
                    Err(error) => format!("{error:#}"),
                };

            if let Err(error) = render_chunk_stream_to_png(&stream, "lp", duration_seconds, None) {
                lp_failures.push(format!("{id}: {error:#}"));
            }

            let line = format!(
                "{id}: ecdc={} stream={} duration={:.3}s chunk_ms={:.3} bundle={} single45_error={}",
                ecdc.len(),
                stream.len(),
                duration_seconds,
                chunk_ms,
                bundle,
                single45_error.replace('\n', " | "),
            );
            println!("45_FAIL {line}");
            single45_failures.push(line);
        }

        println!(
            "Mobygratis corpus exact render scan: {} near-capacity candidate(s), {} single45 failure(s), {} LP failure(s)",
            preflight_candidates,
            single45_failures.len(),
            lp_failures.len()
        );
        for line in &single45_failures {
            println!("45->LP {line}");
        }
        for line in &lp_failures {
            println!("LP_FAIL {line}");
        }

        assert!(
            !lp_failures.is_empty() || !single45_failures.is_empty(),
            "expected at least one single45 render failure in the current Mobygratis corpus"
        );
        assert!(
            lp_failures.is_empty(),
            "some single45-overflowing Mobygratis records do not fit LP:\n{}",
            lp_failures.join("\n")
        );
    }

    #[test]
    #[ignore]
    fn regenerate_golden_records() {
        for golden in GOLDENS {
            let dir = fixture_dir(golden.id);
            let png_path = dir.join(golden.record_png_name);
            let manifest_path = dir.join("manifest.json");
            let stream = chunk_stream_for_payload(golden.id, golden.payload_name);

            let rendered = render_chunk_stream_to_png(
                &stream,
                golden.profile,
                WESTSIDE_DURATION_SECONDS,
                None,
            )
            .unwrap();

            let (decoded_profile, decoded) =
                record_decode::decode_record_png_to_chunk_stream(&rendered.png_bytes).unwrap();

            assert_eq!(decoded_profile, golden.profile);
            assert_eq!(decoded.bytes, stream);

            fs::write(&png_path, &rendered.png_bytes).unwrap();

            if golden.id == "lori-asha-westside-single45-hq" {
                let noisy_png = make_picture_noise_png(&rendered.png_bytes);
                fs::write(
                    dir.join("lori-asha-westside-single45-hq.picture-noise.record.png"),
                    noisy_png,
                )
                .unwrap();
            }

            update_manifest_payload_bytes(manifest_path, stream.len());

            println!("regenerated {} {} stream bytes", golden.id, stream.len());
        }
    }

    #[test]
    fn renders_single45_golden_and_decodes_back_to_payload() {
        assert_render_decodes_to_payload(
            "lori-asha-westside-single45-hq",
            "single45",
            "lori-asha-westside-single45-hq.ecdc",
        );
    }

    #[test]
    fn renders_lp_golden_and_decodes_back_to_payload() {
        assert_render_decodes_to_payload(
            "lori-asha-westside-lp-hq",
            "lp",
            "lori-asha-westside-lp-hq.ecdc",
        );
    }
}
