// Copyright © Wavey, Inc.
// Licensed under the Wavey Artist Source Licence.
// Patent pending. All patent rights are reserved except as expressly granted by the licence.
// Commercial licensing: licence@yl.vin

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose, Engine as _};
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{load_from_memory, ExtendedColorType, ImageEncoder};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

pub const SIDECAR_MAGIC: &[u8; 4] = b"BSC1";
pub const SIDECAR_CONTAINER_VERSION: u8 = 1;
pub const SIDECAR_POINTER_VERSION: u8 = 1;
pub const SIDECAR_POINTER_LENGTH: usize = 48;
pub const SIDECAR_POINTER_SCHEME_PAIRSIGN_SAFE_LUMA_V2: u8 = 1;
pub const SIDECAR_POINTER_CARRIER_LABEL: u8 = 0x01;
pub const SIDECAR_POINTER_CARRIER_INTERGROOVE: u8 = 0x02;
pub const SIDECAR_POINTER_CARRIER_LEAD_IN_DEADWAX: u8 = 0x04;
pub const SIDECAR_SCHEME_PAIRSIGN_SAFE_LUMA_V2: &str = "pairsign-safe-luma-v2";
pub const SIDECAR_DEFAULT_SEED: u32 = 0x4b50_4752;
pub const SIDECAR_PAIR_SIGN_DELTA: i16 = 4;
pub const SIDECAR_PAIR_MAGNITUDE_DELTA: i16 = 12;
pub const SIDECAR_PAIR_MAGNITUDE_THRESHOLD: f64 = 16.0;
pub const SIDECAR_SAFE_V2_MIN_SCORE: u16 = 20;
pub const SIDECAR_TYPE_OPAQUE: u8 = 0;
pub const SIDECAR_TYPE_UTF8_TEXT: u8 = 1;
pub const SIDECAR_TYPE_IMAGE: u8 = 2;
pub const SIDECAR_TYPE_JSON: u8 = 3;
pub const SIDECAR_CODEC_RAW: u8 = 0;
pub const SIDECAR_CODEC_BROTLI: u8 = 1;
pub const SIDECAR_CODEC_ZSTD: u8 = 2;
pub const SIDECAR_CODEC_AVIF: u8 = 3;
pub const SIDECAR_RAW_LENGTH_ABSENT: u32 = u32::MAX;
pub const DISPLAY_HEADER_MAGIC: &[u8; 4] = b"BDH1";
pub const DISPLAY_HEADER_VERSION: u8 = 1;
pub const DISPLAY_HEADER_LENGTH: usize = 128;
pub const DISPLAY_HEADER_NAME: &str = "bitneedle-display-header.bin";
pub const DISPLAY_HEADER_MIME: &str = "application/vnd.bitneedle.display-header";
pub const PACKAGE_METADATA_ITEM_NAME: &str = "bitneedle-package-metadata.json";
pub const PACKAGE_METADATA_MIME: &str = "application/vnd.bitneedle.package-metadata+json";
pub const PACKAGE_PHOTO_MIME: &str = "image/avif";
pub const PACKAGE_COVER_ITEM_NAME: &str = "album-cover.avif";
pub const PACKAGE_PATTERN_SIDECAR_ITEM_NAME: &str = "bitneedle-pattern-map";
pub const PACKAGE_PATTERN_SIDECAR_MIME: &str = "application/vnd.bitneedle.pattern-map";
const DISPLAY_HEADER_FLAG_COVER_SHOWN: u16 = 1 << 0;
const DISPLAY_HEADER_FLAG_COVER_EFFECTS: u16 = 1 << 1;
const DISPLAY_HEADER_FLAG_COVER_RGB_GRAIN: u16 = 1 << 2;
const DISPLAY_HEADER_FLAG_INNER_SLEEVE_SHOWN: u16 = 1 << 3;
const DISPLAY_HEADER_FLAG_COVER_EMBEDDED: u16 = 1 << 4;
const DEFAULT_COVER_POSTERIZE_LEVELS: f64 = 64.0;
const DEFAULT_COVER_SATURATION: f64 = 2.0;
const DEFAULT_COVER_CONTRAST: f64 = 1.3;
const DEFAULT_COVER_RGB_GRAIN_OPACITY: f64 = 0.84;
const COVER_RGB_GRAIN_OPACITY_MIN: f64 = 0.4;
const COVER_CROP_ZOOM_MIN: f64 = 1.0;
const COVER_CROP_ZOOM_MAX: f64 = 6.0;
const COVER_PREVIEW_SIZE: f64 = 576.0;
const PACKAGE_PHOTO_MAX_INNER: u32 = 576;
const PACKAGE_PHOTO_MAX_QUANTIZER: u8 = 63;
const PACKAGE_PHOTO_DEFAULT_QUANTIZER: u8 = 32;
const PACKAGE_PHOTO_SEARCH_MIN_QUANTIZER: u8 = 35;
const PACKAGE_PHOTO_SEARCH_START_QUANTIZER: u8 = 47;
const PACKAGE_PHOTO_SEARCH_REFINEMENT_RADIUS: u8 = 2;
const PACKAGE_COVER_MAX_BUDGET_DIVISOR: u64 = 3;
const PACKAGE_SIDECAR_SCHEME: &str = "pairsign-safe-luma-v2";
const PACKAGE_SIDECAR_CARRIERS: [&str; 2] = ["label", "intergroove"];
const PACKAGE_COLOR_MODE_RGB: &str = "rgb";
const PACKAGE_COLOR_MODE_GRAYSCALE: &str = "grayscale";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarRenderSummary {
    pub container: String,
    pub scheme: String,
    pub carriers: Vec<String>,
    pub seed: u32,
    pub bsc1_bytes: usize,
    pub sha256: String,
    pub carrier_pixels: usize,
    pub carrier_pairs: usize,
    pub capacity_bytes: usize,
    pub used_pairs: usize,
    pub unused_pairs: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarRenderOptions {
    pub scheme: Option<String>,
    pub label_tuning: Option<SidecarLabelTuningOptions>,
    pub seed: Option<u32>,
    pub carriers: Option<Vec<String>>,
    pub bsc1_base64: Option<String>,
    pub items: Option<Vec<SidecarItemInput>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarLabelTuningOptions {
    pub enabled: Option<bool>,
    pub strength: Option<f64>,
    pub target_luma: Option<f64>,
    pub grain: Option<i16>,
}

#[derive(Debug, Clone)]
pub struct PreparedSidecarLabelTuning {
    pub strength: f64,
    pub target_luma: f64,
    pub grain: i16,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarItemInput {
    #[serde(rename = "type", alias = "itemType")]
    pub item_type: serde_json::Value,
    pub codec: serde_json::Value,
    pub name: Option<String>,
    pub mime: Option<String>,
    pub data_base64: Option<String>,
    pub text: Option<String>,
    pub json: Option<serde_json::Value>,
    pub raw_byte_length: Option<u32>,
    #[serde(default)]
    pub flags: u8,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarContainerValidation {
    pub ok: bool,
    pub version: u8,
    pub flags: u8,
    pub item_count: usize,
    pub total_length: usize,
    pub items: Vec<SidecarItemValidation>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarItemValidation {
    pub item_type: u8,
    pub item_type_name: String,
    pub codec: u8,
    pub codec_name: String,
    pub flags: u8,
    pub raw_byte_length: Option<u32>,
    pub stored_byte_length: u32,
    pub name: String,
    pub mime: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarDecodedItems {
    pub ok: bool,
    pub validation: SidecarContainerValidation,
    pub items: Vec<SidecarDecodedItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarDecodedItem {
    pub item_type: u8,
    pub item_type_name: String,
    pub codec: u8,
    pub codec_name: String,
    pub flags: u8,
    pub raw_byte_length: Option<u32>,
    pub stored_byte_length: u32,
    pub decoded_byte_length: usize,
    pub name: String,
    pub mime: String,
    pub stored_data_base64: String,
    pub data_base64: String,
    pub text: Option<String>,
    pub json: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageDisplayHeaderInput {
    pub cover_shown: Option<bool>,
    pub cover_effects: Option<bool>,
    pub cover_rgb_grain: Option<bool>,
    pub inner_sleeve_shown: Option<bool>,
    pub cover_embedded: Option<bool>,
    pub design_label: Option<String>,
    pub design_class_name: Option<String>,
    pub posterize: Option<f64>,
    pub saturation: Option<f64>,
    pub contrast: Option<f64>,
    pub rgb_grain_opacity: Option<f64>,
    pub rgb_grain_blend_index: Option<f64>,
    pub crop: Option<PackageDisplayHeaderCrop>,
    pub quantizer: Option<f64>,
    pub inner: Option<f64>,
    pub sleeve_tone_color: Option<String>,
    pub sleeve_sparkle_opacity: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageDisplayHeaderCrop {
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub zoom: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageMetadataInput {
    #[serde(default)]
    pub photos: Vec<PackageMetadataPhotoInput>,
    pub cover: Option<PackageMetadataCoverInput>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageMetadataPhotoInput {
    pub id: Option<String>,
    pub name: Option<String>,
    pub status: Option<String>,
    pub has_avif: Option<bool>,
    pub credit: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageMetadataCoverInput {
    pub has_file: Option<bool>,
    pub has_preview: Option<bool>,
    pub source_photo_id: Option<String>,
    pub source_width: Option<f64>,
    pub source_height: Option<f64>,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub inner: Option<f64>,
    pub crop: Option<PackageDisplayHeaderCrop>,
    pub embedded: Option<bool>,
    pub shown: Option<bool>,
    pub design_label: Option<String>,
    pub effects_enabled: Option<bool>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackagePhotoItemInput {
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageImageCacheKeyInput {
    pub kind: Option<String>,
    pub file_name: Option<String>,
    pub file_size: Option<serde_json::Value>,
    pub file_last_modified: Option<serde_json::Value>,
    pub quantizer: Option<serde_json::Value>,
    pub inner: Option<serde_json::Value>,
    pub crop: Option<serde_json::Value>,
    pub color_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageImageEncodeCacheKey {
    pub key: String,
    pub file_key: String,
    pub crop_key: String,
    pub quantizer: u8,
    pub inner: u32,
    pub crop: PackageNormalizedCrop,
    pub color_mode: String,
    pub monochrome: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageBestFitCacheKeyInput {
    pub kind: Option<String>,
    pub file_name: Option<String>,
    pub file_size: Option<serde_json::Value>,
    pub file_last_modified: Option<serde_json::Value>,
    pub budget_bytes: Option<serde_json::Value>,
    pub inner: Option<serde_json::Value>,
    pub crop_key: Option<String>,
    pub color_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageBestFitCacheKey {
    pub key: String,
    pub file_key: String,
    pub budget_bytes: u64,
    pub inner: u32,
    pub crop_key: String,
    pub color_mode: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageNormalizedCrop {
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageQuantizerSearchPlanInput {
    pub min_quantizer: Option<serde_json::Value>,
    pub max_quantizer: Option<serde_json::Value>,
    pub start_quantizer: Option<serde_json::Value>,
    pub refinement_radius: Option<serde_json::Value>,
    #[serde(default)]
    pub trials: Vec<PackageQuantizerTrialInput>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageQuantizerTrialInput {
    pub quantizer: Option<serde_json::Value>,
    pub fits: Option<bool>,
    pub budget_bytes: Option<serde_json::Value>,
    pub contribution_bytes: Option<serde_json::Value>,
    pub bsc1_bytes: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageQuantizerSearchPlan {
    pub min_quantizer: u8,
    pub max_quantizer: u8,
    pub start_quantizer: u8,
    pub done: bool,
    pub next_quantizer: Option<u8>,
    pub note: String,
    pub best_quantizer: Option<u8>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageFitBudgetInput {
    pub capacity: Option<serde_json::Value>,
    pub base_bsc1_bytes: Option<serde_json::Value>,
    pub photo_count: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageFitBudget {
    pub capacity: u64,
    pub base_bsc1_bytes: u64,
    pub available_bytes: u64,
    pub remaining_bytes: u64,
    pub cover_budget_bytes: u64,
    pub photo_count: u64,
    pub per_photo_budget_bytes: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageSidecarRenderOptionsInput {
    pub seed: Option<serde_json::Value>,
    pub capacity_bytes: Option<serde_json::Value>,
    pub payload_bytes: Option<serde_json::Value>,
    pub bsc1_base64: Option<String>,
    pub items: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarDecodeResult {
    pub ok: bool,
    pub descriptor: serde_json::Value,
    pub validation: SidecarContainerValidation,
    pub bsc1_byte_length: usize,
    pub sha256: String,
    pub carrier_pixels: usize,
    pub carrier_pairs: usize,
    pub capacity_bytes: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarCapacity {
    pub scheme: String,
    pub carriers: Vec<String>,
    pub carrier_pixels: usize,
    pub carrier_pairs: usize,
    pub capacity_bits: usize,
    pub capacity_bytes: usize,
    pub bits_per_pair: f64,
    pub two_bit_pairs: usize,
}

#[derive(Debug, Clone)]
pub struct SidecarHeaderPointer {
    pub scheme: String,
    pub carriers: Vec<SidecarCarrier>,
    pub seed: u32,
    pub length: usize,
    pub sha256: String,
    pub sha256_bytes: [u8; 32],
}

#[derive(Debug, Clone)]
pub struct PreparedSidecar {
    pub bytes: Vec<u8>,
    pub scheme: String,
    pub label_tuning: Option<PreparedSidecarLabelTuning>,
    pub carriers: Vec<SidecarCarrier>,
    pub seed: u32,
    pub sha256: String,
    pub sha256_bytes: [u8; 32],
}

#[derive(Debug, Clone)]
struct RecordPngContext {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
    record_profile: String,
    descriptor: record_descriptor::RecordDescriptor,
}

#[derive(Debug, Clone, Copy)]
struct SidecarCarrierRegions {
    label: bool,
    payload_intergroove: bool,
    lead_in_deadwax: bool,
}

// Text-avoid geometry was formerly carried in arbitrary BRD1 JSON metadata.
// The compact descriptor no longer carries that blob, so sidecar ordering is
// now independent of label compositor hints.
#[derive(Clone)]
struct TextAvoidSpec;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecordRewriteOptions {
    header_canonical_url: Option<String>,
    header_created_at: Option<u64>,
    header_release_commitment_sha256: Option<String>,
    header_signature_key_id: Option<String>,
    header_signature: Option<String>,
    sidecar: Option<SidecarRenderOptions>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SidecarCarrier {
    Label,
    Intergroove,
    LeadInDeadwax,
}

impl SidecarCarrier {
    pub fn name(self) -> &'static str {
        sidecar_carrier_name(self)
    }
}

/// The reserved item name the sidecar's own attestation is stored under.
pub const SIDECAR_ATTESTATION_ITEM_NAME: &str = "attestation";

const SIDECAR_ATTESTATION_DOMAIN: &[u8] = b"bitneedle.record-sidecar.attestation.v1";

/// What the sidecar's attestation signs.
///
/// The sidecar is the one part of a pressed record that is meant to be
/// rewritten — editions are issued, labels are re-authored — so its
/// signature cannot live in the immutable descriptor. It lives here, in the
/// sidecar itself, and is replaced along with everything else each time the
/// sidecar is written. Whoever does the writing signs what they wrote.
///
/// The preimage is the record's descriptor commitment followed by every
/// other item in stored order, each contributing its name and the digest of
/// its stored bytes. Binding to the descriptor is what stops a signed
/// sidecar being lifted off one record and dropped onto another; the
/// attestation item itself is excluded, because it cannot contain its own
/// signature.
pub fn sidecar_attestation_identity_bytes(
    descriptor_commitment: &[u8; 32],
    items: &[SidecarDecodedItem],
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(SIDECAR_ATTESTATION_DOMAIN);
    out.extend_from_slice(descriptor_commitment);
    let signed: Vec<&SidecarDecodedItem> = items
        .iter()
        .filter(|item| item.name != SIDECAR_ATTESTATION_ITEM_NAME)
        .collect();
    out.extend_from_slice(&(signed.len() as u32).to_be_bytes());
    for item in signed {
        out.push(item.item_type);
        out.push(item.codec);
        out.extend_from_slice(&(item.name.len() as u32).to_be_bytes());
        out.extend_from_slice(item.name.as_bytes());
        out.extend_from_slice(&(item.stored_byte_length).to_be_bytes());
        out.extend_from_slice(&sha256_digest_bytes(item.stored_data_base64.as_bytes()));
    }
    out
}

/// SHA-256 of [`sidecar_attestation_identity_bytes`]: the digest a sidecar
/// attestation carries and a signer signs.
pub fn sidecar_attestation_commitment(
    descriptor_commitment: &[u8; 32],
    items: &[SidecarDecodedItem],
) -> [u8; 32] {
    sha256_digest_bytes(&sidecar_attestation_identity_bytes(
        descriptor_commitment,
        items,
    ))
}

/// A sidecar's attestation, as it is stored and read back.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarAttestation {
    pub version: u8,
    /// Base64url, no padding: the digest from
    /// [`sidecar_attestation_commitment`].
    pub commitment: String,
    /// Who signed. Opaque to the sidecar; resolved by whoever verifies.
    pub key_id: String,
    /// Base64url, no padding: 64 raw Ed25519 bytes.
    pub signature: String,
}

pub const SIDECAR_ATTESTATION_VERSION: u8 = 1;

impl SidecarAttestation {
    pub fn validate(&self) -> Result<()> {
        if self.version != SIDECAR_ATTESTATION_VERSION {
            bail!("unsupported sidecar attestation version {}", self.version);
        }
        if self.key_id.trim().is_empty() {
            bail!("sidecar attestation key ID must not be empty");
        }
        if decode_attestation_field(&self.commitment, "commitment")?.len() != 32 {
            bail!("sidecar attestation commitment must be 32 bytes");
        }
        if decode_attestation_field(&self.signature, "signature")?.len() != 64 {
            bail!("sidecar attestation signature must be 64 bytes");
        }
        Ok(())
    }

    /// The digest this attestation claims, as bytes.
    pub fn commitment_bytes(&self) -> Result<[u8; 32]> {
        let bytes = decode_attestation_field(&self.commitment, "commitment")?;
        bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("sidecar attestation commitment must be 32 bytes"))
    }

    pub fn signature_bytes(&self) -> Result<Vec<u8>> {
        decode_attestation_field(&self.signature, "signature")
    }
}

fn decode_attestation_field(value: &str, label: &str) -> Result<Vec<u8>> {
    let trimmed = value.trim();
    general_purpose::URL_SAFE_NO_PAD
        .decode(trimmed)
        .or_else(|_| general_purpose::URL_SAFE.decode(trimmed))
        .or_else(|_| general_purpose::STANDARD.decode(trimmed))
        .with_context(|| format!("sidecar attestation {label} is not valid base64"))
}

/// The attestation carried by a decoded sidecar, if it carries one.
pub fn sidecar_attestation(items: &[SidecarDecodedItem]) -> Result<Option<SidecarAttestation>> {
    let Some(item) = items
        .iter()
        .find(|item| item.name == SIDECAR_ATTESTATION_ITEM_NAME)
    else {
        return Ok(None);
    };
    let json = item
        .json
        .as_ref()
        .context("the sidecar attestation item is not JSON")?;
    let attestation: SidecarAttestation = serde_json::from_value(json.clone())
        .context("the sidecar attestation is not readable")?;
    attestation.validate()?;
    Ok(Some(attestation))
}

/// Whether a sidecar's attestation is over the items actually present.
///
/// This is the structural half of verification: it says the signed digest
/// matches what the sidecar now contains and that it is bound to this
/// record. Whether the key is one to trust is a separate question, and one
/// this crate deliberately does not answer.
pub fn sidecar_attestation_covers(
    attestation: &SidecarAttestation,
    descriptor_commitment: &[u8; 32],
    items: &[SidecarDecodedItem],
) -> Result<bool> {
    Ok(attestation.commitment_bytes()?
        == sidecar_attestation_commitment(descriptor_commitment, items))
}

pub fn sha256_digest_bytes(bytes: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

pub fn sha256_base64url(bytes: &[u8]) -> String {
    general_purpose::URL_SAFE_NO_PAD.encode(sha256_digest_bytes(bytes))
}

pub fn decode_base64_text(value: &str, label: &str) -> Result<Vec<u8>> {
    let trimmed = value.trim();
    general_purpose::URL_SAFE_NO_PAD
        .decode(trimmed)
        .or_else(|_| general_purpose::URL_SAFE.decode(trimmed))
        .or_else(|_| general_purpose::STANDARD.decode(trimmed))
        .with_context(|| format!("{label} is not valid base64"))
}

pub fn sidecar_type_name(item_type: u8) -> String {
    match item_type {
        SIDECAR_TYPE_OPAQUE => "opaque".to_string(),
        SIDECAR_TYPE_UTF8_TEXT => "utf8Text".to_string(),
        SIDECAR_TYPE_IMAGE => "image".to_string(),
        SIDECAR_TYPE_JSON => "json".to_string(),
        value => format!("private:{value}"),
    }
}

pub fn sidecar_codec_name(codec: u8) -> String {
    match codec {
        SIDECAR_CODEC_RAW => "raw".to_string(),
        SIDECAR_CODEC_BROTLI => "brotli".to_string(),
        SIDECAR_CODEC_ZSTD => "zstd".to_string(),
        SIDECAR_CODEC_AVIF => "avif".to_string(),
        value => format!("private:{value}"),
    }
}

fn parse_sidecar_registry_value(
    value: &serde_json::Value,
    label: &str,
    names: &[(&str, u8)],
) -> Result<u8> {
    if let Some(number) = value.as_u64() {
        return u8::try_from(number).with_context(|| format!("{label} exceeds u8 range"));
    }
    let Some(raw) = value.as_str() else {
        bail!("{label} must be a string or integer");
    };
    let normalized = raw.trim().to_ascii_lowercase().replace([' ', '-', '_'], "");
    for (name, code) in names {
        if normalized == *name {
            return Ok(*code);
        }
    }
    bail!("unknown {label}: {raw}");
}

pub fn parse_sidecar_item_type(value: &serde_json::Value) -> Result<u8> {
    parse_sidecar_registry_value(
        value,
        "sidecar item type",
        &[
            ("opaque", SIDECAR_TYPE_OPAQUE),
            ("bytes", SIDECAR_TYPE_OPAQUE),
            ("binary", SIDECAR_TYPE_OPAQUE),
            ("utf8text", SIDECAR_TYPE_UTF8_TEXT),
            ("text", SIDECAR_TYPE_UTF8_TEXT),
            ("utf8", SIDECAR_TYPE_UTF8_TEXT),
            ("image", SIDECAR_TYPE_IMAGE),
            ("photo", SIDECAR_TYPE_IMAGE),
            ("json", SIDECAR_TYPE_JSON),
        ],
    )
}

pub fn parse_sidecar_codec(value: &serde_json::Value) -> Result<u8> {
    parse_sidecar_registry_value(
        value,
        "sidecar codec",
        &[
            ("raw", SIDECAR_CODEC_RAW),
            ("none", SIDECAR_CODEC_RAW),
            ("brotli", SIDECAR_CODEC_BROTLI),
            ("br", SIDECAR_CODEC_BROTLI),
            ("zstd", SIDECAR_CODEC_ZSTD),
            ("zstandard", SIDECAR_CODEC_ZSTD),
            ("avif", SIDECAR_CODEC_AVIF),
        ],
    )
}

pub fn validate_sidecar_registry_ranges(item_type: u8, codec: u8) -> Result<()> {
    if (4..=31).contains(&item_type) {
        bail!("sidecar item type {item_type} is reserved");
    }
    if (4..=31).contains(&codec) {
        bail!("sidecar codec {codec} is reserved");
    }
    Ok(())
}

pub fn validate_sidecar_name(value: &str, label: &str) -> Result<()> {
    if value.chars().any(|ch| {
        let code = ch as u32;
        code <= 0x1f || code == 0x7f
    }) {
        bail!("sidecar {label} must not contain control characters");
    }
    Ok(())
}

pub fn looks_like_avif(bytes: &[u8]) -> bool {
    if bytes.len() < 16 || &bytes[4..8] != b"ftyp" {
        return false;
    }
    bytes[8..]
        .chunks(4)
        .any(|chunk| chunk == b"avif" || chunk == b"avis")
}

pub fn validate_sidecar_item_payload(
    item_type: u8,
    codec: u8,
    raw_byte_length: u32,
    stored: &[u8],
    mime: &str,
) -> Result<()> {
    validate_sidecar_registry_ranges(item_type, codec)?;
    if codec == SIDECAR_CODEC_AVIF && item_type != SIDECAR_TYPE_IMAGE {
        bail!("AVIF sidecar codec is only valid for image items");
    }
    if item_type == SIDECAR_TYPE_IMAGE && codec != SIDECAR_CODEC_AVIF {
        bail!("image sidecar items must use AVIF in this version");
    }
    if matches!(item_type, SIDECAR_TYPE_UTF8_TEXT | SIDECAR_TYPE_JSON)
        && codec == SIDECAR_CODEC_AVIF
    {
        bail!("text and JSON sidecar items cannot use AVIF");
    }
    if codec == SIDECAR_CODEC_RAW && raw_byte_length != stored.len() as u32 {
        bail!("raw sidecar item rawByteLength must equal stored length");
    }
    if item_type == SIDECAR_TYPE_UTF8_TEXT && codec == SIDECAR_CODEC_RAW {
        std::str::from_utf8(stored).context("raw UTF-8 text sidecar item is not valid UTF-8")?;
    }
    if item_type == SIDECAR_TYPE_JSON && codec == SIDECAR_CODEC_RAW {
        serde_json::from_slice::<serde_json::Value>(stored)
            .context("raw JSON sidecar item is not valid JSON")?;
    }
    if item_type == SIDECAR_TYPE_IMAGE && codec == SIDECAR_CODEC_AVIF {
        if !looks_like_avif(stored) {
            bail!("AVIF sidecar image does not look like an AVIF file");
        }
        if !mime.is_empty() && !mime.eq_ignore_ascii_case("image/avif") {
            bail!("AVIF sidecar image MIME type must be image/avif");
        }
    }
    Ok(())
}

pub fn default_sidecar_mime(item_type: u8, codec: u8) -> &'static str {
    match (item_type, codec) {
        (SIDECAR_TYPE_UTF8_TEXT, _) => "text/plain;charset=utf-8",
        (SIDECAR_TYPE_JSON, _) => "application/json",
        (SIDECAR_TYPE_IMAGE, SIDECAR_CODEC_AVIF) => "image/avif",
        _ => "",
    }
}

fn write_ascii_padded(target: &mut [u8], offset: usize, length: usize, value: &str) {
    let end = offset.saturating_add(length).min(target.len());
    target[offset..end].fill(0);
    let clean: String = value
        .chars()
        .map(|ch| {
            let code = ch as u32;
            if (0x20..=0x7e).contains(&code) {
                ch
            } else {
                ' '
            }
        })
        .collect();
    for (index, byte) in clean
        .as_bytes()
        .iter()
        .copied()
        .take(end - offset)
        .enumerate()
    {
        target[offset + index] = byte;
    }
}

fn write_u16be(target: &mut [u8], offset: usize, value: u16) {
    target[offset] = (value >> 8) as u8;
    target[offset + 1] = value as u8;
}

fn write_u32be(target: &mut [u8], offset: usize, value: u32) {
    target[offset] = (value >> 24) as u8;
    target[offset + 1] = (value >> 16) as u8;
    target[offset + 2] = (value >> 8) as u8;
    target[offset + 3] = value as u8;
}

fn clamp_range(value: Option<f64>, min: f64, max: f64, fallback: f64) -> f64 {
    let Some(number) = value else {
        return fallback;
    };
    if !number.is_finite() {
        return fallback;
    }
    number.clamp(min, max)
}

fn unit_to_u16(value: Option<f64>, fallback: f64) -> u16 {
    (clamp_range(value, 0.0, 1.0, fallback) * u16::MAX as f64).round() as u16
}

fn ratio_to_permille(value: Option<f64>, min: f64, max: f64, fallback: f64) -> u16 {
    (clamp_range(value, min, max, fallback) * 1000.0)
        .round()
        .clamp(0.0, u16::MAX as f64) as u16
}

fn display_header_flags(input: &PackageDisplayHeaderInput) -> u16 {
    let mut flags = 0;
    if input.cover_shown.unwrap_or(false) {
        flags |= DISPLAY_HEADER_FLAG_COVER_SHOWN;
    }
    if input.cover_effects.unwrap_or(false) {
        flags |= DISPLAY_HEADER_FLAG_COVER_EFFECTS;
    }
    if input.cover_rgb_grain.unwrap_or(false) {
        flags |= DISPLAY_HEADER_FLAG_COVER_RGB_GRAIN;
    }
    if input.inner_sleeve_shown.unwrap_or(false) {
        flags |= DISPLAY_HEADER_FLAG_INNER_SLEEVE_SHOWN;
    }
    if input.cover_embedded.unwrap_or(false) {
        flags |= DISPLAY_HEADER_FLAG_COVER_EMBEDDED;
    }
    flags
}

use bytes2rgb::normalized_hex_color;

pub fn build_package_display_header_bytes(input: &PackageDisplayHeaderInput) -> Vec<u8> {
    let mut bytes = vec![0u8; DISPLAY_HEADER_LENGTH];
    let crop = input.crop.as_ref();
    write_ascii_padded(
        &mut bytes,
        0,
        4,
        &String::from_utf8_lossy(DISPLAY_HEADER_MAGIC),
    );
    bytes[4] = DISPLAY_HEADER_VERSION;
    bytes[5] = DISPLAY_HEADER_LENGTH as u8;
    write_u16be(&mut bytes, 6, display_header_flags(input));
    write_ascii_padded(
        &mut bytes,
        16,
        40,
        input.design_label.as_deref().unwrap_or(""),
    );
    write_ascii_padded(
        &mut bytes,
        56,
        8,
        input.design_class_name.as_deref().unwrap_or(""),
    );
    bytes[64] =
        clamp_range(input.posterize, 2.0, 64.0, DEFAULT_COVER_POSTERIZE_LEVELS).round() as u8;
    write_u16be(
        &mut bytes,
        65,
        ratio_to_permille(input.saturation, 0.0, 2.0, DEFAULT_COVER_SATURATION),
    );
    write_u16be(
        &mut bytes,
        67,
        ratio_to_permille(input.contrast, 0.0, 2.0, DEFAULT_COVER_CONTRAST),
    );
    write_u16be(
        &mut bytes,
        69,
        ratio_to_permille(
            input.rgb_grain_opacity,
            COVER_RGB_GRAIN_OPACITY_MIN,
            1.0,
            DEFAULT_COVER_RGB_GRAIN_OPACITY,
        ),
    );
    bytes[71] = clamp_range(input.rgb_grain_blend_index, 0.0, u8::MAX as f64, 0.0).round() as u8;
    write_u16be(
        &mut bytes,
        72,
        unit_to_u16(crop.and_then(|item| item.x), 0.5),
    );
    write_u16be(
        &mut bytes,
        74,
        unit_to_u16(crop.and_then(|item| item.y), 0.5),
    );
    write_u16be(
        &mut bytes,
        76,
        ratio_to_permille(
            crop.and_then(|item| item.zoom),
            COVER_CROP_ZOOM_MIN,
            COVER_CROP_ZOOM_MAX,
            1.0,
        ),
    );
    bytes[78] = input
        .quantizer
        .map(|value| clamp_range(Some(value), 0.0, 63.0, 32.0).round() as u8)
        .unwrap_or(255);
    bytes[79] = 0;
    write_u16be(
        &mut bytes,
        80,
        input
            .inner
            .map(|value| clamp_range(Some(value), 1.0, 576.0, 576.0).round() as u16)
            .unwrap_or(0),
    );
    write_ascii_padded(
        &mut bytes,
        82,
        7,
        &normalized_hex_color(input.sleeve_tone_color.as_deref()),
    );
    write_u16be(
        &mut bytes,
        89,
        ratio_to_permille(input.sleeve_sparkle_opacity, 0.0, 1.0, 1.0),
    );

    let payload_crc = record_core::crc32_ieee(&bytes[16..]);
    write_u32be(&mut bytes, 8, payload_crc);
    let mut header_for_crc = bytes.clone();
    header_for_crc[12..16].fill(0);
    let header_crc = record_core::crc32_ieee(&header_for_crc);
    write_u32be(&mut bytes, 12, header_crc);
    bytes
}

pub fn build_package_display_header_bytes_from_json(raw: &str) -> Result<Vec<u8>> {
    let input: PackageDisplayHeaderInput =
        serde_json::from_str(raw).context("display header options JSON is invalid")?;
    Ok(build_package_display_header_bytes(&input))
}

pub fn build_package_display_header_item_json_from_input(
    input: &PackageDisplayHeaderInput,
) -> serde_json::Value {
    let bytes = build_package_display_header_bytes(input);
    serde_json::json!({
        "type": "opaque",
        "codec": "raw",
        "name": DISPLAY_HEADER_NAME,
        "mime": DISPLAY_HEADER_MIME,
        "rawByteLength": bytes.len(),
        "dataBase64": general_purpose::STANDARD.encode(bytes),
    })
}

pub fn build_package_display_header_item_json(raw: &str) -> Result<String> {
    let input: PackageDisplayHeaderInput =
        serde_json::from_str(raw).context("display header options JSON is invalid")?;
    Ok(build_package_display_header_item_json_from_input(&input).to_string())
}

fn clean_package_text(value: Option<&str>) -> String {
    value
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn package_photo_item_name(value: Option<&str>) -> String {
    let raw = value.unwrap_or("photo").trim();
    let stem = raw
        .rsplit_once('.')
        .map(|(prefix, _)| prefix)
        .unwrap_or(raw)
        .trim();
    format!("{}.avif", if stem.is_empty() { "photo" } else { stem })
}

fn package_photo_is_ready(photo: &PackageMetadataPhotoInput) -> bool {
    matches!(photo.status.as_deref(), Some("ready") | Some("too-large"))
        && photo.has_avif.unwrap_or(false)
}

fn rounded_positive_dimension(primary: Option<f64>, fallback: Option<f64>) -> f64 {
    let value = primary.or(fallback).unwrap_or(COVER_PREVIEW_SIZE);
    if value.is_finite() {
        value.round().max(1.0)
    } else {
        COVER_PREVIEW_SIZE
    }
}

fn cover_crop_metadata_json(cover: &PackageMetadataCoverInput) -> serde_json::Value {
    let crop = cover.crop.as_ref();
    let x = clamp_range(crop.and_then(|item| item.x), 0.0, 1.0, 0.5);
    let y = clamp_range(crop.and_then(|item| item.y), 0.0, 1.0, 0.5);
    let zoom = clamp_range(
        crop.and_then(|item| item.zoom),
        COVER_CROP_ZOOM_MIN,
        COVER_CROP_ZOOM_MAX,
        1.0,
    );
    let source_width = rounded_positive_dimension(cover.source_width, cover.width);
    let source_height = rounded_positive_dimension(cover.source_height, cover.height);
    let output_size = clamp_range(cover.inner, 1.0, 576.0, COVER_PREVIEW_SIZE).round();
    let scale = (output_size / source_width).max(output_size / source_height) * zoom;
    let sw = source_width.min(output_size / scale);
    let sh = source_height.min(output_size / scale);
    let max_sx = (source_width - sw).max(0.0);
    let max_sy = (source_height - sh).max(0.0);
    serde_json::json!({
        "normalized": {
            "x": (x * 1_000_000.0).round() / 1_000_000.0,
            "y": (y * 1_000_000.0).round() / 1_000_000.0,
            "zoom": (zoom * 1_000_000.0).round() / 1_000_000.0,
        },
        "source": {
            "width": source_width as u32,
            "height": source_height as u32,
        },
        "rectangle": {
            "x": (max_sx * x).round() as u32,
            "y": (max_sy * y).round() as u32,
            "width": sw.round() as u32,
            "height": sh.round() as u32,
        },
        "output": {
            "width": output_size as u32,
            "height": output_size as u32,
        },
    })
}

fn package_cover_metadata_json(input: &PackageMetadataInput) -> Option<serde_json::Value> {
    let cover = input.cover.as_ref()?;
    if !cover.has_file.unwrap_or(false) || !cover.has_preview.unwrap_or(false) {
        return None;
    }

    let source_photo_id = clean_package_text(cover.source_photo_id.as_deref());
    let source_photo = if source_photo_id.is_empty() {
        None
    } else {
        input
            .photos
            .iter()
            .enumerate()
            .find(|(_, photo)| photo.id.as_deref() == Some(source_photo_id.as_str()))
    };
    let source = if let Some((index, photo)) = source_photo {
        serde_json::json!({
            "type": "photoInsert",
            "position": index + 1,
            "sourceName": photo.name.as_deref().unwrap_or(""),
            "itemName": package_photo_item_name(photo.name.as_deref()),
        })
    } else {
        serde_json::json!({
            "type": if cover.embedded.unwrap_or(false) { "embeddedCover" } else { "coverUpload" },
            "sourceName": cover.name.as_deref().unwrap_or(""),
            "itemName": if cover.embedded.unwrap_or(false) { "album-cover.avif" } else { "" },
        })
    };

    Some(serde_json::json!({
        "role": "cover",
        "source": source,
        "crop": cover_crop_metadata_json(cover),
        "display": {
            "shown": cover.shown.unwrap_or(false),
            "design": cover.design_label.as_deref().unwrap_or(""),
            "effectsEnabled": cover.effects_enabled.unwrap_or(false),
        },
    }))
}

pub fn build_package_metadata_items_json_from_input(
    input: &PackageMetadataInput,
) -> serde_json::Value {
    let photo_credits = input
        .photos
        .iter()
        .filter(|photo| package_photo_is_ready(photo))
        .enumerate()
        .filter_map(|(index, photo)| {
            let credit = clean_package_text(photo.credit.as_deref());
            if credit.is_empty() {
                return None;
            }
            Some(serde_json::json!({
                "position": index + 1,
                "sourceName": photo.name.as_deref().unwrap_or(""),
                "itemName": package_photo_item_name(photo.name.as_deref()),
                "credit": credit,
            }))
        })
        .collect::<Vec<_>>();
    let cover = package_cover_metadata_json(input);
    if photo_credits.is_empty() && cover.is_none() {
        return serde_json::Value::Array(Vec::new());
    }

    let mut metadata = serde_json::json!({
        "kind": "bitneedle.packageMetadata",
        "version": 1,
    });
    if let Some(cover) = cover {
        metadata["cover"] = cover;
    }
    if !photo_credits.is_empty() {
        metadata["photoCredits"] = serde_json::Value::Array(photo_credits);
    }

    serde_json::json!([{
        "type": "json",
        "codec": "raw",
        "name": PACKAGE_METADATA_ITEM_NAME,
        "mime": PACKAGE_METADATA_MIME,
        "json": metadata,
    }])
}

pub fn build_package_metadata_items_json(raw: &str) -> Result<String> {
    let input: PackageMetadataInput =
        serde_json::from_str(raw).context("package metadata options JSON is invalid")?;
    Ok(build_package_metadata_items_json_from_input(&input).to_string())
}

pub fn build_package_photo_item_json_from_input(
    input: &PackagePhotoItemInput,
    avif_bytes: &[u8],
) -> serde_json::Value {
    serde_json::json!({
        "type": "image",
        "codec": "avif",
        "name": package_photo_item_name(input.name.as_deref()),
        "mime": PACKAGE_PHOTO_MIME,
        "dataBase64": general_purpose::STANDARD.encode(avif_bytes),
    })
}

pub fn build_package_photo_item_json(options_json: &str, avif_bytes: &[u8]) -> Result<String> {
    let input: PackagePhotoItemInput =
        serde_json::from_str(options_json).context("package photo item options JSON is invalid")?;
    Ok(build_package_photo_item_json_from_input(&input, avif_bytes).to_string())
}

pub fn build_package_cover_item_json(avif_bytes: &[u8]) -> String {
    serde_json::json!({
        "type": "image",
        "codec": "avif",
        "name": PACKAGE_COVER_ITEM_NAME,
        "mime": PACKAGE_PHOTO_MIME,
        "dataBase64": general_purpose::STANDARD.encode(avif_bytes),
    })
    .to_string()
}

pub fn resolve_package_image_encode_cache_key_json(options_json: &str) -> Result<String> {
    let input: PackageImageCacheKeyInput = serde_json::from_str(options_json)
        .context("package image cache key options are invalid")?;
    let key = resolve_package_image_encode_cache_key(&input)?;
    serde_json::to_string(&key).context("failed to serialize package image cache key")
}

pub fn resolve_package_image_encode_cache_key(
    input: &PackageImageCacheKeyInput,
) -> Result<PackageImageEncodeCacheKey> {
    let kind = input
        .kind
        .as_deref()
        .map(str::trim)
        .unwrap_or("photo")
        .to_ascii_lowercase();
    let file_key = package_file_cache_key(input);
    let quantizer = package_photo_quantizer(input.quantizer.as_ref());
    let inner = package_photo_inner(input.inner.as_ref());
    let crop = normalize_package_crop(input.crop.as_ref());
    let crop_key = package_crop_cache_key(&crop);
    let color_mode = normalize_package_color_mode(input.color_mode.as_deref());
    let key = if kind == "cover" {
        format!("{file_key}:cover:{quantizer}:{inner}:{crop_key}:{color_mode}")
    } else {
        format!("{file_key}:{quantizer}:{inner}:{crop_key}:{color_mode}")
    };

    Ok(PackageImageEncodeCacheKey {
        key,
        file_key,
        crop_key,
        quantizer,
        inner,
        crop,
        monochrome: package_color_mode_is_monochrome(&color_mode),
        color_mode,
    })
}

pub fn resolve_package_best_fit_cache_key_json(options_json: &str) -> Result<String> {
    let input: PackageBestFitCacheKeyInput = serde_json::from_str(options_json)
        .context("package best-fit cache key options are invalid")?;
    let key = resolve_package_best_fit_cache_key(&input)?;
    serde_json::to_string(&key).context("failed to serialize package best-fit cache key")
}

pub fn resolve_package_best_fit_cache_key(
    input: &PackageBestFitCacheKeyInput,
) -> Result<PackageBestFitCacheKey> {
    let kind = input
        .kind
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("photo")
        .to_string();
    let file_key = package_file_cache_key_from_parts(
        input.file_name.as_deref(),
        input.file_size.as_ref(),
        input.file_last_modified.as_ref(),
    );
    let budget_bytes = package_u64(input.budget_bytes.as_ref());
    let inner = package_photo_inner(input.inner.as_ref());
    let crop_key = input
        .crop_key
        .as_deref()
        .filter(|value| !value.is_empty())
        .unwrap_or("fit")
        .to_string();
    let color_mode = normalize_package_color_mode(input.color_mode.as_deref());
    let key = [
        kind.as_str(),
        &format!(
            "fit-v7-square-crop-qbest{}-{}-{}",
            PACKAGE_PHOTO_SEARCH_START_QUANTIZER,
            PACKAGE_PHOTO_SEARCH_MIN_QUANTIZER,
            PACKAGE_PHOTO_MAX_QUANTIZER
        ),
        file_key.as_str(),
        &budget_bytes.to_string(),
        &inner.to_string(),
        crop_key.as_str(),
        color_mode.as_str(),
    ]
    .join(":");

    Ok(PackageBestFitCacheKey {
        key,
        file_key,
        budget_bytes,
        inner,
        crop_key,
        color_mode,
    })
}

pub fn package_quantizer_search_plan_json(options_json: &str) -> Result<String> {
    let input: PackageQuantizerSearchPlanInput = serde_json::from_str(options_json)
        .context("package quantizer search options are invalid")?;
    let plan = package_quantizer_search_plan(&input);
    serde_json::to_string(&plan).context("failed to serialize package quantizer search plan")
}

pub fn package_quantizer_search_plan(
    input: &PackageQuantizerSearchPlanInput,
) -> PackageQuantizerSearchPlan {
    let min_q = package_effective_min_quantizer(input.min_quantizer.as_ref());
    let max_q = package_photo_quantizer(input.max_quantizer.as_ref()).max(min_q);
    let start_q = package_photo_quantizer(input.start_quantizer.as_ref()).clamp(min_q, max_q);
    let radius = package_photo_quantizer(input.refinement_radius.as_ref())
        .min(PACKAGE_PHOTO_MAX_QUANTIZER)
        .max(0);
    let radius = if input.refinement_radius.is_some() {
        radius
    } else {
        PACKAGE_PHOTO_SEARCH_REFINEMENT_RADIUS
    };

    let trials = package_quantizer_trials_by_quantizer(input, min_q, max_q);
    let best_q = best_package_quantizer_fit(&trials);
    let done = |best_q: Option<u8>| PackageQuantizerSearchPlan {
        min_quantizer: min_q,
        max_quantizer: max_q,
        start_quantizer: start_q,
        done: true,
        next_quantizer: None,
        note: String::new(),
        best_quantizer: best_q,
    };
    let next = |quantizer: u8, note: &str, best_q: Option<u8>| PackageQuantizerSearchPlan {
        min_quantizer: min_q,
        max_quantizer: max_q,
        start_quantizer: start_q,
        done: false,
        next_quantizer: Some(quantizer),
        note: note.to_string(),
        best_quantizer: best_q,
    };

    if !trials.contains_key(&start_q) {
        return next(start_q, "start", best_q);
    }
    let first_fits = trials
        .get(&start_q)
        .is_some_and(|trial| trial.fits.unwrap_or(false));
    if first_fits {
        if start_q != min_q && !trials.contains_key(&min_q) {
            return next(min_q, "floor", best_q);
        }
        if start_q != min_q
            && !trials
                .get(&min_q)
                .is_some_and(|trial| trial.fits.unwrap_or(false))
        {
            if let Some(mid) = next_missing_binary_quantizer(min_q, start_q, &trials) {
                return next(mid, "bisect", best_q);
            }
        }
        if let Some(refine_q) = next_missing_refinement_quantizer(best_q, min_q, radius, &trials) {
            return next(refine_q, "refine", best_q);
        }
        return done(best_q);
    }

    if start_q == max_q {
        return done(best_q);
    }
    if !trials.contains_key(&max_q) {
        return next(max_q, "ceiling", best_q);
    }
    if !trials
        .get(&max_q)
        .is_some_and(|trial| trial.fits.unwrap_or(false))
    {
        return done(best_q);
    }
    if let Some(mid) = next_missing_binary_quantizer(start_q, max_q, &trials) {
        return next(mid, "bisect", best_q);
    }
    if let Some(refine_q) = next_missing_refinement_quantizer(best_q, min_q, radius, &trials) {
        return next(refine_q, "refine", best_q);
    }
    done(best_q)
}

pub fn package_fit_budget_json(options_json: &str) -> Result<String> {
    let input: PackageFitBudgetInput =
        serde_json::from_str(options_json).context("package fit budget options are invalid")?;
    let budget = package_fit_budget(&input);
    serde_json::to_string(&budget).context("failed to serialize package fit budget")
}

pub fn package_fit_budget(input: &PackageFitBudgetInput) -> PackageFitBudget {
    let capacity = package_u64(input.capacity.as_ref());
    let base_bsc1_bytes = package_u64(input.base_bsc1_bytes.as_ref());
    let photo_count = package_u64(input.photo_count.as_ref());
    let available_bytes = capacity.saturating_sub(base_bsc1_bytes);
    let cover_budget_bytes = if capacity > 0 {
        available_bytes / PACKAGE_COVER_MAX_BUDGET_DIVISOR
    } else {
        0
    };
    let remaining_bytes = capacity.saturating_sub(base_bsc1_bytes);
    let per_photo_budget_bytes = if capacity > 0 && photo_count > 0 {
        remaining_bytes / photo_count
    } else {
        0
    };
    PackageFitBudget {
        capacity,
        base_bsc1_bytes,
        available_bytes,
        remaining_bytes,
        cover_budget_bytes,
        photo_count,
        per_photo_budget_bytes,
    }
}

pub fn build_package_sidecar_render_options_json(options_json: &str) -> Result<String> {
    let input: PackageSidecarRenderOptionsInput =
        serde_json::from_str(options_json).context("package sidecar render options are invalid")?;
    Ok(build_package_sidecar_render_options(&input).to_string())
}

pub fn build_package_sidecar_render_options(
    input: &PackageSidecarRenderOptionsInput,
) -> serde_json::Value {
    let payload_bytes = package_u64(input.payload_bytes.as_ref());
    let capacity_bytes = package_u64(input.capacity_bytes.as_ref());
    let seed = package_u64(input.seed.as_ref());
    let utilization = if capacity_bytes > 0 && payload_bytes > 0 {
        ((payload_bytes as f64) / (capacity_bytes as f64)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let strength = if capacity_bytes > 0 {
        0.12 + utilization * 0.36
    } else {
        0.22
    };
    let strength = ((strength.clamp(0.0, 0.5) * 100.0).round()) / 100.0;
    let grain = if utilization > 0.85 {
        5
    } else if utilization > 0.55 {
        4
    } else {
        3
    };

    let mut sidecar = serde_json::Map::new();
    sidecar.insert("seed".to_string(), serde_json::json!(seed));
    sidecar.insert(
        "carriers".to_string(),
        serde_json::json!(PACKAGE_SIDECAR_CARRIERS.to_vec()),
    );
    sidecar.insert(
        "scheme".to_string(),
        serde_json::json!(PACKAGE_SIDECAR_SCHEME),
    );
    sidecar.insert(
        "labelTuning".to_string(),
        serde_json::json!({
            "enabled": true,
            "targetLuma": 128,
            "strength": strength,
            "grain": grain,
        }),
    );

    if let Some(bsc1_base64) = input
        .bsc1_base64
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        sidecar.insert("bsc1Base64".to_string(), serde_json::json!(bsc1_base64));
    } else {
        sidecar.insert(
            "items".to_string(),
            input
                .items
                .clone()
                .unwrap_or_else(|| serde_json::Value::Array(Vec::new())),
        );
    }

    serde_json::json!({ "sidecar": serde_json::Value::Object(sidecar) })
}

pub fn package_preserved_pattern_items_json(decoded_json: &str) -> Result<String> {
    let decoded: serde_json::Value =
        serde_json::from_str(decoded_json).context("decoded sidecar JSON is invalid")?;
    let items = package_preserved_pattern_items(&decoded)?;
    serde_json::to_string(&items).context("failed to serialize preserved pattern items")
}

pub fn package_preserved_pattern_items(
    decoded: &serde_json::Value,
) -> Result<Vec<serde_json::Value>> {
    let Some(items) = decoded.get("items").and_then(serde_json::Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut preserved = Vec::new();
    for item in items {
        if package_decoded_item_is_pattern_map(item) {
            preserved.push(package_preserved_pattern_item(item)?);
        }
    }
    Ok(preserved)
}

fn package_decoded_item_is_pattern_map(item: &serde_json::Value) -> bool {
    decoded_item_text_field(item, "name")
        .is_some_and(|value| value == PACKAGE_PATTERN_SIDECAR_ITEM_NAME)
        || decoded_item_text_field(item, "mime")
            .is_some_and(|value| value == PACKAGE_PATTERN_SIDECAR_MIME)
}

fn package_preserved_pattern_item(item: &serde_json::Value) -> Result<serde_json::Value> {
    let item_type = decoded_item_u8_field(item, "itemType")?;
    let codec = decoded_item_u8_field(item, "codec")?;
    let mut preserved = serde_json::Map::new();
    preserved.insert(
        "type".to_string(),
        package_sidecar_input_type_value(item_type),
    );
    preserved.insert(
        "codec".to_string(),
        package_sidecar_input_codec_value(codec),
    );
    preserved.insert(
        "name".to_string(),
        serde_json::Value::String(
            decoded_item_text_field(item, "name")
                .filter(|value| !value.is_empty())
                .unwrap_or(PACKAGE_PATTERN_SIDECAR_ITEM_NAME)
                .to_string(),
        ),
    );
    preserved.insert(
        "mime".to_string(),
        serde_json::Value::String(
            decoded_item_text_field(item, "mime")
                .filter(|value| !value.is_empty())
                .unwrap_or(PACKAGE_PATTERN_SIDECAR_MIME)
                .to_string(),
        ),
    );
    preserved.insert(
        "dataBase64".to_string(),
        serde_json::Value::String(
            decoded_item_text_field(item, "storedDataBase64")
                .context("pattern sidecar item is missing storedDataBase64")?
                .to_string(),
        ),
    );
    if let Some(raw_byte_length) = decoded_item_optional_u64_field(item, "rawByteLength") {
        preserved.insert(
            "rawByteLength".to_string(),
            serde_json::json!(raw_byte_length),
        );
    }
    if let Some(flags) = decoded_item_optional_u64_field(item, "flags").filter(|value| *value != 0)
    {
        preserved.insert("flags".to_string(), serde_json::json!(flags));
    }
    Ok(serde_json::Value::Object(preserved))
}

fn package_sidecar_input_type_value(item_type: u8) -> serde_json::Value {
    match item_type {
        SIDECAR_TYPE_OPAQUE => serde_json::json!("opaque"),
        SIDECAR_TYPE_UTF8_TEXT => serde_json::json!("text"),
        SIDECAR_TYPE_IMAGE => serde_json::json!("image"),
        SIDECAR_TYPE_JSON => serde_json::json!("json"),
        value => serde_json::json!(value),
    }
}

fn package_sidecar_input_codec_value(codec: u8) -> serde_json::Value {
    match codec {
        SIDECAR_CODEC_RAW => serde_json::json!("raw"),
        SIDECAR_CODEC_BROTLI => serde_json::json!("brotli"),
        SIDECAR_CODEC_ZSTD => serde_json::json!("zstd"),
        SIDECAR_CODEC_AVIF => serde_json::json!("avif"),
        value => serde_json::json!(value),
    }
}

fn decoded_item_u8_field(item: &serde_json::Value, field: &str) -> Result<u8> {
    let value = item
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .with_context(|| format!("decoded sidecar item is missing numeric {field}"))?;
    u8::try_from(value).with_context(|| format!("decoded sidecar item {field} exceeds u8 range"))
}

fn decoded_item_optional_u64_field(item: &serde_json::Value, field: &str) -> Option<u64> {
    item.get(field).and_then(serde_json::Value::as_u64)
}

fn decoded_item_text_field<'a>(item: &'a serde_json::Value, field: &str) -> Option<&'a str> {
    item.get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn package_quantizer_trials_by_quantizer(
    input: &PackageQuantizerSearchPlanInput,
    min_q: u8,
    max_q: u8,
) -> BTreeMap<u8, PackageQuantizerTrialInput> {
    let mut trials = BTreeMap::new();
    for trial in &input.trials {
        let q = package_photo_quantizer(trial.quantizer.as_ref()).clamp(min_q, max_q);
        trials.insert(q, trial.clone());
    }
    trials
}

fn best_package_quantizer_fit(trials: &BTreeMap<u8, PackageQuantizerTrialInput>) -> Option<u8> {
    let mut best = None;
    for (q, trial) in trials {
        if !trial.fits.unwrap_or(false) {
            continue;
        }
        let Some(current_q) = best else {
            best = Some(*q);
            continue;
        };
        if *q < current_q
            || (*q == current_q
                && package_trial_budget_ratio(trial)
                    > trials
                        .get(&current_q)
                        .map(package_trial_budget_ratio)
                        .unwrap_or(1.0))
        {
            best = Some(*q);
        }
    }
    best
}

fn next_missing_binary_quantizer(
    mut low: u8,
    mut high: u8,
    trials: &BTreeMap<u8, PackageQuantizerTrialInput>,
) -> Option<u8> {
    while high.saturating_sub(low) > 1 {
        let mid = low + (high - low) / 2;
        let Some(trial) = trials.get(&mid) else {
            return Some(mid);
        };
        if trial.fits.unwrap_or(false) {
            high = mid;
        } else {
            low = mid;
        }
    }
    None
}

fn next_missing_refinement_quantizer(
    best_q: Option<u8>,
    min_q: u8,
    radius: u8,
    trials: &BTreeMap<u8, PackageQuantizerTrialInput>,
) -> Option<u8> {
    let best_q = best_q?;
    let lower = best_q.saturating_sub(radius).max(min_q);
    (lower..best_q).find(|q| !trials.contains_key(q))
}

fn package_trial_budget_ratio(trial: &PackageQuantizerTrialInput) -> f64 {
    let budget = package_number(trial.budget_bytes.as_ref()).unwrap_or(0.0);
    let contribution = package_number(trial.contribution_bytes.as_ref())
        .or_else(|| package_number(trial.bsc1_bytes.as_ref()))
        .unwrap_or(0.0);
    if budget > 0.0 {
        contribution / budget
    } else {
        1.0
    }
}

fn package_file_cache_key(input: &PackageImageCacheKeyInput) -> String {
    package_file_cache_key_from_parts(
        input.file_name.as_deref(),
        input.file_size.as_ref(),
        input.file_last_modified.as_ref(),
    )
}

fn package_file_cache_key_from_parts(
    file_name: Option<&str>,
    file_size: Option<&serde_json::Value>,
    file_last_modified: Option<&serde_json::Value>,
) -> String {
    format!(
        "{}:{}:{}",
        file_name.unwrap_or(""),
        package_u64(file_size),
        package_u64(file_last_modified)
    )
}

fn package_photo_quantizer(value: Option<&serde_json::Value>) -> u8 {
    let Some(number) = package_number(value) else {
        return PACKAGE_PHOTO_DEFAULT_QUANTIZER;
    };
    number
        .round()
        .clamp(0.0, f64::from(PACKAGE_PHOTO_MAX_QUANTIZER)) as u8
}

fn package_effective_min_quantizer(value: Option<&serde_json::Value>) -> u8 {
    package_photo_quantizer(value).max(PACKAGE_PHOTO_SEARCH_MIN_QUANTIZER)
}

fn package_photo_inner(value: Option<&serde_json::Value>) -> u32 {
    let Some(number) = package_number(value) else {
        return PACKAGE_PHOTO_MAX_INNER;
    };
    number
        .round()
        .clamp(1.0, f64::from(PACKAGE_PHOTO_MAX_INNER)) as u32
}

fn normalize_package_crop(value: Option<&serde_json::Value>) -> PackageNormalizedCrop {
    let object = value.and_then(serde_json::Value::as_object);
    PackageNormalizedCrop {
        x: package_range(object.and_then(|object| object.get("x")), 0.0, 1.0, 0.5),
        y: package_range(object.and_then(|object| object.get("y")), 0.0, 1.0, 0.5),
        zoom: package_range(
            object.and_then(|object| object.get("zoom")),
            COVER_CROP_ZOOM_MIN,
            COVER_CROP_ZOOM_MAX,
            1.0,
        ),
    }
}

fn package_crop_cache_key(crop: &PackageNormalizedCrop) -> String {
    format!("crop-{:.4}-{:.4}-{:.4}", crop.x, crop.y, crop.zoom)
}

fn normalize_package_color_mode(value: Option<&str>) -> String {
    match value.unwrap_or("").to_ascii_lowercase().as_str() {
        PACKAGE_COLOR_MODE_RGB => PACKAGE_COLOR_MODE_RGB.to_string(),
        PACKAGE_COLOR_MODE_GRAYSCALE => PACKAGE_COLOR_MODE_GRAYSCALE.to_string(),
        _ => PACKAGE_COLOR_MODE_RGB.to_string(),
    }
}

fn package_color_mode_is_monochrome(value: &str) -> bool {
    value == PACKAGE_COLOR_MODE_GRAYSCALE
}

fn package_range(value: Option<&serde_json::Value>, min: f64, max: f64, fallback: f64) -> f64 {
    package_number(value)
        .filter(|number| number.is_finite())
        .unwrap_or(fallback)
        .clamp(min, max)
}

fn package_u64(value: Option<&serde_json::Value>) -> u64 {
    match value {
        Some(serde_json::Value::Number(number)) => number.as_u64().unwrap_or_else(|| {
            number
                .as_f64()
                .filter(|number| number.is_finite() && *number > 0.0)
                .map(|number| number.floor() as u64)
                .unwrap_or(0)
        }),
        Some(serde_json::Value::String(text)) => text
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|number| number.is_finite() && *number > 0.0)
            .map(|number| number.floor() as u64)
            .unwrap_or(0),
        _ => 0,
    }
}

fn package_number(value: Option<&serde_json::Value>) -> Option<f64> {
    match value {
        Some(serde_json::Value::Number(number)) => number.as_f64(),
        Some(serde_json::Value::String(text)) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|number| number.is_finite())
}

fn sidecar_item_stored_bytes(
    input: &SidecarItemInput,
    item_type: u8,
    codec: u8,
) -> Result<Vec<u8>> {
    if let Some(data_base64) = input.data_base64.as_deref() {
        return decode_base64_text(data_base64, "sidecar item dataBase64");
    }
    if codec == SIDECAR_CODEC_RAW && item_type == SIDECAR_TYPE_UTF8_TEXT {
        if let Some(text) = input.text.as_deref() {
            return Ok(text.as_bytes().to_vec());
        }
    }
    if codec == SIDECAR_CODEC_RAW && item_type == SIDECAR_TYPE_JSON {
        if let Some(json) = input.json.as_ref() {
            return serde_json::to_vec(json).context("failed to serialize sidecar JSON item");
        }
    }
    bail!("sidecar item requires dataBase64, except raw text/json items may use text/json")
}

pub fn build_sidecar_container_from_items(items: &[SidecarItemInput]) -> Result<Vec<u8>> {
    if items.len() > u16::MAX as usize {
        bail!("sidecar item count exceeds u16 limit");
    }
    let mut item_bytes = Vec::new();
    for input in items {
        let item_type = parse_sidecar_item_type(&input.item_type)?;
        let codec = parse_sidecar_codec(&input.codec)?;
        if input.flags != 0 {
            bail!("sidecar item flags must be 0 in this version");
        }
        let stored = sidecar_item_stored_bytes(input, item_type, codec)?;
        if stored.len() > u32::MAX as usize {
            bail!("sidecar item stored data exceeds u32 length limit");
        }
        let raw_byte_length = if codec == SIDECAR_CODEC_RAW {
            u32::try_from(stored.len()).context("sidecar raw item exceeds u32 length limit")?
        } else {
            input.raw_byte_length.unwrap_or(SIDECAR_RAW_LENGTH_ABSENT)
        };
        let name = input.name.as_deref().unwrap_or("");
        let mime = input
            .mime
            .as_deref()
            .unwrap_or(default_sidecar_mime(item_type, codec));
        validate_sidecar_name(name, "item name")?;
        validate_sidecar_name(mime, "MIME type")?;
        validate_sidecar_item_payload(item_type, codec, raw_byte_length, &stored, mime)?;
        let name_bytes = name.as_bytes();
        let mime_bytes = mime.as_bytes();
        if name_bytes.len() > u16::MAX as usize {
            bail!("sidecar item name exceeds u16 length limit");
        }
        if mime_bytes.len() > u16::MAX as usize {
            bail!("sidecar item MIME type exceeds u16 length limit");
        }
        item_bytes.push(item_type);
        item_bytes.push(codec);
        item_bytes.push(input.flags);
        item_bytes.push(0);
        item_bytes.extend_from_slice(&raw_byte_length.to_be_bytes());
        item_bytes.extend_from_slice(&(stored.len() as u32).to_be_bytes());
        item_bytes.extend_from_slice(&(name_bytes.len() as u16).to_be_bytes());
        item_bytes.extend_from_slice(&(mime_bytes.len() as u16).to_be_bytes());
        item_bytes.extend_from_slice(name_bytes);
        item_bytes.extend_from_slice(mime_bytes);
        item_bytes.extend_from_slice(&stored);
    }

    let total_length = 12usize
        .checked_add(item_bytes.len())
        .context("sidecar container length overflow")?;
    if total_length > u32::MAX as usize {
        bail!("sidecar container exceeds u32 length limit");
    }
    let mut out = Vec::with_capacity(total_length);
    out.extend_from_slice(SIDECAR_MAGIC);
    out.push(SIDECAR_CONTAINER_VERSION);
    out.push(0);
    out.extend_from_slice(&(items.len() as u16).to_be_bytes());
    out.extend_from_slice(&(total_length as u32).to_be_bytes());
    out.extend_from_slice(&item_bytes);
    validate_sidecar_container(&out)?;
    Ok(out)
}

pub fn build_sidecar_container_from_items_json(raw: &str) -> Result<Vec<u8>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct SidecarItemsWrapper {
        items: Vec<SidecarItemInput>,
    }

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("sidecar items JSON is empty");
    }
    let items = if trimmed.starts_with('[') {
        serde_json::from_str::<Vec<SidecarItemInput>>(trimmed)
            .context("sidecar items JSON must be an item array")?
    } else {
        serde_json::from_str::<SidecarItemsWrapper>(trimmed)
            .context("sidecar items JSON must be an item array or {\"items\": [...] }")?
            .items
    };
    build_sidecar_container_from_items(&items)
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

pub fn validate_sidecar_container(bytes: &[u8]) -> Result<SidecarContainerValidation> {
    if bytes.len() < 12 {
        bail!("sidecar container is too short");
    }
    if &bytes[..4] != SIDECAR_MAGIC {
        bail!("sidecar container magic is unsupported");
    }
    let version = bytes[4];
    if version != SIDECAR_CONTAINER_VERSION {
        bail!("unsupported sidecar container version {version}");
    }
    let flags = bytes[5];
    if flags != 0 {
        bail!("sidecar container flags must be 0 in this version");
    }
    let item_count = read_u16be(bytes, 6, "sidecar item count")? as usize;
    let total_length = read_u32be(bytes, 8, "sidecar total length")? as usize;
    if total_length != bytes.len() {
        bail!("sidecar total length does not match byte stream length");
    }

    let mut offset = 12usize;
    let mut items = Vec::with_capacity(item_count);
    for _ in 0..item_count {
        if offset + 16 > bytes.len() {
            bail!("sidecar item header is truncated");
        }
        let item_type = bytes[offset];
        let codec = bytes[offset + 1];
        let item_flags = bytes[offset + 2];
        let reserved = bytes[offset + 3];
        if item_flags != 0 {
            bail!("sidecar item flags must be 0 in this version");
        }
        if reserved != 0 {
            bail!("sidecar item reserved byte must be 0");
        }
        let raw_byte_length = read_u32be(bytes, offset + 4, "sidecar item raw length")?;
        let stored_byte_length = read_u32be(bytes, offset + 8, "sidecar item stored length")?;
        let name_len = read_u16be(bytes, offset + 12, "sidecar item name length")? as usize;
        let mime_len = read_u16be(bytes, offset + 14, "sidecar item MIME length")? as usize;
        offset += 16;
        let item_end = offset
            .checked_add(name_len)
            .and_then(|value| value.checked_add(mime_len))
            .and_then(|value| value.checked_add(stored_byte_length as usize))
            .context("sidecar item length overflow")?;
        if item_end > bytes.len() {
            bail!("sidecar item payload is truncated");
        }
        let name = std::str::from_utf8(&bytes[offset..offset + name_len])
            .context("sidecar item name is not valid UTF-8")?
            .to_string();
        offset += name_len;
        let mime = std::str::from_utf8(&bytes[offset..offset + mime_len])
            .context("sidecar item MIME type is not valid ASCII/UTF-8")?
            .to_string();
        offset += mime_len;
        let stored = &bytes[offset..offset + stored_byte_length as usize];
        offset += stored_byte_length as usize;

        validate_sidecar_name(&name, "item name")?;
        validate_sidecar_name(&mime, "MIME type")?;
        validate_sidecar_item_payload(item_type, codec, raw_byte_length, stored, &mime)?;
        items.push(SidecarItemValidation {
            item_type,
            item_type_name: sidecar_type_name(item_type),
            codec,
            codec_name: sidecar_codec_name(codec),
            flags: item_flags,
            raw_byte_length: (raw_byte_length != SIDECAR_RAW_LENGTH_ABSENT)
                .then_some(raw_byte_length),
            stored_byte_length,
            name,
            mime,
        });
    }
    if offset != bytes.len() {
        bail!("sidecar container has trailing bytes");
    }
    Ok(SidecarContainerValidation {
        ok: true,
        version,
        flags,
        item_count,
        total_length,
        items,
    })
}

fn decode_sidecar_item_payload(
    item_type: u8,
    codec: u8,
    raw_byte_length: u32,
    stored: &[u8],
) -> Result<Vec<u8>> {
    let decoded = match codec {
        SIDECAR_CODEC_RAW | SIDECAR_CODEC_AVIF => stored.to_vec(),
        SIDECAR_CODEC_BROTLI => {
            let mut reader = brotli::Decompressor::new(stored, 4096);
            let mut out = Vec::new();
            reader
                .read_to_end(&mut out)
                .context("failed to decompress Brotli sidecar item")?;
            out
        }
        SIDECAR_CODEC_ZSTD => bail!("zstd sidecar item extraction is not implemented"),
        _ => stored.to_vec(),
    };
    if raw_byte_length != SIDECAR_RAW_LENGTH_ABSENT && decoded.len() != raw_byte_length as usize {
        bail!(
            "decoded sidecar item length {} does not match declared raw length {}",
            decoded.len(),
            raw_byte_length
        );
    }
    if item_type == SIDECAR_TYPE_UTF8_TEXT {
        std::str::from_utf8(&decoded)
            .context("decoded UTF-8 text sidecar item is not valid UTF-8")?;
    }
    if item_type == SIDECAR_TYPE_JSON {
        serde_json::from_slice::<serde_json::Value>(&decoded)
            .context("decoded JSON sidecar item is not valid JSON")?;
    }
    Ok(decoded)
}

pub fn decode_sidecar_container_items(bytes: &[u8]) -> Result<SidecarDecodedItems> {
    let validation = validate_sidecar_container(bytes)?;
    let item_count = read_u16be(bytes, 6, "sidecar item count")? as usize;
    let mut offset = 12usize;
    let mut items = Vec::with_capacity(item_count);
    for _ in 0..item_count {
        if offset + 16 > bytes.len() {
            bail!("sidecar item header is truncated");
        }
        let item_type = bytes[offset];
        let codec = bytes[offset + 1];
        let flags = bytes[offset + 2];
        let raw_byte_length = read_u32be(bytes, offset + 4, "sidecar item raw length")?;
        let stored_byte_length = read_u32be(bytes, offset + 8, "sidecar item stored length")?;
        let name_len = read_u16be(bytes, offset + 12, "sidecar item name length")? as usize;
        let mime_len = read_u16be(bytes, offset + 14, "sidecar item MIME length")? as usize;
        offset += 16;
        let item_end = offset
            .checked_add(name_len)
            .and_then(|value| value.checked_add(mime_len))
            .and_then(|value| value.checked_add(stored_byte_length as usize))
            .context("sidecar item length overflow")?;
        if item_end > bytes.len() {
            bail!("sidecar item payload is truncated");
        }
        let name = std::str::from_utf8(&bytes[offset..offset + name_len])
            .context("sidecar item name is not valid UTF-8")?
            .to_string();
        offset += name_len;
        let mime = std::str::from_utf8(&bytes[offset..offset + mime_len])
            .context("sidecar item MIME type is not valid ASCII/UTF-8")?
            .to_string();
        offset += mime_len;
        let stored = &bytes[offset..offset + stored_byte_length as usize];
        offset += stored_byte_length as usize;
        let decoded = decode_sidecar_item_payload(item_type, codec, raw_byte_length, stored)?;
        let text = if item_type == SIDECAR_TYPE_UTF8_TEXT {
            Some(
                std::str::from_utf8(&decoded)
                    .context("decoded UTF-8 text sidecar item is not valid UTF-8")?
                    .to_string(),
            )
        } else {
            None
        };
        let json = if item_type == SIDECAR_TYPE_JSON {
            Some(
                serde_json::from_slice::<serde_json::Value>(&decoded)
                    .context("decoded JSON sidecar item is not valid JSON")?,
            )
        } else {
            None
        };
        items.push(SidecarDecodedItem {
            item_type,
            item_type_name: sidecar_type_name(item_type),
            codec,
            codec_name: sidecar_codec_name(codec),
            flags,
            raw_byte_length: (raw_byte_length != SIDECAR_RAW_LENGTH_ABSENT)
                .then_some(raw_byte_length),
            stored_byte_length,
            decoded_byte_length: decoded.len(),
            name,
            mime,
            stored_data_base64: general_purpose::STANDARD.encode(stored),
            data_base64: general_purpose::STANDARD.encode(&decoded),
            text,
            json,
        });
    }
    if offset != bytes.len() {
        bail!("sidecar container has trailing bytes");
    }
    Ok(SidecarDecodedItems {
        ok: true,
        validation,
        items,
    })
}

pub fn parse_sidecar_carrier(raw: &str) -> Result<SidecarCarrier> {
    match raw
        .trim()
        .to_ascii_lowercase()
        .replace([' ', '-', '_'], "")
        .as_str()
    {
        "label" => Ok(SidecarCarrier::Label),
        "intergroove" | "intragroove" | "groove" => Ok(SidecarCarrier::Intergroove),
        "leadindeadwax" | "leaddeadwax" | "leadin" | "leadout" | "deadwax" | "runout" => {
            Ok(SidecarCarrier::LeadInDeadwax)
        }
        _ => bail!("unknown sidecar carrier: {raw}"),
    }
}

pub fn sidecar_carrier_name(carrier: SidecarCarrier) -> &'static str {
    match carrier {
        SidecarCarrier::Label => "label",
        SidecarCarrier::Intergroove => "intergroove",
        SidecarCarrier::LeadInDeadwax => "leadInDeadwax",
    }
}

pub fn normalize_sidecar_carriers(raw: Option<&[String]>) -> Result<Vec<SidecarCarrier>> {
    let mut carriers = Vec::new();
    if let Some(raw) = raw {
        for value in raw {
            let carrier = parse_sidecar_carrier(value)?;
            if !carriers.contains(&carrier) {
                carriers.push(carrier);
            }
        }
    } else {
        carriers.push(SidecarCarrier::Label);
        carriers.push(SidecarCarrier::Intergroove);
    }
    if carriers.is_empty() {
        bail!("sidecar carriers must not be empty");
    }
    Ok(carriers)
}

pub fn default_sidecar_carriers() -> Vec<SidecarCarrier> {
    vec![SidecarCarrier::Label, SidecarCarrier::Intergroove]
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

fn load_png_rgba(png_bytes: &[u8]) -> Result<(usize, usize, Vec<u8>)> {
    let image = load_from_memory(png_bytes)
        .context("failed to decode record PNG")?
        .to_rgba8();
    let (width, height) = image.dimensions();
    Ok((width as usize, height as usize, image.into_raw()))
}

fn decode_record_png_context(
    png_bytes: &[u8],
    record_profile: Option<&str>,
) -> Result<RecordPngContext> {
    let (width, height, rgba) = load_png_rgba(png_bytes)?;
    let (record_profile, descriptor) =
        decode_record_descriptor_resolving_profile(png_bytes, record_profile)?;

    Ok(RecordPngContext {
        width,
        height,
        rgba,
        record_profile,
        descriptor,
    })
}

fn write_rgba_png(width: usize, height: usize, rgba: &[u8]) -> Result<Vec<u8>> {
    if rgba.len() != width.saturating_mul(height).saturating_mul(4) {
        bail!("RGBA buffer length does not match width * height * 4");
    }

    let mut out = Vec::new();
    let encoder =
        PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::NoFilter);
    encoder
        .write_image(rgba, width as u32, height as u32, ExtendedColorType::Rgba8)
        .context("failed to encode RGBA PNG")?;
    Ok(out)
}

fn sidecar_label_inner_radius(geometry: &record_core::RecordProfileGeometry) -> i32 {
    record_core::label_sidecar_inner_radius_from_profile_geometry(geometry)
}

fn sidecar_label_outer_radius(geometry: &record_core::RecordProfileGeometry) -> i32 {
    record_core::label_sidecar_outer_radius_from_profile_geometry(geometry)
}

fn sidecar_lead_in_outer_radius(geometry: &record_core::RecordProfileGeometry) -> i32 {
    (geometry.outer_radius - record_core::HEADER_SPIRAL_OUTER_EDGE_INSET)
        .max(geometry.payload_outer_radius + 1)
}

fn sidecar_descriptor_geometry(
    width: usize,
    height: usize,
    b_value: f64,
    record_profile: &str,
) -> Result<serde_json::Value> {
    let geometry = record_core::describe_record_profile(record_profile)?;
    Ok(serde_json::json!({
        "recordProfile": geometry.record_profile,
        "width": width,
        "height": height,
        "center": {
            "x": width as f64 / 2.0,
            "y": height as f64 / 2.0,
        },
        "bValue": b_value,
        "label": {
            "innerRadius": sidecar_label_inner_radius(&geometry),
            "outerRadius": sidecar_label_outer_radius(&geometry),
        },
        "intergroove": {
            "innerRadius": geometry.payload_inner_radius,
            "outerRadius": geometry.payload_outer_radius,
            "excludesPayloadSpiralPixels": true,
        },
        "leadInDeadwax": {
            "leadIn": {
                "innerRadius": geometry.payload_outer_radius,
                "outerRadius": sidecar_lead_in_outer_radius(&geometry),
            },
            "deadwax": {
                "innerRadius": sidecar_label_outer_radius(&geometry),
                "outerRadius": geometry.payload_inner_radius,
            },
            "excludesMetadataSpiralPixels": true,
            "excludesPayloadSpiralPixels": true,
        },
    }))
}

fn build_sidecar_protected_metadata_pixels(
    width: usize,
    height: usize,
    record_profile: &str,
) -> Result<Vec<bool>> {
    let mut protected = vec![false; width * height];
    for pixel_index in
        record_core::build_header_spiral_indices(width, height, record_profile, None, None, None)?
    {
        protected[pixel_index] = true;
    }
    for pixel_index in
        record_core::build_trailer_spiral_indices(width, height, record_profile, None, None, None)?
    {
        protected[pixel_index] = true;
    }
    Ok(protected)
}

fn text_avoid_spec_from_descriptor(
    _descriptor: &record_descriptor::RecordDescriptor,
) -> Option<TextAvoidSpec> {
    None
}

fn apply_text_avoid_spec(
    _protected: &mut [bool],
    _width: usize,
    _height: usize,
    _text_avoid: Option<&TextAvoidSpec>,
) {
}

fn label_sidecar_pixels(
    width: usize,
    height: usize,
    record_profile: &str,
    text_avoid: Option<&TextAvoidSpec>,
) -> Result<Vec<usize>> {
    let geometry = record_core::describe_record_profile(record_profile)?;
    let mut protected =
        build_sidecar_protected_metadata_pixels(width, height, &geometry.record_profile)?;
    apply_text_avoid_spec(&mut protected, width, height, text_avoid);
    let center_x = width as f64 / 2.0;
    let center_y = height as f64 / 2.0;
    let inner = sidecar_label_inner_radius(&geometry) as f64;
    let outer = sidecar_label_outer_radius(&geometry) as f64;
    let mut pixels = Vec::new();

    for y in 0..height {
        for x in 0..width {
            let dx = x as f64 + 0.5 - center_x;
            let dy = y as f64 + 0.5 - center_y;
            let distance = (dx * dx + dy * dy).sqrt();
            if distance <= inner || distance >= outer {
                continue;
            }
            let pixel = y * width + x;
            if !protected[pixel] {
                pixels.push(pixel);
            }
        }
    }

    Ok(pixels)
}

fn sidecar_pixel_in_carrier_regions(
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    geometry: &record_core::RecordProfileGeometry,
    payload_intergroove_is_available: bool,
    regions: SidecarCarrierRegions,
) -> bool {
    let center_x = width as f64 / 2.0;
    let center_y = height as f64 / 2.0;
    let dx = x as f64 + 0.5 - center_x;
    let dy = y as f64 + 0.5 - center_y;
    let distance = (dx * dx + dy * dy).sqrt();
    let label_outer_radius = sidecar_label_outer_radius(geometry) as f64;
    let in_label = regions.label
        && distance > sidecar_label_inner_radius(geometry) as f64
        && distance < label_outer_radius;
    let in_intergroove = regions.payload_intergroove
        && distance > geometry.payload_inner_radius as f64
        && distance < geometry.payload_outer_radius as f64
        && payload_intergroove_is_available;
    let in_lead_in = regions.lead_in_deadwax
        && payload_intergroove_is_available
        && distance > geometry.payload_outer_radius as f64
        && distance < sidecar_lead_in_outer_radius(geometry) as f64;
    let in_deadwax = regions.lead_in_deadwax
        && payload_intergroove_is_available
        && distance > label_outer_radius
        && distance < geometry.payload_inner_radius as f64;
    in_label || in_intergroove || in_lead_in || in_deadwax
}

fn build_sidecar_carrier_region_pairs(
    width: usize,
    height: usize,
    b_value: f64,
    record_profile: &str,
    regions: SidecarCarrierRegions,
    seed: u32,
    text_avoid: Option<&TextAvoidSpec>,
) -> Result<Vec<(usize, usize)>> {
    let geometry = record_core::describe_record_profile(record_profile)?;
    let mask = if regions.payload_intergroove || regions.lead_in_deadwax {
        Some(record_core::build_spiral_mask(
            width,
            height,
            b_value,
            &geometry.record_profile,
            None,
            None,
            None,
        )?)
    } else {
        None
    };
    let mut protected =
        build_sidecar_protected_metadata_pixels(width, height, &geometry.record_profile)?;
    apply_text_avoid_spec(&mut protected, width, height, text_avoid);
    let mut pairs = Vec::new();

    for y in 0..height {
        let mut x = 0usize;
        while x + 1 < width {
            let first = y * width + x;
            let second = first + 1;
            if protected[first] || protected[second] {
                x += 1;
                continue;
            }

            let first_payload_intergroove_available = mask
                .as_ref()
                .map(|mask| mask.kinds[first] == 0)
                .unwrap_or(false);
            let second_payload_intergroove_available = mask
                .as_ref()
                .map(|mask| mask.kinds[second] == 0)
                .unwrap_or(false);

            let first_ok = sidecar_pixel_in_carrier_regions(
                x,
                y,
                width,
                height,
                &geometry,
                first_payload_intergroove_available,
                regions,
            );
            let second_ok = sidecar_pixel_in_carrier_regions(
                x + 1,
                y,
                width,
                height,
                &geometry,
                second_payload_intergroove_available,
                regions,
            );

            if first_ok && second_ok {
                pairs.push((first, second));
                x += 2;
            } else {
                x += 1;
            }
        }
    }

    shuffle_pairs_mulberry32(&mut pairs, seed);
    Ok(pairs)
}

fn build_sidecar_carrier_pairs(
    width: usize,
    height: usize,
    b_value: f64,
    record_profile: &str,
    carriers: &[SidecarCarrier],
    seed: u32,
    text_avoid: Option<&TextAvoidSpec>,
) -> Result<Vec<(usize, usize)>> {
    build_sidecar_carrier_region_pairs(
        width,
        height,
        b_value,
        record_profile,
        SidecarCarrierRegions {
            label: carriers.contains(&SidecarCarrier::Label),
            payload_intergroove: carriers.contains(&SidecarCarrier::Intergroove),
            lead_in_deadwax: carriers.contains(&SidecarCarrier::LeadInDeadwax),
        },
        seed,
        text_avoid,
    )
}

fn sidecar_capacity_entry_json(
    width: usize,
    height: usize,
    b_value: f64,
    record_profile: &str,
    rgba: &[u8],
    scheme: &str,
    carriers: Vec<SidecarCarrier>,
    text_avoid: Option<&TextAvoidSpec>,
) -> Result<serde_json::Value> {
    let carrier_pairs = build_sidecar_carrier_pairs(
        width,
        height,
        b_value,
        record_profile,
        &carriers,
        SIDECAR_DEFAULT_SEED,
        text_avoid,
    )?;
    let capacity = sidecar_capacity_for_pairs(scheme, &carriers, &carrier_pairs, rgba)?;
    serde_json::to_value(capacity).context("failed to serialize sidecar capacity")
}

fn sidecar_pointer_from_descriptor(
    descriptor: &record_descriptor::RecordDescriptor,
) -> Result<Option<SidecarHeaderPointer>> {
    descriptor
        .bsc_pointer
        .as_deref()
        .map(decode_sidecar_header_pointer)
        .transpose()
}

fn decode_record_png_sidecar_with_context(
    png_bytes: &[u8],
    record_profile: Option<&str>,
) -> Result<(Vec<u8>, SidecarDecodeResult)> {
    let context = decode_record_png_context(png_bytes, record_profile)?;
    let pointer = sidecar_pointer_from_descriptor(&context.descriptor)?;
    let scheme = pointer
        .as_ref()
        .map(|pointer| pointer.scheme.clone())
        .unwrap_or_else(|| SIDECAR_SCHEME_PAIRSIGN_SAFE_LUMA_V2.to_string());
    let carriers = pointer
        .as_ref()
        .map(|pointer| pointer.carriers.clone())
        .unwrap_or_else(default_sidecar_carriers);
    let seed = pointer
        .as_ref()
        .map(|pointer| pointer.seed)
        .unwrap_or(SIDECAR_DEFAULT_SEED);
    let text_avoid = text_avoid_spec_from_descriptor(&context.descriptor);
    let carrier_pairs = build_sidecar_carrier_pairs(
        context.width,
        context.height,
        context.descriptor.b_value(),
        &context.record_profile,
        &carriers,
        seed,
        text_avoid.as_ref(),
    )?;
    let capacity_bytes = sidecar_capacity_bytes_for_scheme(&scheme, carrier_pairs.len())?;
    let length = if let Some(pointer) = pointer.as_ref() {
        pointer.length
    } else {
        let prefix =
            decode_pairsign_sidecar_bytes_from_pairs(&context.rgba, &carrier_pairs, &scheme, 12)?;
        if prefix.len() < 12 || &prefix[..4] != SIDECAR_MAGIC {
            bail!("record does not contain a typed sidecar stream");
        }
        u32::from_be_bytes(prefix[8..12].try_into().expect("slice length")) as usize
    };

    if length > capacity_bytes {
        bail!(
            "sidecar stream length {} exceeds carrier capacity {}",
            length,
            capacity_bytes
        );
    }

    let (bts1, result) = decode_sidecar_from_pairs(&context.rgba, &carrier_pairs, &scheme, length)?;

    if let Some(pointer) = pointer.as_ref() {
        let actual: [u8; 32] = Sha256::digest(&bts1).into();
        if actual != pointer.sha256_bytes {
            bail!("record sidecar SHA-256 does not match header pointer");
        }
    }

    Ok((bts1, result))
}

fn decode_base64url_field(value: &str, field: &str) -> Result<Vec<u8>> {
    general_purpose::URL_SAFE_NO_PAD
        .decode(value.trim())
        .with_context(|| format!("invalid base64url in {field}"))
}

fn descriptor_input_with_rewrite_options(
    descriptor: &record_descriptor::RecordDescriptor,
    options: &RecordRewriteOptions,
    sidecar: &PreparedSidecar,
) -> Result<record_cut::descriptor::RecordDescriptorInput> {
    let pointer = sidecar_header_pointer_from_prepared(sidecar);

    let signed_release_reference = match (
        options.header_release_commitment_sha256.as_deref(),
        options.header_signature_key_id.as_deref(),
        options.header_signature.as_deref(),
    ) {
        (Some(commitment), Some(key_id), Some(signature)) => {
            let commitment = decode_base64url_field(commitment, "headerReleaseCommitmentSha256")?;
            let commitment: [u8; 32] = commitment.try_into().map_err(|value: Vec<u8>| {
                anyhow::anyhow!(
                    "headerReleaseCommitmentSha256 must decode to 32 bytes, got {}",
                    value.len()
                )
            })?;
            let key_id = key_id.as_bytes().to_vec();
            if key_id.is_empty() {
                bail!("headerSignatureKeyId must not be empty");
            }
            let signature = decode_base64url_field(signature, "headerSignature")?;
            if signature.len() != 64 {
                bail!(
                    "headerSignature must decode to 64 bytes, got {}",
                    signature.len()
                );
            }

            Some(record_descriptor::SignedReleaseReference {
                version: record_descriptor::SIGNED_RELEASE_REFERENCE_VERSION,
                release_commitment_sha256: commitment,
                key_id,
                signature,
            })
        }
        (None, None, None) => descriptor.signed_release_reference.clone(),
        _ => bail!(
            "headerReleaseCommitmentSha256, headerSignatureKeyId, and headerSignature must be supplied together"
        ),
    };

    Ok(record_cut::descriptor::RecordDescriptorInput {
        record_profile: descriptor.record_profile.clone(),
        stream_byte_length: descriptor.stream_byte_length,
        payload_encoding: Some(descriptor.payload_encoding.clone()),
        title: descriptor.title.clone(),
        artist: descriptor.artist.clone(),
        release_id: descriptor
            .release_id
            .map(record_descriptor::release_id_to_text),
        catalog_number: descriptor.catalog_number.clone(),
        label: descriptor.label.clone(),
        artwork_credit: descriptor.artwork_credit.clone(),
        canonical_url: options
            .header_canonical_url
            .clone()
            .or_else(|| descriptor.canonical_url.clone()),
        created_at: options.header_created_at.or(descriptor.created_at),
        copyright_year: descriptor.copyright_year,
        copyright_holder: descriptor.copyright_holder.clone(),
        signed_release_reference,
        bsc_pointer: Some(encode_sidecar_header_pointer(&pointer)?),
        tone_spans: descriptor.tone_spans.clone(),
        cache_encryption: descriptor.cache_encryption.clone(),
            chain_anchor: None,
        additional_signatures: Vec::new(),
        isrcs: Vec::new(),
        upc: None,
        deferred_attestation: None,
})
}

fn descriptor_input_with_cache_encryption(
    descriptor: &record_descriptor::RecordDescriptor,
    cache_encryption: record_descriptor::CacheEncryptionDescriptor,
) -> record_cut::descriptor::RecordDescriptorInput {
    record_cut::descriptor::RecordDescriptorInput {
        record_profile: descriptor.record_profile.clone(),
        stream_byte_length: descriptor.stream_byte_length,
        payload_encoding: Some(descriptor.payload_encoding.clone()),
        title: descriptor.title.clone(),
        artist: descriptor.artist.clone(),
        release_id: descriptor
            .release_id
            .map(record_descriptor::release_id_to_text),
        catalog_number: descriptor.catalog_number.clone(),
        label: descriptor.label.clone(),
        artwork_credit: descriptor.artwork_credit.clone(),
        canonical_url: descriptor.canonical_url.clone(),
        created_at: descriptor.created_at,
        copyright_year: descriptor.copyright_year,
        copyright_holder: descriptor.copyright_holder.clone(),
        signed_release_reference: descriptor.signed_release_reference.clone(),
        bsc_pointer: descriptor.bsc_pointer.clone(),
        tone_spans: descriptor.tone_spans.clone(),
        cache_encryption: Some(cache_encryption),
        chain_anchor: None,
        additional_signatures: Vec::new(),
        isrcs: Vec::new(),
        upc: None,
        deferred_attestation: None,
    }
}

fn paint_descriptor_spiral(
    data: &mut [u8],
    width: usize,
    height: usize,
    record_profile: &str,
    main_b_value: f64,
    descriptor: &record_cut::descriptor::RecordDescriptorInput,
) -> Result<record_descriptor::RecordDescriptor> {
    let header_indices =
        record_core::build_header_spiral_indices(width, height, record_profile, None, None, None)?;
    let trailer_indices =
        record_core::build_trailer_spiral_indices(width, height, record_profile, None, None, None)?;
    let mut metadata_indices = header_indices.clone();
    metadata_indices.extend_from_slice(&trailer_indices);
    let byte_capacity =
        record_descriptor::metadata_byte_capacity_for_pixel_count(metadata_indices.len());
    let descriptor_bytes = record_cut::descriptor::encode_record_descriptor_stream(
        main_b_value,
        descriptor,
        byte_capacity,
    )?;
    record_cut::descriptor::paint_metadata_bytes_as_grayscale(
        data,
        &metadata_indices,
        &descriptor_bytes,
    );

    record_descriptor::decode_record_descriptor_bytes(&descriptor_bytes)
}

pub fn estimate_record_png_sidecar_capacity_json(
    png_bytes: &[u8],
    record_profile: Option<&str>,
) -> Result<String> {
    let context = decode_record_png_context(png_bytes, record_profile)?;
    let scheme = SIDECAR_SCHEME_PAIRSIGN_SAFE_LUMA_V2;
    let text_avoid = text_avoid_spec_from_descriptor(&context.descriptor);
    let text_avoid = text_avoid.as_ref();
    serde_json::to_string(&serde_json::json!({
        "scheme": scheme,
        "container": "BTS1",
        "recordProfile": context.record_profile,
        "bValue": context.descriptor.b_value(),
        "geometry": sidecar_descriptor_geometry(
            context.width,
            context.height,
            context.descriptor.b_value(),
            &context.record_profile,
        )?,
        "carrierOrdering": {
            "pairOrder": "safe-luma-desc-horizontal-adjacent",
            "shuffle": "seeded-safe-score-tiebreak",
        },
        "bitPacking": {
            "bitsPerPair": 2,
            "bitOrder": "msb-first",
            "one": "first-luma-greater-than-second",
            "zero": "first-luma-less-than-or-equal-to-second",
            "extraBit": "abs(first-luma-second-luma)>=threshold on sufficiently safe pairs",
            "extraZero": format!("pair-diff < {SIDECAR_PAIR_MAGNITUDE_THRESHOLD}"),
            "extraOne": format!("pair-diff >= {SIDECAR_PAIR_MAGNITUDE_THRESHOLD}"),
        },
        "luma": {
            "model": "rec709",
            "coefficients": [0.2126, 0.7152, 0.0722],
        },
        "writer": {
            "pairDelta": SIDECAR_PAIR_SIGN_DELTA,
            "pairMagnitudeDelta": SIDECAR_PAIR_MAGNITUDE_DELTA,
            "preserveLocalPairAverage": true,
        },
        "safePairScoring": {
            "score": "min(abs(luma-first-background-mid), abs(luma-second-background-mid))",
            "sort": "descending",
            "tieBreak": "seeded-safe-score-tiebreak",
            "twoBitThreshold": SIDECAR_SAFE_V2_MIN_SCORE,
        },
        "label": sidecar_capacity_entry_json(
            context.width,
            context.height,
            context.descriptor.b_value(),
            &context.record_profile,
            &context.rgba,
            scheme,
            vec![SidecarCarrier::Label],
            text_avoid,
        )?,
        "intergroove": sidecar_capacity_entry_json(
            context.width,
            context.height,
            context.descriptor.b_value(),
            &context.record_profile,
            &context.rgba,
            scheme,
            vec![SidecarCarrier::Intergroove],
            text_avoid,
        )?,
        "combined": sidecar_capacity_entry_json(
            context.width,
            context.height,
            context.descriptor.b_value(),
            &context.record_profile,
            &context.rgba,
            scheme,
            vec![SidecarCarrier::Label, SidecarCarrier::Intergroove],
            text_avoid,
        )?,
        "leadInDeadwax": sidecar_capacity_entry_json(
            context.width,
            context.height,
            context.descriptor.b_value(),
            &context.record_profile,
            &context.rgba,
            scheme,
            vec![SidecarCarrier::LeadInDeadwax],
            text_avoid,
        )?,
        "expandedIntergroove": sidecar_capacity_entry_json(
            context.width,
            context.height,
            context.descriptor.b_value(),
            &context.record_profile,
            &context.rgba,
            scheme,
            vec![SidecarCarrier::Intergroove, SidecarCarrier::LeadInDeadwax],
            text_avoid,
        )?,
        "expandedCombined": sidecar_capacity_entry_json(
            context.width,
            context.height,
            context.descriptor.b_value(),
            &context.record_profile,
            &context.rgba,
            scheme,
            vec![
                SidecarCarrier::Label,
                SidecarCarrier::Intergroove,
                SidecarCarrier::LeadInDeadwax
            ],
            text_avoid,
        )?,
    }))
    .context("failed to serialize record PNG sidecar capacity")
}

pub fn decode_record_png_sidecar_items_json(
    png_bytes: &[u8],
    record_profile: Option<&str>,
) -> Result<String> {
    let (bts1, _) = decode_record_png_sidecar_with_context(png_bytes, record_profile)?;
    let decoded = decode_sidecar_container_items(&bts1)?;
    serde_json::to_string(&decoded).context("failed to serialize decoded record sidecar items")
}

pub fn decode_record_png_sidecar_bytes(
    png_bytes: &[u8],
    record_profile: Option<&str>,
) -> Result<Vec<u8>> {
    decode_record_png_sidecar_with_context(png_bytes, record_profile).map(|(bytes, _)| bytes)
}

/// Restore a Patternize groove permutation before payload decoding.
///
/// Returns `Ok(None)` for an ordinary record or a sidecar that contains no
/// BNPM item. The restored PNG retains its descriptor and sidecar pixels; only
/// payload-groove pixels are put back in their exact pre-Patternize order.
pub fn restore_patternized_record_png(
    png_bytes: &[u8],
    record_profile: Option<&str>,
) -> Result<Option<Vec<u8>>> {
    let mut context = decode_record_png_context(png_bytes, record_profile)?;
    if sidecar_pointer_from_descriptor(&context.descriptor)?.is_none() {
        return Ok(None);
    }
    let (bts1, _) =
        decode_record_png_sidecar_with_context(png_bytes, Some(&context.record_profile))
            .context("failed to decode Patternize reverse-map sidecar")?;
    let decoded =
        decode_sidecar_container_items(&bts1).context("Patternize sidecar container is invalid")?;
    let Some(item) = decoded.items.iter().find(|item| {
        item.name == PACKAGE_PATTERN_SIDECAR_ITEM_NAME || item.mime == PACKAGE_PATTERN_SIDECAR_MIME
    }) else {
        return Ok(None);
    };
    let reverse_map = decode_base64_text(&item.data_base64, "Patternize reverse map")?;
    let mask = record_core::build_spiral_mask(
        context.width,
        context.height,
        context.descriptor.b_value(),
        &context.record_profile,
        None,
        None,
        None,
    )?;
    let groove_indices = mask
        .ordered_pixel_indices
        .iter()
        .copied()
        .take_while(|pixel_index| context.rgba[pixel_index * 4 + 3] != 0)
        .collect::<Vec<_>>();
    record_patternize::restore(&mut context.rgba, &groove_indices, &reverse_map)
        .context("failed to restore Patternize groove pixels")?;
    write_rgba_png(context.width, context.height, &context.rgba).map(Some)
}

pub fn rewrite_record_png(
    png_bytes: &[u8],
    render_options_json: &str,
    record_profile: Option<&str>,
) -> Result<(Vec<u8>, SidecarRenderSummary)> {
    let render_options = if render_options_json.trim().is_empty() {
        RecordRewriteOptions::default()
    } else {
        serde_json::from_str::<RecordRewriteOptions>(render_options_json)
            .with_context(|| format!("Could not parse render options: {render_options_json}"))?
    };
    let mut context = decode_record_png_context(png_bytes, record_profile)?;
    let sidecar = prepare_sidecar_render(render_options.sidecar.as_ref())?
        .context("record sidecar embedding requires sidecar render options")?;
    let text_avoid = text_avoid_spec_from_descriptor(&context.descriptor);

    if sidecar.carriers.contains(&SidecarCarrier::Label) {
        if let Some(tuning) = sidecar.label_tuning.as_ref() {
            let pixels = label_sidecar_pixels(
                context.width,
                context.height,
                &context.record_profile,
                text_avoid.as_ref(),
            )?;
            tune_sidecar_pixels(&mut context.rgba, &pixels, tuning);
        }
    }

    let carrier_pairs = build_sidecar_carrier_region_pairs(
        context.width,
        context.height,
        context.descriptor.b_value(),
        &context.record_profile,
        SidecarCarrierRegions {
            label: sidecar.carriers.contains(&SidecarCarrier::Label),
            payload_intergroove: sidecar.carriers.contains(&SidecarCarrier::Intergroove),
            lead_in_deadwax: sidecar.carriers.contains(&SidecarCarrier::LeadInDeadwax),
        },
        sidecar.seed,
        text_avoid.as_ref(),
    )?;
    let summary = paint_sidecar_bytes_into_pairs(&mut context.rgba, &carrier_pairs, &sidecar)?;
    let descriptor_input =
        descriptor_input_with_rewrite_options(&context.descriptor, &render_options, &sidecar)?;
    paint_descriptor_spiral(
        &mut context.rgba,
        context.width,
        context.height,
        &context.record_profile,
        context.descriptor.b_value(),
        &descriptor_input,
    )?;
    let rewritten = write_rgba_png(context.width, context.height, &context.rgba)?;
    Ok((rewritten, summary))
}

/// Adds or replaces the cache-encryption descriptor without decoding or
/// re-encoding the record payload. Only the redundant descriptor spirals are
/// repainted; artwork, audio carriers, and sidecar carriers remain unchanged.
pub fn patch_record_png_cache_encryption_secret(
    png_bytes: &[u8],
    secret_base64url: &str,
    record_profile: Option<&str>,
) -> Result<Vec<u8>> {
    let mut context = decode_record_png_context(png_bytes, record_profile)?;
    let cache_encryption =
        record_descriptor::CacheEncryptionDescriptor::from_secret_base64url(secret_base64url)?;
    let descriptor = descriptor_input_with_cache_encryption(&context.descriptor, cache_encryption);
    paint_descriptor_spiral(
        &mut context.rgba,
        context.width,
        context.height,
        &context.record_profile,
        context.descriptor.b_value(),
        &descriptor,
    )?;
    write_rgba_png(context.width, context.height, &context.rgba)
}

pub fn normalize_sidecar_scheme(raw: Option<&str>) -> Result<String> {
    let Some(raw) = raw else {
        return Ok(SIDECAR_SCHEME_PAIRSIGN_SAFE_LUMA_V2.to_string());
    };
    let normalized = raw.trim().to_ascii_lowercase();
    match normalized.as_str() {
        SIDECAR_SCHEME_PAIRSIGN_SAFE_LUMA_V2 => Ok(normalized),
        _ => bail!("unsupported sidecar carrier scheme {raw}"),
    }
}

pub fn sidecar_pointer_scheme_id(scheme: &str) -> Result<u8> {
    match normalize_sidecar_scheme(Some(scheme))?.as_str() {
        SIDECAR_SCHEME_PAIRSIGN_SAFE_LUMA_V2 => Ok(SIDECAR_POINTER_SCHEME_PAIRSIGN_SAFE_LUMA_V2),
        _ => unreachable!("scheme normalized"),
    }
}

pub fn sidecar_pointer_scheme_name(scheme_id: u8) -> Result<String> {
    match scheme_id {
        SIDECAR_POINTER_SCHEME_PAIRSIGN_SAFE_LUMA_V2 => {
            Ok(SIDECAR_SCHEME_PAIRSIGN_SAFE_LUMA_V2.to_string())
        }
        _ => bail!("unsupported sidecar pointer scheme id {scheme_id}"),
    }
}

pub fn sidecar_pointer_carrier_flags(carriers: &[SidecarCarrier]) -> u8 {
    let mut flags = 0u8;
    if carriers.contains(&SidecarCarrier::Label) {
        flags |= SIDECAR_POINTER_CARRIER_LABEL;
    }
    if carriers.contains(&SidecarCarrier::Intergroove) {
        flags |= SIDECAR_POINTER_CARRIER_INTERGROOVE;
    }
    if carriers.contains(&SidecarCarrier::LeadInDeadwax) {
        flags |= SIDECAR_POINTER_CARRIER_LEAD_IN_DEADWAX;
    }
    flags
}

pub fn sidecar_pointer_carriers(flags: u8) -> Result<Vec<SidecarCarrier>> {
    if flags
        & !(SIDECAR_POINTER_CARRIER_LABEL
            | SIDECAR_POINTER_CARRIER_INTERGROOVE
            | SIDECAR_POINTER_CARRIER_LEAD_IN_DEADWAX)
        != 0
    {
        bail!("sidecar pointer has unsupported carrier flags {flags:#04x}");
    }
    let mut carriers = Vec::new();
    if flags & SIDECAR_POINTER_CARRIER_LABEL != 0 {
        carriers.push(SidecarCarrier::Label);
    }
    if flags & SIDECAR_POINTER_CARRIER_INTERGROOVE != 0 {
        carriers.push(SidecarCarrier::Intergroove);
    }
    if flags & SIDECAR_POINTER_CARRIER_LEAD_IN_DEADWAX != 0 {
        carriers.push(SidecarCarrier::LeadInDeadwax);
    }
    if carriers.is_empty() {
        bail!("sidecar pointer has no carriers");
    }
    Ok(carriers)
}

pub fn encode_sidecar_header_pointer(pointer: &SidecarHeaderPointer) -> Result<Vec<u8>> {
    let length = u32::try_from(pointer.length).context("sidecar length exceeds u32")?;
    let mut out = Vec::with_capacity(SIDECAR_POINTER_LENGTH);
    out.extend_from_slice(SIDECAR_MAGIC);
    out.push(SIDECAR_POINTER_VERSION);
    out.push(sidecar_pointer_scheme_id(&pointer.scheme)?);
    out.push(sidecar_pointer_carrier_flags(&pointer.carriers));
    out.push(0);
    out.extend_from_slice(&pointer.seed.to_be_bytes());
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(&pointer.sha256_bytes);
    debug_assert_eq!(out.len(), SIDECAR_POINTER_LENGTH);
    Ok(out)
}

pub fn decode_sidecar_header_pointer(payload: &[u8]) -> Result<SidecarHeaderPointer> {
    if payload.len() != SIDECAR_POINTER_LENGTH {
        bail!("sidecar pointer has invalid length {}", payload.len());
    }
    if &payload[..4] != SIDECAR_MAGIC {
        bail!("sidecar pointer magic is unsupported");
    }
    let version = payload[4];
    if version != SIDECAR_POINTER_VERSION {
        bail!("unsupported sidecar pointer version {version}");
    }
    let scheme = sidecar_pointer_scheme_name(payload[5])?;
    let carriers = sidecar_pointer_carriers(payload[6])?;
    if payload[7] != 0 {
        bail!("sidecar pointer reserved byte must be 0");
    }
    let seed = u32::from_be_bytes(payload[8..12].try_into().expect("slice length"));
    if seed == 0 {
        bail!("sidecar seed must be nonzero");
    }
    let length = u32::from_be_bytes(payload[12..16].try_into().expect("slice length")) as usize;
    if length == 0 {
        bail!("sidecar length must be nonzero");
    }
    let mut sha256_bytes = [0u8; 32];
    sha256_bytes.copy_from_slice(&payload[16..48]);
    let sha256 = general_purpose::URL_SAFE_NO_PAD.encode(sha256_bytes);
    Ok(SidecarHeaderPointer {
        scheme,
        carriers,
        seed,
        length,
        sha256,
        sha256_bytes,
    })
}

pub fn sidecar_header_pointer_json(pointer: &SidecarHeaderPointer) -> serde_json::Value {
    serde_json::json!({
        "v": SIDECAR_POINTER_VERSION,
        "c": "BSC1",
        "s": pointer.scheme.as_str(),
        "r": pointer.carriers.iter().map(|carrier| sidecar_carrier_name(*carrier)).collect::<Vec<_>>(),
        "n": pointer.seed,
        "l": pointer.length,
        "h": pointer.sha256.as_str(),
    })
}

pub fn sidecar_header_pointer_from_prepared(sidecar: &PreparedSidecar) -> SidecarHeaderPointer {
    SidecarHeaderPointer {
        scheme: sidecar.scheme.clone(),
        carriers: sidecar.carriers.clone(),
        seed: sidecar.seed,
        length: sidecar.bytes.len(),
        sha256: sidecar.sha256.clone(),
        sha256_bytes: sidecar.sha256_bytes,
    }
}

pub fn prepare_sidecar_label_tuning(
    raw: Option<&SidecarLabelTuningOptions>,
) -> Option<PreparedSidecarLabelTuning> {
    let raw = raw?;
    if !raw.enabled.unwrap_or(true) {
        return None;
    }
    Some(PreparedSidecarLabelTuning {
        strength: raw.strength.unwrap_or(0.22).clamp(0.0, 1.0),
        target_luma: raw.target_luma.unwrap_or(128.0).clamp(0.0, 255.0),
        grain: raw.grain.unwrap_or(3).clamp(0, 31),
    })
}

pub fn prepare_sidecar_render(
    options: Option<&SidecarRenderOptions>,
) -> Result<Option<PreparedSidecar>> {
    let Some(options) = options else {
        return Ok(None);
    };
    let has_raw = options
        .bsc1_base64
        .as_deref()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    let has_items = options
        .items
        .as_ref()
        .map(|items| !items.is_empty())
        .unwrap_or(false);
    if has_raw == has_items {
        bail!("sidecar requires exactly one of bsc1Base64 or non-empty items");
    }
    let bytes = if has_raw {
        let raw = options.bsc1_base64.as_deref().expect("checked has_raw");
        let bytes = decode_base64_text(raw, "sidecar bsc1Base64")?;
        validate_sidecar_container(&bytes)?;
        bytes
    } else {
        build_sidecar_container_from_items(options.items.as_deref().expect("checked has_items"))?
    };
    let carriers = normalize_sidecar_carriers(options.carriers.as_deref())?;
    let scheme = normalize_sidecar_scheme(options.scheme.as_deref())?;
    let label_tuning = prepare_sidecar_label_tuning(options.label_tuning.as_ref());
    let seed = options.seed.unwrap_or(SIDECAR_DEFAULT_SEED);
    if seed == 0 {
        bail!("sidecar seed must be nonzero");
    }
    if scheme != SIDECAR_SCHEME_PAIRSIGN_SAFE_LUMA_V2 {
        bail!("sidecar stream uses a fixed carrier scheme in this version");
    }
    let sha256_bytes = sha256_digest_bytes(&bytes);
    let sha256 = general_purpose::URL_SAFE_NO_PAD.encode(sha256_bytes);
    Ok(Some(PreparedSidecar {
        bytes,
        scheme,
        label_tuning,
        carriers,
        seed,
        sha256,
        sha256_bytes,
    }))
}

pub fn mulberry32_next(state: &mut u32) -> u32 {
    *state = state.wrapping_add(0x6d2b_79f5);
    let mut t = *state;
    t = (t ^ (t >> 15)).wrapping_mul(t | 1);
    t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
    t ^ (t >> 14)
}

pub fn shuffle_pairs_mulberry32(pairs: &mut [(usize, usize)], seed: u32) {
    let mut state = seed;
    for i in (1..pairs.len()).rev() {
        let j = (mulberry32_next(&mut state) as usize) % (i + 1);
        pairs.swap(i, j);
    }
}

pub use bytes2rgb::luma_rec709 as sidecar_luma_rec709;

pub fn metadata_dither(pixel_index: usize, sequence_index: usize, salt: usize) -> u8 {
    let mut value = pixel_index as u64;
    value ^= (sequence_index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    value ^= (salt as u64).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^= value >> 31;
    (value & 0xff) as u8
}

fn sidecar_transparent_pair_fallback_base(first_pixel: usize, second_pixel: usize) -> i16 {
    128i16 + (metadata_dither(first_pixel, second_pixel, 181) % 31) as i16 - 15i16
}

pub fn sidecar_capacity_bytes_for_scheme(scheme: &str, carrier_pairs: usize) -> Result<usize> {
    match normalize_sidecar_scheme(Some(scheme))?.as_str() {
        SIDECAR_SCHEME_PAIRSIGN_SAFE_LUMA_V2 => Ok(carrier_pairs / 4),
        _ => unreachable!("scheme normalized"),
    }
}

pub fn sidecar_pair_bit_width_for_scheme(
    scheme: &str,
    _rgba: &[u8],
    _first_pixel: usize,
    _second_pixel: usize,
) -> Result<usize> {
    match normalize_sidecar_scheme(Some(scheme))?.as_str() {
        SIDECAR_SCHEME_PAIRSIGN_SAFE_LUMA_V2 => Ok(2),
        _ => unreachable!("scheme normalized"),
    }
}

pub fn sidecar_bit_capacity_for_pairs(
    scheme: &str,
    pairs: &[(usize, usize)],
    rgba: &[u8],
) -> Result<usize> {
    let mut bits = 0usize;
    for &(first, second) in pairs {
        bits = bits
            .checked_add(sidecar_pair_bit_width_for_scheme(
                scheme, rgba, first, second,
            )?)
            .context("sidecar pair bit capacity overflow")?;
    }
    Ok(bits)
}

pub fn sidecar_capacity_for_pairs(
    scheme: &str,
    carriers: &[SidecarCarrier],
    pairs: &[(usize, usize)],
    rgba: &[u8],
) -> Result<SidecarCapacity> {
    let bit_capacity = sidecar_bit_capacity_for_pairs(scheme, pairs, rgba)?;
    let two_bit_pairs = pairs
        .iter()
        .filter_map(|&(first, second)| {
            sidecar_pair_bit_width_for_scheme(scheme, rgba, first, second).ok()
        })
        .filter(|width| *width == 2)
        .count();
    Ok(SidecarCapacity {
        scheme: normalize_sidecar_scheme(Some(scheme))?,
        carriers: carriers
            .iter()
            .map(|carrier| sidecar_carrier_name(*carrier).to_string())
            .collect(),
        carrier_pixels: pairs.len() * 2,
        carrier_pairs: pairs.len(),
        capacity_bits: bit_capacity,
        capacity_bytes: bit_capacity / 8,
        bits_per_pair: if pairs.is_empty() {
            0.0
        } else {
            bit_capacity as f64 / pairs.len() as f64
        },
        two_bit_pairs,
    })
}

fn clamp_sidecar_byte(value: i16) -> u8 {
    value.clamp(0, 255) as u8
}

pub fn tune_sidecar_pixels(
    data: &mut [u8],
    pixel_indices: &[usize],
    tuning: &PreparedSidecarLabelTuning,
) {
    for &pixel in pixel_indices {
        let offset = pixel * 4;
        if offset + 3 >= data.len() || data[offset + 3] == 0 {
            continue;
        }
        let grain = if tuning.grain > 0 {
            let span = (tuning.grain as u8).saturating_mul(2).saturating_add(1);
            (metadata_dither(pixel, pixel, 223) % span) as i16 - tuning.grain
        } else {
            0
        };
        let target = (tuning.target_luma + grain as f64).clamp(0.0, 255.0);
        let luma = sidecar_luma_rec709(data, pixel);
        let delta = ((target - luma) * tuning.strength).round() as i16;
        for channel in 0..3 {
            data[offset + channel] = clamp_sidecar_byte(data[offset + channel] as i16 + delta);
        }
        data[offset + 3] = 255;
    }
}

fn sidecar_payload_bit(bytes: &[u8], bit_index: usize) -> u8 {
    let byte = bytes[bit_index / 8];
    let shift = 7 - (bit_index % 8);
    (byte >> shift) & 0x01
}

fn push_sidecar_decoded_bit(out: &mut [u8], bit_index: usize, bit: u8) {
    if bit != 0 {
        let byte_index = bit_index / 8;
        let shift = 7 - (bit_index % 8);
        out[byte_index] |= 1 << shift;
    }
}

fn paint_sidecar_pair_bit(
    data: &mut [u8],
    first_pixel: usize,
    second_pixel: usize,
    bit: u8,
    magnitude_bit: Option<u8>,
) {
    let first_offset = first_pixel * 4;
    let second_offset = second_pixel * 4;
    let delta = if magnitude_bit == Some(1) {
        SIDECAR_PAIR_MAGNITUDE_DELTA
    } else {
        SIDECAR_PAIR_SIGN_DELTA
    };
    let first_delta = if bit == 1 { delta } else { -delta };
    let second_delta = -first_delta;
    let transparent = data[first_offset + 3] == 0 || data[second_offset + 3] == 0;
    let fallback_base = sidecar_transparent_pair_fallback_base(first_pixel, second_pixel);
    for channel in 0..3 {
        let average = if transparent {
            fallback_base
        } else {
            (data[first_offset + channel] as i16 + data[second_offset + channel] as i16 + 1) / 2
        }
        .clamp(delta, 255 - delta);
        data[first_offset + channel] = clamp_sidecar_byte(average + first_delta);
        data[second_offset + channel] = clamp_sidecar_byte(average + second_delta);
    }
    if data[first_offset + 3] == 0 {
        data[first_offset + 3] = 255;
    }
    if data[second_offset + 3] == 0 {
        data[second_offset + 3] = 255;
    }
}

pub fn decode_pairsign_sidecar_bytes_from_pairs(
    rgba: &[u8],
    pairs: &[(usize, usize)],
    scheme: &str,
    byte_length: usize,
) -> Result<Vec<u8>> {
    let bit_capacity = sidecar_bit_capacity_for_pairs(scheme, pairs, rgba)?;
    if byte_length.saturating_mul(8) > bit_capacity {
        bail!(
            "sidecar descriptor length {} exceeds pair-sign carrier capacity {}",
            byte_length,
            bit_capacity / 8
        );
    }
    let target_bits = byte_length
        .checked_mul(8)
        .context("sidecar decode bit length overflow")?;
    let mut out = vec![0u8; byte_length];
    let mut bit_index = 0usize;
    for &(first, second) in pairs {
        if bit_index >= target_bits {
            break;
        }
        let first_luma = sidecar_luma_rec709(rgba, first);
        let second_luma = sidecar_luma_rec709(rgba, second);
        let sign_bit = if first_luma > second_luma { 1u8 } else { 0u8 };
        push_sidecar_decoded_bit(&mut out, bit_index, sign_bit);
        bit_index += 1;
        if bit_index < target_bits
            && sidecar_pair_bit_width_for_scheme(scheme, rgba, first, second)? == 2
        {
            let magnitude_bit =
                if (first_luma - second_luma).abs() >= SIDECAR_PAIR_MAGNITUDE_THRESHOLD {
                    1u8
                } else {
                    0u8
                };
            push_sidecar_decoded_bit(&mut out, bit_index, magnitude_bit);
            bit_index += 1;
        }
    }
    if bit_index < target_bits {
        bail!("sidecar carrier did not provide enough bits to decode descriptor length");
    }
    Ok(out)
}

pub fn paint_sidecar_bytes_into_pairs(
    data: &mut [u8],
    pairs: &[(usize, usize)],
    sidecar: &PreparedSidecar,
) -> Result<SidecarRenderSummary> {
    let carrier_pair_count = pairs.len();
    let bit_capacity = sidecar_bit_capacity_for_pairs(&sidecar.scheme, pairs, data)?;
    let capacity_bytes = bit_capacity / 8;
    let payload_bits = sidecar
        .bytes
        .len()
        .checked_mul(8)
        .context("sidecar payload bit length overflow")?;
    if payload_bits > bit_capacity {
        bail!(
            "sidecar stream is {} bytes but selected carriers only fit {} bytes",
            sidecar.bytes.len(),
            capacity_bytes
        );
    }

    let mut pair_index = 0usize;
    let mut bit_index = 0usize;
    while bit_index < payload_bits {
        let (first_pixel, second_pixel) = pairs[pair_index];
        let sign_bit = sidecar_payload_bit(&sidecar.bytes, bit_index);
        bit_index += 1;
        let pair_bit_width =
            sidecar_pair_bit_width_for_scheme(&sidecar.scheme, data, first_pixel, second_pixel)?;
        let magnitude_bit = if pair_bit_width == 2 {
            let bit = if bit_index < payload_bits {
                let bit = sidecar_payload_bit(&sidecar.bytes, bit_index);
                bit_index += 1;
                bit
            } else {
                0
            };
            Some(bit)
        } else {
            None
        };
        paint_sidecar_pair_bit(data, first_pixel, second_pixel, sign_bit, magnitude_bit);
        pair_index += 1;
    }
    let carriers = sidecar
        .carriers
        .iter()
        .map(|carrier| sidecar_carrier_name(*carrier).to_string())
        .collect::<Vec<_>>();
    Ok(SidecarRenderSummary {
        container: "BSC1".to_string(),
        scheme: sidecar.scheme.clone(),
        carriers,
        seed: sidecar.seed,
        bsc1_bytes: sidecar.bytes.len(),
        sha256: sidecar.sha256.clone(),
        carrier_pixels: carrier_pair_count * 2,
        carrier_pairs: carrier_pair_count,
        capacity_bytes,
        used_pairs: pair_index,
        unused_pairs: carrier_pair_count.saturating_sub(pair_index),
    })
}

pub fn decode_sidecar_from_pairs(
    rgba: &[u8],
    pairs: &[(usize, usize)],
    scheme: &str,
    byte_length: usize,
) -> Result<(Vec<u8>, SidecarDecodeResult)> {
    let bsc1 = decode_pairsign_sidecar_bytes_from_pairs(rgba, pairs, scheme, byte_length)?;
    let validation = validate_sidecar_container(&bsc1)?;
    let sha256 = sha256_base64url(&bsc1);
    let capacity = sidecar_capacity_bytes_for_scheme(scheme, pairs.len())?;
    let descriptor = serde_json::json!({
        "container": "BSC1",
        "scheme": normalize_sidecar_scheme(Some(scheme))?,
        "length": byte_length,
    });
    let result = SidecarDecodeResult {
        ok: true,
        descriptor,
        validation,
        bsc1_byte_length: bsc1.len(),
        sha256,
        carrier_pixels: pairs.len() * 2,
        carrier_pairs: pairs.len(),
        capacity_bytes: capacity,
    };
    Ok((bsc1, result))
}

#[cfg(test)]
mod attestation_tests {
    use super::*;

    fn item(name: &str, data: &str) -> SidecarDecodedItem {
        SidecarDecodedItem {
            item_type: 1,
            item_type_name: "json".to_string(),
            codec: 0,
            codec_name: "raw".to_string(),
            flags: 0,
            raw_byte_length: None,
            stored_byte_length: data.len() as u32,
            decoded_byte_length: data.len(),
            name: name.to_string(),
            mime: "application/json".to_string(),
            stored_data_base64: data.to_string(),
            data_base64: data.to_string(),
            text: None,
            json: None,
        }
    }

    #[test]
    fn the_attestation_covers_every_item_but_itself() {
        let record = [3u8; 32];
        let items = vec![item("press", "one"), item("photo", "two")];
        let commitment = sidecar_attestation_commitment(&record, &items);

        // Adding the attestation item does not change what it signs.
        let mut with_attestation = items.clone();
        with_attestation.push(item(SIDECAR_ATTESTATION_ITEM_NAME, "signature"));
        assert_eq!(
            commitment,
            sidecar_attestation_commitment(&record, &with_attestation),
            "an attestation cannot be part of what it attests"
        );
    }

    #[test]
    fn rewriting_an_item_breaks_the_attestation() {
        let record = [3u8; 32];
        let items = vec![item("press", "one")];
        let commitment = sidecar_attestation_commitment(&record, &items);
        let rewritten = vec![item("press", "one, edited")];
        assert_ne!(
            commitment,
            sidecar_attestation_commitment(&record, &rewritten),
            "editing a sidecar must invalidate the signature over it"
        );
    }

    #[test]
    fn an_attestation_does_not_travel_between_records() {
        let items = vec![item("press", "one")];
        assert_ne!(
            sidecar_attestation_commitment(&[3u8; 32], &items),
            sidecar_attestation_commitment(&[4u8; 32], &items),
            "the same sidecar on another record is another commitment"
        );
    }

    #[test]
    fn coverage_is_answered_without_asking_about_trust() {
        let record = [3u8; 32];
        let items = vec![item("press", "one")];
        let attestation = SidecarAttestation {
            version: SIDECAR_ATTESTATION_VERSION,
            commitment: general_purpose::URL_SAFE_NO_PAD
                .encode(sidecar_attestation_commitment(&record, &items)),
            key_id: "yl.vin".to_string(),
            signature: general_purpose::URL_SAFE_NO_PAD.encode([1u8; 64]),
        };
        attestation.validate().expect("a well formed attestation");
        assert!(sidecar_attestation_covers(&attestation, &record, &items).expect("checks"));

        let rewritten = vec![item("press", "edited")];
        assert!(!sidecar_attestation_covers(&attestation, &record, &rewritten).expect("checks"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_text_container_round_trips() {
        let json = r#"[{"type":"text","codec":"raw","name":"liner.txt","text":"hello"}]"#;
        let bsc1 = build_sidecar_container_from_items_json(json).unwrap();
        let validation = validate_sidecar_container(&bsc1).unwrap();
        assert_eq!(validation.item_count, 1);
        let decoded = decode_sidecar_container_items(&bsc1).unwrap();
        assert_eq!(decoded.items[0].text.as_deref(), Some("hello"));
    }

    #[test]
    fn pointer_round_trips() {
        let options: SidecarRenderOptions = serde_json::from_str(
            r#"{"seed":324508639,"items":[{"type":"text","codec":"raw","text":"hello"}]}"#,
        )
        .unwrap();
        let sidecar = prepare_sidecar_render(Some(&options)).unwrap().unwrap();
        let pointer = sidecar_header_pointer_from_prepared(&sidecar);
        let encoded = encode_sidecar_header_pointer(&pointer).unwrap();
        let decoded = decode_sidecar_header_pointer(&encoded).unwrap();
        assert_eq!(decoded.seed, 324508639);
        assert_eq!(decoded.length, sidecar.bytes.len());
        assert_eq!(decoded.sha256, sidecar.sha256);
    }

    #[test]
    fn pointer_round_trips_expanded_carriers() {
        let options: SidecarRenderOptions = serde_json::from_str(
            r#"{"carriers":["label","intergroove","leadInDeadwax"],"items":[{"type":"text","codec":"raw","text":"hello"}]}"#,
        )
        .unwrap();
        let sidecar = prepare_sidecar_render(Some(&options)).unwrap().unwrap();
        let pointer = sidecar_header_pointer_from_prepared(&sidecar);
        let encoded = encode_sidecar_header_pointer(&pointer).unwrap();
        let decoded = decode_sidecar_header_pointer(&encoded).unwrap();
        assert_eq!(
            decoded.carriers,
            vec![
                SidecarCarrier::Label,
                SidecarCarrier::Intergroove,
                SidecarCarrier::LeadInDeadwax,
            ]
        );
    }

    #[test]
    fn package_display_header_builder_owns_layout_and_checksums() {
        let json = r##"{
            "coverShown": true,
            "coverEffects": true,
            "coverRgbGrain": true,
            "innerSleeveShown": true,
            "coverEmbedded": true,
            "designLabel": "Design Ω",
            "designClassName": "posterized",
            "posterize": 18,
            "saturation": 0.72,
            "contrast": 0.64,
            "rgbGrainOpacity": 0.86,
            "rgbGrainBlendIndex": 3,
            "crop": { "x": 0.5, "y": 0.25, "zoom": 1.25 },
            "quantizer": null,
            "inner": null,
            "sleeveToneColor": "#abc",
            "sleeveSparkleOpacity": 0.5
        }"##;
        let bytes = build_package_display_header_bytes_from_json(json).unwrap();
        assert_eq!(bytes.len(), DISPLAY_HEADER_LENGTH);
        assert_eq!(&bytes[0..4], DISPLAY_HEADER_MAGIC);
        assert_eq!(bytes[4], DISPLAY_HEADER_VERSION);
        assert_eq!(bytes[5], DISPLAY_HEADER_LENGTH as u8);
        assert_eq!(read_u16be(&bytes, 6, "display header flags").unwrap(), 0x1f);
        assert_eq!(String::from_utf8_lossy(&bytes[16..22]), "Design");
        assert_eq!(bytes[22], b' ');
        assert_eq!(String::from_utf8_lossy(&bytes[56..64]), "posteriz");
        assert_eq!(bytes[64], 18);
        assert_eq!(read_u16be(&bytes, 65, "saturation").unwrap(), 720);
        assert_eq!(read_u16be(&bytes, 67, "contrast").unwrap(), 640);
        assert_eq!(read_u16be(&bytes, 69, "grain").unwrap(), 860);
        assert_eq!(bytes[71], 3);
        assert_eq!(read_u16be(&bytes, 72, "crop x").unwrap(), 32768);
        assert_eq!(read_u16be(&bytes, 74, "crop y").unwrap(), 16384);
        assert_eq!(read_u16be(&bytes, 76, "crop zoom").unwrap(), 1250);
        assert_eq!(bytes[78], 255);
        assert_eq!(read_u16be(&bytes, 80, "inner").unwrap(), 0);
        assert_eq!(String::from_utf8_lossy(&bytes[82..89]), "#AABBCC");
        assert_eq!(read_u16be(&bytes, 89, "sparkle opacity").unwrap(), 500);

        let payload_crc = read_u32be(&bytes, 8, "payload CRC").unwrap();
        assert_eq!(payload_crc, record_core::crc32_ieee(&bytes[16..]));
        let mut header_for_crc = bytes.clone();
        header_for_crc[12..16].fill(0);
        let header_crc = read_u32be(&bytes, 12, "header CRC").unwrap();
        assert_eq!(header_crc, record_core::crc32_ieee(&header_for_crc));

        let item: serde_json::Value =
            serde_json::from_str(&build_package_display_header_item_json(json).unwrap()).unwrap();
        assert_eq!(item["type"], "opaque");
        assert_eq!(item["codec"], "raw");
        assert_eq!(item["name"], DISPLAY_HEADER_NAME);
        assert_eq!(item["mime"], DISPLAY_HEADER_MIME);
        assert_eq!(item["rawByteLength"], DISPLAY_HEADER_LENGTH);
        let item_bytes = general_purpose::STANDARD
            .decode(item["dataBase64"].as_str().unwrap())
            .unwrap();
        assert_eq!(item_bytes, bytes);
    }

    #[test]
    fn package_metadata_item_builder_owns_cover_and_credit_metadata() {
        let json = r#"{
            "photos": [
                {
                    "id": "insert-1",
                    "name": "Insert One.jpg",
                    "status": "ready",
                    "hasAvif": true,
                    "credit": "  Alice   B.  "
                },
                {
                    "id": "insert-2",
                    "name": "Insert Two.png",
                    "status": "ready",
                    "hasAvif": true,
                    "credit": ""
                },
                {
                    "id": "insert-3",
                    "name": "Draft.png",
                    "status": "queued",
                    "hasAvif": false,
                    "credit": "Skipped"
                }
            ],
            "cover": {
                "hasFile": true,
                "hasPreview": true,
                "sourcePhotoId": "insert-1",
                "sourceWidth": 1200,
                "sourceHeight": 800,
                "inner": 576,
                "crop": { "x": 0.25, "y": 0.5, "zoom": 2 },
                "embedded": false,
                "shown": true,
                "designLabel": "Poster",
                "effectsEnabled": true,
                "name": "cover.png"
            }
        }"#;
        let items = build_package_metadata_items_json_from_input(
            &serde_json::from_str::<PackageMetadataInput>(json).unwrap(),
        );
        let item = items.as_array().unwrap().first().unwrap();
        assert_eq!(item["type"], "json");
        assert_eq!(item["codec"], "raw");
        assert_eq!(item["name"], PACKAGE_METADATA_ITEM_NAME);
        assert_eq!(item["mime"], PACKAGE_METADATA_MIME);

        let metadata = &item["json"];
        assert_eq!(metadata["kind"], "bitneedle.packageMetadata");
        assert_eq!(metadata["version"], 1);
        assert_eq!(metadata["photoCredits"].as_array().unwrap().len(), 1);
        assert_eq!(metadata["photoCredits"][0]["position"], 1);
        assert_eq!(metadata["photoCredits"][0]["sourceName"], "Insert One.jpg");
        assert_eq!(metadata["photoCredits"][0]["itemName"], "Insert One.avif");
        assert_eq!(metadata["photoCredits"][0]["credit"], "Alice B.");

        assert_eq!(metadata["cover"]["role"], "cover");
        assert_eq!(metadata["cover"]["source"]["type"], "photoInsert");
        assert_eq!(metadata["cover"]["source"]["position"], 1);
        assert_eq!(metadata["cover"]["source"]["itemName"], "Insert One.avif");
        assert_eq!(metadata["cover"]["display"]["shown"], true);
        assert_eq!(metadata["cover"]["display"]["design"], "Poster");
        assert_eq!(metadata["cover"]["display"]["effectsEnabled"], true);
        assert_eq!(metadata["cover"]["crop"]["normalized"]["x"], 0.25);
        assert_eq!(metadata["cover"]["crop"]["normalized"]["y"], 0.5);
        assert_eq!(metadata["cover"]["crop"]["normalized"]["zoom"], 2.0);
        assert_eq!(metadata["cover"]["crop"]["source"]["width"], 1200);
        assert_eq!(metadata["cover"]["crop"]["source"]["height"], 800);
        assert_eq!(metadata["cover"]["crop"]["rectangle"]["x"], 200);
        assert_eq!(metadata["cover"]["crop"]["rectangle"]["y"], 200);
        assert_eq!(metadata["cover"]["crop"]["rectangle"]["width"], 400);
        assert_eq!(metadata["cover"]["crop"]["rectangle"]["height"], 400);
        assert_eq!(metadata["cover"]["crop"]["output"]["width"], 576);
        assert_eq!(metadata["cover"]["crop"]["output"]["height"], 576);
    }

    #[test]
    fn package_metadata_item_builder_omits_empty_metadata() {
        let items = build_package_metadata_items_json(r#"{"photos":[]}"#).unwrap();
        assert_eq!(items, "[]");
    }

    #[test]
    fn package_image_item_builders_own_names_and_payload_shape() {
        let photo_item = build_package_photo_item_json(
            r#"{"name":" Insert One.final.png "}"#,
            &[0xde, 0xad, 0xbe, 0xef],
        )
        .unwrap();
        let photo: serde_json::Value = serde_json::from_str(&photo_item).unwrap();
        assert_eq!(photo["type"], "image");
        assert_eq!(photo["codec"], "avif");
        assert_eq!(photo["name"], "Insert One.final.avif");
        assert_eq!(photo["mime"], PACKAGE_PHOTO_MIME);
        assert_eq!(photo["dataBase64"], "3q2+7w==");

        let cover_item = build_package_cover_item_json(&[1, 2, 3]);
        let cover: serde_json::Value = serde_json::from_str(&cover_item).unwrap();
        assert_eq!(cover["type"], "image");
        assert_eq!(cover["codec"], "avif");
        assert_eq!(cover["name"], PACKAGE_COVER_ITEM_NAME);
        assert_eq!(cover["mime"], PACKAGE_PHOTO_MIME);
        assert_eq!(cover["dataBase64"], "AQID");
    }

    #[test]
    fn package_encode_cache_key_resolver_owns_normalization() {
        let key = resolve_package_image_encode_cache_key_json(
            r#"{
                "kind": "cover",
                "fileName": "cover.png",
                "fileSize": 12,
                "fileLastModified": "34",
                "quantizer": 47.4,
                "inner": 999,
                "crop": { "x": 1.5, "y": -1, "zoom": 9 },
                "colorMode": "sepia"
            }"#,
        )
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&key).unwrap();
        assert_eq!(
            parsed["key"],
            "cover.png:12:34:cover:47:576:crop-1.0000-0.0000-6.0000:rgb"
        );
        assert_eq!(parsed["fileKey"], "cover.png:12:34");
        assert_eq!(parsed["cropKey"], "crop-1.0000-0.0000-6.0000");
        assert_eq!(parsed["quantizer"], 47);
        assert_eq!(parsed["inner"], 576);
        assert_eq!(parsed["crop"]["x"], 1.0);
        assert_eq!(parsed["crop"]["y"], 0.0);
        assert_eq!(parsed["crop"]["zoom"], 6.0);
        assert_eq!(parsed["colorMode"], "rgb");
        assert_eq!(parsed["monochrome"], false);

        let grayscale_key = resolve_package_image_encode_cache_key_json(
            r#"{
                "kind": "photo",
                "fileName": "insert.jpg",
                "colorMode": "grayscale"
            }"#,
        )
        .unwrap();
        let grayscale: serde_json::Value = serde_json::from_str(&grayscale_key).unwrap();
        assert_eq!(grayscale["colorMode"], "grayscale");
        assert_eq!(grayscale["monochrome"], true);
    }

    #[test]
    fn package_best_fit_cache_key_resolver_owns_cache_protocol() {
        let key = resolve_package_best_fit_cache_key_json(
            r#"{
                "kind": "photo",
                "fileName": "insert.jpg",
                "fileSize": 100,
                "fileLastModified": 200,
                "budgetBytes": "12.9",
                "inner": 128,
                "cropKey": "crop-0.2500-0.5000-2.0000",
                "colorMode": "grayscale"
            }"#,
        )
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&key).unwrap();
        assert_eq!(
            parsed["key"],
            "photo:fit-v7-square-crop-qbest47-35-63:insert.jpg:100:200:12:128:crop-0.2500-0.5000-2.0000:grayscale"
        );
        assert_eq!(parsed["fileKey"], "insert.jpg:100:200");
        assert_eq!(parsed["budgetBytes"], 12);
        assert_eq!(parsed["inner"], 128);
        assert_eq!(parsed["cropKey"], "crop-0.2500-0.5000-2.0000");
        assert_eq!(parsed["colorMode"], "grayscale");
    }

    #[test]
    fn package_quantizer_search_planner_drives_floor_and_binary_search() {
        let plan = package_quantizer_search_plan(
            &serde_json::from_str::<PackageQuantizerSearchPlanInput>(
                r#"{"minQuantizer":0,"maxQuantizer":63,"startQuantizer":47,"trials":[]}"#,
            )
            .unwrap(),
        );
        assert_eq!(plan.next_quantizer, Some(47));
        assert_eq!(plan.note, "start");
        assert!(!plan.done);

        let plan = package_quantizer_search_plan(
            &serde_json::from_str::<PackageQuantizerSearchPlanInput>(
                r#"{
                    "minQuantizer":0,
                    "maxQuantizer":63,
                    "startQuantizer":47,
                    "trials":[{"quantizer":47,"fits":true}]
                }"#,
            )
            .unwrap(),
        );
        assert_eq!(plan.next_quantizer, Some(35));
        assert_eq!(plan.note, "floor");

        let plan = package_quantizer_search_plan(
            &serde_json::from_str::<PackageQuantizerSearchPlanInput>(
                r#"{
                    "minQuantizer":0,
                    "maxQuantizer":63,
                    "startQuantizer":47,
                    "trials":[
                        {"quantizer":47,"fits":true},
                        {"quantizer":35,"fits":false}
                    ]
                }"#,
            )
            .unwrap(),
        );
        assert_eq!(plan.next_quantizer, Some(41));
        assert_eq!(plan.note, "bisect");
        assert_eq!(plan.best_quantizer, Some(47));
    }

    #[test]
    fn package_quantizer_search_planner_drives_ceiling_and_refinement() {
        let plan = package_quantizer_search_plan(
            &serde_json::from_str::<PackageQuantizerSearchPlanInput>(
                r#"{
                    "minQuantizer":0,
                    "maxQuantizer":63,
                    "startQuantizer":47,
                    "trials":[{"quantizer":47,"fits":false}]
                }"#,
            )
            .unwrap(),
        );
        assert_eq!(plan.next_quantizer, Some(63));
        assert_eq!(plan.note, "ceiling");
        assert_eq!(plan.best_quantizer, None);

        let plan = package_quantizer_search_plan(
            &serde_json::from_str::<PackageQuantizerSearchPlanInput>(
                r#"{
                    "minQuantizer":0,
                    "maxQuantizer":63,
                    "startQuantizer":47,
                    "trials":[
                        {"quantizer":47,"fits":false},
                        {"quantizer":63,"fits":true}
                    ]
                }"#,
            )
            .unwrap(),
        );
        assert_eq!(plan.next_quantizer, Some(55));
        assert_eq!(plan.note, "bisect");

        let plan = package_quantizer_search_plan(
            &serde_json::from_str::<PackageQuantizerSearchPlanInput>(
                r#"{
                    "minQuantizer":0,
                    "maxQuantizer":63,
                    "startQuantizer":47,
                    "trials":[
                        {"quantizer":47,"fits":false},
                        {"quantizer":63,"fits":true},
                        {"quantizer":55,"fits":true},
                        {"quantizer":51,"fits":true},
                        {"quantizer":49,"fits":true},
                        {"quantizer":48,"fits":true}
                    ]
                }"#,
            )
            .unwrap(),
        );
        assert_eq!(plan.next_quantizer, Some(46));
        assert_eq!(plan.note, "refine");
        assert_eq!(plan.best_quantizer, Some(48));
    }

    #[test]
    fn package_fit_budget_owns_cover_and_photo_budget_policy() {
        let budget = package_fit_budget(
            &serde_json::from_str::<PackageFitBudgetInput>(
                r#"{"capacity":9000,"baseBsc1Bytes":3000,"photoCount":4}"#,
            )
            .unwrap(),
        );
        assert_eq!(budget.capacity, 9000);
        assert_eq!(budget.base_bsc1_bytes, 3000);
        assert_eq!(budget.available_bytes, 6000);
        assert_eq!(budget.cover_budget_bytes, 2000);
        assert_eq!(budget.remaining_bytes, 6000);
        assert_eq!(budget.photo_count, 4);
        assert_eq!(budget.per_photo_budget_bytes, 1500);

        let over_budget = package_fit_budget(
            &serde_json::from_str::<PackageFitBudgetInput>(
                r#"{"capacity":100,"baseBsc1Bytes":250,"photoCount":2}"#,
            )
            .unwrap(),
        );
        assert_eq!(over_budget.available_bytes, 0);
        assert_eq!(over_budget.cover_budget_bytes, 0);
        assert_eq!(over_budget.remaining_bytes, 0);
        assert_eq!(over_budget.per_photo_budget_bytes, 0);
    }

    #[test]
    fn package_sidecar_render_options_own_protocol_and_label_tuning() {
        let options = build_package_sidecar_render_options(
            &serde_json::from_str::<PackageSidecarRenderOptionsInput>(
                r#"{"seed":42,"capacityBytes":1000,"payloadBytes":600,"items":[{"type":"text"}]}"#,
            )
            .unwrap(),
        );
        assert_eq!(options["sidecar"]["seed"], 42);
        assert_eq!(
            options["sidecar"]["carriers"],
            serde_json::json!(["label", "intergroove"])
        );
        assert_eq!(options["sidecar"]["scheme"], "pairsign-safe-luma-v2");
        assert_eq!(options["sidecar"]["labelTuning"]["enabled"], true);
        assert_eq!(options["sidecar"]["labelTuning"]["targetLuma"], 128);
        assert_eq!(options["sidecar"]["labelTuning"]["strength"], 0.34);
        assert_eq!(options["sidecar"]["labelTuning"]["grain"], 4);
        assert_eq!(options["sidecar"]["items"][0]["type"], "text");
        assert!(options["sidecar"]["bsc1Base64"].is_null());

        let bsc1_options = build_package_sidecar_render_options(
            &serde_json::from_str::<PackageSidecarRenderOptionsInput>(
                r#"{"seed":7,"capacityBytes":0,"payloadBytes":1200,"bsc1Base64":"AAAA"}"#,
            )
            .unwrap(),
        );
        assert_eq!(bsc1_options["sidecar"]["labelTuning"]["strength"], 0.22);
        assert_eq!(bsc1_options["sidecar"]["labelTuning"]["grain"], 3);
        assert_eq!(bsc1_options["sidecar"]["bsc1Base64"], "AAAA");
        assert!(bsc1_options["sidecar"]["items"].is_null());
    }

    #[test]
    fn package_pattern_preservation_owns_decoded_item_protocol_mapping() {
        let decoded = serde_json::json!({
            "items": [
                {
                    "itemType": 3,
                    "codec": 0,
                    "flags": 2,
                    "rawByteLength": 123,
                    "name": PACKAGE_PATTERN_SIDECAR_ITEM_NAME,
                    "mime": PACKAGE_PATTERN_SIDECAR_MIME,
                    "storedDataBase64": "stored-bytes",
                    "dataBase64": "decoded-bytes"
                },
                {
                    "itemType": 2,
                    "codec": 3,
                    "name": "album-cover.avif",
                    "mime": "image/avif",
                    "storedDataBase64": "cover"
                }
            ]
        });
        let items = package_preserved_pattern_items(&decoded).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["type"], "json");
        assert_eq!(items[0]["codec"], "raw");
        assert_eq!(items[0]["name"], PACKAGE_PATTERN_SIDECAR_ITEM_NAME);
        assert_eq!(items[0]["mime"], PACKAGE_PATTERN_SIDECAR_MIME);
        assert_eq!(items[0]["dataBase64"], "stored-bytes");
        assert_eq!(items[0]["rawByteLength"], 123);
        assert_eq!(items[0]["flags"], 2);
    }

    #[test]
    fn pair_sign_round_trips_bytes() {
        let options: SidecarRenderOptions =
            serde_json::from_str(r#"{"items":[{"type":"text","codec":"raw","text":"hello"}]}"#)
                .unwrap();
        let sidecar = prepare_sidecar_render(Some(&options)).unwrap().unwrap();
        let mut rgba = vec![128u8; 512 * 4];
        for pixel in 0..512 {
            rgba[pixel * 4 + 3] = 255;
        }
        let pairs = (0..256)
            .map(|index| (index * 2, index * 2 + 1))
            .collect::<Vec<_>>();
        paint_sidecar_bytes_into_pairs(&mut rgba, &pairs, &sidecar).unwrap();
        let decoded = decode_pairsign_sidecar_bytes_from_pairs(
            &rgba,
            &pairs,
            SIDECAR_SCHEME_PAIRSIGN_SAFE_LUMA_V2,
            sidecar.bytes.len(),
        )
        .unwrap();
        assert_eq!(decoded, sidecar.bytes);
    }
}
