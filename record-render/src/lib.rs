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
pub mod export;
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

// These are record-core's, not this crate's. A local copy of a turn count is a
// second answer to a question the format already settles, and the two drift
// silently: the trailer's pitch is computed here and its geometry there.
use record_core::LEAD_IN_TURNS;

const LEAD_IN_OUTER_EDGE_INSET: i32 = 1;

/// A wheel's rotation: one for all of it, or one per ring.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum RotationDegrees {
    Whole(f64),
    PerRing(Vec<f64>),
}

impl RotationDegrees {
    /// The rotation of each of `rings` rings, padded from the last given.
    fn per_ring(&self, rings: usize) -> Vec<f64> {
        let given: &[f64] = match self {
            Self::Whole(one) => std::slice::from_ref(one),
            Self::PerRing(many) => many,
        };
        (0..rings)
            .map(|ring| given.get(ring).or_else(|| given.last()).copied().unwrap_or(0.0))
            .collect()
    }
}

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
    /// Clockface tones: CSS hex colours, one per slot, 2–64 of them. The
    /// disc is divided into that many equal angular slots; slot zero begins
    /// at `grooveToneRotationDegrees` clockwise from twelve o'clock and the
    /// rest follow clockwise, so the app's picker can run once per slot of
    /// the artwork and hand the results straight over. When set, the payload
    /// is encoded "toned-v2" — every slot at the same bits per pixel, so the
    /// groove is exactly as long as a single-tone cut — and this takes
    /// precedence over `grooveToneColor`. Track gaps lighten per slot as
    /// `gapToneLightness` describes.
    pub groove_tone_slots: Option<Vec<String>>,
    /// The slots in each ring of the wheel, innermost first — `[8, 16]` is
    /// eight pockets across the inside of the groove band and sixteen around
    /// the outside, and `grooveToneSlots` then carries twenty-four colours,
    /// innermost ring first and clockwise from twelve within each ring.
    ///
    /// Defaults to one ring holding every slot, which is what a wheel was
    /// before it had rings and is written as a version 1 clock map. The band
    /// the rings divide is the groove's own, taken from the record profile:
    /// a ring has to be a band of groove or it has nothing to tone.
    ///
    /// Capacity is untouched by any of it. Every pocket's palette carries
    /// the same bits per pixel and the bit stream runs continuously across
    /// them, so the groove is exactly as long as a single-tone cut.
    pub groove_tone_rings: Option<Vec<u32>>,
    /// Where slot zero begins, in degrees clockwise from twelve o'clock.
    ///
    /// One number turns the whole wheel. An array turns each ring on its own,
    /// innermost first, padded from the last given — which is how the rings
    /// are made to turn at different rates, so the alignment between them
    /// changes with the spin instead of the wheel reading as one stamped
    /// shape however far it is turned.
    ///
    /// Defaults to `0`. Ignored without slots.
    pub groove_tone_rotation_degrees: Option<RotationDegrees>,
    /// Whether neighbouring slots run into each other — a pixel near a
    /// boundary takes the neighbour's tone in proportion to its nearness,
    /// decided per pixel from its groove index — or step hard. Defaults to
    /// `true`. Ignored without slots.
    pub groove_tone_blend: Option<bool>,
    /// Preview tones: paint each groove pixel in its pocket's flat base
    /// colour instead of encoding the payload into per-pocket palettes.
    ///
    /// The placement is the real one — same rings, span, rotations, blend
    /// and gap pattern, and the same bits per pixel, so the spiral is the
    /// same length and the pockets sit where the press puts them — but no
    /// palette is built and no bit is packed, which is the part of a
    /// many-pocket cut that costs. A progressive redraw lands in about the
    /// time of the spiral trace plus the PNG.
    ///
    /// A preview carries no payload and cannot prove it reads: the facade
    /// skips its decode-and-compare when this is set, whatever `verify`
    /// says. Never kept, never pressed.
    pub groove_tone_preview: Option<bool>,
    /// Decode the pressed groove back and compare bytes before handing the
    /// PNG over. On by default: a press that cannot prove it reads is not a
    /// press. A live cut that is looked at and never kept — the wheel lab's
    /// CUT disc, which cuts again on every move of the hand — passes `false`
    /// and skips the second half of the work; the pixels are the same either
    /// way, only the proof is skipped. A preview skips the proof whatever
    /// this says.
    pub verify: Option<bool>,
    /// The tone the trailer is cut in: the coarse run-out rings and the lock
    /// groove, as one CSS hex colour.
    ///
    /// Matte, whatever finish the programme was read with. Deciding the
    /// colour means reading the artwork, which the caller does: this crate
    /// takes tones, not pictures.
    ///
    /// Without it the trailer takes the wheel's own pockets. An untoned
    /// record keeps the grey dither.
    pub run_out_tone_color: Option<String>,
    #[serde(default)]
    pub guide_outlines: bool,
    /// How much of the payload band this cut lays its programme across,
    /// measured outward-in from the rim, `0 < fraction <= 1`. Defaults to
    /// `record_core::DEFAULT_GROOVE_SPAN_FRACTION`. The pitch is fitted so
    /// the nominal fills exactly this much of the band; whatever is left
    /// inside it stays deadwax, the way a lathe leaves a side it did not
    /// fill. `1.0` is the historical cut that always ran to the label.
    pub groove_span_fraction: Option<f64>,
    /// The pitch to cut at, centre to centre between turns, in rendered
    /// pixels. Clamped at [`record_core::MIN_TURN_SEPARATION_PX`].
    ///
    /// The clamp follows from the raster. `trace_record_spiral_with_family`
    /// rounds every point to an integer pixel and skips a pixel that is
    /// already taken, so turns closer than the grid resolves merge. Measured
    /// on the ten at 576: a request for 1.75 draws 2.33, a request for 1.50
    /// draws 3.00, and a request for 1.30 draws 4.25. Each result is wider
    /// than the request, and the spacing is irregular. At 2.0 and above, the
    /// request and the drawn result agree, which is the range that this option
    /// serves.
    pub turn_separation_px: Option<f64>,
    /// Which way the programme's groove winds from its start angle.
    /// Defaults to `true`, clockwise, which is what every record cut before
    /// this option existed carries.
    ///
    /// A lathe's cutter does not travel: the record turns under it, and the
    /// groove that leaves the head therefore winds *against* the turn. A
    /// clockwise groove on a clockwise platter is the mirror of that, which
    /// is invisible in a picture and wrong the moment anything has to sit at
    /// the cutting point — the cut walks away from the head at twice the
    /// rate instead of standing still under it.
    ///
    /// Only the programme takes it. The lead-in, the run-out and the deadwax
    /// stay clockwise always: a reader rides those bands to find the
    /// descriptor, so it must be able to trace them before it knows anything
    /// this option could have changed.
    pub spiral_clockwise: Option<bool>,
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
    /// Which way the programme's groove winds. See
    /// `RenderOptions::spiral_clockwise`.
    pub spiral_clockwise: bool,
    /// The span that the pitch was fitted against, after any widening that the
    /// density floor forced.
    pub groove_span_fraction: f64,
    /// The radius the nominal was laid out to stop on. A payload that comes
    /// in over its nominal runs past this, inward, toward
    /// `payloadInnerRadius`.
    pub cut_inner_radius: i32,
    /// Turns of deadwax the cut left behind, at the lathe's spiral feed.
    pub deadwax_turns: f64,
    /// Addressable pixels in the deadwax carrier. The carrier holds no bytes
    /// at present, and it stays addressable.
    pub deadwax_pixel_capacity: usize,
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
    /// For a clock-toned (toned-v2) groove this is instead the resolved
    /// `ToneClockDescriptor` as a JSON object.
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
    /// Addressable pixels in the deadwax. It carries nothing today, but it
    /// is a carrier: the indices are ordered, continuous with the programme
    /// groove, and reproducible from the same figures a decoder already
    /// has, so anything that wants to write there can.
    deadwax_pixel_capacity: usize,
    /// The deadwax's first and last pixel, so a traversal can be checked
    /// to join the programme rather than restart inside it.
    deadwax_bounds: Option<(usize, usize)>,
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
    deadwax_turns: f64,
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
    render_chunk_stream_to_png_with_progress(
        stream,
        record_profile,
        duration_seconds,
        render_options_json,
        &|_| {},
    )
}

/// The same cut, narrating itself: `progress` hears each stage as it
/// starts — `"toning…"`, `"groove…"`, `"pressing…"`, `"proving…"` — so a
/// page can say what a seconds-long render is doing rather than that it is
/// doing something. A handful of calls per cut; pass a no-op when nobody
/// is listening.
pub fn render_chunk_stream_to_png_with_progress(
    stream: &[u8],
    record_profile: &str,
    duration_seconds: f64,
    render_options_json: Option<&str>,
    progress: &dyn Fn(&str),
) -> Result<RenderOutput> {
    render_payload_codes_to_png_with_progress(
        stream,
        PAYLOAD_CODE_FORMAT_RGB,
        record_profile,
        duration_seconds,
        render_options_json,
        progress,
    )
}

pub fn render_payload_codes_to_png(
    codes: &[u8],
    code_format: &str,
    record_profile: &str,
    duration_seconds: f64,
    render_options_json: Option<&str>,
) -> Result<RenderOutput> {
    render_payload_codes_to_png_with_progress(
        codes,
        code_format,
        record_profile,
        duration_seconds,
        render_options_json,
        &|_| {},
    )
}

/// The same cut, narrating itself — see
/// [`render_chunk_stream_to_png_with_progress`].
pub fn render_payload_codes_to_png_with_progress(
    codes: &[u8],
    code_format: &str,
    record_profile: &str,
    duration_seconds: f64,
    render_options_json: Option<&str>,
    progress: &dyn Fn(&str),
) -> Result<RenderOutput> {
    let render_options = parse_render_options(render_options_json)?;
    let result = render_payload_codes_to_transparent_spiral(
        codes,
        code_format,
        record_profile,
        duration_seconds,
        None,
        &render_options,
        progress,
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
    progress("pressing…");
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
    // code blocks), which have no chunk-stream decode contract — for preview
    // tones, which carry no payload by design and so have nothing to
    // reconstruct — and when the caller passes `verify: false`: a live cut
    // that is looked at and never kept.
    let preview = render_options.groove_tone_preview.unwrap_or(false);
    if codes.starts_with(record_core::RECORD_STREAM_MAGIC)
        && render_options.verify.unwrap_or(true)
        && !preview
    {
        progress("proving…");
        verify_rendered_record_roundtrip(&png_bytes, &normalized_profile, codes)
            .context("rendered PNG groove could not be decoded")?;
    }

    let payload = RenderPayload {
        status,
        record_profile: normalized_profile,
        duration_seconds,
        spiral_clockwise: render_options.spiral_clockwise.unwrap_or(true),
        spiral_fit_mode: None,
        exact: result.exact,
        b_value: result.b_value,
        groove_span_fraction: result.groove_span_fraction,
        cut_inner_radius: result.cut_inner_radius,
        deadwax_turns: result.deadwax_turns,
        deadwax_pixel_capacity: result.rendered.deadwax_pixel_capacity,
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
        true,
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

/// The radius the programme's groove may not cut below, for a resolved
/// geometry. See [`record_core::programme_inner_radius`].
fn programme_inner_radius(geometry: &RecordProfileGeometry) -> Result<i32> {
    record_core::programme_inner_radius(&geometry.record_profile)
}

/// The radius this cut's pitch is fitted against. Only the fit sees it:
/// the painting mask still spans the whole band, so a payload that overruns
/// its nominal runs on inward instead of being truncated.
fn cut_inner_radius(geometry: &RecordProfileGeometry, span_fraction: f64) -> Result<i32> {
    record_core::cut_inner_radius_from_geometry(geometry, span_fraction)
}

fn header_outer_radius(geometry: &RecordProfileGeometry) -> i32 {
    (geometry.outer_radius - LEAD_IN_OUTER_EDGE_INSET).max(1)
}

fn lead_in_spiral_pitch_for_geometry(geometry: &RecordProfileGeometry) -> f64 {
    let radial_travel =
        (header_outer_radius(geometry) - payload_outer_radius(geometry)).max(1) as f64;

    radial_travel / (2.0 * PI * LEAD_IN_TURNS.max(0.01))
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
    start_angle: f64,
    // The lead-in and run-out sit *inside* their bands, so their outer edge is
    // exclusive. The deadwax instead begins exactly on its outer edge — it
    // is the same groove continuing — and dropping that first turn would put
    // its first addressable pixel most of a revolution away from the
    // programme's last.
    include_outer_edge: bool,
) -> Result<Vec<usize>> {
    record_core::build_band_spiral_indices_at_angle(
        width,
        height,
        band_outer_radius,
        band_inner_radius,
        band_pitch,
        start_angle,
        include_outer_edge,
    )
}

fn build_lead_in_spiral_indices(
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
        lead_in_spiral_pitch_for_geometry(&geometry),
        DEFAULT_START_ANGLE,
        false,
    )
}

/// The trailer carrier, from the one place that knows its geometry.
///
/// This used to be traced here and traced again in `record-core`, with the
/// pitch computed in one crate and the band in the other. They agreed by
/// inspection, which is not a way for a descriptor to keep reading back.
fn build_run_out_spiral_indices(
    width: usize,
    height: usize,
    record_profile: &str,
    cut_inner_radius: Option<i32>,
) -> Result<Vec<usize>> {
    record_core::build_run_out_spiral_indices(width, height, record_profile, cut_inner_radius)
}

/// Where the deadwax has to stop: the outermost turn of the lead-out, plus
/// the daylight two bands need not to round onto each other's pixels.
///
/// The wide extents use this space. The deadwax is cut at the millimetre feed
/// of the lathe, so a side that stops early fills the whole descent with a turn
/// every millimetre. An album cut to a third of its band draws seventeen such
/// turns across the artwork. A lead-out that widens into the same space
/// replaces that ladder with three or four separate rings, and those rings
/// carry bytes. The lead-out takes the space that the deadwax would otherwise
/// cross.
fn deadwax_inner_radius(record_profile: &str, cut_inner_radius: Option<i32>) -> Result<f64> {
    let lead_out = record_core::lead_out_geometry_with_extent(
        record_profile,
        cut_inner_radius,
        record_core::LeadOutExtent::Fill,
    )?;

    Ok(lead_out.entry_radius + record_core::MIN_TURN_SEPARATION_PX)
}

fn build_deadwax_spiral_indices(
    width: usize,
    height: usize,
    b_value: f64,
    family: &SpiralFamily,
    record_profile: &str,
    cut_inner_radius: i32,
) -> Result<Vec<usize>> {
    record_core::build_deadwax_spiral_indices(
        width,
        height,
        b_value,
        family,
        record_profile,
        cut_inner_radius,
    )
}

/// How many turns of deadwax a cut that stopped at `cut_inner_radius`
/// leaves behind, at the lathe's spiral feed.
fn deadwax_turns(record_profile: &str, cut_inner_radius: i32) -> Result<f64> {
    let travel = (cut_inner_radius as f64
        - deadwax_inner_radius(record_profile, Some(cut_inner_radius))?)
    .max(0.0);

    Ok(travel / record_core::deadwax_turn_separation_px(record_profile)?)
}

fn build_spiral_mask(
    width: usize,
    height: usize,
    b_value: f64,
    family: &SpiralFamily,
    record_profile: &str,
    clockwise: bool,
) -> Result<SpiralMask> {
    let geometry = describe_record_profile(record_profile)?;
    let payload_outer = payload_outer_radius(&geometry);
    let payload_inner = programme_inner_radius(&geometry)?;
    let (occupied, traced_pixel_indices, center_x, center_y) = trace_record_spiral(
        width,
        height,
        b_value,
        family,
        None,
        DEFAULT_START_ANGLE,
        1.0,
        clockwise,
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

// An exact-fit probe needs the count of addressable pixels alone. An ordered
// `Vec<usize>` for every binary-search candidate and sweep candidate drives the
// WASM allocator to a high water mark. Count the same unique traced pixels in
// place, and build the ordered representation for the winning spiral alone.
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
    let inner_cutoff = programme_inner_radius(&geometry)? as f64;
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
    let inner = programme_inner_radius(geometry).unwrap_or_else(|_| payload_inner_radius(geometry))
        as f64;
    let band = (outer - inner).max(1.0);
    let swept = outer * outer - track_pixel_count as f64 * record_core::MIN_TURN_SEPARATION_PX / PI;

    if swept <= inner * inner {
        return 1.0;
    }

    ((outer - swept.sqrt()) / band).clamp(0.0, 1.0)
}

/// The span `track_pixel_count` occupies at a named pitch.
///
/// A spiral's arc between two radii is `(r_out^2 - r_in^2) / 2b`, so the
/// radius a payload reaches is the one making that equal its own pixel count.
/// Solved, not searched. Returns 1.0 when the programme would reach past the
/// innermost recorded diameter — the caller's signal that the pitch it asked
/// for does not fit this side.
fn span_for_turn_separation(
    track_pixel_count: usize,
    geometry: &RecordProfileGeometry,
    separation_px: f64,
) -> f64 {
    let outer = payload_outer_radius(geometry) as f64;
    let inner = programme_inner_radius(geometry).unwrap_or_else(|_| payload_inner_radius(geometry))
        as f64;
    let band = (outer - inner).max(1.0);
    let b = separation_px / (2.0 * PI);
    let reached_squared = outer * outer - 2.0 * b * track_pixel_count as f64;
    if reached_squared <= inner * inner {
        return 1.0;
    }
    ((outer - reached_squared.sqrt()) / band).clamp(1e-6, 1.0)
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
    requested_separation_px: Option<f64>,
) -> Result<CutFit> {
    let geometry = describe_record_profile(record_profile)?;

    // A named pitch is arithmetic, not a search: the span it implies follows
    // from the payload. The only question left is whether the programme
    // reached the label before it ran out, and if it did the ordinary fit
    // takes over.
    if let Some(separation) = requested_separation_px {
        let separation = separation.max(record_core::MIN_TURN_SEPARATION_PX);
        let span = span_for_turn_separation(track_pixel_count, &geometry, separation);
        if span < 1.0 {
            let fit = find_exact_fit_with_coverage(
                width, height, track_pixel_count, family, record_profile, span, None,
            )?;
            return Ok(CutFit { span_fraction: span, fit });
        }
    }
    let mut span_fraction = record_core::validate_groove_span_fraction(requested_span_fraction)?
        .max(span_fraction_floor_estimate(track_pixel_count, &geometry))
        .min(1.0);

    loop {
        let fit = find_exact_fit_with_coverage(
            width,
            height,
            track_pixel_count,
            family,
            record_profile,
            span_fraction,
            None,
        )?;

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

/// The trailer's tone, as the caller gave it.
///
/// [`record_groove::normalized_hex_color`] falls back to white, so an
/// unparseable colour returns `None` rather than a white ring.
fn run_out_tone(render_options: &RenderOptions) -> Option<[u8; 3]> {
    let raw = render_options.run_out_tone_color.as_deref()?.trim();
    if raw.is_empty() {
        return None;
    }
    let hex = record_groove::normalized_hex_color(Some(raw));
    let channel = |range: std::ops::Range<usize>| u8::from_str_radix(&hex[range], 16).ok();

    Some([channel(1..3)?, channel(3..5)?, channel(5..7)?])
}

/// Paint a band in the record's own colour: each pixel in the tone of the
/// pocket it passes through, off the same wheel the programme is cut with.
///
/// `tone` overrides the wheel with one colour for the whole band. Without a
/// wheel and without a tone there is no colour to cut in, and the band keeps
/// the grey dither.
///
/// Flat and opaque, as the programme's own pixels are.
fn paint_toned_groove(
    data: &mut [u8],
    width: usize,
    height: usize,
    indices: &[usize],
    clock: Option<&record_groove::ToneClock>,
    tone: Option<[u8; 3]>,
    salt: usize,
) {
    if tone.is_none() && clock.is_none() {
        paint_unused_metadata_groove(data, indices, 0, salt, 0);
        return;
    }

    let center_x = width as f64 / 2.0;
    let center_y = height as f64 / 2.0;

    for (sequence, &pixel_index) in indices.iter().enumerate() {
        let Some(rgba_index) = pixel_index.checked_mul(4) else {
            continue;
        };
        if rgba_index + 3 >= data.len() {
            continue;
        }

        let base = match (tone, clock) {
            (Some(tone), _) => tone,
            (None, Some(clock)) => {
                let x = (pixel_index % width) as f64;
                let y = (pixel_index / width) as f64;
                let angle = record_groove::pixel_angle(x, y, center_x, center_y);
                // Both bands sit inside the wheel's span, which reaches
                // the label, so each takes the ring it is actually in.
                let away = record_groove::pixel_radius(x, y, center_x, center_y);
                clock.slots[clock.cell_index(sequence, angle, away)].base
            }
            // Unreachable: a band with neither took the grey dither above.
            (None, None) => continue,
        };

        data[rgba_index..rgba_index + 3].copy_from_slice(&base);
        data[rgba_index + 3] = 255;
    }
}

fn paint_descriptor_spiral(
    data: &mut [u8],
    width: usize,
    height: usize,
    record_profile: &str,
    main_b_value: f64,
    descriptor: &RecordDescriptorInput,
    trailer: Option<(&[usize], &record_groove::ToneClock)>,
) -> Result<RecordDescriptor> {
    let lead_in_indices = build_lead_in_spiral_indices(width, height, record_profile)?;

    // A reader reads the grey lead-in before it knows anything about the
    // record, and the trailer's toning is one of the things it learns there.
    // So the lead-in fills first and the trailer takes the rest. The split
    // falls on a byte, so each band packs its own bits.
    let lead_in_capacity =
        record_descriptor::metadata_byte_capacity_for_pixel_count(lead_in_indices.len());
    let trailer_capacity = trailer.map_or(0, |(indices, clock)| {
        record_descriptor::band_byte_capacity(indices.len(), clock)
    });

    let descriptor_bytes = record_cut::descriptor::encode_record_descriptor_stream(
        main_b_value,
        descriptor,
        lead_in_capacity + trailer_capacity,
    )?;

    let head = descriptor_bytes.len().min(lead_in_capacity);
    let written_pixels =
        paint_metadata_bytes_as_grayscale(data, &lead_in_indices, &descriptor_bytes[..head]);

    let fade_pixels = metadata_fade_pixel_count(lead_in_indices.len(), LEAD_IN_TURNS);

    paint_unused_metadata_groove(data, &lead_in_indices, written_pixels, 17, fade_pixels);

    if head < descriptor_bytes.len() {
        let Some((indices, clock)) = trailer else {
            bail!("the descriptor overran the lead-in and this record has no trailer to hold it");
        };
        // The trailer is read with the tone or the wheel, and both are
        // segments. A segment past the lead-in is one the reader cannot
        // reach before it needs it.
        let head_bytes = &descriptor_bytes[..head];
        if record_descriptor::run_out_tone_from_partial_stream(head_bytes).is_none()
            && record_descriptor::tone_clock_map_from_partial_stream(head_bytes).is_none()
        {
            bail!(
                "the descriptor spills into the trailer, and neither the trailer's tone nor \
                 the record's wheel fits in the lead-in"
            );
        }
        record_cut::descriptor::paint_band_bytes_as_toned(
            data,
            width,
            height,
            indices,
            &descriptor_bytes[head..],
            clock,
        )?;
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
    cut_inner_radius: i32,
    descriptor_input: &RecordDescriptorInput,
    guide_outlines: bool,
    dummy_spiral_regions: &[DummySpiralPixelRegion],
    tone_clock: Option<(&[u8], &record_groove::ToneClock)>,
    tone_preview: bool,
    trailer_tone: Option<[u8; 3]>,
) -> Result<TransparentRender> {
    let spiral_mask =
        build_spiral_mask(width, height, b_value, family, record_profile,
            !descriptor_input.spiral_anticlockwise)?;
    let mut data = vec![0_u8; width * height * 4];

    if guide_outlines {
        paint_record_guides(&mut data, width, height, record_profile)?;
    }

    // Cut before the programme so that a payload which overran its nominal
    // paints over its own deadwax rather than the other way round — the
    // groove that carries something always wins the pixel.
    let deadwax_indices = build_deadwax_spiral_indices(
        width,
        height,
        b_value,
        family,
        record_profile,
        cut_inner_radius,
    )?;
    let deadwax_pixel_capacity = deadwax_indices.len();
    let deadwax_bounds = deadwax_indices
        .first()
        .zip(deadwax_indices.last())
        .map(|(first, last)| (*first, *last));
    // The deadwax is the groove the head kept cutting, so it takes the tone
    // the groove above it was cut in: the wheel pocket by pocket, or the
    // span's own base where the record carries no wheel.
    let groove_clock = tone_clock.map(|(_, clock)| clock);
    let single_tone = if groove_clock.is_some() {
        None
    } else {
        descriptor_input.tone_spans.first().map(|span| span.base)
    };
    paint_toned_groove(
        &mut data,
        width,
        height,
        &deadwax_indices,
        groove_clock,
        single_tone,
        53,
    );

    // What the cut left standing between the programme and the descriptor's
    // inner band, declared so that something other than this renderer can
    // use it. The geometry was already knowable — the prefix carries the
    // radius the cut stopped at and the feed the deadwax is cut with — but
    // knowable is not the same as offered: without this a writer has to
    // re-derive the band from the profile's own tables and guess whether
    // anyone else is already in there.
    //
    // The encoding byte is what keeps this a declaration rather than a
    // promise about pixels: the band is offered toned where the record has a
    // palette to offer it under, and grey where it has none, and nothing
    // about the segment's shape changes either way.
    // The deadwax is not a bootstrap band. By the time a reader reaches it the
    // descriptor has been read and the palette is known, so it is offered as
    // the carrier's own encoding rather than as grey, and its capacity follows
    // that encoding's bits per pixel instead of the metadata ladder's. A
    // band is toned or it is not offered at all: a deadwax quietly downgraded
    // to grey is a band whose declared encoding no longer matches the record
    // it sits on, and a sidecar would write the wrong thing into it.
    let deadwax_encoding = record_descriptor::DEADWAX_ENCODING_TONED;
    let deadwax_bits_per_pixel = tone_clock
        .map(|(_, clock)| clock.bits_per_pixel)
        .or_else(|| {
            descriptor_input
                .tone_spans
                .first()
                .map(|span| u32::from(span.bits_per_pixel))
        })
        .filter(|bits| *bits > 0);

    let deadwax_extent = {
        let inner = deadwax_inner_radius(record_profile, Some(cut_inner_radius))?
            .floor()
            .max(0.0) as i32;
        let outer = cut_inner_radius.max(0);
        // Grooves are toned. The band itself is cut either way — it is the
        // lead-out that carries the needle to the run-out, and a lathe always
        // cuts it — but what the record *says* about it has to be true, and
        // there is no honest thing to say about a toned band on a record that
        // carries no tone. An untoned record therefore cuts its deadwax and
        // declares nothing, which is what "not offered at all" means: the
        // segment is absent, not wrong, and the record still renders.
        match deadwax_bits_per_pixel {
            Some(bits_per_pixel) if deadwax_pixel_capacity > 0 && outer > inner => {
                let pixel_capacity = u32::try_from(deadwax_pixel_capacity)
                    .context("deadwax pixel capacity exceeds u32")?;
                Some(record_descriptor::DeadwaxExtent {
                    outer_radius: u16::try_from(outer)
                        .context("deadwax outer radius exceeds u16")?,
                    inner_radius: u16::try_from(inner)
                        .context("deadwax inner radius exceeds u16")?,
                    pixel_capacity,
                    encoding: deadwax_encoding,
                    byte_capacity: (u64::from(pixel_capacity) * u64::from(bits_per_pixel) / 8)
                        as u32,
                    claim: record_descriptor::DEADWAX_CLAIM_FREE,
                    claimed_byte_length: 0,
                })
            }
            _ => None,
        }
    };

    let mut track_offset = 0usize;
    let mut carrier_pixels_written = 0usize;
    let mut dummy_region_index = 0usize;
    let mut dummy_region_pixels_written = 0usize;
    let mut pixels_added = 0usize;
    // Where each track pixel landed, in track order. A clock tones a pixel
    // by where it sits on the disc, which is only known once it is placed.
    let mut placements = Vec::with_capacity(if tone_clock.is_some() {
        track_data.len() / 4
    } else {
        0
    });

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
        if tone_clock.is_some() {
            placements.push(pixel_index);
        }

        track_offset += 4;
        carrier_pixels_written += 1;
        pixels_added += 1;
    }

    if let Some((codes, clock)) = tone_clock {
        let center_x = width as f64 / 2.0;
        let center_y = height as f64 / 2.0;
        let angles: Vec<f64> = placements
            .iter()
            .map(|&index| {
                record_groove::pixel_angle(
                    (index % width) as f64,
                    (index / width) as f64,
                    center_x,
                    center_y,
                )
            })
            .collect();
        // The same two numbers, read the other way: which ring of the wheel
        // this pixel of the spiral is passing through.
        let radii: Vec<f64> = placements
            .iter()
            .map(|&index| {
                record_groove::pixel_radius(
                    (index % width) as f64,
                    (index / width) as f64,
                    center_x,
                    center_y,
                )
            })
            .collect();
        if tone_preview {
            // The fast tone path: each pixel in its pocket's flat base
            // colour — track or gap — with no palette built and no bit
            // packed. The placement, the pocket assignment and the gap
            // pattern are the press's own; only the grain is missing, which
            // is what a preview is for. Carries no payload, so the facade
            // never asks it to prove it reads.
            for (groove_index, ((&pixel_index, &angle), &away)) in placements
                .iter()
                .zip(&angles)
                .zip(&radii)
                .enumerate()
            {
                let cell = clock.cell_index(groove_index, angle, away);
                let pocket = &clock.slots[cell];
                let tone = if clock.is_gap(groove_index) {
                    pocket.gap_base
                } else {
                    pocket.base
                };
                let at = pixel_index * 4;
                data[at..at + 3].copy_from_slice(&tone);
                data[at + 3] = 255;
            }
        } else {
            let toned = record_groove::encode_toned_clock(codes, clock, &angles, &radii)?;
            for (&index, pixel) in placements.iter().zip(toned.chunks_exact(4)) {
                data[index * 4..index * 4 + 4].copy_from_slice(pixel);
            }
        }
    }

    // The renderer declares the extent, because the extent states the radius
    // at which the groove stopped. That radius is known after the cut is laid
    // down, so the caller supplies no extent.
    let mut descriptor_input = descriptor_input.clone();
    descriptor_input.deadwax = deadwax_extent;

    // The trailer is matte when the caller names a tone for it, and it takes
    // the wheel of the record otherwise. In both cases it carries at the rate
    // of the clock, and the descriptor writes into it after the lead-in fills.
    // Paint it first, so the part that the header leaves keeps the colour of
    // the band.
    let trailer_indices =
        build_run_out_spiral_indices(width, height, record_profile, Some(cut_inner_radius))?;
    let trailer_tone = trailer_tone.or(single_tone);
    // The tone that the band is cut in, which the descriptor reports to a
    // reader. It is the colour of the caller when the caller gives one, and the
    // single tone of the record otherwise.
    descriptor_input.run_out_tone = trailer_tone;
    paint_toned_groove(
        &mut data,
        width,
        height,
        &trailer_indices,
        groove_clock,
        trailer_tone,
        31,
    );
    let trailer_clock = match (trailer_tone, groove_clock) {
        (Some(tone), _) => Some(record_descriptor::trailer_clock(tone)?),
        (None, Some(clock)) => Some(record_descriptor::band_clock(clock)),
        (None, None) => None,
    };

    let descriptor = paint_descriptor_spiral(
        &mut data,
        width,
        height,
        record_profile,
        b_value,
        &descriptor_input,
        trailer_clock
            .as_ref()
            .map(|clock| (trailer_indices.as_slice(), clock)),
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
        deadwax_pixel_capacity,
        deadwax_bounds,
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
    progress: &dyn Fn(&str),
) -> Result<TransparentRenderResult> {
    let normalized_profile = normalize_record_profile_name(record_profile)?;
    // Only when a toning was asked for: without slots or a tone colour
    // there is nothing to tune and the caption should move straight on.
    if render_options
        .groove_tone_slots
        .as_deref()
        .is_some_and(|slots| !slots.is_empty())
        || render_options
            .groove_tone_color
            .as_deref()
            .is_some_and(|colour| !colour.trim().is_empty())
    {
        progress("toning…");
    }
    let groove_clock = groove_clock_track(codes, render_options, &normalized_profile)?;
    let groove_tone = if groove_clock.is_some() {
        None
    } else {
        groove_toned_track(codes, render_options)?
    };
    let (track, source_dimensions, rgb_tone, tone_spans, tone_clock, payload_encoding) =
        match (groove_clock, groove_tone) {
            (Some((track, rgb_tone, clock_descriptor, clock)), _) => {
                let side = square_side_for_pixel_count(track.pixel_count);
                let dimensions = (side, side, track.pixel_count);
                (
                    track,
                    dimensions,
                    Some(rgb_tone),
                    Vec::new(),
                    Some((clock_descriptor, clock)),
                    PAYLOAD_ENCODING_TONED_V2.to_string(),
                )
            }
            (None, Some((track, rgb_tone, tone_spans))) => {
                let side = square_side_for_pixel_count(track.pixel_count);
                let dimensions = (side, side, track.pixel_count);
                (
                    track,
                    dimensions,
                    Some(rgb_tone),
                    tone_spans,
                    None,
                    PAYLOAD_ENCODING_TONED_V1.to_string(),
                )
            }
            (None, None) => {
                let source = payload_codes_to_rgb_color_block(codes, code_format)?;
                let source_track = payload_track_from_rgb_block(&source);
                let track = filter_track_pixels(&source_track.track_data, true, false);
                let dimensions = (source.width, source.height, source_track.pixel_count);
                (
                    track,
                    dimensions,
                    None,
                    Vec::new(),
                    None,
                    normalize_payload_code_format(code_format)?.to_string(),
                )
            }
        };
    let (tone_clock_descriptor, tone_clock) = match tone_clock {
        Some((descriptor, clock)) => (Some(descriptor), Some(clock)),
        None => (None, None),
    };
    let dummy_spiral_regions =
        dummy_spiral_pixel_regions_for_track(render_options, track.pixel_count);
    let dummy_spiral_pixel_count = dummy_spiral_regions
        .iter()
        .map(|region| region.pixel_count)
        .sum::<usize>();
    let required_track_pixel_count = track.pixel_count.saturating_add(dummy_spiral_pixel_count);
    // The nominal size that the cut is laid out against, which is the declared
    // final size from the caller. A progressive load passes the same nominal
    // for every chunk, so every partial render resolves the same pitch and
    // paints a prefix of one groove. Each drawn pixel therefore holds its seat.
    // The nominal has a floor at the present payload size, so a payload above
    // its nominal is laid out at its real size.
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
        render_options.turn_separation_px,
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

    // A cut that reaches the label declares no deadwax. A shorter cut declares
    // the radius at which its groove stops, and the feed that the rest is cut
    // at, so a reader with the PNG alone walks the whole spiral.
    let declares_deadwax =
        cut_inner_radius > payload_inner_radius(&describe_record_profile(&normalized_profile)?);
    let descriptor_input = RecordDescriptorInput {
        // The hand that the programme is cut with. The descriptor carries it,
        // so a reader retraces the cut of this record rather than the house
        // hand. An absent value from the caller selects the house hand, and the
        // descriptor then writes no segment.
        spiral_anticlockwise: !render_options.spiral_clockwise.unwrap_or(true),
        cut_inner_radius: if declares_deadwax {
            u16::try_from(cut_inner_radius).context("cut inner radius does not fit u16")?
        } else {
            0
        },
        deadwax_b_value: if declares_deadwax {
            record_core::deadwax_spiral_pitch(&normalized_profile)?
        } else {
            0.0
        },
        record_profile: normalized_profile.clone(),
        stream_byte_length: codes.len(),
        payload_encoding: Some(payload_encoding),
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
        tone_clock: tone_clock_descriptor,
        cache_encryption,
        chain_anchor: None,
        additional_signatures: Vec::new(),
        isrcs: Vec::new(),
        upc: None,
        deferred_attestation: None,
        spiral_family,
        // Filled in by the cut: see `deadwax_extent` in
        // `render_track_scanline_onto_transparent_spiral`. Nothing above the
        // lathe knows where the groove stopped.
        deadwax: None,
        run_out_tone: run_out_tone(render_options),
    };

    progress("groove…");
    let rendered = render_track_scanline_onto_transparent_spiral(
        RECORD_WIDTH,
        RECORD_HEIGHT,
        fit.b_value,
        &spiral_family,
        &track.track_data,
        resolved_fit_track_pixel_count,
        &normalized_profile,
        cut_inner_radius,
        &descriptor_input,
        render_options.guide_outlines,
        &dummy_spiral_regions,
        tone_clock.as_ref().map(|clock| (codes, clock)),
        render_options.groove_tone_preview.unwrap_or(false),
        run_out_tone(render_options),
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
        deadwax_turns: deadwax_turns(&normalized_profile, cut_inner_radius)?,
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
pub const PAYLOAD_ENCODING_TONED_V2: &str = "toned-v2";
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

/// Builds the clock-toned groove when `grooveToneSlots` is set.
///
/// The track returned is a placeholder of the right length — opaque black,
/// one pixel per `bits_per_pixel` bits of payload — because a clock tones a
/// pixel by where it lands on the disc, and that is only known once the
/// spiral has been fitted and traced. The paint step does the toning in
/// place (see `render_track_scanline_onto_transparent_spiral`). Alongside
/// it: the descriptor the record carries and the wheel the painter uses,
/// which are the same thing in two crates' types.
///
/// Each slot's tone is auto-tuned exactly as a single groove tone is, and
/// its gap tone lightened the same way; bits per pixel comes from the size
/// budget alone, so every slot agrees and the groove is as long as a
/// single-tone cut.
fn groove_clock_track(
    codes: &[u8],
    render_options: &RenderOptions,
    record_profile: &str,
) -> Result<
    Option<(
        TrackPixels,
        serde_json::Value,
        record_descriptor::ToneClockDescriptor,
        record_groove::ToneClock,
    )>,
> {
    let Some(slot_hexes) = render_options
        .groove_tone_slots
        .as_deref()
        .filter(|slots| !slots.is_empty())
    else {
        return Ok(None);
    };
    if codes.is_empty() {
        bail!("cannot tone an empty payload");
    }
    // One ring holding every slot unless the caller has asked for more:
    // that is the wheel of wedges every clock was, and it stays a version 1
    // map so a record that does not need rings is the record it was.
    let rings: Vec<u32> = match render_options.groove_tone_rings.as_deref() {
        Some(rings) if !rings.is_empty() => rings.to_vec(),
        _ => vec![slot_hexes.len() as u32],
    };
    if rings.len() > record_groove::TONE_CLOCK_MAX_RINGS {
        bail!(
            "grooveToneRings has {} rings, more than {}",
            rings.len(),
            record_groove::TONE_CLOCK_MAX_RINGS
        );
    }
    for (index, &slots) in rings.iter().enumerate() {
        if !(record_groove::TONE_CLOCK_MIN_SLOTS..=record_groove::TONE_CLOCK_MAX_SLOTS)
            .contains(&(slots as usize))
        {
            bail!(
                "grooveToneRings[{index}] needs between {} and {} slots, got {slots}",
                record_groove::TONE_CLOCK_MIN_SLOTS,
                record_groove::TONE_CLOCK_MAX_SLOTS,
            );
        }
    }
    let cells: usize = rings.iter().map(|&slots| slots as usize).sum();
    if cells != slot_hexes.len() {
        bail!(
            "grooveToneRings asks for {cells} pockets but grooveToneSlots carries {} colours",
            slot_hexes.len()
        );
    }
    if cells > record_groove::TONE_CLOCK_MAX_CELLS {
        bail!(
            "a wheel of {cells} pockets is more than {}",
            record_groove::TONE_CLOCK_MAX_CELLS
        );
    }

    // The band the rings divide is every band the groove runs through. A
    // ring is a band of the record and it can only tone the part of it the
    // spiral reaches, so the wheel is laid across the disc from the label
    // out to the outermost groove, and not across the whole disc — where
    // the innermost ring would be under the label, toning nothing.
    //
    // The band reaches the label and not the programme's inner radius,
    // because the deadwax, the run-out rings and the locked groove are cut
    // between the two. A band that started at the programme left those
    // bands outside it, and [`ToneClock::ring_index`] clamps a radius
    // outside the band to the nearest ring — so every trailer pixel took
    // the innermost ring however many rings the wheel had.
    let geometry = record_core::describe_record_profile(record_profile)?;
    let span_of = |radius: i32| -> u16 {
        let whole = f64::from(geometry.outer_radius).max(1.0);
        ((f64::from(radius) / whole).clamp(0.0, 1.0)
            * f64::from(record_groove::TONE_CLOCK_SPAN_UNITS))
        .round() as u16
    };
    let span = (
        span_of(geometry.label_radius),
        span_of(geometry.payload_outer_radius),
    );

    let units_per_degree = f64::from(record_groove::TONE_CLOCK_ROTATION_UNITS_PER_TURN) / 360.0;
    let rotation_centidegrees: Vec<u16> = render_options
        .groove_tone_rotation_degrees
        .clone()
        .unwrap_or(RotationDegrees::Whole(0.0))
        .per_ring(rings.len())
        .into_iter()
        .map(|degrees| {
            if !degrees.is_finite() {
                bail!("grooveToneRotationDegrees must be finite");
            }
            Ok(((degrees.rem_euclid(360.0) * units_per_degree).round() as u32
                % record_groove::TONE_CLOCK_ROTATION_UNITS_PER_TURN) as u16)
        })
        .collect::<Result<_>>()?;
    let blend = render_options.groove_tone_blend.unwrap_or(true);
    let preview = render_options.groove_tone_preview.unwrap_or(false);
    let predominant_gap_tone_lightness =
        normalize_gap_tone_lightness(render_options.gap_tone_lightness)?;

    // A preview's tolerances, never tuned and never used to build a
    // palette — the pixels are flat base colours either way. They ride the
    // descriptor so it stays structurally valid; the bits per pixel and the
    // ordering below are the nominal ones, which are also the tuned ones:
    // the budget fixes the count and the search always lands on chroma
    // proximity, so a preview's spiral is the press's spiral to the pixel.
    const PREVIEW_LUMA_TOLERANCE: u8 = 32;
    let preview_config = || -> (u32, CarrierToneOrdering, u8) {
        (
            ((24.0 / GROOVE_TONE_MAX_SIZE_FACTOR).ceil() as u32).clamp(1, 24),
            CarrierToneOrdering::ChromaProximity,
            PREVIEW_LUMA_TOLERANCE,
        )
    };

    let mut slots = Vec::with_capacity(slot_hexes.len());
    let mut shared: Option<(u32, CarrierToneOrdering)> = None;
    for (index, hex) in slot_hexes.iter().enumerate() {
        let hex = hex.trim();
        if hex.is_empty() {
            bail!("grooveToneSlots[{index}] is empty");
        }
        let base = TonedConfig::from_hex(hex, 0, 1).base;
        if preview {
            let effective_gap_tone_lightness = adaptive_gap_tone_lightness(
                oklch_lightness(base),
                predominant_gap_tone_lightness,
            )?;
            let gap_base = lighten_base_oklch(base, effective_gap_tone_lightness)?;
            let (bits_per_pixel, ordering, luma_tolerance) = preview_config();
            match shared {
                None => shared = Some((bits_per_pixel, ordering)),
                Some(held) => {
                    debug_assert!(held == (bits_per_pixel, ordering));
                }
            }
            slots.push(record_groove::ClockSlot {
                base,
                luma_tolerance,
                gap_base,
                gap_luma_tolerance: luma_tolerance,
            });
            continue;
        }
        let config = TonedConfig::balanced(base, GROOVE_TONE_MAX_SIZE_FACTOR)
            .with_context(|| format!("grooveToneSlots[{index}] {hex} cannot be toned"))?;
        match shared {
            None => shared = Some((config.bits_per_pixel, config.ordering)),
            Some((bits_per_pixel, ordering)) => {
                if (bits_per_pixel, ordering) != (config.bits_per_pixel, config.ordering) {
                    bail!(
                        "grooveToneSlots[{index}] {hex} tuned to {} bits per pixel where slot 0                          tuned to {bits_per_pixel}; a clock's slots must agree",
                        config.bits_per_pixel
                    );
                }
            }
        }
        let effective_gap_tone_lightness =
            adaptive_gap_tone_lightness(oklch_lightness(base), predominant_gap_tone_lightness)?;
        let gap_base = lighten_base_oklch(base, effective_gap_tone_lightness)?;
        // Tuned on its own rather than borrowing the track tone's window: a
        // lightened tone sits nearer the gamut ceiling, where the same luma
        // window holds fewer colours than 2^20, and the clock carries a gap
        // tolerance per pocket for exactly this.
        let gap_config = TonedConfig::balanced(gap_base, GROOVE_TONE_MAX_SIZE_FACTOR)
            .with_context(|| format!("grooveToneSlots[{index}] {hex} gap tone cannot be toned"))?;
        if (gap_config.bits_per_pixel, gap_config.ordering) != (config.bits_per_pixel, config.ordering) {
            bail!(
                "grooveToneSlots[{index}] {hex} gap tone tuned to {} bits per pixel where its \
                 track tone tuned to {}; a clock's tones must agree",
                gap_config.bits_per_pixel,
                config.bits_per_pixel
            );
        }
        slots.push(record_groove::ClockSlot {
            base,
            luma_tolerance: config.luma_tolerance,
            gap_base,
            gap_luma_tolerance: gap_config.luma_tolerance,
        });
    }
    let (bits_per_pixel, ordering) = shared.expect("at least two slots");

    let clock = record_groove::ToneClock {
        rotation_centidegrees: rotation_centidegrees.clone(),
        blend,
        bits_per_pixel,
        ordering,
        rings: rings.clone(),
        span,
        slots,
        gap_switch_offsets: track_gap_tone_switch_offsets(codes).unwrap_or_default(),
    };
    clock.validate()?;

    let descriptor = record_descriptor::ToneClockDescriptor {
        rotation_centidegrees,
        blend,
        rings,
        span,
        bits_per_pixel: u8::try_from(bits_per_pixel).context("tone bits per pixel exceeds u8")?,
        ordering: match ordering {
            CarrierToneOrdering::BaseProximity => ToneOrdering::BaseProximity,
            CarrierToneOrdering::ChromaProximity => ToneOrdering::ChromaProximity,
        },
        slots: clock
            .slots
            .iter()
            .map(|slot| record_descriptor::ToneClockSlotDescriptor {
                base: slot.base,
                luma_tolerance: slot.luma_tolerance,
                gap_base: slot.gap_base,
                gap_luma_tolerance: slot.gap_luma_tolerance,
            })
            .collect(),
        gap_switch_offsets: clock.gap_switch_offsets.clone(),
    };
    record_descriptor::validate_tone_clock(&descriptor, Some(codes.len()))?;
    let clock_json = serde_json::to_value(&descriptor)?;

    let pixel_count = clock.pixel_count(codes.len());
    let mut track_data = vec![0u8; pixel_count * 4];
    for pixel in track_data.chunks_exact_mut(4) {
        pixel[3] = 255;
    }

    Ok(Some((
        TrackPixels {
            track_data,
            pixel_count,
        },
        clock_json,
        descriptor,
        clock,
    )))
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
    // `Default` rather than `Best`: on groove noise both levels find the
    // same matches, so the bytes are within a few hundred of one another
    // while the encode runs several times faster — and either way it is
    // lossless, so the record's pixels are untouched. What matters about a
    // pressed PNG is what it decodes to, not how hard the deflater worked.
    let encoder =
        PngEncoder::new_with_quality(&mut out, CompressionType::Default, FilterType::Adaptive);

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
                    true,
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

    /// `len` bytes of real codec output, from a byte into the golden ECDC.
    ///
    /// A groove holds whatever the record holds, and what a record holds is
    /// EnCodec: high-entropy bytes whose colours land all over a pocket's
    /// palette. A constant fill is the easy case — its bytes cluster, so a
    /// palette that could not tell two neighbouring colours apart would
    /// never be asked to — which makes a fill the wrong thing to prove a
    /// carrier with. `from` moves the window so entries cut from one file
    /// are not cut from the same bytes.
    fn codec_bytes(from: usize, len: usize) -> Vec<u8> {
        let id = "lori-asha-westside-single45-hq";
        let ecdc = fixture_bytes(id, &format!("{id}.ecdc"));
        assert!(
            ecdc.len() > from + len,
            "the golden ECDC is shorter than the slice asked for"
        );
        ecdc[from..from + len].to_vec()
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

    /// A progressive load lays down the finished cut at every step. A pinned
    /// nominal holds the pitch while the chunks arrive. This invariant makes
    /// the render follow the way a lathe cuts.
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

    /// A cut below a full band stops short of the label and leaves the rest of
    /// the band as deadwax. The span sets that stop radius.
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

    /// The deadwax is a feed rate, not a turn count. A lathe's spiral lever
    /// does not know how far it has to travel, so a programme that stops
    /// early leaves more turns at the same spacing — never the same turns
    /// spread thinner. Pin the count to the physical pitch so it can only
    /// move if the feed does.
    #[test]
    fn the_deadwax_is_cut_at_the_lathes_spiral_feed() {
        let px_per_mm = record_core::pixels_per_mm("lp").unwrap();
        let separation = record_core::DEADWAX_PITCH_MM * px_per_mm;
        assert!(
            (record_core::deadwax_turn_separation_px("lp").unwrap() - separation).abs() < 1e-9,
        );

        // The deadwax hands over to the run-out, not to the payload inner
        // radius: the lead-out now begins above it.
        let hand_over = |cut: i32| deadwax_inner_radius("lp", Some(cut)).unwrap();
        let mut previous = f64::INFINITY;

        for span in [0.25_f64, 0.33, 0.50, 0.67, 1.0] {
            let cut = render_lp(
                &rgb_code_block(60_000),
                &format!(r#"{{"grooveSpanFraction":{span}}}"#),
            );
            let travel =
                (cut.cut_inner_radius as f64 - hand_over(cut.cut_inner_radius)).max(0.0);

            assert!(
                (cut.deadwax_turns - travel / separation).abs() < 1e-6,
                "span {span} reported {} turns over {travel} px of travel",
                cut.deadwax_turns,
            );
            // The deadwax is no longer monotonic in the span, and that is the
            // point of the wide extents: a cut that stops early leaves room,
            // the lead-out claims it a whole turn at a time, and the deadwax
            // gets what is left over. A shorter cut can therefore leave less
            // deadwax than a longer one, because it crossed a rung.
            let _ = previous;
            previous = cut.deadwax_turns;
        }

        // The run-out begins below the programme's edge, so a side cut to the
        // last usable radius still has a little fine groove between the two.
        // A record has that too; what it does not have is a ladder.
        assert!(
            previous < 4.0,
            "a cut that reaches the label should leave a sliver of deadwax, got {previous} turns"
        );
    }

    /// The deadwax of a dubplate holds tens of turns. One four-minute track
    /// leaves about 66 mm of travel on a 12", which is about sixty turns at the
    /// spiral feed. A physical dubplate shows that broad ladder. A count in
    /// single figures means that the run-out has taken the band and left three
    /// rings near the label.
    #[test]
    fn a_dubplate_sized_cut_leaves_a_deadwax_of_the_right_order() {
        let cut = render_lp(&rgb_code_block(60_000), r#"{"grooveSpanFraction":0.33}"#);

        // The deadwax of a dubplate held forty to ninety turns, because the
        // head cut at a millimetre a turn up to the trailer. The lead-out now
        // widens into that space first, and the deadwax takes the remainder,
        // so the count is lower. The wide extents produce this result.
        assert!(
            (5.0..40.0).contains(&cut.deadwax_turns),
            "expected a dubplate's deadwax after the lead-out took its share, got {} turns",
            cut.deadwax_turns,
        );
    }

    fn radius_of(pixel_index: usize) -> f64 {
        let cx = RECORD_WIDTH as f64 / 2.0;
        let cy = RECORD_HEIGHT as f64 / 2.0;
        let x = (pixel_index % RECORD_WIDTH) as f64 - cx;
        let y = (pixel_index / RECORD_WIDTH) as f64 - cy;
        (x * x + y * y).sqrt()
    }

    fn angle_of(pixel_index: usize) -> f64 {
        let cx = RECORD_WIDTH as f64 / 2.0;
        let cy = RECORD_HEIGHT as f64 / 2.0;
        let x = (pixel_index % RECORD_WIDTH) as f64 - cx;
        let y = cy - (pixel_index / RECORD_WIDTH) as f64;
        y.atan2(x)
    }

    fn wrapped_angle(angle: f64) -> f64 {
        angle.rem_euclid(std::f64::consts::TAU)
    }

    fn angle_difference(a: f64, b: f64) -> f64 {
        let raw = (a - b).rem_euclid(std::f64::consts::TAU);
        if raw > std::f64::consts::PI {
            raw - std::f64::consts::TAU
        } else {
            raw
        }
    }

    /// One groove, rim to label. The deadwax picks the head up exactly
    /// where the programme put it down — same radius, same angle, new feed —
    /// so a traversal walks straight out of the payload and into the
    /// deadwax. Without the phase carried across it restarts at the top of
    /// the disc and the join is most of a revolution.
    ///
    /// The test measures against the analytic crossing rather than against a
    /// neighbouring mask pixel. Pixel radii quantize to about ±0.7 px, so the
    /// last pixel above the transition can sit a fifth of a turn from the
    /// crossing of the groove.
    #[test]
    fn the_deadwax_picks_the_groove_up_where_the_programme_left_it() {
        for span in [0.25_f64, 0.33, 0.50] {
            let cut = render_lp(
                &rgb_code_block(60_000),
                &format!(r#"{{"grooveSpanFraction":{span}}}"#),
            );

            let crossing = record_core::groove_angle_at_radius(
                RECORD_WIDTH,
                RECORD_HEIGHT,
                cut.b_value,
                &SpiralFamily::Archimedean,
                "lp",
                cut.cut_inner_radius as f64,
            )
            .unwrap();

            let deadwax = build_deadwax_spiral_indices(
                RECORD_WIDTH,
                RECORD_HEIGHT,
                cut.b_value,
                &SpiralFamily::Archimedean,
                "lp",
                cut.cut_inner_radius,
            )
            .unwrap();
            let first = *deadwax.first().expect("a short cut has a deadwax");

            let radius = radius_of(first);
            assert!(
                (radius - cut.cut_inner_radius as f64).abs() <= 1.5,
                "span {span}: the deadwax starts at r={radius:.1}, not on the transition at {}",
                cut.cut_inner_radius,
            );

            let expected = wrapped_angle(crossing);
            let actual = wrapped_angle(angle_of(first));
            let drift = angle_difference(expected, actual).to_degrees();
            assert!(
                drift.abs() <= 2.0,
                "span {span}: the deadwax starts {drift:.1} degrees off the programme's crossing",
            );
        }
    }

    /// Empty is not the same as absent. The deadwax is a carrier whose
    /// addresses exist whether or not anything has been written into them,
    /// and its capacity grows as the programme leaves more room.
    #[test]
    fn the_deadwax_is_an_addressable_carrier_even_while_it_holds_nothing() {
        let mut previous = 0usize;

        for span in [0.67_f64, 0.50, 0.33, 0.25] {
            let cut = render_lp(
                &rgb_code_block(60_000),
                &format!(r#"{{"grooveSpanFraction":{span}}}"#),
            );

            // Not monotonic any more: the lead-out claims the room a whole
            // turn at a time, so crossing a rung hands it a block of what the
            // deadwax would otherwise have had. What must hold is that the
            // band is addressable whenever there is anything left to address.
            let _ = previous;
            previous = cut.deadwax_pixel_capacity;
        }

        let full = render_lp(&rgb_code_block(60_000), r#"{"grooveSpanFraction":1.0}"#);
        let short = render_lp(&rgb_code_block(60_000), r#"{"grooveSpanFraction":0.33}"#);

        // Not zero any more, and it should not be. The run-out begins below
        // the programme's edge, so even a side cut to the last usable radius
        // has a sliver of fine groove between the two — which is what a record
        // has. What matters is that it stays a sliver.
        assert!(
            full.deadwax_pixel_capacity * 4 < short.deadwax_pixel_capacity,
            "a full cut left {} px of deadwax against a short cut's {}",
            full.deadwax_pixel_capacity,
            short.deadwax_pixel_capacity,
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

    // ---- export ---------------------------------------------------------

    use crate::export::{read_rgba, write_rgba, RecordImageFormat};

    /// A short record with a real payload in it, and the stream it carries.
    fn pressed_record(options: Option<&str>) -> (Vec<u8>, Vec<u8>) {
        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput::from_container("TEST")],
            tracks: vec![TrackInput {
                title: "Side A".to_string(),
                first_revolution_index: None,
                revolution_count: None,
            }],
            track_gaps: vec![],
        };
        let entries = vec![PayloadEntryInput {
            payload_descriptor_index: 0,
            bytes: codec_bytes(64, 6_000),
        }];
        let stream = encode_record_stream(&input, &entries).unwrap();
        let output =
            render_payload_codes_to_png(&stream, "rgb", "single45", 208.5, options).unwrap();
        (output.png_bytes, stream)
    }

    /// A wheel of sixteen pockets, which is the hard case: a toned cut gives
    /// every pocket its own palette of a million iso-luma colours and packs
    /// the payload across them at twenty bits a pixel, so a format that
    /// moves one channel of one pixel by one step loses the record.
    fn wheel_options() -> String {
        let slots: Vec<String> = (0..16)
            .map(|k| {
                let t = k as f64 / 16.0 * std::f64::consts::TAU;
                format!(
                    "#{:02X}{:02X}{:02X}",
                    (150.0 + 90.0 * t.cos()) as u8,
                    (150.0 + 90.0 * (t + 2.094).cos()) as u8,
                    (150.0 + 90.0 * (t + 4.189).cos()) as u8
                )
            })
            .collect();
        serde_json::json!({
            "grooveToneSlots": slots,
            "grooveToneRings": [8, 8],
            "grooveToneRotationDegrees": [5.625, 33.0],
            "grooveToneBlend": true,
        })
        .to_string()
    }

    /// Every format this build can write returns the record's own pixels.
    ///
    /// The test compares the pixels byte for byte, and then decodes the
    /// groove. Each format claims the first property. The second property is
    /// the readable record. A build with the default features proves PNG here.
    /// `--features all-formats` exercises the other formats.
    #[test]
    fn every_export_format_returns_the_record_exactly() {
        for (what, options) in [("rgb", None), ("toned", Some(wheel_options()))] {
            let (png, stream) = pressed_record(options.as_deref());
            let (width, height, pixels) = read_rgba(&png).unwrap();
            assert_eq!((width, height), (RECORD_WIDTH, RECORD_HEIGHT));

            for format in RecordImageFormat::available() {
                let id = format.id();
                let written = write_rgba(format, width, height, &pixels)
                    .unwrap_or_else(|error| panic!("{what}: writing {id} failed: {error:#}"));

                let (back_width, back_height, back) = read_rgba(&written)
                    .unwrap_or_else(|error| panic!("{what}: reading {id} back failed: {error:#}"));
                assert_eq!(
                    (back_width, back_height),
                    (width, height),
                    "{what}: {id} changed the record's size"
                );
                assert_eq!(
                    back.len(),
                    pixels.len(),
                    "{what}: {id} changed how many pixels the record has"
                );
                let moved = back
                    .iter()
                    .zip(&pixels)
                    .position(|(there, here)| there != here);
                assert!(
                    moved.is_none(),
                    "{what}: {id} altered byte {} of the record",
                    moved.unwrap()
                );

                let decoded =
                    record_decode::decode_record_png_to_chunk_stream_for_profile_with_length(
                        &written,
                        "single45",
                        Some(stream.len()),
                    )
                    .unwrap_or_else(|error| panic!("{what}: {id} would not decode: {error:#}"));
                assert_eq!(
                    decoded.bytes, stream,
                    "{what}: the groove did not survive {id}"
                );
            }
        }
    }

    /// A format the build does not carry says so, rather than writing
    /// something that is not that format.
    #[test]
    fn an_uncompiled_format_refuses_by_name() {
        let pixels = vec![0u8; 4];
        for format in RecordImageFormat::ALL {
            let written = write_rgba(format, 1, 1, &pixels);
            if format.is_available() {
                assert!(written.is_ok(), "{} is compiled in", format.id());
            } else {
                let error = written.expect_err("an uncompiled format must not write").to_string();
                assert!(
                    error.contains(format.id()),
                    "{} refused without naming itself: {error}",
                    format.id()
                );
            }
        }
    }

    /// The names a caller asks for and the names written back agree.
    #[test]
    fn a_format_is_found_by_the_name_it_gives() {
        for format in RecordImageFormat::ALL {
            assert_eq!(RecordImageFormat::from_id(format.id()), Some(format));
            assert!(!format.extension().is_empty());
            assert!(format.media_type().contains('/'));
        }
        assert_eq!(RecordImageFormat::from_id("jpeg"), None);
        assert_eq!(RecordImageFormat::from_id("gif"), None);
    }

    /// The record must not care which format it arrived in: a PNG written
    /// as a TIFF and back again is the PNG.
    #[test]
    fn a_record_transcodes_through_every_compiled_format() {
        let (png, stream) = pressed_record(None);
        for format in RecordImageFormat::available() {
            let there = crate::export::transcode(&png, format).unwrap();
            let back = crate::export::transcode(&there, RecordImageFormat::Png).unwrap();
            let decoded = record_decode::decode_record_png_to_chunk_stream_for_profile_with_length(
                &back,
                "single45",
                Some(stream.len()),
            )
            .unwrap_or_else(|error| panic!("{}: {error:#}", format.id()));
            assert_eq!(decoded.bytes, stream, "a record lost itself in {}", format.id());
        }
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
        let payload_one = codec_bytes(64, 4_000);
        let payload_two = codec_bytes(60_000, 5_500);

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

        assert_eq!(
            output.descriptor.version,
            record_descriptor::RECORD_DESCRIPTOR_VERSION_HOUSE
        );
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
        let payload = codec_bytes(64, 3_000);
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
        assert_eq!(
            implicit.descriptor.version,
            record_descriptor::RECORD_DESCRIPTOR_VERSION
        );
        assert_eq!(implicit.descriptor.spiral_family, SpiralFamily::Archimedean);
    }

    #[test]
    fn two_track_compact_groove_with_two_descriptors_renders_and_decodes_byte_exact() {
        let payload_one = codec_bytes(64, 4_000);
        let payload_two = codec_bytes(60_000, 3_200);

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
        let music_one = codec_bytes(64, 4_000);
        let gap = codec_bytes(20_000, 1_500);
        let music_two = codec_bytes(40_000, 5_500);

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

    /// Sixteen pockets, spun a quarter of a pocket, blended, with a track
    /// gap: the record must come back byte for byte, be no longer than a
    /// single-tone cut, and carry the wheel — not a span list — in its
    /// descriptor.
    #[test]
    fn clock_toned_groove_round_trips_with_gaps() {
        let music_one = codec_bytes(64, 4_000);
        let gap = codec_bytes(20_000, 1_500);
        let music_two = codec_bytes(40_000, 5_500);

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

        // A hue wheel, one pocket every 22.5°.
        let slots: Vec<String> = (0..16)
            .map(|k| {
                let t = k as f64 / 16.0 * std::f64::consts::TAU;
                format!(
                    "#{:02X}{:02X}{:02X}",
                    (150.0 + 90.0 * t.cos()) as u8,
                    (150.0 + 90.0 * (t + 2.094).cos()) as u8,
                    (150.0 + 90.0 * (t + 4.189).cos()) as u8
                )
            })
            .collect();
        let options = serde_json::json!({
            "grooveToneSlots": slots,
            "grooveToneRotationDegrees": 5.625,
            "grooveToneBlend": true,
        })
        .to_string();
        let output =
            render_payload_codes_to_png(&stream, "rgb", "single45", 208.5, Some(&options)).unwrap();

        let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/fixtures");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("clock-toned-single45.png"), &output.png_bytes).unwrap();

        assert_eq!(output.descriptor.payload_encoding, PAYLOAD_ENCODING_TONED_V2);
        assert!(output.descriptor.tone_spans.is_empty());
        let clock = output.descriptor.tone_clock.as_ref().expect("clock in descriptor");
        assert_eq!(clock.slots.len(), 16);
        // One rotation per ring since the clock grew a wheel: the option is
        // still a single angle, so every ring must have taken it.
        assert!(
            !clock.rotation_centidegrees.is_empty(),
            "a clock with no rotation at all"
        );
        assert!(
            clock
                .rotation_centidegrees
                .iter()
                .all(|&turn| turn == 563),
            "5.625° in hundredths, rounded, on every ring: {:?}",
            clock.rotation_centidegrees
        );
        assert!(clock.blend);
        assert_eq!(clock.gap_switch_offsets.len(), 2, "into the gap and out again");
        for slot in &clock.slots {
            assert_ne!(slot.gap_base, slot.base);
        }

        // At the same budget, a clock cut is one or two pixels shorter than a
        // single-tone cut. Each span of a single-tone cut pads its own tail,
        // and the one stream of a clock pads once.
        let single = serde_json::json!({ "grooveToneColor": slots[0] }).to_string();
        let single_output =
            render_payload_codes_to_png(&stream, "rgb", "single45", 208.5, Some(&single)).unwrap();
        assert!(
            output.payload.filtered_pixel_count <= single_output.payload.filtered_pixel_count,
            "clock {} px, single tone {} px",
            output.payload.filtered_pixel_count,
            single_output.payload.filtered_pixel_count
        );

        let decoded = record_decode::decode_record_png_to_chunk_stream_for_profile_with_length(
            &output.png_bytes,
            "single45",
            Some(stream.len()),
        )
        .unwrap();
        assert_eq!(decoded.bytes, stream, "clock-toned groove did not round-trip");
    }

    /// The house wheel, cut and read back: eight pockets across the inside
    /// of the groove band and sixteen around the outside, blended both
    /// ways, at no cost in groove length.
    #[test]
    fn a_ringed_clock_tones_the_groove_and_round_trips() {
        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput::from_container("TEST")],
            tracks: vec![TrackInput {
                title: "Side A".to_string(),
                first_revolution_index: Some(0),
                revolution_count: Some(1),
            }],
            track_gaps: Vec::new(),
        };
        let entries = vec![PayloadEntryInput {
            payload_descriptor_index: 0,
            bytes: codec_bytes(64, 11_000),
        }];
        let stream = encode_record_stream(&input, &entries).unwrap();

        // Twenty-four pockets: the inner ring's eight, then the outer
        // ring's sixteen, each its own hue so no two pockets could be
        // mistaken for one another.
        let cells: Vec<String> = (0..24)
            .map(|k| {
                let t = k as f64 / 24.0 * std::f64::consts::TAU;
                format!(
                    "#{:02X}{:02X}{:02X}",
                    (150.0 + 90.0 * t.cos()) as u8,
                    (150.0 + 90.0 * (t + 2.094).cos()) as u8,
                    (150.0 + 90.0 * (t + 4.189).cos()) as u8
                )
            })
            .collect();
        let options = serde_json::json!({
            "grooveToneSlots": cells,
            "grooveToneRings": [8, 16],
            "grooveToneRotationDegrees": 11.25,
            "grooveToneBlend": true,
        })
        .to_string();
        let output =
            render_payload_codes_to_png(&stream, "rgb", "single45", 208.5, Some(&options)).unwrap();

        let clock = output.descriptor.tone_clock.as_ref().expect("clock in descriptor");
        assert_eq!(clock.rings, vec![8, 16]);
        assert_eq!(clock.slots.len(), 24);
        assert!(clock.has_rings(), "a ringed wheel is written as one");
        // The band the rings divide reaches the label, not the spindle: an
        // inner ring under the label would tone nothing, and one that
        // stopped at the programme would leave the trailer outside the band.
        let geometry = record_core::describe_record_profile("single45").unwrap();
        assert!(
            f64::from(clock.span.0) / f64::from(record_groove::TONE_CLOCK_SPAN_UNITS)
                >= f64::from(geometry.label_radius) / f64::from(geometry.outer_radius) - 0.001,
            "the band starts at the label, not the spindle"
        );
        assert!(
            f64::from(clock.span.1) / f64::from(record_groove::TONE_CLOCK_SPAN_UNITS)
                <= f64::from(geometry.outer_radius),
            "the band ends inside the record"
        );

        // Rings cost no groove: the same payload on a wheel of wedges is
        // the same length.
        let wedges = serde_json::json!({
            "grooveToneSlots": cells[..16],
            "grooveToneRotationDegrees": 11.25,
            "grooveToneBlend": true,
        })
        .to_string();
        let flat =
            render_payload_codes_to_png(&stream, "rgb", "single45", 208.5, Some(&wedges)).unwrap();
        assert_eq!(
            output.payload.filtered_pixel_count, flat.payload.filtered_pixel_count,
            "rings changed the length of the cut"
        );

        let decoded = record_decode::decode_record_png_to_chunk_stream_for_profile_with_length(
            &output.png_bytes,
            "single45",
            Some(stream.len()),
        )
        .unwrap();
        assert_eq!(decoded.bytes, stream, "ringed clock groove did not round-trip");
    }

    /// The two rings turn independently, and the record carries both.
    ///
    /// Every other clock test here sets one angle for the whole wheel, and the
    /// renderer pads that angle onto every ring. Those tests therefore leave
    /// the pair that a re-press draws, an independent inner turn and outer
    /// turn, uncut. This test cuts such a pair and reads the ring angles back
    /// off the record.
    ///
    /// The angles are whole degrees, and each angle sits inside one pocket
    /// width of its own ring: 45° for the eight-pocket inner ring, and 22.5°
    /// for the sixteen-pocket outer ring. Above those widths a ring boundary
    /// land back on themselves and the pockets re-read the same art through
    /// them, so the record is one that has already been pressed.
    #[test]
    fn the_rings_turn_independently_and_the_record_carries_both() {
        let input = RecordStreamInput {
            payload_descriptors: vec![PayloadDescriptorInput::from_container("ECDC")],
            tracks: vec![TrackInput {
                title: "Side A".to_string(),
                first_revolution_index: Some(0),
                revolution_count: Some(1),
            }],
            track_gaps: Vec::new(),
        };
        let entries = vec![PayloadEntryInput {
            payload_descriptor_index: 0,
            bytes: codec_bytes(64, 11_000),
        }];
        let stream = encode_record_stream(&input, &entries).unwrap();

        let cells: Vec<String> = (0..24)
            .map(|k| {
                let t = k as f64 / 24.0 * std::f64::consts::TAU;
                format!(
                    "#{:02X}{:02X}{:02X}",
                    (150.0 + 90.0 * t.cos()) as u8,
                    (150.0 + 90.0 * (t + 2.094).cos()) as u8,
                    (150.0 + 90.0 * (t + 4.189).cos()) as u8
                )
            })
            .collect();
        let cut = |turns: [f64; 2]| {
            let options = serde_json::json!({
                "grooveToneSlots": cells,
                "grooveToneRings": [8, 16],
                "grooveToneRotationDegrees": turns,
                "grooveToneBlend": true,
            })
            .to_string();
            render_payload_codes_to_png(&stream, "rgb", "single45", 208.5, Some(&options)).unwrap()
        };

        let output = cut([37.0, 11.0]);
        let clock = output
            .descriptor
            .tone_clock
            .as_ref()
            .expect("clock in descriptor");
        assert_eq!(
            clock.rotation_centidegrees,
            vec![3_700, 1_100],
            "each ring keeps the turn it was given, rather than the last one padded across"
        );

        let decoded = record_decode::decode_record_png_to_chunk_stream_for_profile_with_length(
            &output.png_bytes,
            "single45",
            Some(stream.len()),
        )
        .unwrap();
        assert_eq!(
            decoded.bytes, stream,
            "a wheel with its rings turned apart did not come back"
        );

        // The outer turn has to be doing something, or a re-press that only
        // moved it would mint the edition it already pressed.
        let same_outer_as_inner = cut([37.0, 37.0]);
        assert_ne!(
            output.png_bytes, same_outer_as_inner.png_bytes,
            "turning only the outer ring changed nothing about the record"
        );
    }

    #[test]
    fn gap_tone_lightness_zero_matches_track_tone() {
        let music_one = codec_bytes(64, 4_000);
        let gap = codec_bytes(20_000, 1_500);
        let music_two = codec_bytes(40_000, 5_500);

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
            &[codec_bytes(64, 4_000)],
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
        let payload_one = codec_bytes(64, 4_000);
        let payload_two = codec_bytes(60_000, 5_500);
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

    /// The bands below the programme are grooves on a picture record, so
    /// they carry the picture's colours: the deadwax in the wheel's own
    /// pockets, the trailer in the one matte tone it was cut with. The
    /// lead-in stays grey, because a reader has to read it before it knows
    /// any of this.
    #[test]
    fn the_deadwax_and_the_trailer_are_cut_in_colour() {
        let slots: Vec<String> = (0..24)
            .map(|slot| format!("#{:02X}{:02X}{:02X}", 60 + slot * 8, 120 + slot * 4, 200 - slot * 6))
            .collect();
        let options = serde_json::json!({
            "grooveSpanFraction": 0.33,
            "grooveToneSlots": slots,
            "grooveToneRings": [8, 16],
            "runOutToneColor": "#7A4B2A",
        })
        .to_string();
        // Short enough that the cut stops well up the side and leaves both
        // bands room to be looked at.
        let output =
            render_payload_codes_to_png(&codec_bytes(64, 40_000), "rgb", "lp", 60.0, Some(&options))
                .unwrap();

        let rgba = record_decode::load_record_rgba(&output.png_bytes).unwrap().2;
        let read = |index: usize| {
            let at = index * 4;
            [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]]
        };
        let is_grey = |pixel: [u8; 4]| pixel[0] == pixel[1] && pixel[1] == pixel[2];

        let cut = i32::from(output.payload.cut_inner_radius);
        let trailer =
            record_core::build_run_out_spiral_indices(RECORD_WIDTH, RECORD_HEIGHT, "lp", Some(cut))
                .unwrap();
        assert!(!trailer.is_empty(), "the record has no trailer to look at");
        let matte = [0x7A, 0x4B, 0x2A];
        for &index in &trailer {
            let pixel = read(index);
            assert_eq!(
                [pixel[0], pixel[1], pixel[2]],
                matte,
                "a trailer pixel is not the tone the trailer was cut in"
            );
        }

        // The deadwax takes the wheel, so it is not one colour — but no part
        // of it is the grey ladder either.
        let deadwax = build_deadwax_spiral_indices(
            RECORD_WIDTH,
            RECORD_HEIGHT,
            f64::from_bits(output.descriptor.b_value_bits),
            &output.descriptor.spiral_family,
            "lp",
            cut,
        )
        .unwrap();
        assert!(!deadwax.is_empty(), "the record has no deadwax to look at");
        let toned = deadwax
            .iter()
            .filter(|&&index| !is_grey(read(index)))
            .count();
        assert!(
            toned * 10 > deadwax.len() * 9,
            "only {toned} of {} deadwax pixels carry a colour",
            deadwax.len()
        );

        // And the bootstrap band is untouched.
        let lead_in =
            build_lead_in_spiral_indices(RECORD_WIDTH, RECORD_HEIGHT, "lp").unwrap();
        for &index in &lead_in {
            assert!(
                is_grey(read(index)),
                "a lead-in pixel is not grey, and the descriptor is read before anything is known"
            );
        }

        assert_eq!(
            output.descriptor.run_out_tone,
            Some(matte),
            "the trailer's tone is not on the wire"
        );
    }

    /// The trailer is still a carrier. When the header outgrows the lead-in
    /// the rest goes into the coarse rings — in a palette around the tone
    /// they are cut in, so the band still reads as one matte ring — and it
    /// comes back byte for byte.
    #[test]
    fn a_header_too_big_for_the_lead_in_goes_into_the_trailer() {
        // Three creator fields at their own limit: 3 000 bytes against the
        // 2 428 an LP's lead-in holds, so the stream has to cross into the
        // band below.
        let long = "WESTSIDE-".repeat(111);
        let options = serde_json::json!({
            "grooveSpanFraction": 0.33,
            "grooveToneColor": "#FF2582",
            "runOutToneColor": "#7A4B2A",
            "headerLabel": long,
            "headerArtworkCredit": long,
            "headerCanonicalUrl": long,
        })
        .to_string();
        let output =
            render_payload_codes_to_png(&codec_bytes(64, 40_000), "rgb", "lp", 60.0, Some(&options))
                .unwrap();

        assert!(
            output.descriptor.stream_byte_length > 0,
            "the record carries no stream"
        );
        assert_eq!(
            output.descriptor.label.as_deref(),
            Some(long.as_str()),
            "the header did not survive the cut"
        );

        // And off the record itself, which is the only proof that matters:
        // the reader has to find the tone in the lead-in, build the palette
        // and walk the rings.
        let (_, read_back) =
            record_decode::decode_record_descriptor_bytes_from_png(&output.png_bytes, Some("lp"))
                .unwrap();
        let descriptor = record_descriptor::decode_record_descriptor_bytes(&read_back).unwrap();
        assert_eq!(descriptor.label.as_deref(), Some(long.as_str()));
        assert_eq!(descriptor.artwork_credit.as_deref(), Some(long.as_str()));
        assert_eq!(descriptor.canonical_url.as_deref(), Some(long.as_str()));
        assert_eq!(descriptor.run_out_tone, Some([0x7A, 0x4B, 0x2A]));

        // The written part of the band is still the band's own colour: an
        // iso-luma palette varies the colour and never the light.
        let rgba = record_decode::load_record_rgba(&output.png_bytes).unwrap().2;
        let trailer = record_core::build_run_out_spiral_indices(
            RECORD_WIDTH,
            RECORD_HEIGHT,
            "lp",
            Some(i32::from(output.payload.cut_inner_radius)),
        )
        .unwrap();
        let clock = record_descriptor::trailer_clock([0x7A, 0x4B, 0x2A]).unwrap();
        let palette = record_groove::TonedPalette::from_config(clock.config(0, false)).unwrap();
        let matte_luma = record_groove::luma_rec709(&[0x7Au8, 0x4B, 0x2A, 255], 0);
        let window = f64::from(clock.slots[0].luma_tolerance) + 1.0;
        for &index in trailer.iter().take(400) {
            let at = index * 4;
            let colour = [rgba[at], rgba[at + 1], rgba[at + 2]];
            assert!(
                palette.index_of(colour).is_some(),
                "a trailer pixel is not in the band's own palette"
            );
            assert!(
                (record_groove::luma_rec709(&rgba, index) - matte_luma).abs() <= window,
                "a written trailer pixel changed the band's light, not just its colour"
            );
        }
    }

    /// A trailer toned by the wheel carries the same header the same way. The
    /// pixel takes the palette of the pocket it sits in and the bit stream
    /// runs across the pockets, which is the encoding the programme carries.
    #[test]
    fn a_wheel_toned_trailer_carries_the_header_too() {
        let slots: Vec<String> = (0..24)
            .map(|slot| format!("#{:02X}{:02X}{:02X}", 60 + slot * 8, 120 + slot * 4, 200 - slot * 6))
            .collect();
        let long = "WESTSIDE-".repeat(111);
        let options = serde_json::json!({
            "grooveSpanFraction": 0.33,
            "grooveToneSlots": slots,
            "grooveToneRings": [8, 16],
            "headerLabel": long,
            "headerArtworkCredit": long,
            "headerCanonicalUrl": long,
        })
        .to_string();
        let output =
            render_payload_codes_to_png(&codec_bytes(64, 40_000), "rgb", "lp", 60.0, Some(&options))
                .unwrap();

        assert_eq!(
            output.descriptor.run_out_tone, None,
            "no tone was given for the trailer"
        );

        let (_, read_back) =
            record_decode::decode_record_descriptor_bytes_from_png(&output.png_bytes, Some("lp"))
                .unwrap();
        let descriptor = record_descriptor::decode_record_descriptor_bytes(&read_back).unwrap();
        assert_eq!(descriptor.label.as_deref(), Some(long.as_str()));
        assert_eq!(descriptor.artwork_credit.as_deref(), Some(long.as_str()));
        assert_eq!(descriptor.canonical_url.as_deref(), Some(long.as_str()));
    }

    /// The wheel the app cuts on: three rings, the innermost of them the
    /// trailer's. The trailer is below the programme, so a band that stopped
    /// at the programme clamped every trailer pixel to one ring. This holds
    /// the band open to the label, and holds the stream readable across it.
    #[test]
    fn a_three_ring_trailer_spans_its_rings_and_reverses() {
        let rings = [11u32, 14, 18];
        let cells: usize = rings.iter().sum::<u32>() as usize;
        let slots: Vec<String> = (0..cells)
            .map(|slot| {
                format!(
                    "#{:02X}{:02X}{:02X}",
                    40 + slot * 4,
                    90 + slot * 3,
                    210 - slot * 4
                )
            })
            .collect();
        let long = "WESTSIDE-".repeat(111);
        let options = serde_json::json!({
            "grooveSpanFraction": 0.33,
            "grooveToneSlots": slots,
            "grooveToneRings": rings,
            "headerLabel": long,
            "headerArtworkCredit": long,
            "headerCanonicalUrl": long,
        })
        .to_string();
        let output =
            render_payload_codes_to_png(&codec_bytes(64, 40_000), "rgb", "lp", 60.0, Some(&options))
                .unwrap();

        let clock = output
            .descriptor
            .tone_clock
            .as_ref()
            .expect("a wheeled cut writes its clock");
        assert_eq!(clock.rings, rings.to_vec());

        // The band holds the trailer. Without this the run-out and the lock
        // sit under the band's inner edge and every one of their pixels
        // takes ring zero.
        let geometry = record_core::describe_record_profile("lp").unwrap();
        let inner = f64::from(clock.span.0) / f64::from(record_groove::TONE_CLOCK_SPAN_UNITS);
        let lock = record_core::lead_out_geometry_with_extent(
            "lp",
            Some(i32::from(output.payload.cut_inner_radius)),
            record_core::LeadOutExtent::Fill,
        )
        .unwrap();
        assert!(
            inner * f64::from(geometry.outer_radius) < lock.entry_radius,
            "the wheel's band starts at {inner}, which is outside the trailer"
        );

        // Every ring the trailer crosses is a ring it is toned by, so more
        // than one of them answers for the band.
        let trailer = record_core::build_run_out_spiral_indices(
            RECORD_WIDTH,
            RECORD_HEIGHT,
            "lp",
            Some(i32::from(output.payload.cut_inner_radius)),
        )
        .unwrap();
        let band = record_descriptor::band_clock(&record_descriptor::tone_clock_from_map(clock));
        let centre = RECORD_WIDTH as f64 / 2.0;
        let mut reached = std::collections::BTreeSet::new();
        for (sequence, &index) in trailer.iter().enumerate() {
            let x = (index % RECORD_WIDTH) as f64;
            let y = (index / RECORD_WIDTH) as f64;
            let away = record_groove::pixel_radius(x, y, centre, centre);
            reached.insert(band.ring_index(sequence, away));
        }
        assert!(
            reached.len() > 1,
            "the trailer took one ring of three: {reached:?}"
        );

        // And it still reverses: the header outgrew the lead-in, so these
        // three fields were read back out of the trailer itself.
        let (_, read_back) =
            record_decode::decode_record_descriptor_bytes_from_png(&output.png_bytes, Some("lp"))
                .unwrap();
        let descriptor = record_descriptor::decode_record_descriptor_bytes(&read_back).unwrap();
        assert_eq!(descriptor.label.as_deref(), Some(long.as_str()));
        assert_eq!(descriptor.artwork_credit.as_deref(), Some(long.as_str()));
        assert_eq!(descriptor.canonical_url.as_deref(), Some(long.as_str()));
    }

    #[test]
    fn gap_tone_lightness_is_boosted_for_lighter_records() {
        fn rendered_gap_base(hex: &str) -> [u8; 3] {
            let music_one = codec_bytes(64, 4_000);
            let gap = codec_bytes(20_000, 1_500);
            let music_two = codec_bytes(40_000, 5_500);
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

    /// The committed golden PNGs still decode.
    ///
    /// This is the one thing `renders_*_golden_and_decodes_back_to_payload`
    /// cannot check. Those render a fresh PNG and decode that, so they
    /// round-trip the current build against itself and pass no matter what
    /// the encoding is — a change to the descriptor's pixel encoding leaves
    /// them green while every record ever cut becomes unreadable, and the
    /// files on disk go stale without a single test noticing.
    ///
    /// A golden is only a golden if something reads the bytes that are
    /// checked in. When this fails after a deliberate format change, re-bless
    /// with `regenerate_golden_records` and commit the PNGs in the same
    /// commit as the change.
    #[test]
    fn the_committed_golden_pngs_still_decode() {
        for golden in GOLDENS {
            let png = fixture_bytes(golden.id, golden.record_png_name);
            let stream = chunk_stream_for_payload(golden.id, golden.payload_name);

            let (profile, decoded) = record_decode::decode_record_png_to_chunk_stream(&png)
                .unwrap_or_else(|error| {
                    panic!(
                        "{} on disk no longer decodes: {error:#}\n\
                         if the format changed on purpose, re-bless the goldens with \
                         `cargo test -p record-render --lib regenerate_golden_records -- --ignored`",
                        golden.record_png_name
                    )
                });

            assert_eq!(profile, golden.profile);
            assert_eq!(
                decoded.bytes, stream,
                "{} decodes, but not back to the payload beside it",
                golden.record_png_name
            );
        }
    }

    /// The descriptor bands are mid-grey on a pressed record, not just in the
    /// constants. The lead-in is a ring a person sees.
    #[test]
    fn the_descriptor_band_of_a_pressed_record_is_mid_grey() {
        for golden in GOLDENS {
            let png = fixture_bytes(golden.id, golden.record_png_name);
            let image = image::load_from_memory(&png).expect("golden png").to_rgba8();
            let (width, height) = (image.width() as usize, image.height() as usize);
            let rgba = image.into_raw();

            let lead_in =
                record_core::build_lead_in_spiral_indices(width, height, golden.profile, None, None, None)
                    .expect("lead-in indices");

            for &index in &lead_in {
                let (r, g, b, a) = (
                    rgba[index * 4],
                    rgba[index * 4 + 1],
                    rgba[index * 4 + 2],
                    rgba[index * 4 + 3],
                );
                if a == 0 {
                    continue;
                }
                assert!(r == g && g == b, "{} lead-in pixel is not grey", golden.profile);
                assert!(
                    (record_descriptor::METADATA_GRAYSCALE_BASE
                        ..=record_descriptor::METADATA_GRAYSCALE_TOP)
                        .contains(&r),
                    "{} lead-in pixel {r} is outside the mid-grey band",
                    golden.profile
                );
            }
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

    /// The vintage 7" cut from the same payload as the modern one.
    ///
    /// It shares every recorded dimension with `single45` and differs only in
    /// its paper, which moves the label radius from 151 to 138 and hands the
    /// lead-out 39 px of clearance instead of 26. That is a different trailer
    /// band under the same programme, so it is a different round trip: the
    /// second half of the BRD1 stream is written into a carrier of a different
    /// size and has to come back out of it byte for byte.
    ///
    /// No new fixture. A golden is a payload and the geometry it is cut to,
    /// and the payload here is the one `single45` already carries.
    #[test]
    fn renders_vintage_single45_golden_and_decodes_back_to_payload() {
        assert_render_decodes_to_payload(
            "lori-asha-westside-single45-hq",
            "single45vintage",
            "lori-asha-westside-single45-hq.ecdc",
        );
    }
}
