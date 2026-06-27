//! Public Bitneedle BRD1 descriptor wire-format and decoding primitives.
//!
//! This crate is the authoritative BRD1 carrier-descriptor wire contract. It
//! describes how the record stream is located and encoded in the PNG carrier.
//! It does not define BRS1 payload-entry semantics, programme-time revolution
//! duration, track timing, GAP timing, or the relationship between payload
//! entries and programme revolutions.
//!
//! It contains no JSON descriptor segments, no Brotli compatibility envelopes,
//! no base64/hex wire representations, and no record-creation policy.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const RECORD_DESCRIPTOR_MAGIC: &[u8; 4] = b"BRD1";
pub const RECORD_DESCRIPTOR_VERSION: u8 = 2;
pub const RECORD_DESCRIPTOR_PREFIX_LENGTH: usize = 19;

pub const METADATA_GRAYSCALE_NIBBLE_BASE: u8 = 120;

/// Fixed by the BRD1 v2 format: release commitments are always SHA-256,
/// signatures are always Ed25519. No per-reference algorithm selector.
pub const SIGNED_RELEASE_REFERENCE_VERSION: u8 = 2;
pub const SIGNED_RELEASE_REFERENCE_HASH_LENGTH: usize = 32;
pub const SIGNED_RELEASE_REFERENCE_SIGNATURE_LENGTH: usize = 64;
pub const SIGNED_RELEASE_REFERENCE_MAX_KEY_ID_LENGTH: usize = u16::MAX as usize;

pub const RECORD_PROFILE_SINGLE45_CODE: u8 = 0;
pub const RECORD_PROFILE_LP_CODE: u8 = 1;
pub const RECORD_PROFILE_SINGLE45: &str = "single45";
pub const RECORD_PROFILE_LP: &str = "lp";

pub const RELEASE_ID_LENGTH: usize = 16;

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
pub const SEGMENT_TONED_CARRIER_MAP: u8 = 22;

pub const PAYLOAD_ENCODING_RGB: &str = "rgb";
pub const PAYLOAD_ENCODING_TONED_V1: &str = "toned-v1";
pub const PAYLOAD_ENCODING_RGB_CODE: u8 = 0;
pub const PAYLOAD_ENCODING_TONED_V1_CODE: u8 = 1;

pub const TONED_CARRIER_MAP_VERSION: u8 = 1;
pub const TONED_ORDERING_BASE_PROXIMITY: u8 = 0;
pub const TONED_ORDERING_CHROMA_PROXIMITY: u8 = 1;
pub const TONED_MIN_BITS_PER_PIXEL: u8 = 1;
pub const TONED_MAX_BITS_PER_PIXEL: u8 = 24;
pub const TONED_MAX_SPAN_COUNT: usize = u16::MAX as usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToneOrdering {
    BaseProximity,
    ChromaProximity,
}

impl ToneOrdering {
    pub fn wire_code(self) -> u8 {
        match self {
            Self::BaseProximity => TONED_ORDERING_BASE_PROXIMITY,
            Self::ChromaProximity => TONED_ORDERING_CHROMA_PROXIMITY,
        }
    }

    pub fn from_wire_code(code: u8) -> Result<Self> {
        match code {
            TONED_ORDERING_BASE_PROXIMITY => Ok(Self::BaseProximity),
            TONED_ORDERING_CHROMA_PROXIMITY => Ok(Self::ChromaProximity),
            _ => bail!("unknown toned carrier ordering code {code}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToneSpanDescriptor {
    pub byte_length: usize,
    pub base: [u8; 3],
    pub luma_tolerance: u8,
    pub bits_per_pixel: u8,
    pub ordering: ToneOrdering,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedToneSpan {
    pub index: usize,
    pub byte_offset: usize,
    pub byte_length: usize,
    pub pixel_offset: usize,
    pub pixel_count: usize,
    pub base: [u8; 3],
    pub luma_tolerance: u8,
    pub bits_per_pixel: u8,
    pub ordering: ToneOrdering,
}

/// One blanket signed-release reference covering the release commitment
/// (see `record_core::commitment::release_commitment`). SHA-256 and Ed25519
/// are fixed by `SIGNED_RELEASE_REFERENCE_VERSION`; there is no per-reference
/// algorithm selector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedReleaseReference {
    pub version: u8,
    pub release_commitment_sha256: [u8; SIGNED_RELEASE_REFERENCE_HASH_LENGTH],
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
        if self.key_id.is_empty() {
            bail!("signature key ID must not be empty");
        }
        if self.key_id.len() > SIGNED_RELEASE_REFERENCE_MAX_KEY_ID_LENGTH {
            bail!("signature key ID exceeds u16 length limit");
        }
        if self.signature.len() != SIGNED_RELEASE_REFERENCE_SIGNATURE_LENGTH {
            bail!("signature must be exactly {SIGNED_RELEASE_REFERENCE_SIGNATURE_LENGTH} bytes");
        }
        Ok(())
    }
}

/// Decoded BRD1 carrier descriptor.
///
/// `record_profile` identifies the canonical Bitneedle carrier profile used to
/// decode the raster geometry. BRD1 does not use this field to assign logical
/// sample counts to BRS1 payload entries; programme timing belongs to BRS1 and
/// codec-specific validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordDescriptor {
    pub version: u8,
    pub checksum_protected: bool,
    pub b_value_bits: u64,
    pub record_profile: String,
    pub stream_byte_length: usize,
    pub payload_encoding: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub release_id: Option<[u8; RELEASE_ID_LENGTH]>,
    pub catalog_number: Option<String>,
    pub label: Option<String>,
    pub artwork_credit: Option<String>,
    pub canonical_url: Option<String>,
    pub created_at: Option<u64>,
    pub signed_release_reference: Option<SignedReleaseReference>,
    pub bsc_pointer: Option<Vec<u8>>,
    pub tone_spans: Vec<ToneSpanDescriptor>,
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

pub fn record_profile_code(record_profile: &str) -> Result<u8> {
    match record_profile {
        RECORD_PROFILE_SINGLE45 => Ok(RECORD_PROFILE_SINGLE45_CODE),
        RECORD_PROFILE_LP => Ok(RECORD_PROFILE_LP_CODE),
        other => bail!("unsupported canonical record profile {other}"),
    }
}

pub fn record_profile_from_code(code: u8) -> Result<String> {
    match code {
        RECORD_PROFILE_SINGLE45_CODE => Ok(RECORD_PROFILE_SINGLE45.to_string()),
        RECORD_PROFILE_LP_CODE => Ok(RECORD_PROFILE_LP.to_string()),
        other => bail!("unknown record profile code {other}"),
    }
}

pub fn payload_encoding_code(payload_encoding: &str) -> Result<u8> {
    match payload_encoding {
        PAYLOAD_ENCODING_RGB => Ok(PAYLOAD_ENCODING_RGB_CODE),
        PAYLOAD_ENCODING_TONED_V1 => Ok(PAYLOAD_ENCODING_TONED_V1_CODE),
        other => bail!("unsupported canonical payload encoding {other}"),
    }
}

pub fn payload_encoding_from_code(code: u8) -> Result<String> {
    match code {
        PAYLOAD_ENCODING_RGB_CODE => Ok(PAYLOAD_ENCODING_RGB.to_string()),
        PAYLOAD_ENCODING_TONED_V1_CODE => Ok(PAYLOAD_ENCODING_TONED_V1.to_string()),
        other => bail!("unknown payload encoding code {other}"),
    }
}

const RELEASE_ID_TAGGED_PREFIX: &str = "rel_";
const RELEASE_ID_ULID_TEXT_LENGTH: usize = 26;
const CROCKFORD_BASE32: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Parse a canonical `rel_`-tagged ULID string into its 16 raw bytes.
pub fn release_id_to_bytes(text: &str) -> Result<[u8; RELEASE_ID_LENGTH]> {
    let ulid_text = text
        .strip_prefix(RELEASE_ID_TAGGED_PREFIX)
        .context("release ID is missing the rel_ prefix")?;
    if ulid_text.len() != RELEASE_ID_ULID_TEXT_LENGTH {
        bail!("release ID must be 26 Crockford Base32 characters");
    }

    let mut bits: u128 = 0;
    for (index, byte) in ulid_text.bytes().enumerate() {
        let upper = byte.to_ascii_uppercase();
        let digit = CROCKFORD_BASE32
            .iter()
            .position(|&candidate| candidate == upper)
            .context("release ID contains a non-canonical Crockford Base32 character")?;

        // A textual ULID contains 130 encoded bits but the value is only
        // 128 bits. Therefore the first Crockford digit may contain only the
        // low two bits (0..=7). Rejecting larger values prevents silent u128
        // truncation and ensures one canonical text representation per value.
        if index == 0 && digit > 7 {
            bail!("release ID exceeds the 128-bit ULID range");
        }

        bits = (bits << 5) | digit as u128;
    }
    Ok(bits.to_be_bytes())
}

/// Format 16 raw release ULID bytes back into the canonical `rel_`-tagged
/// display string.
pub fn release_id_to_text(bytes: [u8; RELEASE_ID_LENGTH]) -> String {
    let mut value = u128::from_be_bytes(bytes);
    let mut chars = [b'0'; RELEASE_ID_ULID_TEXT_LENGTH];
    for index in (0..RELEASE_ID_ULID_TEXT_LENGTH).rev() {
        chars[index] = CROCKFORD_BASE32[(value & 0x1f) as usize];
        value >>= 5;
    }
    let mut text = String::with_capacity(RELEASE_ID_TAGGED_PREFIX.len() + RELEASE_ID_ULID_TEXT_LENGTH);
    text.push_str(RELEASE_ID_TAGGED_PREFIX);
    text.push_str(std::str::from_utf8(&chars).expect("Crockford Base32 alphabet is ASCII"));
    text
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

pub fn validate_tone_span(span: &ToneSpanDescriptor, index: usize) -> Result<()> {
    if span.byte_length == 0 {
        bail!("tone span {index} byte length must be greater than zero");
    }
    if !(TONED_MIN_BITS_PER_PIXEL..=TONED_MAX_BITS_PER_PIXEL)
        .contains(&span.bits_per_pixel)
    {
        bail!(
            "tone span {index} bits per pixel must be between {} and {}",
            TONED_MIN_BITS_PER_PIXEL,
            TONED_MAX_BITS_PER_PIXEL
        );
    }
    Ok(())
}

pub fn resolve_tone_spans(
    spans: &[ToneSpanDescriptor],
    expected_byte_length: Option<usize>,
) -> Result<Vec<ResolvedToneSpan>> {
    if spans.is_empty() {
        bail!("toned-v1 carrier map must contain at least one span");
    }
    if spans.len() > TONED_MAX_SPAN_COUNT {
        bail!("tone span count exceeds u16 range");
    }

    let mut byte_offset = 0usize;
    let mut pixel_offset = 0usize;
    let mut resolved = Vec::with_capacity(spans.len());

    for (index, span) in spans.iter().enumerate() {
        validate_tone_span(span, index)?;
        let bit_length = span
            .byte_length
            .checked_mul(8)
            .context("tone span bit length overflow")?;
        let pixel_count =
            bit_length.div_ceil(usize::from(span.bits_per_pixel));

        resolved.push(ResolvedToneSpan {
            index,
            byte_offset,
            byte_length: span.byte_length,
            pixel_offset,
            pixel_count,
            base: span.base,
            luma_tolerance: span.luma_tolerance,
            bits_per_pixel: span.bits_per_pixel,
            ordering: span.ordering,
        });

        byte_offset = byte_offset
            .checked_add(span.byte_length)
            .context("tone span total byte length overflow")?;
        pixel_offset = pixel_offset
            .checked_add(pixel_count)
            .context("tone span total pixel count overflow")?;
    }

    if let Some(expected) = expected_byte_length {
        if byte_offset != expected {
            bail!(
                "tone spans cover {byte_offset} bytes, expected {expected}"
            );
        }
    }

    Ok(resolved)
}

pub fn toned_pixel_count(
    spans: &[ToneSpanDescriptor],
    expected_byte_length: Option<usize>,
) -> Result<usize> {
    Ok(resolve_tone_spans(spans, expected_byte_length)?
        .last()
        .map(|span| span.pixel_offset + span.pixel_count)
        .unwrap_or(0))
}

pub fn encode_toned_carrier_map(
    spans: &[ToneSpanDescriptor],
    expected_byte_length: Option<usize>,
) -> Result<Vec<u8>> {
    resolve_tone_spans(spans, expected_byte_length)?;

    let mut out = Vec::new();
    out.push(TONED_CARRIER_MAP_VERSION);
    out.extend_from_slice(
        &u16::try_from(spans.len())
            .context("tone span count exceeds u16")?
            .to_be_bytes(),
    );

    for span in spans {
        push_varuint(
            &mut out,
            u64::try_from(span.byte_length)
                .context("tone span byte length exceeds u64")?,
        );
        out.extend_from_slice(&span.base);
        out.push(span.luma_tolerance);
        out.push(span.bits_per_pixel);
        out.push(span.ordering.wire_code());
    }

    Ok(out)
}

pub fn decode_toned_carrier_map(
    bytes: &[u8],
    expected_byte_length: Option<usize>,
) -> Result<Vec<ToneSpanDescriptor>> {
    let mut cursor = ByteCursor::new(bytes);
    let version = cursor.read_u8("toned carrier map version")?;
    if version != TONED_CARRIER_MAP_VERSION {
        bail!("unsupported toned carrier map version {version}");
    }

    let count = usize::from(cursor.read_u16be("tone span count")?);
    if count == 0 {
        bail!("toned-v1 carrier map must contain at least one span");
    }

    let mut spans = Vec::with_capacity(count);
    for index in 0..count {
        let byte_length = usize::try_from(
            cursor.read_varuint("tone span byte length")?,
        )
        .context("tone span byte length exceeds usize")?;
        let base = [
            cursor.read_u8("tone span base red")?,
            cursor.read_u8("tone span base green")?,
            cursor.read_u8("tone span base blue")?,
        ];
        let luma_tolerance =
            cursor.read_u8("tone span luma tolerance")?;
        let bits_per_pixel =
            cursor.read_u8("tone span bits per pixel")?;
        let ordering = ToneOrdering::from_wire_code(
            cursor.read_u8("tone span ordering")?,
        )?;

        let span = ToneSpanDescriptor {
            byte_length,
            base,
            luma_tolerance,
            bits_per_pixel,
            ordering,
        };
        validate_tone_span(&span, index)?;
        spans.push(span);
    }

    if cursor.remaining() != 0 {
        bail!(
            "toned carrier map contains {} trailing bytes",
            cursor.remaining()
        );
    }

    resolve_tone_spans(&spans, expected_byte_length)?;
    Ok(spans)
}

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

pub fn decode_signed_release_reference(bytes: &[u8]) -> Result<SignedReleaseReference> {
    let mut cursor = ByteCursor::new(bytes);

    let version = cursor.read_u8("signed release reference version")?;
    let release_commitment_sha256 = cursor
        .read_bytes(
            SIGNED_RELEASE_REFERENCE_HASH_LENGTH,
            "release commitment SHA-256",
        )?
        .try_into()
        .expect("length checked");
    let key_id_len = cursor.read_u16be("signature key ID length")? as usize;
    let key_id = cursor.read_bytes(key_id_len, "signature key ID")?.to_vec();
    let signature = cursor
        .read_bytes(SIGNED_RELEASE_REFERENCE_SIGNATURE_LENGTH, "signature")?
        .to_vec();

    if cursor.remaining() != 0 {
        bail!(
            "signed release reference contains {} trailing bytes",
            cursor.remaining()
        );
    }

    let reference = SignedReleaseReference {
        version,
        release_commitment_sha256,
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
    let mut tone_spans = None;

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
                if payload.len() != 4 {
                    bail!("stream byte length segment has invalid length");
                }
                let raw_len = u32::from_be_bytes(payload.try_into().expect("slice length"));
                if raw_len == 0 {
                    bail!("stream byte length must not be zero");
                }
                stream_byte_length = Some(raw_len as usize);
            }
            SEGMENT_RECORD_PROFILE => {
                if payload.len() != 1 {
                    bail!("record profile segment has invalid length");
                }
                assign_once(
                    &mut record_profile,
                    record_profile_from_code(payload[0])?,
                    "record profile",
                )?
            }
            SEGMENT_PAYLOAD_ENCODING => {
                if payload.len() != 1 {
                    bail!("payload encoding segment has invalid length");
                }
                assign_once(
                    &mut payload_encoding,
                    payload_encoding_from_code(payload[0])?,
                    "payload encoding",
                )?
            }
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
            SEGMENT_RELEASE_ID => {
                if payload.len() != RELEASE_ID_LENGTH {
                    bail!("release ID segment has invalid length");
                }
                assign_once(
                    &mut release_id,
                    <[u8; RELEASE_ID_LENGTH]>::try_from(payload).expect("length checked"),
                    "release ID",
                )?
            }
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
            SEGMENT_CREATED_AT => {
                if payload.len() != 8 {
                    bail!("created-at segment has invalid length");
                }
                assign_once(
                    &mut created_at,
                    u64::from_be_bytes(payload.try_into().expect("slice length")),
                    "created-at timestamp",
                )?
            }
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
            SEGMENT_TONED_CARRIER_MAP => {
                if tone_spans.is_some() {
                    bail!("duplicate toned carrier map segment");
                }
                tone_spans = Some(decode_toned_carrier_map(payload, None)?);
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

    let record_profile =
        record_profile.context("record profile segment is missing")?;
    let stream_byte_length = stream_byte_length
        .context("stream byte length segment is missing")?;
    let payload_encoding =
        payload_encoding.context("payload encoding segment is missing")?;
    let tone_spans = tone_spans.unwrap_or_default();

    match payload_encoding.as_str() {
        PAYLOAD_ENCODING_RGB => {
            if !tone_spans.is_empty() {
                bail!("rgb payload encoding must not include a toned carrier map");
            }
        }
        PAYLOAD_ENCODING_TONED_V1 => {
            if tone_spans.is_empty() {
                bail!("toned-v1 payload encoding requires a toned carrier map");
            }
            resolve_tone_spans(&tone_spans, Some(stream_byte_length))?;
        }
        other => bail!("unsupported canonical payload encoding {other}"),
    }

    Ok(RecordDescriptor {
        version: prefix.version,
        checksum_protected: true,
        b_value_bits: prefix.b_value_bits,
        record_profile,
        stream_byte_length,
        payload_encoding,
        title: title.flatten(),
        artist: artist.flatten(),
        release_id,
        catalog_number: catalog_number.flatten(),
        label: label.flatten(),
        artwork_credit: artwork_credit.flatten(),
        canonical_url: canonical_url.flatten(),
        created_at,
        signed_release_reference,
        bsc_pointer,
        tone_spans,
    })
}

pub fn compute_descriptor_crc32(bytes: &[u8]) -> u32 {
    record_core::crc32_ieee(bytes)
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
                        bail!(
                            "{label} uses non-canonical overlong varuint encoding"
                        );
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
    fn record_profile_codes_round_trip() {
        assert_eq!(record_profile_code("single45").unwrap(), 0);
        assert_eq!(record_profile_code("lp").unwrap(), 1);
        assert_eq!(record_profile_from_code(0).unwrap(), "single45");
        assert_eq!(record_profile_from_code(1).unwrap(), "lp");
        assert!(record_profile_from_code(2).is_err());
    }

    #[test]
    fn payload_encoding_codes_round_trip() {
        assert_eq!(payload_encoding_code("rgb").unwrap(), 0);
        assert_eq!(payload_encoding_code("toned-v1").unwrap(), 1);
        assert_eq!(payload_encoding_from_code(0).unwrap(), "rgb");
        assert_eq!(payload_encoding_from_code(1).unwrap(), "toned-v1");
        assert!(payload_encoding_from_code(2).is_err());
    }

    #[test]
    fn release_id_text_round_trips_through_bytes() {
        let bytes = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x10, 0x20, 0x30, 0x40, 0x50, 0x60,
            0x70, 0x80,
        ];
        let text = release_id_to_text(bytes);
        assert!(text.starts_with("rel_"));
        assert_eq!(text.len(), 4 + 26);
        assert_eq!(release_id_to_bytes(&text).unwrap(), bytes);
    }

    #[test]
    fn release_id_rejects_missing_prefix() {
        assert!(release_id_to_bytes("01ARZ3NDEKTSV4RRFFQ69G5FAV").is_err());
    }

    #[test]
    fn release_id_rejects_values_above_the_ulid_range() {
        assert!(
            release_id_to_bytes("rel_Z1ARZ3NDEKTSV4RRFFQ69G5FAV").is_err()
        );
    }

    #[test]
    fn release_id_accepts_the_maximum_canonical_ulid() {
        let text = "rel_7ZZZZZZZZZZZZZZZZZZZZZZZZZ";
        let bytes = release_id_to_bytes(text).unwrap();
        assert_eq!(release_id_to_text(bytes), text);
    }

    #[test]
    fn binary_reference_round_trips_through_decoder() {
        let mut bytes = Vec::new();
        bytes.push(SIGNED_RELEASE_REFERENCE_VERSION);
        bytes.extend_from_slice(&[0x11; 32]);
        bytes.extend_from_slice(&3u16.to_be_bytes());
        bytes.extend_from_slice(b"key");
        bytes.extend_from_slice(&[0x22; 64]);

        let decoded = decode_signed_release_reference(&bytes).unwrap();
        assert_eq!(decoded.release_commitment_sha256, [0x11; 32]);
        assert_eq!(decoded.key_id, b"key");
        assert_eq!(decoded.signature, vec![0x22; 64]);
    }
    #[test]
    fn toned_carrier_map_round_trips() {
        let spans = vec![
            ToneSpanDescriptor {
                byte_length: 1024,
                base: [255, 192, 203],
                luma_tolerance: 16,
                bits_per_pixel: 21,
                ordering: ToneOrdering::ChromaProximity,
            },
            ToneSpanDescriptor {
                byte_length: 513,
                base: [20, 40, 80],
                luma_tolerance: 8,
                bits_per_pixel: 18,
                ordering: ToneOrdering::BaseProximity,
            },
        ];

        let bytes =
            encode_toned_carrier_map(&spans, Some(1537)).unwrap();
        let decoded =
            decode_toned_carrier_map(&bytes, Some(1537)).unwrap();

        assert_eq!(decoded, spans);
    }

    #[test]
    fn toned_offsets_are_derived() {
        let spans = vec![
            ToneSpanDescriptor {
                byte_length: 5,
                base: [1, 2, 3],
                luma_tolerance: 0,
                bits_per_pixel: 8,
                ordering: ToneOrdering::BaseProximity,
            },
            ToneSpanDescriptor {
                byte_length: 7,
                base: [4, 5, 6],
                luma_tolerance: 1,
                bits_per_pixel: 4,
                ordering: ToneOrdering::ChromaProximity,
            },
        ];

        let resolved = resolve_tone_spans(&spans, Some(12)).unwrap();
        assert_eq!(resolved[0].byte_offset, 0);
        assert_eq!(resolved[1].byte_offset, 5);
        assert_eq!(resolved[0].pixel_count, 5);
        assert_eq!(resolved[1].pixel_offset, 5);
        assert_eq!(resolved[1].pixel_count, 14);
    }

    #[test]
    fn toned_map_rejects_overlong_varuint() {
        let bytes = [
            TONED_CARRIER_MAP_VERSION,
            0,
            1,
            0x81,
            0x00,
            0,
            0,
            0,
            0,
            8,
            TONED_ORDERING_BASE_PROXIMITY,
        ];
        assert!(decode_toned_carrier_map(&bytes, None).is_err());
    }

}
