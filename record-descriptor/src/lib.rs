use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose, Engine as _};
use serde::{Deserialize, Serialize};
use std::io::Read;

pub const RECORD_DESCRIPTOR_MAGIC: &[u8; 4] = b"BRD1";
pub const RECORD_DESCRIPTOR_VERSION: u8 = 1;
pub const RECORD_DESCRIPTOR_PREFIX_LENGTH: usize = 19;
pub const RECORD_DESCRIPTOR_TEXT_LIMIT: usize = 96;
pub const RECORD_DESCRIPTOR_CREATOR_METADATA_TEXT_LIMIT: usize = 1024;
pub const RECORD_DESCRIPTOR_SIGNED_RELEASE_TEXT_LIMIT: usize = 4096;
pub const RECORD_DESCRIPTOR_CHAIN_RECEIPT_TEXT_LIMIT: usize = 8192;
pub const RECORD_DESCRIPTOR_COMPRESSION_BROTLI: u8 = 1;
pub const RECORD_DESCRIPTOR_BROTLI_QUALITY: u32 = 11;
pub const STREAM_BYTE_LENGTH_ABSENT: u64 = u64::MAX;

pub const METADATA_GRAYSCALE_NIBBLE_BASE: u8 = 120;
pub const UNUSED_METADATA_GROOVE_RGB_MIN: u8 = 112;
pub const UNUSED_METADATA_GROOVE_RGB_SPAN: u8 = 32;
pub const UNUSED_METADATA_GROOVE_ALPHA: u8 = 128;
pub const UNUSED_METADATA_GROOVE_FADE_TURNS: f64 = 0.5;

pub const SEGMENT_DESCRIPTOR_CRC32: u8 = 1;
pub const SEGMENT_STREAM_BYTE_LENGTH: u8 = 2;
pub const SEGMENT_GENERATION_VERSION: u8 = 3;
pub const SEGMENT_RECORD_PROFILE: u8 = 4;
pub const SEGMENT_TITLE: u8 = 5;
pub const SEGMENT_ARTIST: u8 = 6;
pub const SEGMENT_PAYLOAD_ENCODING: u8 = 7;
pub const SEGMENT_RELEASE_ID: u8 = 8;
pub const SEGMENT_CATALOG_NUMBER: u8 = 9;
pub const SEGMENT_LABEL: u8 = 10;
pub const SEGMENT_ARTWORK_CREDIT: u8 = 11;
pub const SEGMENT_LICENSE: u8 = 12;
pub const SEGMENT_CANONICAL_URL: u8 = 13;
pub const SEGMENT_CREATED_AT: u8 = 14;
pub const SEGMENT_ARBITRARY_METADATA: u8 = 15;
pub const SEGMENT_SIGNED_RELEASE_MANIFEST: u8 = 16;
pub const SEGMENT_SIGNATURE_ALGORITHM: u8 = 17;
pub const SEGMENT_SIGNATURE_KEY_ID: u8 = 18;
pub const SEGMENT_SIGNATURE: u8 = 19;
pub const SEGMENT_MANIFEST_SHA256: u8 = 20;
pub const SEGMENT_STEGO_SIDECAR_DESCRIPTOR: u8 = 21;
pub const SEGMENT_CHAIN_REGISTRATION_RECEIPT: u8 = 22;
pub const SEGMENT_ARBITRARY_METADATA_BROTLI: u8 = 23;
pub const SEGMENT_SIGNED_RELEASE_MANIFEST_BROTLI: u8 = 24;
pub const SEGMENT_CHAIN_REGISTRATION_RECEIPT_BROTLI: u8 = 25;


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordDescriptor {
    pub version: u8,
    pub checksum_protected: bool,
    pub b_value: f64,
    pub record_profile: Option<String>,
    pub stream_byte_length: Option<usize>,
    pub generation_version: Option<String>,
    pub payload_encoding: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub release_id: Option<String>,
    pub catalog_number: Option<String>,
    pub label: Option<String>,
    pub artwork_credit: Option<String>,
    pub license: Option<String>,
    pub canonical_url: Option<String>,
    pub created_at: Option<String>,
    pub arbitrary_metadata: Option<String>,
    pub signed_release_manifest: Option<String>,
    pub signature_algorithm: Option<String>,
    pub signature_key_id: Option<String>,
    pub signature: Option<String>,
    pub manifest_sha256: Option<String>,
    pub stego_sidecar_descriptor: Option<String>,
    pub chain_registration_receipt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescriptorPrefix {
    pub version: u8,
    pub payload_len: usize,
    pub segment_count: usize,
    pub segment_stream_len: usize,
    pub b_value_bits: u64,
}

pub fn metadata_pixel_count_for_byte_length(byte_length: usize) -> usize {
    byte_length.saturating_mul(2)
}

pub fn metadata_byte_capacity_for_pixel_count(pixel_count: usize) -> usize {
    pixel_count / 2
}

pub fn metadata_bytes_from_grayscale_rgba(
    rgba: &[u8],
    indices: &[usize],
    byte_length: usize,
    label: &str,
) -> Result<Vec<u8>> {
    let pixel_count = metadata_pixel_count_for_byte_length(byte_length);

    if indices.len() < pixel_count {
        bail!("{label} spiral capacity is too small");
    }

    let mut bytes = Vec::with_capacity(byte_length);

    for byte_number in 0..byte_length {
        let mut nibbles = [0_u8; 2];

        for nibble_index in 0..2 {
            let pixel_index = indices[byte_number * 2 + nibble_index];
            let rgba_index = pixel_index * 4;

            if rgba_index + 3 >= rgba.len() {
                bail!("{label} spiral pixel index is outside RGBA buffer");
            }

            let red = rgba[rgba_index];
            let green = rgba[rgba_index + 1];
            let blue = rgba[rgba_index + 2];
            let alpha = rgba[rgba_index + 3];

            if alpha == 0 {
                bail!("{label} spiral pixel is empty");
            }

            if red != green || green != blue {
                bail!("{label} metadata pixel is not grayscale");
            }

            let Some(nibble) = red.checked_sub(METADATA_GRAYSCALE_NIBBLE_BASE) else {
                bail!("{label} metadata pixel is outside light grayscale encoding");
            };

            if nibble > 0x0f {
                bail!("{label} metadata pixel is outside light grayscale encoding");
            }

            nibbles[nibble_index] = nibble;
        }

        bytes.push((nibbles[0] << 4) | nibbles[1]);
    }

    Ok(bytes)
}


pub fn decode_record_descriptor_bytes(bytes: &[u8]) -> Result<RecordDescriptor> {
    let prefix = decode_descriptor_prefix(bytes)?;

    if prefix.version != RECORD_DESCRIPTOR_VERSION {
        bail!("record descriptor version mismatch");
    }

    if prefix.payload_len != RECORD_DESCRIPTOR_PREFIX_LENGTH + prefix.segment_stream_len {
        bail!("record descriptor segment stream length mismatch");
    }

    let body = &bytes[RECORD_DESCRIPTOR_PREFIX_LENGTH..prefix.payload_len];
    let mut offset = 0usize;
    let mut crc32_range = None;
    let mut crc32 = None;
    let mut stream_byte_length = None;
    let mut generation_version = None;
    let mut payload_encoding = None;
    let mut record_profile = None;
    let mut title = None;
    let mut artist = None;
    let mut release_id = None;
    let mut catalog_number = None;
    let mut label = None;
    let mut artwork_credit = None;
    let mut license = None;
    let mut canonical_url = None;
    let mut created_at = None;
    let mut arbitrary_metadata = None;
    let mut signed_release_manifest = None;
    let mut signature_algorithm = None;
    let mut signature_key_id = None;
    let mut signature = None;
    let mut manifest_sha256 = None;
    let mut stego_sidecar_descriptor = None;
    let mut chain_registration_receipt = None;

    for _ in 0..prefix.segment_count {
        if offset + 3 > body.len() {
            bail!("record descriptor segment is truncated");
        }

        let kind = body[offset];
        let len = u16::from_be_bytes(
            body[offset + 1..offset + 3]
                .try_into()
                .expect("slice length"),
        ) as usize;

        let payload_start = offset + 3;
        let payload_end = payload_start + len;

        if payload_end > body.len() {
            bail!("record descriptor segment payload is truncated");
        }

        let payload = &body[payload_start..payload_end];

        match kind {
            SEGMENT_DESCRIPTOR_CRC32 => {
                if crc32.is_some() {
                    bail!("duplicate record descriptor CRC32 segment");
                }

                if payload.len() != 4 {
                    bail!("record descriptor CRC32 segment has invalid length");
                }

                crc32 = Some(u32::from_be_bytes(
                    payload.try_into().expect("slice length"),
                ));

                let absolute_start = RECORD_DESCRIPTOR_PREFIX_LENGTH + payload_start;
                crc32_range = Some(absolute_start..absolute_start + payload.len());
            }
            SEGMENT_STREAM_BYTE_LENGTH => {
                if payload.len() != 8 {
                    bail!("record descriptor stream byte length segment has invalid length");
                }

                let raw_len = u64::from_be_bytes(payload.try_into().expect("slice length"));

                stream_byte_length = if raw_len == STREAM_BYTE_LENGTH_ABSENT {
                    None
                } else {
                    Some(
                        usize::try_from(raw_len)
                            .context("record descriptor stream byte length exceeds usize")?,
                    )
                };
            }
            SEGMENT_GENERATION_VERSION => {
                generation_version = decode_optional_text(payload, "generation version")?;
            }
            SEGMENT_PAYLOAD_ENCODING => {
                payload_encoding = decode_optional_text(payload, "payload encoding")?;
            }
            SEGMENT_RECORD_PROFILE => {
                record_profile = decode_optional_text(payload, "record profile")?;
            }
            SEGMENT_TITLE => {
                title = decode_optional_text(payload, "title")?;
            }
            SEGMENT_ARTIST => {
                artist = decode_optional_text(payload, "artist")?;
            }
            SEGMENT_RELEASE_ID => {
                release_id = decode_optional_text(payload, "release ID")?;
            }
            SEGMENT_CATALOG_NUMBER => {
                catalog_number = decode_optional_text(payload, "catalog number")?;
            }
            SEGMENT_LABEL => {
                label = decode_optional_text(payload, "label")?;
            }
            SEGMENT_ARTWORK_CREDIT => {
                artwork_credit = decode_optional_text(payload, "artwork credit")?;
            }
            SEGMENT_LICENSE => {
                license = decode_optional_text(payload, "license")?;
            }
            SEGMENT_CANONICAL_URL => {
                canonical_url = decode_optional_text(payload, "canonical URL")?;
            }
            SEGMENT_CREATED_AT => {
                created_at = decode_optional_text(payload, "created-at timestamp")?;
            }
            SEGMENT_ARBITRARY_METADATA => {
                arbitrary_metadata = decode_optional_text(payload, "arbitrary metadata")?;
            }
            SEGMENT_ARBITRARY_METADATA_BROTLI => {
                arbitrary_metadata =
                    decode_optional_compressed_text(payload, "arbitrary metadata")?;
            }
            SEGMENT_SIGNED_RELEASE_MANIFEST => {
                signed_release_manifest = decode_optional_text(payload, "signed release manifest")?;
            }
            SEGMENT_SIGNED_RELEASE_MANIFEST_BROTLI => {
                signed_release_manifest =
                    decode_optional_compressed_text(payload, "signed release manifest")?;
            }
            SEGMENT_SIGNATURE_ALGORITHM => {
                signature_algorithm = decode_optional_text(payload, "signature algorithm")?;
            }
            SEGMENT_SIGNATURE_KEY_ID => {
                signature_key_id = decode_optional_text(payload, "signature key ID")?;
            }
            SEGMENT_SIGNATURE => {
                signature = decode_optional_text(payload, "release signature")?;
            }
            SEGMENT_MANIFEST_SHA256 => {
                manifest_sha256 = decode_optional_text(payload, "manifest SHA-256")?;
            }
            SEGMENT_STEGO_SIDECAR_DESCRIPTOR => {
                stego_sidecar_descriptor = Some(general_purpose::STANDARD.encode(payload));
            }
            SEGMENT_CHAIN_REGISTRATION_RECEIPT => {
                chain_registration_receipt =
                    decode_optional_text(payload, "chain registration receipt")?;
            }
            SEGMENT_CHAIN_REGISTRATION_RECEIPT_BROTLI => {
                chain_registration_receipt =
                    decode_optional_compressed_text(payload, "chain registration receipt")?;
            }
            _ => {}
        }

        offset = payload_end;
    }

    if offset != body.len() {
        bail!("record descriptor segment stream has trailing bytes");
    }

    let expected = crc32.context("record descriptor CRC32 segment is missing")?;
    let range = crc32_range.context("record descriptor CRC32 segment is missing")?;
    let mut canonical = bytes[..prefix.payload_len].to_vec();
    canonical[range].fill(0);

    let actual = compute_descriptor_crc32(&canonical);

    if actual != expected {
        bail!("record descriptor CRC32 mismatch");
    }

    let b_value = f64::from_bits(prefix.b_value_bits);

    if !(b_value.is_finite() && b_value > 0.0) {
        bail!("decoded invalid b_value");
    }

    Ok(RecordDescriptor {
        version: prefix.version,
        checksum_protected: true,
        b_value,
        record_profile,
        stream_byte_length,
        generation_version,
        payload_encoding,
        title,
        artist,
        release_id,
        catalog_number,
        label,
        artwork_credit,
        license,
        canonical_url,
        created_at,
        arbitrary_metadata,
        signed_release_manifest,
        signature_algorithm,
        signature_key_id,
        signature,
        manifest_sha256,
        stego_sidecar_descriptor,
        chain_registration_receipt,
    })
}

pub fn decode_descriptor_prefix(bytes: &[u8]) -> Result<DescriptorPrefix> {
    if bytes.len() < RECORD_DESCRIPTOR_PREFIX_LENGTH {
        bail!("record descriptor payload too short");
    }

    if &bytes[..4] != RECORD_DESCRIPTOR_MAGIC {
        bail!("record descriptor magic mismatch");
    }

    let version = bytes[4];
    let payload_len = u16::from_be_bytes(bytes[5..7].try_into().expect("slice length")) as usize;
    let segment_count = u16::from_be_bytes(bytes[7..9].try_into().expect("slice length")) as usize;
    let segment_stream_len =
        u16::from_be_bytes(bytes[9..11].try_into().expect("slice length")) as usize;
    let b_value_bits = u64::from_be_bytes(bytes[11..19].try_into().expect("slice length"));

    if payload_len < RECORD_DESCRIPTOR_PREFIX_LENGTH || payload_len > bytes.len() {
        bail!("record descriptor payload length is invalid");
    }

    Ok(DescriptorPrefix {
        version,
        payload_len,
        segment_count,
        segment_stream_len,
        b_value_bits,
    })
}

pub fn compute_descriptor_crc32(bytes: &[u8]) -> u32 {
    record_core::crc32_ieee(bytes)
}

fn brotli_decompress_payload(payload: &[u8], raw_len: usize) -> Result<Vec<u8>> {
    let mut reader = brotli::Decompressor::new(payload, 4096);
    let mut out = Vec::with_capacity(raw_len);
    reader
        .read_to_end(&mut out)
        .context("failed to Brotli-decompress record descriptor segment")?;

    if out.len() != raw_len {
        bail!(
            "decompressed record descriptor segment length {} does not match declared length {}",
            out.len(),
            raw_len
        );
    }

    Ok(out)
}


fn decode_optional_text(payload: &[u8], label: &str) -> Result<Option<String>> {
    if payload.is_empty() {
        return Ok(None);
    }

    Ok(Some(String::from_utf8(payload.to_vec()).with_context(
        || format!("record descriptor {label} is not valid UTF-8"),
    )?))
}

fn decode_optional_compressed_text(payload: &[u8], label: &str) -> Result<Option<String>> {
    if payload.is_empty() {
        return Ok(None);
    }

    if payload.len() < 5 {
        bail!("record descriptor compressed {label} segment is truncated");
    }

    if payload[0] != RECORD_DESCRIPTOR_COMPRESSION_BROTLI {
        bail!("record descriptor compressed {label} uses unsupported compression codec");
    }

    let raw_len = u32::from_be_bytes(payload[1..5].try_into().expect("slice length")) as usize;
    let decompressed = brotli_decompress_payload(&payload[5..], raw_len)?;

    decode_optional_text(&decompressed, label)
}
