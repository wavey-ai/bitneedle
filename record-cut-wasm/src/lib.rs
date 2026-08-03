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
use base64::{engine::general_purpose, Engine as _};
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder};
use record_cut::{
    encode_record_stream, PayloadDescriptorInput, PayloadEntryInput, RecordStreamInput,
    TrackGapInput, TrackInput,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
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

#[wasm_bindgen(js_name = buildBitneedlePackage)]
pub fn wasm_build_bitneedle_package(
    brd1_bytes: &[u8],
    brs1_bytes: &[u8],
    bsc1_bytes: &[u8],
) -> Result<Vec<u8>, JsValue> {
    let bsc1 = (!bsc1_bytes.is_empty()).then_some(bsc1_bytes);
    record_package::encode_package(brd1_bytes, brs1_bytes, bsc1).map_err(to_js_error)
}

#[wasm_bindgen(js_name = inspectBitneedlePackageJson)]
pub fn wasm_inspect_bitneedle_package_json(package_bytes: &[u8]) -> Result<String, JsValue> {
    let inspection = record_package::inspect_package(package_bytes).map_err(to_js_error)?;
    serde_json::to_string(&inspection).map_err(to_js_error)
}

#[wasm_bindgen(js_name = extractBitneedlePackageSection)]
pub fn wasm_extract_bitneedle_package_section(
    package_bytes: &[u8],
    section_name: &str,
) -> Result<Vec<u8>, JsValue> {
    let kind = record_package::PackageSectionKind::from_name(section_name).map_err(to_js_error)?;
    let package = record_package::parse_package(package_bytes).map_err(to_js_error)?;
    package
        .section(kind)
        .map(ToOwned::to_owned)
        .ok_or_else(|| JsValue::from_str(&format!("BPK1 has no {} section", kind.name())))
}

#[wasm_bindgen(js_name = buildSidecarContainer)]
pub fn wasm_build_sidecar_container(items_json: &str) -> Result<Vec<u8>, JsValue> {
    record_sidecar::build_sidecar_container_from_items_json(items_json).map_err(to_js_error)
}

#[wasm_bindgen(js_name = buildPackageDisplayHeader)]
pub fn wasm_build_package_display_header(options_json: &str) -> Result<Vec<u8>, JsValue> {
    record_sidecar::build_package_display_header_bytes_from_json(options_json).map_err(to_js_error)
}

#[wasm_bindgen(js_name = buildPackageDisplayHeaderItemJson)]
pub fn wasm_build_package_display_header_item_json(options_json: &str) -> Result<String, JsValue> {
    record_sidecar::build_package_display_header_item_json(options_json).map_err(to_js_error)
}

#[wasm_bindgen(js_name = buildPackageMetadataItemsJson)]
pub fn wasm_build_package_metadata_items_json(options_json: &str) -> Result<String, JsValue> {
    record_sidecar::build_package_metadata_items_json(options_json).map_err(to_js_error)
}

#[wasm_bindgen(js_name = buildPackagePhotoItemJson)]
pub fn wasm_build_package_photo_item_json(
    options_json: &str,
    avif_bytes: &[u8],
) -> Result<String, JsValue> {
    record_sidecar::build_package_photo_item_json(options_json, avif_bytes).map_err(to_js_error)
}

#[wasm_bindgen(js_name = buildPackageCoverItemJson)]
pub fn wasm_build_package_cover_item_json(avif_bytes: &[u8]) -> String {
    record_sidecar::build_package_cover_item_json(avif_bytes)
}

#[wasm_bindgen(js_name = resolvePackageImageEncodeCacheKeyJson)]
pub fn wasm_resolve_package_image_encode_cache_key_json(
    options_json: &str,
) -> Result<String, JsValue> {
    record_sidecar::resolve_package_image_encode_cache_key_json(options_json).map_err(to_js_error)
}

#[wasm_bindgen(js_name = resolvePackageBestFitCacheKeyJson)]
pub fn wasm_resolve_package_best_fit_cache_key_json(options_json: &str) -> Result<String, JsValue> {
    record_sidecar::resolve_package_best_fit_cache_key_json(options_json).map_err(to_js_error)
}

#[wasm_bindgen(js_name = packageQuantizerSearchPlanJson)]
pub fn wasm_package_quantizer_search_plan_json(options_json: &str) -> Result<String, JsValue> {
    record_sidecar::package_quantizer_search_plan_json(options_json).map_err(to_js_error)
}

#[wasm_bindgen(js_name = packageFitBudgetJson)]
pub fn wasm_package_fit_budget_json(options_json: &str) -> Result<String, JsValue> {
    record_sidecar::package_fit_budget_json(options_json).map_err(to_js_error)
}

#[wasm_bindgen(js_name = buildPackageSidecarRenderOptionsJson)]
pub fn wasm_build_package_sidecar_render_options_json(
    options_json: &str,
) -> Result<String, JsValue> {
    record_sidecar::build_package_sidecar_render_options_json(options_json).map_err(to_js_error)
}

#[wasm_bindgen(js_name = packagePreservedPatternItemsJson)]
pub fn wasm_package_preserved_pattern_items_json(decoded_json: &str) -> Result<String, JsValue> {
    record_sidecar::package_preserved_pattern_items_json(decoded_json).map_err(to_js_error)
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

#[wasm_bindgen(js_name = estimateRecordPngSidecarCapacityJson)]
pub fn wasm_estimate_record_png_sidecar_capacity_json(
    png_bytes: &[u8],
    record_profile: Option<String>,
) -> Result<String, JsValue> {
    record_sidecar::estimate_record_png_sidecar_capacity_json(png_bytes, record_profile.as_deref())
        .map_err(to_js_error)
}

#[wasm_bindgen(js_name = rewriteRecordPng)]
pub fn wasm_rewrite_record_png(
    png_bytes: &[u8],
    render_options_json: &str,
    record_profile: Option<String>,
) -> Result<WasmRenderResult, JsValue> {
    let (png_bytes, sidecar) = rewrite_record_png_preserving_pattern_items(
        png_bytes,
        render_options_json,
        record_profile.as_deref(),
    )
    .map_err(to_js_error)?;
    let payload_json = serde_json::to_string(&serde_json::json!({
        "status": "ok",
        "sidecar": sidecar,
    }))
    .map_err(to_js_error)?;
    let header =
        decode_record_header_json(&png_bytes, record_profile.as_deref()).map_err(to_js_error)?;
    Ok(WasmRenderResult {
        png_bytes: png_bytes.to_vec(),
        payload_json,
        header_json: header,
    })
}

fn rewrite_record_png_preserving_pattern_items(
    png_bytes: &[u8],
    render_options_json: &str,
    record_profile: Option<&str>,
) -> Result<(Vec<u8>, record_sidecar::SidecarRenderSummary)> {
    let render_options_json = merge_rewrite_options_with_preserved_pattern_items(
        png_bytes,
        render_options_json,
        record_profile,
    )?;
    record_sidecar::rewrite_record_png(png_bytes, &render_options_json, record_profile)
}

fn merge_rewrite_options_with_preserved_pattern_items(
    png_bytes: &[u8],
    render_options_json: &str,
    record_profile: Option<&str>,
) -> Result<String> {
    let preserved = preserved_pattern_items_for_png(png_bytes, record_profile)?;
    if preserved.is_empty() {
        return Ok(render_options_json.to_string());
    }

    let mut options = if render_options_json.trim().is_empty() {
        serde_json::Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str::<serde_json::Value>(render_options_json)
            .with_context(|| format!("Could not parse render options: {render_options_json}"))?
    };
    let Some(sidecar) = options
        .get_mut("sidecar")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return Ok(render_options_json.to_string());
    };
    if sidecar.get("bsc1Base64").is_some() {
        return Ok(render_options_json.to_string());
    }
    let items_value = sidecar
        .entry("items".to_string())
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    let items = items_value
        .as_array_mut()
        .context("rewrite sidecar.items must be an array to preserve Patternize map")?;
    items.retain(|item| !sidecar_item_is_pattern_map(item));
    items.extend(preserved);
    Ok(options.to_string())
}

fn preserved_pattern_items_for_png(
    png_bytes: &[u8],
    record_profile: Option<&str>,
) -> Result<Vec<serde_json::Value>> {
    let decoded_json =
        match record_sidecar::decode_record_png_sidecar_items_json(png_bytes, record_profile) {
            Ok(value) => value,
            Err(_) => return Ok(Vec::new()),
        };
    let decoded: serde_json::Value =
        serde_json::from_str(&decoded_json).context("decoded sidecar JSON is invalid")?;
    record_sidecar::package_preserved_pattern_items(&decoded)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatternizeExploreOptions {
    layout_id: Option<u32>,
    block_size: Option<u32>,
    mode: Option<String>,
    amount: Option<f64>,
    embed_sidecar: Option<bool>,
    sidecar_items: Option<Vec<serde_json::Value>>,
}

#[wasm_bindgen(js_name = patternizeRecordPngExplore)]
pub fn wasm_patternize_record_png_explore(
    png_bytes: &[u8],
    options_json: &str,
    record_profile: Option<String>,
) -> Result<WasmRenderResult, JsValue> {
    patternize_record_png_explore(png_bytes, options_json, record_profile.as_deref())
        .map_err(to_js_error)
}

fn patternize_record_png_explore(
    png_bytes: &[u8],
    options_json: &str,
    record_profile: Option<&str>,
) -> Result<WasmRenderResult> {
    let options = if options_json.trim().is_empty() {
        PatternizeExploreOptions {
            layout_id: None,
            block_size: None,
            mode: None,
            amount: None,
            embed_sidecar: None,
            sidecar_items: None,
        }
    } else {
        serde_json::from_str(options_json)
            .with_context(|| format!("Could not parse patternize options: {options_json}"))?
    };
    let (profile, descriptor) = record_decode::decode_record_descriptor_from_png(
        png_bytes,
        record_profile.filter(|value| !value.trim().is_empty()),
    )
    .context("failed to decode record descriptor before Patternize")?;
    let image = image::load_from_memory(png_bytes)
        .context("failed to decode record PNG before Patternize")?
        .to_rgba8();
    let (width, height) = image.dimensions();
    let (width, height) = (width as usize, height as usize);
    let mut rgba = image.into_raw();
    let mask = record_core::build_spiral_mask(
        width,
        height,
        descriptor.b_value(),
        &profile,
        None,
        None,
        None,
    )?;
    let groove_indices = mask
        .ordered_pixel_indices
        .iter()
        .copied()
        .take_while(|pixel_index| rgba[pixel_index * 4 + 3] != 0)
        .collect::<Vec<_>>();
    if groove_indices.len() < 2 {
        bail!("record PNG has no patternizable groove pixels");
    }
    let layout_id = options.layout_id.unwrap_or(0).min(511);
    let block_size = options.block_size.unwrap_or(128).clamp(2, 4096) as usize;
    let amount = options.amount.unwrap_or(20.0).clamp(1.0, 100.0);
    let mode = options.mode.unwrap_or_else(|| "partialSort".to_string());
    let reverse_map = record_patternize::patternize(
        &mut rgba,
        &groove_indices,
        &record_patternize::PatternizeOptions {
            seed: layout_id,
            amount,
            block_size,
            channels: 4,
        },
    )?;
    if reverse_map.is_empty() {
        bail!("Patternize did not select any groove blocks");
    }
    let visual = record_patternize::visual_score(&rgba, &groove_indices, 4);
    let pattern_png = write_patternized_rgba_png(width, height, &rgba)?;
    let capacity: serde_json::Value = serde_json::from_str(
        &record_sidecar::estimate_record_png_sidecar_capacity_json(&pattern_png, Some(&profile))?,
    )?;
    let sidecar_items = patternize_sidecar_items(&reverse_map, options.sidecar_items.as_ref());
    let required_bytes = record_sidecar::build_sidecar_container_from_items_json(
        &serde_json::to_string(&sidecar_items)
            .context("failed to serialize Patternize sidecar items")?,
    )
    .map(|bytes| bytes.len())
    .unwrap_or_else(|_| {
        12usize
            .saturating_add(16)
            .saturating_add(record_sidecar::PACKAGE_PATTERN_SIDECAR_ITEM_NAME.len())
            .saturating_add(record_sidecar::PACKAGE_PATTERN_SIDECAR_MIME.len())
            .saturating_add(reverse_map.len())
    });
    let fit = patternize_storage_fit(&capacity, required_bytes);
    let embed_sidecar = options.embed_sidecar.unwrap_or(true);
    let (result_png, encoded_sidecar) = if embed_sidecar {
        let render_options = json!({
            "sidecar": {
                "scheme": record_sidecar::SIDECAR_SCHEME_PAIRSIGN_SAFE_LUMA_V2,
                "seed": record_sidecar::SIDECAR_DEFAULT_SEED,
                "carriers": ["label", "intergroove", "leadInDeadwax"],
                "items": sidecar_items,
            },
        })
        .to_string();
        let (rewritten, summary) =
            record_sidecar::rewrite_record_png(&pattern_png, &render_options, Some(&profile))
                .context("failed to embed Patternize reverse map")?;
        let decoded: serde_json::Value = serde_json::from_str(
            &record_sidecar::decode_record_png_sidecar_items_json(&rewritten, Some(&profile))?,
        )?;
        let map_round_trips = decoded
            .get("items")
            .and_then(serde_json::Value::as_array)
            .and_then(|items| {
                items.iter().find(|item| {
                    item.get("name").and_then(serde_json::Value::as_str)
                        == Some(record_sidecar::PACKAGE_PATTERN_SIDECAR_ITEM_NAME)
                })
            })
            .and_then(|item| item.get("dataBase64"))
            .and_then(serde_json::Value::as_str)
            .and_then(|value| record_sidecar::decode_base64_text(value, "Patternize map").ok())
            .is_some_and(|decoded_map| decoded_map == reverse_map);
        if !map_round_trips {
            bail!("embedded Patternize reverse map did not round-trip");
        }
        (
            rewritten,
            json!({
                "ok": true,
                "output": "stegoRecord",
                "mapRoundTrips": true,
                "reverseMapBytes": reverse_map.len(),
                "sidecar": summary,
            }),
        )
    } else {
        (
            pattern_png,
            json!({
                "ok": false,
                "skipped": true,
                "reason": "sidecar embedding disabled for search preview",
            }),
        )
    };
    let report = json!({
        "status": "ok",
        "pattern": {
            "layoutId": layout_id,
            "blockSize": block_size,
            "mode": mode,
            "amount": amount / 100.0,
            "amountPercent": amount,
            "sourceGroovePixels": groove_indices.len(),
            "patternableGroovePixels": groove_indices.len(),
            "groovePixels": groove_indices.len(),
            "reversePathBytes": reverse_map.len(),
            "visualDefinition": visual,
        },
        "storage": {
            "stego": fit,
            "encodedSidecar": encoded_sidecar,
        },
        "capacity": capacity,
    });
    Ok(WasmRenderResult {
        png_bytes: result_png,
        payload_json: report.to_string(),
        header_json: "{}".to_string(),
    })
}

pub fn patternize_record_png_native(
    png_bytes: &[u8],
    options_json: &str,
    record_profile: Option<&str>,
) -> Result<NativeRenderResult> {
    patternize_record_png_explore(
        png_bytes,
        options_json,
        record_profile,
    )
    .map(Into::into)
}

fn patternize_sidecar_items(
    reverse_map: &[u8],
    extra_items: Option<&Vec<serde_json::Value>>,
) -> Vec<serde_json::Value> {
    let mut items = Vec::new();
    if let Some(extra_items) = extra_items {
        items.extend(
            extra_items
                .iter()
                .filter(|item| !sidecar_item_is_pattern_map(item))
                .cloned(),
        );
    }
    items.push(json!({
        "type": "opaque",
        "codec": "raw",
        "name": record_sidecar::PACKAGE_PATTERN_SIDECAR_ITEM_NAME,
        "mime": record_sidecar::PACKAGE_PATTERN_SIDECAR_MIME,
        "dataBase64": general_purpose::URL_SAFE_NO_PAD.encode(reverse_map),
    }));
    items
}

fn sidecar_item_is_pattern_map(item: &serde_json::Value) -> bool {
    item.get("name")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|value| value == record_sidecar::PACKAGE_PATTERN_SIDECAR_ITEM_NAME)
        || item
            .get("mime")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| value == record_sidecar::PACKAGE_PATTERN_SIDECAR_MIME)
}

fn write_patternized_rgba_png(width: usize, height: usize, rgba: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::NoFilter)
        .write_image(rgba, width as u32, height as u32, ExtendedColorType::Rgba8)
        .context("failed to encode Patternize PNG")?;
    Ok(out)
}

fn patternize_storage_fit(
    capacity: &serde_json::Value,
    required_bytes: usize,
) -> serde_json::Value {
    let fit = |name: &str| {
        let capacity_bytes = capacity
            .get(name)
            .and_then(|entry| entry.get("capacityBytes"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as usize;
        json!({
            "capacityBytes": capacity_bytes,
            "fits": capacity_bytes >= required_bytes,
            "spareBytes": capacity_bytes.saturating_sub(required_bytes),
        })
    };
    json!({
        "requiredBytes": required_bytes,
        "label": fit("label"),
        "intergroove": fit("intergroove"),
        "combined": fit("combined"),
        "leadInDeadwax": fit("leadInDeadwax"),
        "expandedIntergroove": fit("expandedIntergroove"),
        "expandedCombined": fit("expandedCombined"),
    })
}

#[wasm_bindgen(js_name = recordProfileSpecsJson)]
pub fn wasm_record_profile_specs_json() -> Result<String, JsValue> {
    let specs = record_core::known_record_profile_names()
        .iter()
        .map(|profile| record_profile_spec(profile))
        .collect::<Result<Vec<_>>>()
        .map_err(to_js_error)?;

    serde_json::to_string(&specs).map_err(to_js_error)
}

#[wasm_bindgen(js_name = normalizeRecordProfileName)]
pub fn wasm_normalize_record_profile_name(record_profile: &str) -> Result<String, JsValue> {
    record_core::normalize_record_profile_name(record_profile).map_err(to_js_error)
}

#[wasm_bindgen(js_name = recordProfileSpecJson)]
pub fn wasm_record_profile_spec_json(record_profile: &str) -> Result<String, JsValue> {
    let spec = record_profile_spec(record_profile).map_err(to_js_error)?;
    serde_json::to_string(&spec).map_err(to_js_error)
}

#[wasm_bindgen(js_name = pressRecordDurationEstimateJson)]
pub fn wasm_press_record_duration_estimate_json(
    record_profile: &str,
    quality: &str,
) -> Result<String, JsValue> {
    let estimate = press_record_duration_estimate(record_profile, quality).map_err(to_js_error)?;
    serde_json::to_string(&estimate).map_err(to_js_error)
}

#[wasm_bindgen(js_name = pressRecordDurationHintJson)]
pub fn wasm_press_record_duration_hint_json(
    record_profile: &str,
    quality: &str,
) -> Result<String, JsValue> {
    let hint = press_record_duration_hint(record_profile, quality).map_err(to_js_error)?;
    serde_json::to_string(&hint).map_err(to_js_error)
}

#[wasm_bindgen(js_name = pressRecordFormatRecommendationJson)]
pub fn wasm_press_record_format_recommendation_json(options_json: &str) -> Result<String, JsValue> {
    press_record_format_recommendation_json(options_json).map_err(to_js_error)
}

#[wasm_bindgen(js_name = pressCertainLpRecordFormatJson)]
pub fn wasm_press_certain_lp_record_format_json(
    duration_seconds: f64,
    current_profile: &str,
    current_quality: &str,
) -> Result<String, JsValue> {
    press_certain_lp_record_format_json(duration_seconds, current_profile, current_quality)
        .map_err(to_js_error)
}

#[wasm_bindgen(js_name = visibleSpiralTurns)]
pub fn wasm_visible_spiral_turns(record_profile: &str, b_value: f64) -> Result<f64, JsValue> {
    record_core::visible_spiral_turns(record_profile, b_value).map_err(to_js_error)
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileSpec {
    name: String,
    record_profile: String,
    label: String,
    rpm: f64,
    spindle_hole_radius: i32,
    label_radius: i32,
    label_clearance: i32,
    payload_inner_radius: i32,
    payload_outer_radius: i32,
    lead_in_outer_radius: i32,
    outer_radius: i32,
    outer_rim_thickness: i32,
    lead_in_band_thickness: i32,
    lead_in_turns: f64,
    run_out_turns: f64,
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

pub fn render_payload_entries_with_descriptor_to_png_fast_native(
    payload_entries: Vec<Vec<u8>>,
    payload_descriptor_json: &str,
    code_format: &str,
    record_profile: &str,
    duration_seconds: f64,
    render_options_json: &str,
) -> Result<NativeRenderResult> {
    let forced_render_options_json = force_fast_fit_render_options_json(render_options_json)?;
    render_payload_entries_with_descriptor_to_png(
        payload_entries,
        payload_descriptor_json,
        code_format,
        record_profile,
        duration_seconds,
        &forced_render_options_json,
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
    #[serde(default)]
    ecdc_descriptor: Option<record_core::PayloadDescriptor>,
    #[serde(default)]
    ecdc_descriptors: Vec<record_core::PayloadDescriptor>,
    #[serde(default)]
    entry_descriptor_indexes: Vec<u8>,
    tracks: Vec<ProgrammeTrackInputJson>,
    #[serde(default)]
    track_gaps: Vec<ProgrammeTrackGapInputJson>,
}

/// The assembled, ordered programme: one or more compatible ECDC descriptors,
/// ordered entry bytes (every entry is ECDC — music revolutions and inter-track
/// gap ambience alike), their per-entry descriptor indexes, the musical
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
    let descriptors = if programme.ecdc_descriptors.is_empty() {
        vec![programme
            .ecdc_descriptor
            .context("programme requires ecdcDescriptor or ecdcDescriptors")?]
    } else {
        programme.ecdc_descriptors
    };
    if descriptors.len() > 256 {
        bail!("programme supports at most 256 ECDC descriptors");
    }
    // Each descriptor must be independently canonical. Mixed 6/12 kbps
    // programmes may differ in codec metadata, but their PCM/revolution
    // geometry must agree so one ordered record timeline remains well-defined.
    for (descriptor_index, descriptor) in descriptors.iter().enumerate() {
        validate_programme_ecdc_descriptor(descriptor).with_context(|| {
            format!(
                "programme ECDC descriptor {descriptor_index} is not a complete canonical ECDC descriptor"
            )
        })?;
    }
    let primary_descriptor = descriptors
        .first()
        .context("programme requires at least one ECDC descriptor")?;
    let sample_rate = primary_descriptor
        .sample_rate
        .context("ecdcDescriptor requires sampleRate")?;
    let channels = primary_descriptor
        .channels
        .context("ecdcDescriptor requires channels")?;
    for (descriptor_index, descriptor) in descriptors.iter().enumerate().skip(1) {
        if descriptor.sample_rate != Some(sample_rate)
            || descriptor.channels != Some(channels)
            || descriptor.block_samples != primary_descriptor.block_samples
            || descriptor.output_offset_samples != primary_descriptor.output_offset_samples
            || descriptor.output_samples != primary_descriptor.output_samples
        {
            bail!(
                "programme ECDC descriptor {descriptor_index} has incompatible PCM/revolution geometry"
            );
        }
    }
    // Validates the profile name even though gap geometry is no longer derived
    // here (gaps are now real ECDC entries supplied by the caller).
    record_core::normalize_record_profile_name(record_profile)?;

    if programme.tracks.is_empty() {
        bail!("programme must contain at least one musical track");
    }

    let total_ecdc = ecdc_entries.len();
    let entry_descriptor_indexes = if programme.entry_descriptor_indexes.is_empty() {
        vec![0u8; total_ecdc]
    } else {
        if programme.entry_descriptor_indexes.len() != total_ecdc {
            bail!(
                "entryDescriptorIndexes length {} does not match supplied ECDC entry count {}",
                programme.entry_descriptor_indexes.len(),
                total_ecdc
            );
        }
        programme.entry_descriptor_indexes
    };
    for (entry_index, descriptor_index) in entry_descriptor_indexes.iter().enumerate() {
        if usize::from(*descriptor_index) >= descriptors.len() {
            bail!(
                "entry descriptor index {} for entry {} exceeds descriptor count {}",
                descriptor_index,
                entry_index,
                descriptors.len()
            );
        }
    }
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
    // ECDC payload under its selected descriptor. The exact programme sample
    // timeline is the sum of every entry's decoded ECDC sample count; gaps
    // contribute their real encoded duration just like music.
    let mut total_samples = 0u64;
    for (entry, descriptor_index) in ecdc_entries.iter().zip(&entry_descriptor_indexes) {
        let descriptor = &descriptors[usize::from(*descriptor_index)];
        total_samples = total_samples.saturating_add(
            record_core::ecdc::headerless_entry_sample_count(entry, descriptor)
                .unwrap_or_else(|_| u64::from(descriptor.output_samples.unwrap_or(0))),
        );
    }

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
        payload_descriptors: vec![PayloadDescriptorInput::from_container(
            normalized_container.to_string(),
        )],
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

fn decode_record_header_json(png_bytes: &[u8], record_profile: Option<&str>) -> Result<String> {
    let decoded = decode_record_png_resolving_profile(png_bytes, record_profile)
        .context("failed to decode Bitneedle record PNG")?;

    let stream = record_core::parse_chunk_stream(&decoded.chunk_stream.bytes)
        .context("failed to parse decoded chunk stream")?;
    let payload = record_core::chunk_stream_payload_bytes(&stream);
    let stream_metadata = stream.metadata.clone();
    let descriptor = &decoded.descriptor;

    let mut value =
        serde_json::to_value(descriptor).context("failed to serialize decoded descriptor")?;
    if let Some(object) = value.as_object_mut() {
        object.insert("version".to_string(), json!(descriptor.version));
        object.insert(
            "checksumProtected".to_string(),
            json!(descriptor.checksum_protected),
        );
        object.insert("recordProfile".to_string(), json!(decoded.record_profile));
        object.insert(
            "streamByteLength".to_string(),
            json!(decoded.chunk_stream.bytes.len()),
        );
        object.insert("payloadByteLength".to_string(), json!(payload.len()));
        object.insert(
            "payloadEncoding".to_string(),
            json!(descriptor.payload_encoding),
        );
        object.insert(
            "descriptorMagic".to_string(),
            json!(record_descriptor_magic()),
        );
        object.insert(
            "chunkStreamMagic".to_string(),
            json!(String::from_utf8_lossy(record_core::RECORD_STREAM_MAGIC).to_string()),
        );
        object.insert(
            "chunkStream".to_string(),
            json!({
                "byteLength": decoded.chunk_stream.bytes.len(),
                "metadataByteLength": stream.metadata_bytes.len(),
                "metadata": stream_metadata,
                "chunkCount": stream.chunks.len(),
                "payloadByteLength": payload.len(),
                "payloadSha256": sha256_base64url(&payload),
            }),
        );
    }

    serde_json::to_string(&value).context("failed to serialize decoded header JSON")
}

fn optional_record_profile(record_profile: Option<&str>) -> Option<&str> {
    record_profile
        .map(str::trim)
        .filter(|profile| !profile.is_empty() && !profile.eq_ignore_ascii_case("auto"))
}

fn record_profile_candidates(record_profile: Option<&str>) -> Result<Vec<String>> {
    let mut seen = BTreeSet::new();
    let mut candidates = Vec::new();

    if let Some(profile) = optional_record_profile(record_profile) {
        let normalized = record_core::normalize_record_profile_name(profile)?;
        seen.insert(normalized.clone());
        candidates.push(normalized);
    }

    for &profile in record_core::known_record_profile_names() {
        let normalized = record_core::normalize_record_profile_name(profile)?;
        if seen.insert(normalized.clone()) {
            candidates.push(normalized);
        }
    }

    Ok(candidates)
}

fn decode_record_descriptor_resolving_profile(
    png_bytes: &[u8],
    record_profile: Option<&str>,
) -> Result<(String, record_descriptor::RecordDescriptor)> {
    let mut failures = Vec::new();

    for profile in record_profile_candidates(record_profile)? {
        match record_decode::decode_record_descriptor_from_png(png_bytes, Some(&profile)) {
            Ok((normalized_profile, descriptor)) => return Ok((normalized_profile, descriptor)),
            Err(error) => failures.push(format!("{profile}: {error:#}")),
        }
    }

    bail!(
        "failed to decode record descriptor for known profiles; tried {}",
        failures.join(" | ")
    )
}

fn decode_record_png_resolving_profile(
    png_bytes: &[u8],
    record_profile: Option<&str>,
) -> Result<record_decode::DecodedRecord> {
    let (normalized_profile, descriptor) =
        decode_record_descriptor_resolving_profile(png_bytes, record_profile)?;
    let chunk_stream = record_decode::decode_record_png_to_chunk_stream_for_profile_with_length(
        png_bytes,
        &normalized_profile,
        Some(descriptor.stream_byte_length),
    )
    .context("failed to decode Bitneedle record chunk stream")?;

    Ok(record_decode::DecodedRecord {
        record_profile: normalized_profile,
        descriptor,
        chunk_stream,
    })
}

fn sha256_base64url(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    base64_url_encode(&digest)
}

fn base64_url_encode(bytes: &[u8]) -> String {
    general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

const PRESS_RECORD_CAPACITY_BYTES_SINGLE45: f64 = 450_000.0;
const PRESS_RECORD_CAPACITY_BYTES_LP: f64 = 600_000.0;

#[derive(Debug, Clone, Copy)]
struct PressByteRateRange {
    min_bytes_per_second: f64,
    max_bytes_per_second: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PressRecordFormat {
    profile: String,
    quality: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PressRecordDurationEstimate {
    best_seconds: f64,
    worst_seconds: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PressRecordDurationHint {
    range: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PressRecordFormatCandidate {
    profile: String,
    quality: String,
    estimate: PressRecordDurationEstimate,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PressRecordFormatRecommendation {
    selected: PressRecordFormat,
    selected_estimate: PressRecordDurationEstimate,
    recommended: PressRecordFormatCandidate,
    duration_seconds: f64,
    fits_recommended: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PressRecordFormatRecommendationInput {
    duration_seconds: Option<f64>,
    current_profile: Option<String>,
    current_quality: Option<String>,
    allowed_profiles: Option<Vec<String>>,
    allowed_qualities: Option<Vec<String>>,
    recommendation_options: Option<Vec<PressRecordFormat>>,
}

fn press_record_duration_estimate(
    record_profile: &str,
    quality: &str,
) -> Result<PressRecordDurationEstimate> {
    let profile = press_normalize_record_profile(record_profile)?;
    let quality = press_normalize_encode_quality(quality);
    let Some(range) = press_byte_rate_range(&profile, quality) else {
        return Ok(PressRecordDurationEstimate {
            best_seconds: 0.0,
            worst_seconds: 0.0,
        });
    };
    let capacity = press_record_capacity_bytes(&profile);
    Ok(PressRecordDurationEstimate {
        best_seconds: capacity / range.min_bytes_per_second,
        worst_seconds: capacity / range.max_bytes_per_second,
    })
}

fn press_record_duration_hint(
    record_profile: &str,
    quality: &str,
) -> Result<PressRecordDurationHint> {
    let estimate = press_record_duration_estimate(record_profile, quality)?;
    let range = if estimate.worst_seconds > 0.0 && estimate.best_seconds > 0.0 {
        press_format_minute_range(estimate.worst_seconds, estimate.best_seconds)
    } else {
        "--".to_string()
    };
    Ok(PressRecordDurationHint { range })
}

fn press_record_format_recommendation_json(options_json: &str) -> Result<String> {
    let input: PressRecordFormatRecommendationInput = serde_json::from_str(options_json)
        .context("press record format recommendation options are invalid")?;
    let recommendation = press_record_format_recommendation(&input)?;
    serde_json::to_string(&recommendation).context("failed to serialize format recommendation")
}

fn press_record_format_recommendation(
    input: &PressRecordFormatRecommendationInput,
) -> Result<Option<PressRecordFormatRecommendation>> {
    let safe_duration = finite_nonnegative(input.duration_seconds.unwrap_or(0.0));
    let selected = PressRecordFormat {
        profile: press_normalize_record_profile(
            input.current_profile.as_deref().unwrap_or("single45"),
        )?,
        quality: press_normalize_encode_quality(input.current_quality.as_deref().unwrap_or("uq"))
            .to_string(),
    };
    let selected_estimate = press_record_duration_estimate(&selected.profile, &selected.quality)?;
    if safe_duration <= selected_estimate.worst_seconds {
        return Ok(None);
    }

    let allowed_profiles = input
        .allowed_profiles
        .as_ref()
        .map(|profiles| {
            profiles
                .iter()
                .map(|profile| press_normalize_record_profile(profile))
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?;
    let allowed_qualities = input.allowed_qualities.as_ref().map(|qualities| {
        qualities
            .iter()
            .map(|quality| press_normalize_encode_quality(quality).to_string())
            .collect::<Vec<_>>()
    });

    let candidate_options = input
        .recommendation_options
        .clone()
        .unwrap_or_else(default_press_recommendation_options)
        .into_iter()
        .map(|option| {
            Ok(PressRecordFormat {
                profile: press_normalize_record_profile(&option.profile)?,
                quality: press_normalize_encode_quality(&option.quality).to_string(),
            })
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .filter(|option| {
            allowed_profiles.as_ref().map_or(true, |profiles| {
                profiles.iter().any(|profile| profile == &option.profile)
            })
        })
        .filter(|option| {
            allowed_qualities.as_ref().map_or(true, |qualities| {
                qualities.iter().any(|quality| quality == &option.quality)
            })
        })
        .collect::<Vec<_>>();

    let candidates = candidate_options
        .iter()
        .map(|option| {
            Ok(PressRecordFormatCandidate {
                profile: option.profile.clone(),
                quality: option.quality.clone(),
                estimate: press_record_duration_estimate(&option.profile, &option.quality)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let recommended = candidates
        .iter()
        .find(|option| option.estimate.worst_seconds >= safe_duration)
        .cloned()
        .or_else(|| {
            let mut sorted = candidates.clone();
            sorted.sort_by(|a, b| {
                b.estimate
                    .worst_seconds
                    .partial_cmp(&a.estimate.worst_seconds)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            sorted.into_iter().next()
        });
    let Some(recommended) = recommended else {
        return Ok(None);
    };
    if recommended.profile == selected.profile && recommended.quality == selected.quality {
        return Ok(None);
    }

    Ok(Some(PressRecordFormatRecommendation {
        selected,
        selected_estimate,
        fits_recommended: recommended.estimate.worst_seconds >= safe_duration,
        recommended,
        duration_seconds: safe_duration,
    }))
}

fn press_certain_lp_record_format_json(
    duration_seconds: f64,
    current_profile: &str,
    current_quality: &str,
) -> Result<String> {
    serde_json::to_string(&press_certain_lp_record_format(
        duration_seconds,
        current_profile,
        current_quality,
    )?)
    .context("failed to serialize LP format recommendation")
}

fn press_certain_lp_record_format(
    duration_seconds: f64,
    current_profile: &str,
    current_quality: &str,
) -> Result<Option<PressRecordFormatRecommendation>> {
    let safe_duration = finite_nonnegative(duration_seconds);
    let selected = PressRecordFormat {
        profile: press_normalize_record_profile(current_profile)?,
        quality: press_normalize_encode_quality(current_quality).to_string(),
    };
    if selected.profile != "single45" {
        return Ok(None);
    }
    let selected_estimate = press_record_duration_estimate(&selected.profile, &selected.quality)?;
    if safe_duration <= selected_estimate.best_seconds {
        return Ok(None);
    }
    let recommended = PressRecordFormatCandidate {
        profile: "lp".to_string(),
        quality: selected.quality.clone(),
        estimate: press_record_duration_estimate("lp", &selected.quality)?,
    };
    Ok(Some(PressRecordFormatRecommendation {
        selected,
        selected_estimate,
        fits_recommended: recommended.estimate.worst_seconds >= safe_duration,
        recommended,
        duration_seconds: safe_duration,
    }))
}

fn press_normalize_record_profile(record_profile: &str) -> Result<String> {
    record_core::normalize_record_profile_name(record_profile).context("unknown record profile")
}

fn press_normalize_encode_quality(quality: &str) -> &'static str {
    match quality.trim().to_ascii_lowercase().as_str() {
        "uq" | "ultra" | "ultraquality" | "best" | "12" | "12k" | "12kbps" => "uq",
        "hq" | "mq" | "medium" | "standard" | "6" | "6k" | "6kbps" => "hq",
        "lq" | "low" | "nano" | "moss" | "mossnano" | "moss-nano" => "lq",
        _ => "uq",
    }
}

fn press_record_capacity_bytes(record_profile: &str) -> f64 {
    if record_profile == "lp" {
        PRESS_RECORD_CAPACITY_BYTES_LP
    } else {
        PRESS_RECORD_CAPACITY_BYTES_SINGLE45
    }
}

fn press_byte_rate_range(record_profile: &str, quality: &str) -> Option<PressByteRateRange> {
    let range = match (record_profile, quality) {
        ("single45", "uq") => PressByteRateRange {
            min_bytes_per_second: 1159.97333,
            max_bytes_per_second: 1264.06513,
        },
        ("lp", "uq") => PressByteRateRange {
            min_bytes_per_second: 1119.551319,
            max_bytes_per_second: 1232.685501,
        },
        ("single45", "hq") => PressByteRateRange {
            min_bytes_per_second: 559.381276,
            max_bytes_per_second: 622.372297,
        },
        ("lp", "hq") => PressByteRateRange {
            min_bytes_per_second: 535.352909,
            max_bytes_per_second: 569.00128,
        },
        ("single45", "lq") => PressByteRateRange {
            min_bytes_per_second: 250.087318,
            max_bytes_per_second: 250.341827,
        },
        ("lp", "lq") => PressByteRateRange {
            min_bytes_per_second: 250.047457,
            max_bytes_per_second: 250.086207,
        },
        _ => return None,
    };
    Some(range)
}

fn default_press_recommendation_options() -> Vec<PressRecordFormat> {
    [
        ("single45", "uq"),
        ("lp", "uq"),
        ("single45", "hq"),
        ("lp", "hq"),
        ("single45", "lq"),
        ("lp", "lq"),
    ]
    .into_iter()
    .map(|(profile, quality)| PressRecordFormat {
        profile: profile.to_string(),
        quality: quality.to_string(),
    })
    .collect()
}

fn press_format_minute_range(low_seconds: f64, high_seconds: f64) -> String {
    let low = press_format_minute_bucket(low_seconds);
    let high = press_format_minute_bucket(high_seconds);
    if low == high {
        low
    } else {
        format!("{low}-{high}")
    }
}

fn press_format_minute_bucket(seconds: f64) -> String {
    format!(
        "{}m",
        press_format_decimal(finite_nonnegative(seconds) / 60.0, 1)
    )
}

fn press_format_decimal(value: f64, digits: usize) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    let mut text = format!("{value:.digits$}");
    if text.contains('.') {
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }
    if text == "-0" {
        "0".to_string()
    } else {
        text
    }
}

fn finite_nonnegative(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

fn record_profile_spec(record_profile: &str) -> Result<ProfileSpec> {
    let geometry = record_core::describe_record_profile(record_profile)?;
    let name = geometry.record_profile.clone();

    Ok(ProfileSpec {
        name: name.clone(),
        record_profile: name.clone(),
        label: record_profile_label(&name).to_string(),
        rpm: record_profile_rpm(&name),
        spindle_hole_radius: geometry.spindle_hole_radius,
        label_radius: geometry.label_radius,
        label_clearance: record_core::label_clearance_from_profile_geometry(&geometry),
        payload_inner_radius: geometry.payload_inner_radius,
        payload_outer_radius: geometry.payload_outer_radius,
        lead_in_outer_radius: geometry.payload_outer_radius + geometry.lead_in_band_thickness,
        outer_radius: geometry.outer_radius,
        outer_rim_thickness: geometry.outer_rim_thickness,
        lead_in_band_thickness: geometry.lead_in_band_thickness,
        lead_in_turns: record_core::HEADER_SPIRAL_TURNS,
        run_out_turns: record_core::TRAILER_SPIRAL_TURNS,
    })
}

fn record_profile_label(record_profile: &str) -> &'static str {
    match record_profile {
        "lp" => "LP",
        _ => "45",
    }
}

fn record_profile_rpm(record_profile: &str) -> f64 {
    match record_profile {
        "lp" => 33.3333333333,
        _ => 45.0,
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
    fn fast_preview_can_hold_geometry_for_progressive_entries() {
        let payload_entries: Vec<u8> = vec![
            0, 0, 0, 20, 142, 148, 51, 24, 50, 43, 204, 119, 248, 149, 116, 149, 137, 70, 212, 74,
            142, 224, 150, 228, 184, 207, 69, 0,
        ];
        let descriptor_json = r##"{"container":"ECDC","codec":"ECDC","sampleRate":48000,"channels":2,"blockSamples":64960,"outputOffsetSamples":480,"outputSamples":64000,"codecMetadata":[123,125]}"##;
        let result = render_payload_entries_with_descriptor_to_png(
            vec![payload_entries],
            descriptor_json,
            "rgb",
            "single45",
            211.33060416666666,
            r##"{"fastFit":true,"fitTrackPixelCount":12000}"##,
        )
        .expect("progressive fast-fit render should accept a fixed track-pixel target");
        let payload: serde_json::Value =
            serde_json::from_str(&result.payload_json()).expect("render payload JSON");
        assert_eq!(payload["fitTrackPixelCount"].as_u64(), Some(12_000));
        assert!(payload["filteredPixelCount"].as_u64().unwrap_or_default() < 12_000);
    }

    #[test]
    fn patternized_record_restores_exact_payload_before_decode() {
        let payload_entry: Vec<u8> = vec![
            0, 0, 0, 20, 142, 148, 51, 24, 50, 43, 204, 119, 248, 149, 116, 149, 137, 70, 212, 74,
            142, 224, 150, 228, 184, 207, 69, 0,
        ];
        let descriptor_json = r##"{"container":"ECDC","codec":"ECDC","sampleRate":48000,"channels":2,"blockSamples":64960,"outputOffsetSamples":480,"outputSamples":64000,"codecMetadata":[123,34,109,34,58,34,101,110,99,111,100,101,99,95,52,56,107,104,122,34,44,34,97,108,34,58,54,52,48,48,48,44,34,110,99,34,58,56,44,34,108,109,34,58,116,114,117,101,44,34,102,112,34,58,56,49,57,50,44,34,109,114,34,58,50,44,34,97,99,118,34,58,50,44,34,116,97,117,34,58,49,46,48,44,34,108,109,104,34,58,34,98,56,99,50,49,100,54,54,53,48,98,54,50,97,48,98,56,99,100,50,49,48,99,54,101,54,50,52,100,102,98,98,51,48,99,51,97,51,57,56,57,52,54,54,53,52,52,98,49,102,53,101,49,100,102,57,54,51,101,99,97,53,49,55,34,44,34,102,108,34,58,50,48,51,125]}"##;
        let rendered = render_payload_entries_with_descriptor_to_png(
            vec![payload_entry],
            descriptor_json,
            "rgb",
            "single45",
            211.33060416666666,
            r##"{"trackListing":[{"number":1,"durationSeconds":0,"startSeconds":0,"endSeconds":0}],"fastFit":true}"##,
        )
        .expect("fixture record should render");
        let original = record_decode::decode_record_png_to_chunk_stream_for_profile(
            &rendered.png_bytes,
            "single45",
        )
        .expect("ordinary record should decode");

        let patternized = patternize_record_png_explore(
            &rendered.png_bytes,
            r##"{"layoutId":7,"blockSize":8,"amount":100,"embedSidecar":true,"sidecarItems":[{"type":"json","codec":"raw","name":"bitneedle-label-thumbnail-v1.json","json":{"version":1,"role":"label-thumbnail-patch","width":32,"height":32,"base":"full-label-thumbnail-avif","patchItemName":"bitneedle-label-thumbnail-patch.avif","patchMime":"image/avif"}},{"type":"image","codec":"avif","name":"bitneedle-label-thumbnail-patch.avif","mime":"image/avif","rawByteLength":16,"dataBase64":"AAAAAGZ0eXBhdmlmAAAAAA=="}]}"##,
            Some("single45"),
        )
        .expect("Patternize should embed a reversible map");
        assert_ne!(patternized.png_bytes, rendered.png_bytes);
        let report: serde_json::Value = serde_json::from_str(&patternized.payload_json)
            .expect("Patternize report should parse");
        assert_eq!(
            report.pointer("/storage/encodedSidecar/ok"),
            Some(&serde_json::Value::Bool(true))
        );
        assert_eq!(
            report.pointer("/storage/encodedSidecar/mapRoundTrips"),
            Some(&serde_json::Value::Bool(true))
        );

        let restored = record_sidecar::restore_patternized_record_png(
            &patternized.png_bytes,
            Some("single45"),
        )
        .expect("reverse-map sidecar should decode")
        .expect("Patternize map should be present");
        let decoded_items: serde_json::Value = serde_json::from_str(
            &record_sidecar::decode_record_png_sidecar_items_json(
                &patternized.png_bytes,
                Some("single45"),
            )
            .expect("sidecar items should decode"),
        )
        .expect("decoded sidecar items should parse");
        let items = decoded_items
            .get("items")
            .and_then(serde_json::Value::as_array)
            .expect("decoded sidecar items should be an array");
        assert!(items.iter().any(|item| {
            item.get("name").and_then(serde_json::Value::as_str)
                == Some("bitneedle-label-thumbnail-v1.json")
        }));
        assert!(items.iter().any(|item| {
            item.get("name").and_then(serde_json::Value::as_str)
                == Some(record_sidecar::PACKAGE_PATTERN_SIDECAR_ITEM_NAME)
        }));
        let decoded =
            record_decode::decode_record_png_to_chunk_stream_for_profile(&restored, "single45")
                .expect("restored record should decode");
        assert_eq!(decoded.bytes, original.bytes);
        assert_eq!(decoded.pixel_count, original.pixel_count);

        let (rewritten, _summary) = rewrite_record_png_preserving_pattern_items(
            &patternized.png_bytes,
            r##"{"sidecar":{"items":[{"type":"json","codec":"raw","name":"bitneedle-issuance-v1.json","json":{"kind":"bitneedle-issuance-v1"}}],"carriers":["label","intergroove","leadInDeadwax"]}}"##,
            Some("single45"),
        )
        .expect("rewrite should preserve Patternize map");
        let rewritten_items: serde_json::Value = serde_json::from_str(
            &record_sidecar::decode_record_png_sidecar_items_json(&rewritten, Some("single45"))
                .expect("rewritten sidecar items should decode"),
        )
        .expect("rewritten sidecar items should parse");
        let rewritten_items = rewritten_items
            .get("items")
            .and_then(serde_json::Value::as_array)
            .expect("rewritten sidecar items should be an array");
        assert!(rewritten_items.iter().any(|item| {
            item.get("name").and_then(serde_json::Value::as_str)
                == Some("bitneedle-issuance-v1.json")
        }));
        assert!(rewritten_items.iter().any(|item| {
            item.get("name").and_then(serde_json::Value::as_str)
                == Some(record_sidecar::PACKAGE_PATTERN_SIDECAR_ITEM_NAME)
        }));
        let restored_rewritten =
            record_sidecar::restore_patternized_record_png(&rewritten, Some("single45"))
                .expect("rewritten reverse-map sidecar should decode")
                .expect("rewritten Patternize map should be present");
        let decoded_rewritten = record_decode::decode_record_png_to_chunk_stream_for_profile(
            &restored_rewritten,
            "single45",
        )
        .expect("rewritten restored record should decode");
        assert_eq!(decoded_rewritten.bytes, original.bytes);
        assert_eq!(decoded_rewritten.pixel_count, original.pixel_count);
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
    fn programme_preserves_mixed_ecdc_descriptor_indexes() {
        let second_descriptor = PROGRAMME_ECDC_DESCRIPTOR.replace(
            r#""codecMetadata":[123,125]"#,
            r#""codecMetadata":[123,32,125]"#,
        );
        let json = format!(
            r#"{{"ecdcDescriptor":{PROGRAMME_ECDC_DESCRIPTOR},"ecdcDescriptors":[{PROGRAMME_ECDC_DESCRIPTOR},{second_descriptor}],"entryDescriptorIndexes":[0,0,1,1],"tracks":[{{"title":"12 kbps","payloadIndexes":[0,1]}},{{"title":"6 kbps","payloadIndexes":[2,3]}}]}}"#
        );
        let assembled = assemble_record_programme(
            vec![
                vec![0xA1u8; 8],
                vec![0xA2u8; 8],
                vec![0xB1u8; 8],
                vec![0xB2u8; 8],
            ],
            &json,
            "single45",
        )
        .unwrap();

        assert_eq!(assembled.descriptors.len(), 2);
        assert_eq!(assembled.entry_descriptor_indexes, vec![0, 0, 1, 1]);

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
        assert_eq!(stream.metadata.payload_descriptors.len(), 2);
        assert_eq!(
            stream
                .metadata
                .payload_entries
                .iter()
                .map(|entry| entry.payload_descriptor_index)
                .collect::<Vec<_>>(),
            vec![0, 0, 1, 1]
        );
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
