use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
    /// Raw SHA-256 digest bytes from the fixed-width BSC1 pointer.
    pub sha256: [u8; 32],
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
    let mut sha256 = [0u8; 32];
    sha256.copy_from_slice(&payload[16..48]);

    Ok(SidecarHeaderPointer {
        scheme,
        carriers,
        seed,
        length,
        sha256,
    })
}

pub fn sidecar_pointer_sha256_base64url(
    pointer: &SidecarHeaderPointer,
) -> String {
    general_purpose::URL_SAFE_NO_PAD.encode(pointer.sha256)
}

pub fn sidecar_header_pointer_json(pointer: &SidecarHeaderPointer) -> serde_json::Value {
    serde_json::json!({
        "v": SIDECAR_POINTER_VERSION,
        "c": "BSC1",
        "s": pointer.scheme.as_str(),
        "r": pointer
            .carriers
            .iter()
            .map(|carrier| sidecar_carrier_name(*carrier))
            .collect::<Vec<_>>(),
        "n": pointer.seed,
        "l": pointer.length,
        "h": sidecar_pointer_sha256_base64url(pointer),
    })
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

    fn push_sidecar_decoded_bit(out: &mut [u8], bit_index: usize, bit: u8) {
        if bit != 0 {
            let byte_index = bit_index / 8;
            let shift = 7 - (bit_index % 8);
            out[byte_index] |= 1 << shift;
        }
    }

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
