//! Public Bitneedle BRD1 descriptor wire-format and decoding primitives.
//!
//! This crate is the authoritative BRD1 wire contract. It contains no JSON
//! descriptor segments, no Brotli compatibility envelopes, no base64/hex wire
//! representations, and no record-creation policy.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const RECORD_DESCRIPTOR_MAGIC: &[u8; 4] = b"BRD1";
pub const RECORD_DESCRIPTOR_VERSION: u8 = 1;
pub const RECORD_DESCRIPTOR_PREFIX_LENGTH: usize = 19;
pub const STREAM_BYTE_LENGTH_ABSENT: u64 = u64::MAX;

pub const METADATA_GRAYSCALE_NIBBLE_BASE: u8 = 120;

pub const SIGNED_RELEASE_REFERENCE_VERSION: u8 = 1;
pub const SIGNED_RELEASE_REFERENCE_MAX_HASH_LENGTH: usize = u16::MAX as usize;
pub const SIGNED_RELEASE_REFERENCE_MAX_KEY_ID_LENGTH: usize = u16::MAX as usize;
pub const SIGNED_RELEASE_REFERENCE_MAX_SIGNATURE_LENGTH: usize = u16::MAX as usize;

pub const SEGMENT_DESCRIPTOR_CRC32: u8 = 1;
pub const SEGMENT_STREAM_BYTE_LENGTH: u8 = 2;
pub const SEGMENT_RECORD_PROFILE: u8 = 4;
pub const SEGMENT_TITLE: u8 = 5;
pub const SEGMENT_ARTIST: u8 = 6;
pub const SEGMENT_PAYLOAD_ENCODING: u8 = 7;
pub const SEGMENT_RELEASE_ID: u8 = 8;
pub const SEGMENT_CATALOG_NUMBER: u8 = 9;
pub const SEGMENT_LABEL: u8 = 10;
pub const SEGMENT_ARTWORK_CREDIT: u8 = 11;
pub const SEGMENT_CANONICAL_URL: u8 = 13;
pub const SEGMENT_CREATED_AT: u8 = 14;
pub const SEGMENT_SIGNED_RELEASE_REFERENCE: u8 = 16;
pub const SEGMENT_BSC_POINTER: u8 = 21;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedReleaseReference {
    pub version: u8,
    pub manifest_hash_algorithm: u8,
    pub manifest_hash: Vec<u8>,
    pub signature_algorithm: u8,
    pub key_id: Vec<u8>,
    pub signature: Vec<u8>,
}

impl SignedReleaseReference {
    pub fn validate(&self) -> Result<()> {
        if self.version != SIGNED_RELEASE_REFERENCE_VERSION {
            bail!(
                "unsupported signed release reference version: {}",
                self.version
            );
        }
        if self.manifest_hash_algorithm == 0 {
            bail!("manifest hash algorithm code must not be zero");
        }
        if self.manifest_hash.is_empty() {
            bail!("manifest hash must not be empty");
        }
        if self.manifest_hash.len() > SIGNED_RELEASE_REFERENCE_MAX_HASH_LENGTH {
            bail!("manifest hash exceeds u16 length limit");
        }
        if self.signature_algorithm == 0 {
            bail!("signature algorithm code must not be zero");
        }
        if self.key_id.is_empty() {
            bail!("signature key ID must not be empty");
        }
        if self.key_id.len() > SIGNED_RELEASE_REFERENCE_MAX_KEY_ID_LENGTH {
            bail!("signature key ID exceeds u16 length limit");
        }
        if self.signature.is_empty() {
            bail!("signature must not be empty");
        }
        if self.signature.len() > SIGNED_RELEASE_REFERENCE_MAX_SIGNATURE_LENGTH {
            bail!("signature exceeds u16 length limit");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordDescriptor {
    pub version: u8,
    pub checksum_protected: bool,
    pub b_value_bits: u64,
    pub record_profile: String,
    pub stream_byte_length: Option<usize>,
    pub payload_encoding: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub release_id: Option<String>,
    pub catalog_number: Option<String>,
    pub label: Option<String>,
    pub artwork_credit: Option<String>,
    pub canonical_url: Option<String>,
    pub created_at: Option<String>,
    pub signed_release_reference: Option<SignedReleaseReference>,
    pub bsc_pointer: Option<Vec<u8>>,
}

impl RecordDescriptor {
    pub fn b_value(&self) -> f64 {
        f64::from_bits(self.b_value_bits)
    }
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
        let mut nibbles = [0u8; 2];
        for nibble_index in 0..2 {
            let pixel_index = indices[byte_number * 2 + nibble_index];
            let rgba_index = pixel_index
                .checked_mul(4)
                .context("metadata RGBA index overflow")?;
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

            let nibble = red
                .checked_sub(METADATA_GRAYSCALE_NIBBLE_BASE)
                .context("metadata pixel is below grayscale nibble range")?;
            if nibble > 0x0f {
                bail!("{label} metadata pixel is outside grayscale nibble range");
            }
            nibbles[nibble_index] = nibble;
        }
        bytes.push((nibbles[0] << 4) | nibbles[1]);
    }
    Ok(bytes)
}

pub fn decode_descriptor_prefix(bytes: &[u8]) -> Result<DescriptorPrefix> {
    if bytes.len() < RECORD_DESCRIPTOR_PREFIX_LENGTH {
        bail!("record descriptor payload too short");
    }
    if &bytes[..4] != RECORD_DESCRIPTOR_MAGIC {
        bail!("record descriptor magic mismatch");
    }

    let version = bytes[4];
    let payload_len =
        u16::from_be_bytes(bytes[5..7].try_into().expect("slice length")) as usize;
    let segment_count =
        u16::from_be_bytes(bytes[7..9].try_into().expect("slice length")) as usize;
    let segment_stream_len =
        u16::from_be_bytes(bytes[9..11].try_into().expect("slice length")) as usize;
    let b_value_bits =
        u64::from_be_bytes(bytes[11..19].try_into().expect("slice length"));

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

pub fn decode_signed_release_reference(bytes: &[u8]) -> Result<SignedReleaseReference> {
    let mut cursor = ByteCursor::new(bytes);

    let version = cursor.read_u8("signed release reference version")?;
    let manifest_hash_algorithm = cursor.read_u8("manifest hash algorithm")?;
    let hash_len = cursor.read_u16be("manifest hash length")? as usize;
    let manifest_hash = cursor.read_bytes(hash_len, "manifest hash")?.to_vec();
    let signature_algorithm = cursor.read_u8("signature algorithm")?;
    let key_id_len = cursor.read_u16be("signature key ID length")? as usize;
    let key_id = cursor.read_bytes(key_id_len, "signature key ID")?.to_vec();
    let signature_len = cursor.read_u16be("signature length")? as usize;
    let signature = cursor.read_bytes(signature_len, "signature")?.to_vec();

    if cursor.remaining() != 0 {
        bail!(
            "signed release reference contains {} trailing bytes",
            cursor.remaining()
        );
    }

    let reference = SignedReleaseReference {
        version,
        manifest_hash_algorithm,
        manifest_hash,
        signature_algorithm,
        key_id,
        signature,
    };
    reference.validate()?;
    Ok(reference)
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
    let mut parsed_segments = 0usize;

    let mut crc32_range = None;
    let mut crc32 = None;
    let mut stream_byte_length = None;
    let mut record_profile = None;
    let mut payload_encoding = None;
    let mut title = None;
    let mut artist = None;
    let mut release_id = None;
    let mut catalog_number = None;
    let mut label = None;
    let mut artwork_credit = None;
    let mut canonical_url = None;
    let mut created_at = None;
    let mut signed_release_reference = None;
    let mut bsc_pointer = None;

    while offset < body.len() {
        if parsed_segments >= prefix.segment_count {
            bail!("record descriptor contains more segments than declared");
        }
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
        let payload_end = payload_start
            .checked_add(len)
            .context("record descriptor segment length overflow")?;
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
                if stream_byte_length.is_some() {
                    bail!("duplicate stream byte length segment");
                }
                if payload.len() != 8 {
                    bail!("stream byte length segment has invalid length");
                }
                let raw_len = u64::from_be_bytes(payload.try_into().expect("slice length"));
                stream_byte_length = Some(if raw_len == STREAM_BYTE_LENGTH_ABSENT {
                    None
                } else {
                    Some(
                        usize::try_from(raw_len)
                            .context("stream byte length exceeds usize")?,
                    )
                });
            }
            SEGMENT_RECORD_PROFILE => assign_once(
                &mut record_profile,
                decode_required_text(payload, "record profile")?,
                "record profile",
            )?,
            SEGMENT_PAYLOAD_ENCODING => assign_once(
                &mut payload_encoding,
                decode_required_text(payload, "payload encoding")?,
                "payload encoding",
            )?,
            SEGMENT_TITLE => assign_once(
                &mut title,
                decode_optional_text(payload, "title")?,
                "title",
            )?,
            SEGMENT_ARTIST => assign_once(
                &mut artist,
                decode_optional_text(payload, "artist")?,
                "artist",
            )?,
            SEGMENT_RELEASE_ID => assign_once(
                &mut release_id,
                decode_optional_text(payload, "release ID")?,
                "release ID",
            )?,
            SEGMENT_CATALOG_NUMBER => assign_once(
                &mut catalog_number,
                decode_optional_text(payload, "catalog number")?,
                "catalog number",
            )?,
            SEGMENT_LABEL => assign_once(
                &mut label,
                decode_optional_text(payload, "label")?,
                "label",
            )?,
            SEGMENT_ARTWORK_CREDIT => assign_once(
                &mut artwork_credit,
                decode_optional_text(payload, "artwork credit")?,
                "artwork credit",
            )?,
            SEGMENT_CANONICAL_URL => assign_once(
                &mut canonical_url,
                decode_optional_text(payload, "canonical URL")?,
                "canonical URL",
            )?,
            SEGMENT_CREATED_AT => assign_once(
                &mut created_at,
                decode_optional_text(payload, "created-at timestamp")?,
                "created-at timestamp",
            )?,
            SEGMENT_SIGNED_RELEASE_REFERENCE => {
                if signed_release_reference.is_some() {
                    bail!("duplicate signed release reference segment");
                }
                signed_release_reference =
                    Some(decode_signed_release_reference(payload)?);
            }
            SEGMENT_BSC_POINTER => {
                if bsc_pointer.is_some() {
                    bail!("duplicate BSC pointer segment");
                }
                if payload.is_empty() {
                    bail!("BSC pointer segment must not be empty");
                }
                bsc_pointer = Some(payload.to_vec());
            }
            _ => bail!("unsupported canonical record descriptor segment type {kind}"),
        }

        offset = payload_end;
        parsed_segments += 1;
    }

    if parsed_segments != prefix.segment_count {
        bail!(
            "record descriptor segment count mismatch: declared {}, parsed {}",
            prefix.segment_count,
            parsed_segments
        );
    }

    let expected = crc32.context("record descriptor CRC32 segment is missing")?;
    let range = crc32_range.context("record descriptor CRC32 segment is missing")?;
    let mut canonical = bytes[..prefix.payload_len].to_vec();
    canonical[range].fill(0);

    if compute_descriptor_crc32(&canonical) != expected {
        bail!("record descriptor CRC32 mismatch");
    }

    let b_value = f64::from_bits(prefix.b_value_bits);
    if !(b_value.is_finite() && b_value > 0.0) {
        bail!("decoded invalid b_value");
    }

    Ok(RecordDescriptor {
        version: prefix.version,
        checksum_protected: true,
        b_value_bits: prefix.b_value_bits,
        record_profile: record_profile.context("record profile segment is missing")?,
        stream_byte_length: stream_byte_length
            .context("stream byte length segment is missing")?,
        payload_encoding: payload_encoding.context("payload encoding segment is missing")?,
        title: title.flatten(),
        artist: artist.flatten(),
        release_id: release_id.flatten(),
        catalog_number: catalog_number.flatten(),
        label: label.flatten(),
        artwork_credit: artwork_credit.flatten(),
        canonical_url: canonical_url.flatten(),
        created_at: created_at.flatten(),
        signed_release_reference,
        bsc_pointer,
    })
}

pub fn compute_descriptor_crc32(bytes: &[u8]) -> u32 {
    record_core::crc32_ieee(bytes)
}

fn decode_required_text(payload: &[u8], label: &str) -> Result<String> {
    let value = decode_text(payload, label)?;
    if value.is_empty() {
        bail!("{label} segment must not be empty");
    }
    Ok(value)
}

fn decode_optional_text(payload: &[u8], label: &str) -> Result<Option<String>> {
    if payload.is_empty() {
        return Ok(None);
    }
    Ok(Some(decode_text(payload, label)?))
}

fn decode_text(payload: &[u8], label: &str) -> Result<String> {
    let value = String::from_utf8(payload.to_vec())
        .with_context(|| format!("record descriptor {label} is not valid UTF-8"))?;
    if value.chars().any(char::is_control) {
        bail!("record descriptor {label} contains control characters");
    }
    Ok(value)
}

fn assign_once<T>(
    destination: &mut Option<T>,
    value: T,
    label: &str,
) -> Result<()> {
    if destination.is_some() {
        bail!("duplicate {label} segment");
    }
    *destination = Some(value);
    Ok(())
}

#[derive(Clone, Copy)]
struct ByteCursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> ByteCursor<'a> {
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
        let end = self
            .offset
            .checked_add(2)
            .with_context(|| format!("{label} offset overflow"))?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .with_context(|| format!("{label} is truncated"))?;
        self.offset = end;
        Ok(u16::from_be_bytes(bytes.try_into().expect("length checked")))
    }

    fn read_bytes(&mut self, length: usize, label: &str) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(length)
            .with_context(|| format!("{label} length overflow"))?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .with_context(|| format!("{label} is truncated"))?;
        self.offset = end;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_reference_round_trips_through_decoder() {
        let mut bytes = Vec::new();
        bytes.push(1);
        bytes.push(1);
        bytes.extend_from_slice(&32u16.to_be_bytes());
        bytes.extend_from_slice(&[0x11; 32]);
        bytes.push(7);
        bytes.extend_from_slice(&3u16.to_be_bytes());
        bytes.extend_from_slice(b"key");
        bytes.extend_from_slice(&64u16.to_be_bytes());
        bytes.extend_from_slice(&[0x22; 64]);

        let decoded = decode_signed_release_reference(&bytes).unwrap();
        assert_eq!(decoded.manifest_hash, vec![0x11; 32]);
        assert_eq!(decoded.key_id, b"key");
        assert_eq!(decoded.signature, vec![0x22; 64]);
    }
}
