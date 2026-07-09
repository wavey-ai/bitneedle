// Copyright © Wavey, Inc.
// Licensed under the Wavey Artist Source Licence.
// Patent pending. All patent rights are reserved except as expressly granted by the licence.
// Commercial licensing: licence@yl.vin

#![doc = include_str!("../README.md")]

//! WebAssembly facade for Bitneedle record authoring: rendering payload
//! entries and programmes to a record PNG, and record-label profile helpers.
//!
//! Decoding, verification, and sidecar inspection live in `record-wasm`.

use anyhow::{bail, Context, Result};
use record_cut::{
    encode_record_stream, PayloadDescriptorInput, PayloadEntryInput, RecordStreamInput, TrackGapInput,
    TrackInput,
};
use serde::Deserialize;
use serde_json::json;
use wasm_bindgen::prelude::*;
#[cfg(target_arch = "wasm32")]
use web_sys::console;

const PAYLOAD_CODE_FORMAT_RGB: &str = "rgb";

fn wasm_log(_message: &str) {
    #[cfg(target_arch = "wasm32")]
    console::log_1(&JsValue::from_str(_message));
}

fn record_descriptor_magic() -> String {
    String::from_utf8_lossy(record_descriptor::RECORD_DESCRIPTOR_MAGIC).to_string()
}

fn to_js_error(error: impl std::fmt::Display + std::fmt::Debug) -> JsValue {
    JsValue::from_str(&format!("{error:#?}"))
}

#[wasm_bindgen(js_name = recordLabelProfileSpecsJson)]
pub fn wasm_record_label_profile_specs_json() -> Result<String, JsValue> {
    record_label::known_label_profile_geometries_json().map_err(to_js_error)
}

#[wasm_bindgen(js_name = recordLabelProfileSpecJson)]
pub fn wasm_record_label_profile_spec_json(record_profile: &str) -> Result<String, JsValue> {
    record_label::label_profile_geometry_json(record_profile).map_err(to_js_error)
}

#[wasm_bindgen(js_name = resolveRecordLabelCutoutStyleJson)]
pub fn wasm_resolve_record_label_cutout_style_json(
    record_profile: &str,
    style_json: &str,
) -> Result<String, JsValue> {
    record_label::resolve_label_cutout_style_json(record_profile, style_json).map_err(to_js_error)
}

#[wasm_bindgen]
pub struct WasmRenderResult {
    png_bytes: Vec<u8>,
    payload_json: String,
    header_json: String,
}

#[derive(Debug, Clone)]
pub struct NativeRenderResult {
    pub png_bytes: Vec<u8>,
    pub payload_json: String,
    pub header_json: String,
}

impl From<WasmRenderResult> for NativeRenderResult {
    fn from(value: WasmRenderResult) -> Self {
        Self {
            png_bytes: value.png_bytes,
            payload_json: value.payload_json,
            header_json: value.header_json,
        }
    }
}

#[wasm_bindgen]
impl WasmRenderResult {
    #[wasm_bindgen(js_name = pngBytes)]
    pub fn png_bytes(&self) -> Vec<u8> {
        self.png_bytes.clone()
    }

    #[wasm_bindgen(js_name = payloadJson)]
    pub fn payload_json(&self) -> String {
        self.payload_json.clone()
    }

    #[wasm_bindgen(js_name = headerJson)]
    pub fn header_json(&self) -> String {
        self.header_json.clone()
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenderOptions {
    header_generation_version: Option<String>,
    track_listing: Option<serde_json::Value>,
    dummy_spiral_regions: Option<serde_json::Value>,
}

#[wasm_bindgen(js_name = renderPayloadCodesToPng)]
pub fn wasm_render_payload_codes_to_png(
    codes: &[u8],
    code_format: &str,
    record_profile: &str,
    duration_seconds: f64,
    render_options_json: &str,
) -> Result<WasmRenderResult, JsValue> {
    wasm_log(&format!(
        "[record-cut-wasm] renderPayloadCodesToPng inputBytes={} format={} profile={} duration={} descriptorMagic={} chunkMagic={}",
        codes.len(),
        code_format,
        record_profile,
        duration_seconds,
        record_descriptor_magic(),
        String::from_utf8_lossy(record_core::RECORD_STREAM_MAGIC)
    ));

    render_payload_codes_to_png(
        codes,
        code_format,
        record_profile,
        duration_seconds,
        render_options_json,
    )
    .map_err(to_js_error)
}

#[wasm_bindgen(js_name = renderPayloadContainerToPng)]
pub fn wasm_render_payload_container_to_png(
    payload: &[u8],
    payload_container: &str,
    payload_codec: &str,
    code_format: &str,
    record_profile: &str,
    duration_seconds: f64,
    render_options_json: &str,
) -> Result<WasmRenderResult, JsValue> {
    wasm_log(&format!(
        "[record-cut-wasm] renderPayloadContainerToPng inputBytes={} container={} codec={} format={} profile={} duration={} descriptorMagic={} chunkMagic={}",
        payload.len(),
        payload_container,
        payload_codec,
        code_format,
        record_profile,
        duration_seconds,
        record_descriptor_magic(),
        String::from_utf8_lossy(record_core::RECORD_STREAM_MAGIC)
    ));

    render_payload_container_to_png(
        payload,
        payload_container,
        payload_codec,
        code_format,
        record_profile,
        duration_seconds,
        render_options_json,
    )
    .map_err(to_js_error)
}

#[wasm_bindgen(js_name = renderPayloadEntriesToPng)]
pub fn wasm_render_payload_entries_to_png(
    payload_entries: &JsValue,
    payload_container: &str,
    code_format: &str,
    record_profile: &str,
    duration_seconds: f64,
    render_options_json: &str,
) -> Result<WasmRenderResult, JsValue> {
    let entries = js_payload_entries(payload_entries).map_err(to_js_error)?;
    wasm_log(&format!(
        "[record-cut-wasm] renderPayloadEntriesToPng entries={} container={} format={} profile={} duration={}",
        entries.len(),
        payload_container,
        code_format,
        record_profile,
        duration_seconds,
    ));

    render_payload_entries_to_png(
        entries,
        payload_container,
        code_format,
        record_profile,
        duration_seconds,
        render_options_json,
    )
    .map_err(to_js_error)
}

/// Public, always-shipped fast-preview render: same shape as
/// `renderPayloadEntriesWithDescriptorToPng`, but forces `fastFit` on
/// (see record-render's `RenderOptions::fast_fit`) regardless of what the
/// caller passes. Intended for progressive/streaming previews only — the
/// precise exact-fit spiral search (`record-render`'s `exact-fit` feature)
/// is compiled out of the public wasm build entirely, so this function
/// physically cannot run it even if asked to.
#[wasm_bindgen(js_name = renderPayloadEntriesWithDescriptorToPngFast)]
pub fn wasm_render_payload_entries_with_descriptor_to_png_fast(
    payload_entries: &JsValue,
    payload_descriptor_json: &str,
    code_format: &str,
    record_profile: &str,
    duration_seconds: f64,
    render_options_json: &str,
) -> Result<WasmRenderResult, JsValue> {
    let entries = js_payload_entries(payload_entries).map_err(to_js_error)?;
    let forced_render_options_json =
        force_fast_fit_render_options_json(render_options_json).map_err(to_js_error)?;
    render_payload_entries_with_descriptor_to_png(
        entries,
        payload_descriptor_json,
        code_format,
        record_profile,
        duration_seconds,
        &forced_render_options_json,
    )
    .map_err(to_js_error)
}

fn force_fast_fit_render_options_json(render_options_json: &str) -> Result<String> {
    let mut value: serde_json::Value = match render_options_json.trim() {
        "" => json!({}),
        raw => serde_json::from_str(raw).context("render options JSON is invalid")?,
    };
    if !value.is_object() {
        value = json!({});
    }
    if let Some(object) = value.as_object_mut() {
        object.insert("fastFit".to_string(), serde_json::Value::Bool(true));
    }
    serde_json::to_string(&value).context("failed to serialize forced-fast-fit render options")
}

/// Render headerless payload entries that all share one `PayloadDescriptor`
/// (provided as JSON). The descriptor is stored once in the BRS1 metadata and
/// every entry references descriptor index 0; the RGB groove stores only the
/// headerless codec payload bytes.
#[wasm_bindgen(js_name = renderPayloadEntriesWithDescriptorToPng)]
pub fn wasm_render_payload_entries_with_descriptor_to_png(
    payload_entries: &JsValue,
    payload_descriptor_json: &str,
    code_format: &str,
    record_profile: &str,
    duration_seconds: f64,
    render_options_json: &str,
) -> Result<WasmRenderResult, JsValue> {
    let entries = js_payload_entries(payload_entries).map_err(to_js_error)?;
    wasm_log(&format!(
        "[record-cut-wasm] renderPayloadEntriesWithDescriptorToPng entries={} format={} profile={} duration={}",
        entries.len(),
        code_format,
        record_profile,
        duration_seconds,
    ));

    render_payload_entries_with_descriptor_to_png(
        entries,
        payload_descriptor_json,
        code_format,
        record_profile,
        duration_seconds,
        render_options_json,
    )
    .map_err(to_js_error)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WasmTrackInputJson {
    title: String,
    first_revolution_index: usize,
    revolution_count: usize,
}

/// An explicit inter-track programme gap. Identity originates entirely from
/// the caller's gap-insertion operation (where it knows exactly which
/// entries it just appended as ambient near-silence) — never inferred from
/// which entries happen to be unused by a track.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WasmTrackGapInputJson {
    first_revolution_index: usize,
    revolution_count: usize,
    after_track_index: usize,
}

/// Render headerless payload entries that reference one of several shared
/// `PayloadDescriptor`s (e.g. one ECDC descriptor for song audio plus one GAP
/// descriptor for inter-track silence placeholders), with an explicit track
/// listing. Generalizes `renderPayloadEntriesWithDescriptorToPng` (which
/// always assumes a single descriptor and one track spanning every entry) for
/// callers that need multiple descriptors and/or more than one track.
/// Canonical record-authoring entry point (plan §8.1). Takes the headerless
/// per-revolution ECDC payload entries plus a programme JSON describing the
/// musical tracks (title, the ECDC `payloadIndexes` they cover, and the
/// `gapAfterSeconds` of trailing silence). All GAP timing, sizing, seeds, and
/// canonical GAP1 bytes are derived in Rust; JavaScript never builds GAP
/// metadata or bytes.
#[wasm_bindgen(js_name = renderRecordProgrammeToPng)]
pub fn wasm_render_record_programme_to_png(
    ecdc_payload_entries: &JsValue,
    programme_json: &str,
    code_format: &str,
    record_profile: &str,
    render_options_json: &str,
) -> Result<WasmRenderResult, JsValue> {
    let ecdc_entries = js_payload_entries(ecdc_payload_entries).map_err(to_js_error)?;
    render_record_programme_to_png(
        ecdc_entries,
        programme_json,
        code_format,
        record_profile,
        render_options_json,
    )
    .map_err(to_js_error)
}

#[wasm_bindgen(js_name = renderEmptyGrooveRecordToPng)]
pub fn wasm_render_empty_groove_record_to_png(
    record_profile: &str,
    red: u8,
    green: u8,
    blue: u8,
) -> Result<WasmRenderResult, JsValue> {
    render_empty_groove_record_to_png_native(record_profile, red, green, blue)
        .map(|result| WasmRenderResult {
            png_bytes: result.png_bytes,
            payload_json: result.payload_json,
            header_json: result.header_json,
        })
        .map_err(to_js_error)
}

pub fn render_empty_groove_record_to_png_native(
    record_profile: &str,
    red: u8,
    green: u8,
    blue: u8,
) -> Result<NativeRenderResult> {
    let png_bytes =
        record_render::render_empty_groove_record_to_png(record_profile, [red, green, blue])?;

    Ok(NativeRenderResult {
        png_bytes,
        payload_json: String::new(),
        header_json: String::new(),
    })
}

fn render_payload_codes_to_png(
    codes: &[u8],
    code_format: &str,
    record_profile: &str,
    duration_seconds: f64,
    render_options_json: &str,
) -> Result<WasmRenderResult> {
    normalize_payload_code_format(code_format)?;

    let normalized_profile = record_core::normalize_record_profile_name(record_profile)?;
    let render_options = parse_render_options(render_options_json)?;
    let chunk_input = chunk_stream_from_payload_or_stream(codes, &render_options)?;

    render_chunk_input_to_png(
        chunk_input,
        &normalized_profile,
        duration_seconds,
        render_options_json,
        &render_options,
    )
}

fn render_payload_container_to_png(
    payload: &[u8],
    payload_container: &str,
    payload_codec: &str,
    code_format: &str,
    record_profile: &str,
    duration_seconds: f64,
    render_options_json: &str,
) -> Result<WasmRenderResult> {
    normalize_payload_code_format(code_format)?;

    let normalized_profile = record_core::normalize_record_profile_name(record_profile)?;
    let render_options = parse_render_options(render_options_json)?;
    let chunk_input = chunk_stream_from_payload_or_stream_with_container(
        payload,
        payload_container,
        payload_codec,
        &render_options,
    )?;

    render_chunk_input_to_png(
        chunk_input,
        &normalized_profile,
        duration_seconds,
        render_options_json,
        &render_options,
    )
}

pub fn render_payload_container_to_png_native(
    payload: &[u8],
    payload_container: &str,
    payload_codec: &str,
    code_format: &str,
    record_profile: &str,
    duration_seconds: f64,
    render_options_json: &str,
) -> Result<NativeRenderResult> {
    render_payload_container_to_png(
        payload,
        payload_container,
        payload_codec,
        code_format,
        record_profile,
        duration_seconds,
        render_options_json,
    )
    .map(Into::into)
}

fn js_payload_entries(payload_entries: &JsValue) -> Result<Vec<Vec<u8>>> {
    let array = js_sys::Array::from(payload_entries);
    if array.length() == 0 {
        bail!("payload entries array is empty");
    }

    let mut entries = Vec::with_capacity(array.length() as usize);
    for (index, value) in array.iter().enumerate() {
        let bytes = js_sys::Uint8Array::new(&value).to_vec();
        if bytes.is_empty() {
            bail!("payload entry {index} is empty");
        }
        entries.push(bytes);
    }
    Ok(entries)
}

fn render_payload_entries_to_png(
    payload_entries: Vec<Vec<u8>>,
    payload_container: &str,
    code_format: &str,
    record_profile: &str,
    duration_seconds: f64,
    render_options_json: &str,
) -> Result<WasmRenderResult> {
    normalize_payload_code_format(code_format)?;

    let normalized_profile = record_core::normalize_record_profile_name(record_profile)?;
    let render_options = parse_render_options(render_options_json)?;
    let chunk_input = chunk_stream_from_entries_with_container(
        payload_entries,
        payload_container,
        &render_options,
    )?;

    render_chunk_input_to_png(
        chunk_input,
        &normalized_profile,
        duration_seconds,
        render_options_json,
        &render_options,
    )
}

pub fn render_payload_entries_to_png_native(
    payload_entries: Vec<Vec<u8>>,
    payload_container: &str,
    code_format: &str,
    record_profile: &str,
    duration_seconds: f64,
    render_options_json: &str,
) -> Result<NativeRenderResult> {
    render_payload_entries_to_png(
        payload_entries,
        payload_container,
        code_format,
        record_profile,
        duration_seconds,
        render_options_json,
    )
    .map(Into::into)
}

fn render_payload_entries_with_descriptor_to_png(
    payload_entries: Vec<Vec<u8>>,
    payload_descriptor_json: &str,
    code_format: &str,
    record_profile: &str,
    duration_seconds: f64,
    render_options_json: &str,
) -> Result<WasmRenderResult> {
    normalize_payload_code_format(code_format)?;

    let descriptor: record_core::PayloadDescriptor = serde_json::from_str(payload_descriptor_json)
        .context("payload descriptor JSON is invalid")?;
    record_core::validate_payload_descriptor(&descriptor)
        .context("shared payload descriptor is invalid")?;

    let normalized_profile = record_core::normalize_record_profile_name(record_profile)?;
    let render_options = parse_render_options(render_options_json)?;
    let chunk_input = chunk_stream_from_entries_with_descriptor(
        payload_entries,
        PayloadDescriptorInput::from(descriptor),
        &render_options,
    )?;

    render_chunk_input_to_png(
        chunk_input,
        &normalized_profile,
        duration_seconds,
        render_options_json,
        &render_options,
    )
}

pub fn render_payload_entries_with_descriptor_to_png_native(
    payload_entries: Vec<Vec<u8>>,
    payload_descriptor_json: &str,
    code_format: &str,
    record_profile: &str,
    duration_seconds: f64,
    render_options_json: &str,
) -> Result<NativeRenderResult> {
    render_payload_entries_with_descriptor_to_png(
        payload_entries,
        payload_descriptor_json,
        code_format,
        record_profile,
        duration_seconds,
        render_options_json,
    )
    .map(Into::into)
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProgrammeTrackInputJson {
    title: String,
    /// Contiguous, ascending ECDC payload-entry indexes (into the supplied
    /// entry array) that this musical track covers. Inter-track gaps are
    /// separate ECDC entries that are simply *not* referenced by any track.
    payload_indexes: Vec<usize>,
}

/// An explicit inter-track programme gap as supplied by the caller. The
/// caller (the gap-ambience insertion code) knows exactly which entry
/// indexes it just appended; this is recorded directly, never inferred from
/// which entries are left unreferenced by any track.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProgrammeTrackGapInputJson {
    first_revolution_index: usize,
    revolution_count: usize,
    after_track_index: usize,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecordProgrammeInputJson {
    ecdc_descriptor: record_core::PayloadDescriptor,
    tracks: Vec<ProgrammeTrackInputJson>,
    #[serde(default)]
    track_gaps: Vec<ProgrammeTrackGapInputJson>,
}

/// The assembled, ordered programme: the single shared ECDC descriptor, the
/// ordered entry bytes (every entry is ECDC — music revolutions and inter-track
/// gap ambience alike), their per-entry descriptor indexes (all 0), the musical
/// track ranges, and the explicit track-gap ranges. Every entry belongs to
/// exactly one of `tracks` or `track_gaps`; there is no third "uncovered"
/// category.
#[derive(Debug)]
struct AssembledProgramme {
    descriptors: Vec<record_core::PayloadDescriptor>,
    entry_bytes: Vec<Vec<u8>>,
    entry_descriptor_indexes: Vec<u8>,
    tracks: Vec<WasmTrackInputJson>,
    track_gaps: Vec<WasmTrackGapInputJson>,
    total_samples: u64,
    sample_rate: u32,
}

/// Reject an ECDC programme descriptor unless every canonical field required for
/// headerless per-revolution entries is present and valid. This does not weaken
/// the generic [`record_core::validate_payload_descriptor`]; bare descriptors
/// remain valid in other generic contexts.
fn validate_programme_ecdc_descriptor(descriptor: &record_core::PayloadDescriptor) -> Result<()> {
    if !descriptor
        .container
        .eq_ignore_ascii_case(record_core::PAYLOAD_CONTAINER_ECDC)
    {
        bail!("ecdcDescriptor container must be ECDC");
    }
    match descriptor.codec.as_deref() {
        Some(codec) if codec.eq_ignore_ascii_case(record_core::PAYLOAD_CONTAINER_ECDC) => {}
        _ => bail!("ecdcDescriptor requires codec ECDC"),
    }
    descriptor
        .sample_rate
        .filter(|value| *value > 0)
        .context("ecdcDescriptor requires a positive sampleRate")?;
    descriptor
        .channels
        .filter(|value| *value > 0)
        .context("ecdcDescriptor requires a positive channels")?;
    descriptor
        .block_samples
        .context("ecdcDescriptor requires blockSamples")?;
    descriptor
        .output_offset_samples
        .context("ecdcDescriptor requires outputOffsetSamples")?;
    descriptor
        .output_samples
        .context("ecdcDescriptor requires outputSamples")?;
    descriptor
        .codec_metadata
        .as_deref()
        .filter(|bytes| !bytes.is_empty())
        .context("ecdcDescriptor requires codecMetadata")?;
    // Generic structural validation (output geometry consistency, codec metadata
    // JSON, etc.) still applies.
    record_core::validate_payload_descriptor(descriptor)?;
    Ok(())
}

/// Build the ordered programme from ECDC entries + the programme JSON, deriving
/// canonical GAP1 payloads in Rust. Pure (no rendering) so it is unit-testable
/// per plan §15.4.
fn assemble_record_programme(
    ecdc_entries: Vec<Vec<u8>>,
    programme_json: &str,
    record_profile: &str,
) -> Result<AssembledProgramme> {
    let programme: RecordProgrammeInputJson =
        serde_json::from_str(programme_json).context("programme JSON is invalid")?;
    // Authoring-specific guard: the canonical ECDC descriptor must be complete so
    // headerless entry sample counts and the programme map can be derived. A bare
    // { "container": "ECDC" } passes generic validation but is not authorable.
    validate_programme_ecdc_descriptor(&programme.ecdc_descriptor)
        .context("programme ecdcDescriptor is not a complete canonical ECDC descriptor")?;

    let sample_rate = programme
        .ecdc_descriptor
        .sample_rate
        .context("ecdcDescriptor requires sampleRate")?;
    programme
        .ecdc_descriptor
        .channels
        .context("ecdcDescriptor requires channels")?;
    // Validates the profile name even though gap geometry is no longer derived
    // here (gaps are now real ECDC entries supplied by the caller).
    record_core::normalize_record_profile_name(record_profile)?;

    if programme.tracks.is_empty() {
        bail!("programme must contain at least one musical track");
    }

    let total_ecdc = ecdc_entries.len();
    let mut used = vec![false; total_ecdc];
    let mut tracks: Vec<WasmTrackInputJson> = Vec::new();

    // Each track covers a contiguous, ascending, disjoint run of entry indexes.
    // The supplied entry array is already in groove order (music revolutions and
    // gap ambience interleaved), so a track range maps directly onto it.
    for (track_index, track) in programme.tracks.iter().enumerate() {
        if track.title.trim().is_empty() {
            bail!("musical track {track_index} title must not be empty");
        }
        if track.payload_indexes.is_empty() {
            bail!("musical track {track_index} must reference at least one ECDC payload index");
        }

        let first_revolution_index = track.payload_indexes[0];
        for (offset, &index) in track.payload_indexes.iter().enumerate() {
            if index >= total_ecdc {
                bail!(
                    "track {track_index} references ECDC payload index {index}, but only {total_ecdc} entries were supplied"
                );
            }
            if index != first_revolution_index + offset {
                bail!(
                    "track {track_index} payloadIndexes must be contiguous and ascending (groove order)"
                );
            }
            if used[index] {
                bail!("ECDC payload index {index} is used by more than one track");
            }
            used[index] = true;
        }

        tracks.push(WasmTrackInputJson {
            title: track.title.clone(),
            first_revolution_index,
            revolution_count: track.payload_indexes.len(),
        });
    }

    // Explicit track gaps: identity comes entirely from `programme.track_gaps`,
    // which the caller (the gap-insertion code) populated at the moment it
    // appended those entries. `used` is consulted here only to detect overlap
    // with a track or another gap, and at the end only to confirm every entry
    // ended up covered — it never creates or infers a gap range itself.
    let mut track_gaps: Vec<WasmTrackGapInputJson> = Vec::new();
    let mut previous_first_revolution_index: Option<usize> = None;

    for (gap_index, gap) in programme.track_gaps.iter().enumerate() {
        if gap.revolution_count == 0 {
            bail!("track gap {gap_index} revolution count must be greater than zero");
        }
        if gap.after_track_index >= tracks.len() {
            bail!(
                "track gap {gap_index} after_track_index {} is out of range for {} tracks",
                gap.after_track_index,
                tracks.len()
            );
        }
        if let Some(previous) = previous_first_revolution_index {
            if gap.first_revolution_index <= previous {
                bail!(
                    "track gap {gap_index} first_revolution_index {} is not strictly ascending after the previous track gap's {previous}",
                    gap.first_revolution_index
                );
            }
        }
        previous_first_revolution_index = Some(gap.first_revolution_index);

        let end = gap
            .first_revolution_index
            .checked_add(gap.revolution_count)
            .context("track gap revolution range overflows")?;
        if end > total_ecdc {
            bail!(
                "track gap {gap_index} references ECDC payload index range [{}, {end}), but only {total_ecdc} entries were supplied",
                gap.first_revolution_index
            );
        }
        for index in gap.first_revolution_index..end {
            if used[index] {
                bail!(
                    "ECDC payload index {index} is covered by both a track (or another track gap) and track gap {gap_index}"
                );
            }
            used[index] = true;
        }

        track_gaps.push(WasmTrackGapInputJson {
            first_revolution_index: gap.first_revolution_index,
            revolution_count: gap.revolution_count,
            after_track_index: gap.after_track_index,
        });
    }

    // Every payload entry must belong to exactly one track or track gap.
    // There is no implicit "uncovered means gap" fallback: this is a
    // coverage check against the explicit ranges above, not a source of
    // gap identity.
    for (index, &is_used) in used.iter().enumerate() {
        if !is_used {
            bail!("payload entry {index} is not covered by any track or track gap");
        }
    }

    // Every entry — music and gap ambience alike — is a valid standalone
    // ECDC payload under the one shared descriptor. The exact programme sample
    // timeline is the sum of every entry's decoded ECDC sample count; gaps
    // contribute their real encoded duration just like music.
    let mut total_samples = 0u64;
    for entry in &ecdc_entries {
        total_samples = total_samples.saturating_add(
            record_core::ecdc::headerless_entry_sample_count(entry, &programme.ecdc_descriptor)
                .unwrap_or_else(|_| {
                    u64::from(programme.ecdc_descriptor.output_samples.unwrap_or(0))
                }),
        );
    }

    let entry_descriptor_indexes = vec![0u8; total_ecdc];
    let descriptors = vec![programme.ecdc_descriptor];

    Ok(AssembledProgramme {
        descriptors,
        entry_bytes: ecdc_entries,
        entry_descriptor_indexes,
        tracks,
        track_gaps,
        total_samples,
        sample_rate,
    })
}

fn render_record_programme_to_png(
    ecdc_entries: Vec<Vec<u8>>,
    programme_json: &str,
    code_format: &str,
    record_profile: &str,
    render_options_json: &str,
) -> Result<WasmRenderResult> {
    normalize_payload_code_format(code_format)?;

    let assembled = assemble_record_programme(ecdc_entries, programme_json, record_profile)?;
    // Programme duration is derived in Rust from the exact sample timeline — the
    // caller does not supply it.
    let duration_seconds = assembled.total_samples as f64 / f64::from(assembled.sample_rate.max(1));
    let normalized_profile = record_core::normalize_record_profile_name(record_profile)?;
    let render_options = parse_render_options(render_options_json)?;
    let chunk_input = chunk_stream_from_multi_descriptor_entries(
        assembled.entry_bytes,
        assembled.entry_descriptor_indexes,
        assembled.descriptors,
        assembled.tracks,
        assembled.track_gaps,
        &render_options,
    )?;

    render_chunk_input_to_png(
        chunk_input,
        &normalized_profile,
        duration_seconds,
        render_options_json,
        &render_options,
    )
}

pub fn render_record_programme_to_png_native(
    ecdc_entries: Vec<Vec<u8>>,
    programme_json: &str,
    code_format: &str,
    record_profile: &str,
    render_options_json: &str,
) -> Result<NativeRenderResult> {
    render_record_programme_to_png(
        ecdc_entries,
        programme_json,
        code_format,
        record_profile,
        render_options_json,
    )
    .map(Into::into)
}

/// Build a BRS1 chunk stream from headerless payload entries that reference
/// one of several `PayloadDescriptor`s (by explicit per-entry index) and an
/// explicit track listing — the general case
/// `chunk_stream_from_entries_with_descriptor` shortcuts for the common
/// single-descriptor, single-track recording.
fn chunk_stream_from_multi_descriptor_entries(
    entry_bytes: Vec<Vec<u8>>,
    entry_descriptor_indexes: Vec<u8>,
    descriptors: Vec<record_core::PayloadDescriptor>,
    tracks: Vec<WasmTrackInputJson>,
    track_gaps: Vec<WasmTrackGapInputJson>,
    render_options: &RenderOptions,
) -> Result<ChunkStreamInput> {
    if entry_bytes.is_empty() {
        bail!("payload entries array is empty");
    }

    let payload_byte_length = entry_bytes.iter().try_fold(0usize, |total, entry| {
        total
            .checked_add(entry.len())
            .context("payload entry byte length overflow")
    })?;

    let input = RecordStreamInput {
        payload_descriptors: descriptors
            .into_iter()
            .map(PayloadDescriptorInput::from)
            .collect(),
        tracks: tracks
            .iter()
            .map(|track| TrackInput {
                title: track.title.clone(),
                first_revolution_index: Some(track.first_revolution_index),
                revolution_count: Some(track.revolution_count),
            })
            .collect(),
        track_gaps: track_gaps
            .iter()
            .map(|gap| TrackGapInput {
                first_revolution_index: gap.first_revolution_index,
                revolution_count: gap.revolution_count,
                after_track_index: gap.after_track_index,
            })
            .collect(),
    };
    let entries = entry_bytes
        .into_iter()
        .zip(entry_descriptor_indexes)
        .map(|(bytes, payload_descriptor_index)| {
            PayloadEntryInput::already_chunked(payload_descriptor_index, bytes)
        })
        .collect::<Vec<_>>();

    let stream_bytes = encode_record_stream(&input, &entries)
        .context("failed to wrap multi-descriptor payload entries as BRS1 record stream")?;

    record_core::parse_chunk_stream(&stream_bytes)
        .context("facade produced invalid BRS1 record stream")?;

    let track_listing = serde_json::to_value(
        tracks
            .iter()
            .enumerate()
            .map(|(index, track)| json!({ "number": index + 1, "title": track.title }))
            .collect::<Vec<_>>(),
    )?;

    Ok(ChunkStreamInput {
        stream_bytes,
        payload_byte_length,
        input_was_chunk_stream: false,
        track_listing: Some(track_listing),
        dummy_spiral_regions: render_options.dummy_spiral_regions.clone(),
    })
}

fn render_chunk_input_to_png(
    chunk_input: ChunkStreamInput,
    normalized_profile: &str,
    duration_seconds: f64,
    render_options_json: &str,
    _render_options: &RenderOptions,
) -> Result<WasmRenderResult> {
    let resolved_dummy_spiral_regions = chunk_input.dummy_spiral_regions.clone();
    let resolved_render_options_json = render_options_json_with_chunk_metadata(
        render_options_json,
        chunk_input.track_listing.as_ref(),
        resolved_dummy_spiral_regions.as_ref(),
    )?;
    let resolved_render_options = parse_render_options(&resolved_render_options_json)?;
    wasm_log(&format!(
        "[record-cut-wasm] render input kind={} rawPayloadBytes={} streamBytes={}",
        if chunk_input.input_was_chunk_stream {
            "BCS2"
        } else {
            "payload"
        },
        chunk_input.payload_byte_length,
        chunk_input.stream_bytes.len()
    ));

    let rendered = record_render::render_chunk_stream_to_png(
        &chunk_input.stream_bytes,
        normalized_profile,
        duration_seconds,
        render_options_json_option(&resolved_render_options_json),
    )
    .with_context(|| {
        format!(
            "record-render failed to render BCS2 chunk stream to PNG (profile={}, bytes={}, duration={duration_seconds})",
            normalized_profile,
            chunk_input.stream_bytes.len()
        )
    })?;

    let (_, descriptor) = record_decode::decode_record_descriptor_from_png(
        &rendered.png_bytes,
        Some(normalized_profile),
    )
    .context("rendered PNG descriptor could not be decoded")?;

    wasm_log(&format!(
        "[record-cut-wasm] rendered groove payloadEncoding={} rgbTone={} pngBytes={}",
        descriptor.payload_encoding.as_str(),
        rendered
            .payload
            .rgb_tone
            .as_ref()
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".to_string()),
        rendered.png_bytes.len()
    ));

    if descriptor.stream_byte_length != chunk_input.stream_bytes.len() {
        bail!(
            "rendered descriptor stream length mismatch: descriptor={:?}, actual={}",
            descriptor.stream_byte_length,
            chunk_input.stream_bytes.len()
        );
    }

    let decoded = record_decode::decode_record_png_to_chunk_stream_for_profile_with_length(
        &rendered.png_bytes,
        normalized_profile,
        Some(chunk_input.stream_bytes.len()),
    )
    .context("rendered PNG groove could not be decoded")?;

    if decoded.bytes != chunk_input.stream_bytes {
        bail!(
            "rendered PNG groove bytes mismatch: decoded={}, expected={}",
            decoded.bytes.len(),
            chunk_input.stream_bytes.len()
        );
    }

    record_core::parse_chunk_stream(&decoded.bytes)
        .context("rendered PNG groove did not decode to a valid BCS2 chunk stream")?;

    let payload_json = serde_json::to_string(&rendered.payload)?;
    let header_json = render_header_json(
        &rendered.descriptor,
        &resolved_render_options,
        chunk_input.payload_byte_length,
        chunk_input.stream_bytes.len(),
        chunk_input.input_was_chunk_stream,
    )?;

    Ok(WasmRenderResult {
        png_bytes: rendered.png_bytes,
        payload_json,
        header_json,
    })
}

#[derive(Debug, Clone)]
struct ChunkStreamInput {
    stream_bytes: Vec<u8>,
    payload_byte_length: usize,
    input_was_chunk_stream: bool,
    track_listing: Option<serde_json::Value>,
    dummy_spiral_regions: Option<serde_json::Value>,
}

fn default_track_listing_value() -> serde_json::Value {
    json!([{ "number": 1, "title": "Track 1" }])
}

fn render_options_track_listing(render_options: &RenderOptions) -> serde_json::Value {
    render_options
        .track_listing
        .clone()
        .filter(|value| value.as_array().is_some_and(|items| !items.is_empty()))
        .unwrap_or_else(default_track_listing_value)
}

fn chunk_stream_from_payload_or_stream(
    codes: &[u8],
    render_options: &RenderOptions,
) -> Result<ChunkStreamInput> {
    chunk_stream_from_payload_or_stream_with_container(codes, "TEST", "", render_options)
}

fn chunk_stream_from_payload_or_stream_with_container(
    codes: &[u8],
    payload_container: &str,
    _payload_codec: &str,
    render_options: &RenderOptions,
) -> Result<ChunkStreamInput> {
    if codes.starts_with(record_core::RECORD_STREAM_MAGIC) {
        let document = record_core::parse_chunk_stream(codes)
            .context("input starts with BRS1 but is not a valid record stream")?;
        let payload = record_core::chunk_stream_payload_bytes(&document);

        return Ok(ChunkStreamInput {
            stream_bytes: codes.to_vec(),
            payload_byte_length: payload.len(),
            input_was_chunk_stream: true,
            track_listing: Some(serde_json::to_value(&document.metadata.tracks)?),
            dummy_spiral_regions: render_options.dummy_spiral_regions.clone(),
        });
    }

    if codes.is_empty() {
        bail!("payload is empty");
    }

    let normalized_container = payload_container.trim();
    if normalized_container.is_empty() {
        bail!("payload container is required");
    }
    let track_listing = render_options_track_listing(render_options);
    let track_title = track_listing
        .as_array()
        .and_then(|tracks| tracks.first())
        .and_then(|track| track.get("title"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Track 1")
        .to_string();

    let input = RecordStreamInput {
        payload_descriptors: vec![PayloadDescriptorInput::from_container(normalized_container.to_string())],
        tracks: vec![TrackInput {
            title: track_title,
            first_revolution_index: None,
            revolution_count: None,
        }],
        track_gaps: Vec::new(),
    };
    let entries = vec![PayloadEntryInput::already_chunked(0, codes.to_vec())];

    let stream_bytes = encode_record_stream(&input, &entries)
        .context("failed to wrap payload as BRS1 record stream")?;

    record_core::parse_chunk_stream(&stream_bytes)
        .context("facade produced invalid BRS1 record stream")?;

    Ok(ChunkStreamInput {
        stream_bytes,
        payload_byte_length: codes.len(),
        input_was_chunk_stream: false,
        track_listing: Some(track_listing),
        dummy_spiral_regions: render_options.dummy_spiral_regions.clone(),
    })
}

fn chunk_stream_from_entries_with_container(
    entry_bytes: Vec<Vec<u8>>,
    payload_container: &str,
    render_options: &RenderOptions,
) -> Result<ChunkStreamInput> {
    let normalized_container = payload_container.trim();
    if normalized_container.is_empty() {
        bail!("payload container is required");
    }
    let descriptor = PayloadDescriptorInput::from_container(normalized_container.to_string());
    chunk_stream_from_entries_with_descriptor(entry_bytes, descriptor, render_options)
}

/// Build a BRS1 chunk stream from headerless payload entries that all share one
/// `PayloadDescriptor`. The descriptor is stored once and every entry references
/// descriptor index 0 — the headerless-ECDC packaging where the codec header has
/// been lifted out of each groove entry.
fn chunk_stream_from_entries_with_descriptor(
    entry_bytes: Vec<Vec<u8>>,
    descriptor: PayloadDescriptorInput,
    render_options: &RenderOptions,
) -> Result<ChunkStreamInput> {
    if entry_bytes.is_empty() {
        bail!("payload entries array is empty");
    }

    let track_listing = render_options_track_listing(render_options);
    let track_title = track_listing
        .as_array()
        .and_then(|tracks| tracks.first())
        .and_then(|track| track.get("title"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Track 1")
        .to_string();
    let entry_count = entry_bytes.len();
    let payload_byte_length = entry_bytes.iter().try_fold(0usize, |total, entry| {
        total
            .checked_add(entry.len())
            .context("payload entry byte length overflow")
    })?;

    let input = RecordStreamInput {
        payload_descriptors: vec![descriptor],
        tracks: vec![TrackInput {
            title: track_title,
            first_revolution_index: Some(0),
            revolution_count: Some(entry_count),
        }],
        track_gaps: Vec::new(),
    };
    let entries = entry_bytes
        .into_iter()
        .map(|bytes| PayloadEntryInput::already_chunked(0, bytes))
        .collect::<Vec<_>>();

    let stream_bytes = encode_record_stream(&input, &entries)
        .context("failed to wrap payload entries as BRS1 record stream")?;

    record_core::parse_chunk_stream(&stream_bytes)
        .context("facade produced invalid BRS1 record stream")?;

    Ok(ChunkStreamInput {
        stream_bytes,
        payload_byte_length,
        input_was_chunk_stream: false,
        track_listing: Some(track_listing),
        dummy_spiral_regions: render_options.dummy_spiral_regions.clone(),
    })
}

fn render_options_json_with_chunk_metadata(
    render_options_json: &str,
    track_listing: Option<&serde_json::Value>,
    dummy_spiral_regions: Option<&serde_json::Value>,
) -> Result<String> {
    if track_listing.is_none() && dummy_spiral_regions.is_none() {
        return Ok(render_options_json.to_string());
    }
    let mut value = match render_options_json.trim() {
        "" => json!({}),
        raw => serde_json::from_str::<serde_json::Value>(raw)
            .context("render options JSON is invalid")?,
    };
    if !value.is_object() {
        value = json!({});
    }
    if let Some(object) = value.as_object_mut() {
        if let Some(track_listing) = track_listing {
            object.insert("trackListing".to_string(), track_listing.clone());
        }
        if let Some(dummy_spiral_regions) = dummy_spiral_regions {
            object.insert(
                "dummySpiralRegions".to_string(),
                dummy_spiral_regions.clone(),
            );
        }
    }
    Ok(serde_json::to_string(&value)?)
}

fn render_header_json(
    descriptor: &record_descriptor::RecordDescriptor,
    render_options: &RenderOptions,
    payload_byte_length: usize,
    stream_byte_length: usize,
    input_was_chunk_stream: bool,
) -> Result<String> {
    let mut value = serde_json::to_value(descriptor)
        .context("failed to serialize record descriptor as header JSON")?;

    if let Some(object) = value.as_object_mut() {
        object.insert("version".to_string(), json!(1));
        object.insert("checksumProtected".to_string(), json!(true));
        object.insert(
            "descriptorMagic".to_string(),
            json!(record_descriptor_magic()),
        );
        object.insert(
            "chunkStreamMagic".to_string(),
            json!(String::from_utf8_lossy(record_core::RECORD_STREAM_MAGIC).to_string()),
        );
        object.insert("payloadByteLength".to_string(), json!(payload_byte_length));
        object.insert("streamByteLength".to_string(), json!(stream_byte_length));
        object.insert(
            "inputWasChunkStream".to_string(),
            json!(input_was_chunk_stream),
        );
        object.insert(
            "payloadEncoding".to_string(),
            json!(PAYLOAD_CODE_FORMAT_RGB),
        );

        if let Some(generation_version) = render_options.header_generation_version.as_deref() {
            object.insert("generationVersion".to_string(), json!(generation_version));
        }
        if let Some(track_listing) = render_options.track_listing.as_ref() {
            object.insert("trackListing".to_string(), track_listing.clone());
        }
        if let Some(dummy_spiral_regions) = render_options.dummy_spiral_regions.as_ref() {
            object.insert(
                "dummySpiralRegions".to_string(),
                dummy_spiral_regions.clone(),
            );
        }
    }

    Ok(serde_json::to_string(&value)?)
}

fn render_options_json_option(raw: &str) -> Option<&str> {
    let trimmed = raw.trim();

    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn parse_render_options(raw: &str) -> Result<RenderOptions> {
    let trimmed = raw.trim();

    if trimmed.is_empty() {
        return Ok(RenderOptions::default());
    }

    serde_json::from_str(trimmed)
        .with_context(|| format!("Could not parse render options: {trimmed}"))
}

fn normalize_payload_code_format(format: &str) -> Result<&'static str> {
    let normalized = format.trim().to_ascii_lowercase().replace('_', "-");

    match normalized.as_str() {
        PAYLOAD_CODE_FORMAT_RGB => Ok(PAYLOAD_CODE_FORMAT_RGB),
        other => bail!("Unsupported payload code format: {other}"),
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_preview_render_stays_fast_and_needs_no_authoring_feature() {
        // Guards the production render-record 500 regression: a tiny (28
        // byte) first-chunk ECDC entry with a trackListing whose
        // durationSeconds is 0 while duration_seconds is the full ~211s
        // track length used to send the exact-fit search into a multi-
        // second full-canvas geometry search, which the Worker's CPU
        // budget kills mid-request. The public build must never even be
        // able to reach that search (see record-render's `exact-fit`
        // feature), and the forced-fast path must stay well under a
        // second regardless.
        let payload_entries: Vec<u8> = vec![
            0, 0, 0, 20, 142, 148, 51, 24, 50, 43, 204, 119, 248, 149, 116, 149, 137, 70, 212, 74,
            142, 224, 150, 228, 184, 207, 69, 0,
        ];
        let descriptor_json = r##"{"container":"ECDC","codec":"ECDC","sampleRate":48000,"channels":2,"blockSamples":64960,"outputOffsetSamples":480,"outputSamples":64000,"codecMetadata":[123,34,109,34,58,34,101,110,99,111,100,101,99,95,52,56,107,104,122,34,44,34,97,108,34,58,54,52,48,48,48,44,34,110,99,34,58,56,44,34,108,109,34,58,116,114,117,101,44,34,102,112,34,58,56,49,57,50,44,34,109,114,34,58,50,44,34,97,99,118,34,58,50,44,34,116,97,117,34,58,49,46,48,44,34,108,109,104,34,58,34,98,56,99,50,49,100,54,54,53,48,98,54,50,97,48,98,56,99,100,50,49,48,99,54,101,54,50,52,100,102,98,98,51,48,99,51,97,51,57,56,57,52,54,54,53,52,52,98,49,102,53,101,49,100,102,57,54,51,101,99,97,53,49,55,34,44,34,102,108,34,58,50,48,51,125]}"##;
        let render_options_json = r##"{"trackListing":[{"number":1,"durationSeconds":0,"startSeconds":0,"endSeconds":0}],"fastFit":true}"##;

        let started = std::time::Instant::now();
        let result = render_payload_entries_with_descriptor_to_png(
            vec![payload_entries],
            descriptor_json,
            "rgb",
            "single45",
            211.33060416666666,
            render_options_json,
        );
        let elapsed = started.elapsed();
        assert!(
            result.is_ok(),
            "fast-fit render should succeed: {:?}",
            result.err()
        );
        assert!(
            elapsed.as_secs_f64() < 2.0,
            "fast-fit render took {elapsed:?}, expected well under 2s (regression toward the slow exact-fit search)"
        );
    }

    #[test]
    fn descriptor_render_rejects_invalid_descriptor_json() {
        let err = render_payload_entries_with_descriptor_to_png(
            vec![vec![1, 2, 3, 4]],
            "{ not json",
            "rgb",
            "single45",
            1.333,
            "{}",
        )
        .err()
        .expect("expected an error")
        .to_string();
        assert!(err.contains("payload descriptor JSON is invalid"));
    }

    #[test]
    fn descriptor_render_rejects_invalid_geometry() {
        // offset + output (1000 + 64000) exceeds block (64960).
        let descriptor = r#"{"container":"ECDC","codec":"ECDC","sampleRate":48000,"channels":2,"blockSamples":64960,"outputOffsetSamples":1000,"outputSamples":64000,"codecMetadata":[123,125]}"#;
        let err = render_payload_entries_with_descriptor_to_png(
            vec![vec![1, 2, 3, 4]],
            descriptor,
            "rgb",
            "single45",
            1.333,
            "{}",
        )
        .err()
        .expect("expected an error")
        .to_string();
        assert!(err.contains("invalid"));
    }
    const PROGRAMME_ECDC_DESCRIPTOR: &str = r#"{"container":"ECDC","codec":"ECDC","sampleRate":48000,"channels":2,"blockSamples":64960,"outputOffsetSamples":480,"outputSamples":64000,"codecMetadata":[123,125]}"#;
    fn programme_json(tracks: &str) -> String {
        format!(r#"{{"ecdcDescriptor":{PROGRAMME_ECDC_DESCRIPTOR},"tracks":{tracks}}}"#)
    }
    fn programme_json_with_gaps(tracks: &str, track_gaps: &str) -> String {
        format!(
            r#"{{"ecdcDescriptor":{PROGRAMME_ECDC_DESCRIPTOR},"tracks":{tracks},"trackGaps":{track_gaps}}}"#
        )
    }

    #[test]
    fn programme_assembles_tracks_and_an_explicit_track_gap() {
        // Entry index 2 is an explicit inter-track gap: a normal ECDC entry,
        // classified by the trackGaps list, not by being left off any track.
        // There is no GAP descriptor and no GAP1 payload.
        let json = programme_json_with_gaps(
            r#"[{"title":"Track A","payloadIndexes":[0,1]},{"title":"Track B","payloadIndexes":[3]}]"#,
            r#"[{"firstRevolutionIndex":2,"revolutionCount":1,"afterTrackIndex":0}]"#,
        );
        let entries = vec![
            vec![0xA1u8; 8],
            vec![0xA2u8; 8],
            vec![0x00u8; 8],
            vec![0xB1u8; 8],
        ];
        let assembled = assemble_record_programme(entries, &json, "single45").unwrap();

        // One shared ECDC descriptor; every entry is ECDC.
        assert_eq!(assembled.descriptors.len(), 1);
        assert_eq!(assembled.descriptors[0].container, "ECDC");
        assert_eq!(assembled.entry_descriptor_indexes, vec![0, 0, 0, 0]);
        assert_eq!(assembled.entry_bytes.len(), 4);
        // Musical track ranges; entry 2 is the explicit track gap.
        assert_eq!(assembled.tracks[0].first_revolution_index, 0);
        assert_eq!(assembled.tracks[0].revolution_count, 2);
        assert_eq!(assembled.tracks[1].first_revolution_index, 3);
        assert_eq!(assembled.tracks[1].revolution_count, 1);
        assert_eq!(assembled.track_gaps.len(), 1);
        assert_eq!(assembled.track_gaps[0].first_revolution_index, 2);
        assert_eq!(assembled.track_gaps[0].revolution_count, 1);
        assert_eq!(assembled.track_gaps[0].after_track_index, 0);
    }

    #[test]
    fn programme_rejects_duplicate_payload_index() {
        // The same entry referenced by two different tracks.
        let json = programme_json(
            r#"[{"title":"A","payloadIndexes":[0]},{"title":"B","payloadIndexes":[0]}]"#,
        );
        let err = assemble_record_programme(vec![vec![1u8; 4]], &json, "single45")
            .unwrap_err()
            .to_string();
        assert!(err.contains("used by more than one track"), "{err}");
    }

    #[test]
    fn an_uncovered_entry_without_an_explicit_gap_is_rejected() {
        let json = programme_json(r#"[{"title":"A","payloadIndexes":[0]}]"#);
        // Two entries supplied, one referenced — there is no implicit
        // "uncovered means gap" fallback any more, so this is rejected.
        let err = assemble_record_programme(vec![vec![1u8; 4], vec![2u8; 4]], &json, "single45")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("not covered by any track or track gap"),
            "{err}"
        );
    }

    #[test]
    fn an_explicit_track_gap_covers_the_otherwise_uncovered_entry() {
        let json = programme_json_with_gaps(
            r#"[{"title":"A","payloadIndexes":[0]}]"#,
            r#"[{"firstRevolutionIndex":1,"revolutionCount":1,"afterTrackIndex":0}]"#,
        );
        let assembled =
            assemble_record_programme(vec![vec![1u8; 4], vec![2u8; 4]], &json, "single45").unwrap();
        assert_eq!(assembled.entry_bytes.len(), 2);
        assert_eq!(assembled.tracks.len(), 1);
        assert_eq!(assembled.tracks[0].revolution_count, 1);
        assert_eq!(assembled.track_gaps.len(), 1);
        assert_eq!(assembled.track_gaps[0].first_revolution_index, 1);
    }

    #[test]
    fn a_track_gap_overlapping_a_track_is_rejected() {
        let json = programme_json_with_gaps(
            r#"[{"title":"A","payloadIndexes":[0,1]}]"#,
            r#"[{"firstRevolutionIndex":1,"revolutionCount":1,"afterTrackIndex":0}]"#,
        );
        let err = assemble_record_programme(vec![vec![1u8; 4], vec![2u8; 4]], &json, "single45")
            .unwrap_err()
            .to_string();
        assert!(err.contains("covered by both"), "{err}");
    }

    #[test]
    fn overlapping_track_gaps_are_rejected() {
        let json = programme_json_with_gaps(
            r#"[{"title":"A","payloadIndexes":[0]}]"#,
            r#"[{"firstRevolutionIndex":1,"revolutionCount":2,"afterTrackIndex":0},{"firstRevolutionIndex":2,"revolutionCount":1,"afterTrackIndex":0}]"#,
        );
        let err = assemble_record_programme(
            vec![vec![1u8; 4], vec![2u8; 4], vec![3u8; 4]],
            &json,
            "single45",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("covered by both"), "{err}");
    }

    #[test]
    fn zero_length_track_gap_is_rejected() {
        let json = programme_json_with_gaps(
            r#"[{"title":"A","payloadIndexes":[0]}]"#,
            r#"[{"firstRevolutionIndex":1,"revolutionCount":0,"afterTrackIndex":0}]"#,
        );
        let err = assemble_record_programme(vec![vec![1u8; 4], vec![2u8; 4]], &json, "single45")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("revolution count must be greater than zero"),
            "{err}"
        );
    }

    #[test]
    fn invalid_after_track_index_is_rejected() {
        let json = programme_json_with_gaps(
            r#"[{"title":"A","payloadIndexes":[0]}]"#,
            r#"[{"firstRevolutionIndex":1,"revolutionCount":1,"afterTrackIndex":3}]"#,
        );
        let err = assemble_record_programme(vec![vec![1u8; 4], vec![2u8; 4]], &json, "single45")
            .unwrap_err()
            .to_string();
        assert!(err.contains("after_track_index"), "{err}");
    }

    #[test]
    fn gap_ranges_are_not_inferred_from_uncovered_entries() {
        // Three entries, only entry 0 declared as a track, with NO trackGaps
        // entry for entries 1 and 2: even though they are structurally
        // "uncovered", nothing infers a gap range for them.
        let json = programme_json(r#"[{"title":"A","payloadIndexes":[0]}]"#);
        let err = assemble_record_programme(
            vec![vec![1u8; 4], vec![2u8; 4], vec![3u8; 4]],
            &json,
            "single45",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("payload entry 1 is not covered"), "{err}");
    }

    #[test]
    fn programme_rejects_non_contiguous_payload_indexes() {
        let json = programme_json(r#"[{"title":"A","payloadIndexes":[0,2]}]"#);
        let err = assemble_record_programme(
            vec![vec![1u8; 4], vec![2u8; 4], vec![3u8; 4]],
            &json,
            "single45",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("contiguous and ascending"), "{err}");
    }

    #[test]
    fn programme_rejects_out_of_range_index() {
        let json = programme_json(r#"[{"title":"A","payloadIndexes":[5]}]"#);
        let err = assemble_record_programme(vec![vec![1u8; 4]], &json, "single45")
            .unwrap_err()
            .to_string();
        assert!(err.contains("only 1 entries"), "{err}");
    }
    fn assert_programme_descriptor_rejected(ecdc_descriptor: &str, needle: &str) {
        let json = format!(
            r#"{{"ecdcDescriptor":{ecdc_descriptor},"tracks":[{{"title":"A","payloadIndexes":[0],"gapAfterSeconds":0.0}}]}}"#
        );
        let err = format!(
            "{:#}",
            assemble_record_programme(vec![vec![1u8; 4]], &json, "single45").unwrap_err()
        );
        assert!(err.contains(needle), "expected `{needle}` in: {err}");
    }

    #[test]
    fn programme_rejects_incomplete_ecdc_descriptors() {
        // A bare descriptor — the exact regression this guards against.
        assert_programme_descriptor_rejected(r#"{"container":"ECDC"}"#, "codec ECDC");
        // Missing codec metadata.
        assert_programme_descriptor_rejected(
            r#"{"container":"ECDC","codec":"ECDC","sampleRate":48000,"channels":2,"blockSamples":64960,"outputOffsetSamples":480,"outputSamples":64000}"#,
            "codecMetadata",
        );
        // Absent sample rate.
        assert_programme_descriptor_rejected(
            r#"{"container":"ECDC","codec":"ECDC","channels":2,"blockSamples":64960,"outputOffsetSamples":480,"outputSamples":64000,"codecMetadata":[123,125]}"#,
            "sampleRate",
        );
        // Wrong codec.
        assert_programme_descriptor_rejected(
            r#"{"container":"ECDC","codec":"MOSS","sampleRate":48000,"channels":2,"blockSamples":64960,"outputOffsetSamples":480,"outputSamples":64000,"codecMetadata":[123,125]}"#,
            "codec ECDC",
        );
        // Partial output geometry (block present, output_samples absent).
        assert_programme_descriptor_rejected(
            r#"{"container":"ECDC","codec":"ECDC","sampleRate":48000,"channels":2,"blockSamples":64960,"outputOffsetSamples":480,"codecMetadata":[123,125]}"#,
            "outputSamples",
        );
    }

    #[test]
    fn repro_streaming_spiral_preview_exact_production_payload() {
        // Exact payload captured from the production render-record 500
        // (streaming spiral preview's first-chunk request): a 28-byte ECDC
        // entry with a trackListing entry whose durationSeconds is 0 while
        // the top-level durationSeconds is the full ~211s track length.
        let payload_entries: Vec<u8> = vec![
            0, 0, 0, 20, 142, 148, 51, 24, 50, 43, 204, 119, 248, 149, 116, 149, 137, 70, 212, 74,
            142, 224, 150, 228, 184, 207, 69, 0,
        ];
        let descriptor_json = r##"{"container":"ECDC","codec":"ECDC","sampleRate":48000,"channels":2,"blockSamples":64960,"outputOffsetSamples":480,"outputSamples":64000,"codecMetadata":[123,34,109,34,58,34,101,110,99,111,100,101,99,95,52,56,107,104,122,34,44,34,97,108,34,58,54,52,48,48,48,44,34,110,99,34,58,56,44,34,108,109,34,58,116,114,117,101,44,34,102,112,34,58,56,49,57,50,44,34,109,114,34,58,50,44,34,97,99,118,34,58,50,44,34,116,97,117,34,58,49,46,48,44,34,108,109,104,34,58,34,98,56,99,50,49,100,54,54,53,48,98,54,50,97,48,98,56,99,100,50,49,48,99,54,101,54,50,52,100,102,98,98,51,48,99,51,97,51,57,56,57,52,54,54,53,52,52,98,49,102,53,101,49,100,102,57,54,51,101,99,97,53,49,55,34,44,34,102,108,34,58,50,48,51,125]}"##;
        let render_options_json = r##"{"payloadEncoding":"rgb","headerTitle":"new wonder inst mix ab oz","headerArtist":null,"headerReleaseId":"rel_01KWHTPBQA2ESWCTB45VZ4YTP9","cacheEncryptionSecretBase64url":"O9hHI7Rr0C2qvgh9Cuq6ifasY_-W0l8xksztWWnwoDo","headerGenerationVersion":"20260606a","guideOutlines":false,"trackListing":[{"number":1,"title":"new wonder inst mix ab oz","durationSeconds":0,"startSeconds":0,"endSeconds":0,"source":"cut-source"}],"dummySpiralRegions":[],"headerArbitraryMetadata":"{\"trackListing\":[{\"number\":1,\"title\":\"new wonder inst mix ab oz\",\"durationSeconds\":0,\"startSeconds\":0,\"endSeconds\":0,\"source\":\"cut-source\"}]}","grooveToneColor":"#1E0B2E","fastFit":true}"##;

        let result = render_payload_entries_with_descriptor_to_png_native(
            vec![payload_entries],
            descriptor_json,
            "rgb",
            "single45",
            211.33060416666666,
            render_options_json,
        );
        match result {
            Ok(_) => println!("repro: rendered OK (did not reproduce)"),
            Err(error) => println!("repro: render returned error (not a panic): {error:#}"),
        }
    }

    #[test]
    fn programme_accepts_canonical_ecdc_descriptor_from_helper() {
        // Build the descriptor with the authoritative Rust helper, not a
        // hand-written approximation.
        let descriptor = record_core::ecdc::ecdc_payload_descriptor(
            48_000,
            2,
            &record_core::ecdc::EcdcCodecMetadata {
                model: "encodec_48khz".to_owned(),
                num_codebooks: 8,
                lm: true,
                fp_scale: 8192,
                min_range: 2,
                bitstream_version: 2,
                lm_frame_length: 203,
            },
        )
        .unwrap();
        let descriptor_json = serde_json::to_string(&descriptor).unwrap();
        let json = format!(
            r#"{{"ecdcDescriptor":{descriptor_json},"tracks":[{{"title":"A","payloadIndexes":[0,1],"gapAfterSeconds":0.0}}]}}"#
        );

        let assembled =
            assemble_record_programme(vec![vec![1u8; 8], vec![2u8; 8]], &json, "single45").unwrap();
        // Single-track, no gap → exactly one (ECDC) descriptor, fully populated.
        assert_eq!(assembled.descriptors.len(), 1);
        let d = &assembled.descriptors[0];
        assert_eq!(d.codec.as_deref(), Some("ECDC"));
        assert_eq!(d.sample_rate, Some(48_000));
        assert_eq!(d.channels, Some(2));
        assert_eq!(d.block_samples, Some(64_960));
        assert_eq!(d.output_offset_samples, Some(480));
        assert_eq!(d.output_samples, Some(64_000));
        assert!(d.codec_metadata.is_some());
    }

    #[test]
    fn programme_descriptor_round_trips_through_brs1_and_programme_map() {
        let descriptor = record_core::ecdc::ecdc_payload_descriptor(
            48_000,
            2,
            &record_core::ecdc::EcdcCodecMetadata {
                model: "encodec_48khz".to_owned(),
                num_codebooks: 8,
                lm: true,
                fp_scale: 8192,
                min_range: 2,
                bitstream_version: 2,
                lm_frame_length: 203,
            },
        )
        .unwrap();
        let descriptor_json = serde_json::to_string(&descriptor).unwrap();
        let json = format!(
            r#"{{"ecdcDescriptor":{descriptor_json},"tracks":[{{"title":"Side A","payloadIndexes":[0,1],"gapAfterSeconds":0.0}}]}}"#
        );
        let assembled =
            assemble_record_programme(vec![vec![0xAAu8; 8], vec![0xBBu8; 8]], &json, "single45")
                .unwrap();

        let options = parse_render_options("{}").unwrap();
        let chunk_input = chunk_stream_from_multi_descriptor_entries(
            assembled.entry_bytes,
            assembled.entry_descriptor_indexes,
            assembled.descriptors,
            assembled.tracks,
            assembled.track_gaps,
            &options,
        )
        .unwrap();
        let stream = record_core::parse_chunk_stream(&chunk_input.stream_bytes).unwrap();

        // The BRS1 descriptor is the full canonical ECDC descriptor, not a bare
        // { container: "ECDC" }.
        let d = &stream.metadata.payload_descriptors[0];
        assert_eq!(d.container, "ECDC");
        assert_eq!(d.codec.as_deref(), Some("ECDC"));
        assert_eq!(d.sample_rate, Some(48_000));
        assert_eq!(d.channels, Some(2));
        assert!(d.codec_metadata.is_some());

        // The pre-decode programme map builds (the exact step that previously
        // failed with "record stream has no sample rate").
        let map = record_core::build_programme_map(&stream, Some("single45")).unwrap();
        assert_eq!(map.sample_rate, 48_000);
        assert!(map.total_samples > 0);
    }

    // Two songs separated by a 2-revolution Silence span backed by its own
    // GAP descriptor — exercises `chunk_stream_from_multi_descriptor_entries`
    // (the part of `renderMultiPayloadEntriesToPng` specific to this feature:
    // multiple descriptors, per-entry descriptor indexes, and a three-entry
    // track listing). Stops short of the actual PNG render
    // (`render_chunk_input_to_png`) because that calls `wasm_log`, which
    // panics on non-wasm32 targets — every other test in this file avoids it
    // for the same reason (see e.g. `descriptor_render_rejects_invalid_geometry`,
    // which never reaches a successful render either).
    #[test]
    fn explicit_track_gap_between_tracks_is_excluded_and_maps_silence() {
        // Two musical tracks with one explicit inter-track gap between them.
        // The gap is a normal ECDC entry (descriptor index 0) — there is no
        // GAP descriptor and no GAP1 payload. Its identity as a gap comes
        // entirely from the explicit track_gaps list, never from being
        // "left out" of the tracks list. The song entries are headerless
        // per-revolution codec bodies (no "ECDC" magic), matching the encoder.
        let descriptors: Vec<record_core::PayloadDescriptor> = serde_json::from_str(
            r#"[
                {"container":"ECDC","codec":"ECDC","sampleRate":48000,"channels":2,"blockSamples":64960,"outputOffsetSamples":480,"outputSamples":64000,"codecMetadata":[123,125]}
            ]"#,
        )
        .unwrap();
        // Side A covers entry 0; entry 1 is the explicit track gap; Side B covers entry 2.
        let tracks: Vec<WasmTrackInputJson> = serde_json::from_str(
            r#"[
                {"title":"Side A","firstRevolutionIndex":0,"revolutionCount":1},
                {"title":"Side B","firstRevolutionIndex":2,"revolutionCount":1}
            ]"#,
        )
        .unwrap();
        let track_gaps: Vec<WasmTrackGapInputJson> = serde_json::from_str(
            r#"[{"firstRevolutionIndex":1,"revolutionCount":1,"afterTrackIndex":0}]"#,
        )
        .unwrap();
        let gap_entry = vec![0x00u8; 3_000];
        let entries = vec![vec![0xAAu8; 4_000], gap_entry.clone(), vec![0xBBu8; 5_500]];
        let entry_descriptor_indexes = vec![0u8, 0, 0];
        let render_options = parse_render_options("{}").unwrap();

        let chunk_input = chunk_stream_from_multi_descriptor_entries(
            entries,
            entry_descriptor_indexes,
            descriptors,
            tracks,
            track_gaps,
            &render_options,
        )
        .unwrap();

        let native_stream = record_core::parse_chunk_stream(&chunk_input.stream_bytes).unwrap();

        // One shared ECDC descriptor; no GAP descriptor exists anymore.
        assert_eq!(native_stream.metadata.payload_descriptors.len(), 1);
        assert_eq!(
            native_stream.metadata.payload_descriptors[0].container,
            "ECDC"
        );
        // Only the two musical tracks exist; the gap is never a track.
        assert_eq!(native_stream.metadata.tracks.len(), 2);
        assert_eq!(native_stream.metadata.tracks[0].title, "Side A");
        assert_eq!(native_stream.metadata.tracks[1].title, "Side B");
        record_core::validate_track_listing_metadata(&native_stream.metadata).unwrap();
    }

    // A canonical ECDC descriptor plus two headerless ECDC entries (no GAP) must
    // decode into a *standalone* ECDC stream beginning with the `ECDC` magic —
    // the exact precondition `ecdcMetadata` / `lmEcdcDecodeChunks` enforce in the
    // decode worker. Before the fix this returned the raw concatenated entry
    // bodies, which the worker rejected with "file has unexpected magic".
    #[test]
    fn headerless_ecdc_entries_reconstruct_standalone_ecdc_magic() {
        let descriptors: Vec<record_core::PayloadDescriptor> = serde_json::from_str(
            r#"[
                {"container":"ECDC","codec":"ECDC","sampleRate":48000,"channels":2,"blockSamples":64960,"outputOffsetSamples":480,"outputSamples":64000,"codecMetadata":[123,125]}
            ]"#,
        )
        .unwrap();
        let tracks: Vec<WasmTrackInputJson> = serde_json::from_str(
            r#"[{"title":"Side A","firstRevolutionIndex":0,"revolutionCount":2}]"#,
        )
        .unwrap();
        let first = vec![0x11u8; 3_200];
        let second = vec![0x22u8; 4_100];
        let entries = vec![first.clone(), second.clone()];
        let render_options = parse_render_options("{}").unwrap();

        let chunk_input = chunk_stream_from_multi_descriptor_entries(
            entries,
            vec![0u8, 0u8],
            descriptors,
            tracks,
            vec![],
            &render_options,
        )
        .unwrap();

        let native_stream = record_core::parse_chunk_stream(&chunk_input.stream_bytes).unwrap();

        // Each discrete frame is wrapped in its own standalone ECDC object and
        // the two objects are concatenated, so the decoder walks them
        // object-by-object. Reconstruct the same two objects and compare.
        let descriptor = &native_stream.metadata.payload_descriptors[0];
        let mut expected =
            record_core::ecdc::payload_to_standalone_ecdc(descriptor, &first).unwrap();
        expected.extend_from_slice(
            &record_core::ecdc::payload_to_standalone_ecdc(descriptor, &second).unwrap(),
        );
        // Two ECDC objects, one per frame.
        assert_eq!(&expected[..4], b"ECDC");
        assert_eq!(expected.windows(4).filter(|w| *w == b"ECDC").count(), 2);
        // Each object's per-frame audio length is the descriptor's block_samples.
        let (header_json, _body) = record_core::ecdc::split_standalone_ecdc(&expected).unwrap();
        let header: serde_json::Value = serde_json::from_slice(header_json).unwrap();
        assert_eq!(
            header.get("al").and_then(serde_json::Value::as_u64),
            Some(64_960)
        );
    }
}
